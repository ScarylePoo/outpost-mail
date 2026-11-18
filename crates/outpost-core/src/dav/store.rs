//! Where one calendar-and-contacts account is kept on this computer.
//!
//! As with mail: every event, task and contact is its own standard file
//! (`.ics`, `.vcf`), exactly as the server gave it, in a folder per
//! calendar or address book. A small SQLite index beside them holds what
//! lists and searches need, and can be rebuilt from the files.
//!
//! What the user changes here is written to the file at once and marked as
//! waiting to be sent; `sync.rs` sends it.

use super::client::{Collection, Kind};
use super::item::{self, Item};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE collection (
    id           INTEGER PRIMARY KEY,
    -- Its path on the server.
    href         TEXT NOT NULL UNIQUE,
    name         TEXT NOT NULL,
    -- 'calendar' or 'addressbook'.
    kind         TEXT NOT NULL,
    events       INTEGER NOT NULL,
    tasks        INTEGER NOT NULL,
    -- The colour the server holds, and the one the user chose here.
    server_color TEXT,
    color        TEXT,
    -- The server's change tag when the collection was last fully synced.
    ctag         TEXT,
    -- 0 if the user has chosen not to sync or show it.
    enabled      INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE item (
    id            INTEGER PRIMARY KEY,
    collection_id INTEGER NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
    -- Its path on the server. Empty until a new item has been sent.
    href          TEXT NOT NULL,
    -- The version last read from, or written to, the server.
    etag          TEXT NOT NULL,
    -- The file's name in the collection's folder.
    file          TEXT NOT NULL,
    uid           TEXT NOT NULL,
    -- 'event', 'task' or 'contact'.
    kind          TEXT NOT NULL,
    summary       TEXT NOT NULL,
    -- Events: unix seconds (see item.rs for all-day). Tasks: `start` is
    -- the due time, or NULL.
    start         INTEGER,
    end           INTEGER,
    all_day       INTEGER NOT NULL DEFAULT 0,
    recurring     INTEGER NOT NULL DEFAULT 0,
    completed     INTEGER NOT NULL DEFAULT 0,
    -- Events: the location. Contacts: addresses, one per line.
    detail        TEXT NOT NULL DEFAULT '',
    -- Lower-cased text that a search looks through.
    search        TEXT NOT NULL DEFAULT '',
    -- 0: as on the server. 1: changed or made here, to be sent.
    -- 2: deleted here, to be deleted there.
    dirty         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX item_collection ON item(collection_id, href);
CREATE INDEX item_time ON item(kind, start);
CREATE INDEX item_dirty ON item(dirty) WHERE dirty != 0;
";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Index(rusqlite::Error),
    /// The item or collection is not there.
    Missing,
    /// The file is not a calendar or contact file Outpost can read.
    Unreadable,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "file error: {e}"),
            StoreError::Index(e) => write!(f, "index error: {e}"),
            StoreError::Missing => write!(f, "it is no longer there"),
            StoreError::Unreadable => write!(f, "the file could not be read as a calendar or contact file"),
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Index(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A calendar, task list or address book, as kept here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCollection {
    pub id: i64,
    pub href: String,
    pub name: String,
    pub kind: Kind,
    /// The user's colour if set, else the server's.
    pub color: Option<String>,
    pub ctag: Option<String>,
    pub enabled: bool,
}

/// What sync knows about an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub id: i64,
    pub href: String,
    pub etag: String,
    pub dirty: u8,
}

/// An item waiting to be sent to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub id: i64,
    pub collection: i64,
    pub href: String,
    pub etag: String,
    pub file: String,
    pub summary: String,
    /// True if it is to be deleted there; otherwise saved.
    pub delete: bool,
}

/// One occurrence of an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub id: i64,
    pub collection: i64,
    pub summary: String,
    pub location: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub recurring: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: i64,
    pub collection: i64,
    pub summary: String,
    pub due: Option<i64>,
    pub due_all_day: bool,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactRow {
    pub id: i64,
    pub collection: i64,
    pub name: String,
    pub emails: Vec<String>,
}

pub struct DavStore {
    dir: PathBuf,
    conn: Mutex<Connection>,
    /// The user's time zone, for times written without one.
    tz: String,
}

fn kind_word(kind: Kind) -> &'static str {
    match kind {
        Kind::Calendar { .. } => "calendar",
        Kind::AddressBook => "addressbook",
    }
}

fn collection_row(r: &rusqlite::Row) -> rusqlite::Result<StoredCollection> {
    let kind: String = r.get(3)?;
    Ok(StoredCollection {
        id: r.get(0)?,
        href: r.get(1)?,
        name: r.get(2)?,
        kind: if kind == "addressbook" { Kind::AddressBook } else { Kind::Calendar { events: r.get(4)?, tasks: r.get(5)? } },
        color: r.get::<_, Option<String>>(7)?.or(r.get(6)?),
        ctag: r.get(8)?,
        enabled: r.get(9)?,
    })
}

const COLLECTION_COLUMNS: &str = "id, href, name, kind, events, tasks, server_color, color, ctag, enabled";

/// What the index holds about an item, from its file.
struct Fields {
    uid: String,
    kind: &'static str,
    summary: String,
    start: Option<i64>,
    end: Option<i64>,
    all_day: bool,
    recurring: bool,
    completed: bool,
    detail: String,
    search: String,
}

impl DavStore {
    /// Opens the store in `dir`, creating it if need be. `tz` is the user's
    /// time zone name (`Europe/Berlin`).
    pub fn open(dir: impl Into<PathBuf>, tz: &str) -> Result<DavStore> {
        let dir = dir.into();
        std::fs::create_dir_all(dir.join("items"))?;
        let conn = Connection::open(dir.join("index.sqlite"))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            // An index from another version: start it again. The files are
            // the record; the next sync fills the index back in.
            conn.execute_batch("DROP TABLE IF EXISTS item; DROP TABLE IF EXISTS collection;")?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(DavStore { dir, conn: Mutex::new(conn), tz: tz.to_string() })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn tz(&self) -> &str {
        &self.tz
    }

    fn folder(&self, collection: i64) -> PathBuf {
        self.dir.join("items").join(collection.to_string())
    }

    fn fields(&self, kind: Kind, data: &str) -> Result<Fields> {
        let lower = |parts: &[&str]| parts.join("\n").to_lowercase();
        match kind {
            Kind::AddressBook => {
                let c = item::read_card(data).ok_or(StoreError::Unreadable)?;
                let detail = c.emails.join("\n");
                Ok(Fields {
                    search: lower(&[&c.name, &detail, &c.organisation, &c.phones.join("\n")]),
                    uid: c.uid,
                    kind: "contact",
                    summary: c.name,
                    start: None,
                    end: None,
                    all_day: false,
                    recurring: false,
                    completed: false,
                    detail,
                })
            }
            Kind::Calendar { .. } => match item::read_calendar(data, &self.tz).ok_or(StoreError::Unreadable)? {
                Item::Event(e) => Ok(Fields {
                    search: lower(&[&e.summary, &e.location]),
                    uid: e.uid,
                    kind: "event",
                    summary: e.summary,
                    start: Some(e.start),
                    end: Some(e.end),
                    all_day: e.all_day,
                    recurring: e.recurring,
                    completed: false,
                    detail: e.location,
                }),
                Item::Task(t) => Ok(Fields {
                    search: lower(&[&t.summary]),
                    uid: t.uid,
                    kind: "task",
                    summary: t.summary,
                    start: t.due,
                    end: None,
                    all_day: t.due_all_day,
                    recurring: false,
                    completed: t.completed,
                    detail: String::new(),
                }),
                Item::Contact(_) => Err(StoreError::Unreadable),
            },
        }
    }

    // ---- collections -------------------------------------------------------

    pub fn collections(&self) -> Result<Vec<StoredCollection>> {
        let conn = self.conn();
        let mut statement = conn.prepare(&format!("SELECT {COLLECTION_COLUMNS} FROM collection ORDER BY kind DESC, lower(name), id"))?;
        let rows = statement.query_map([], collection_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn collection(&self, id: i64) -> Result<StoredCollection> {
        self.conn()
            .query_row(&format!("SELECT {COLLECTION_COLUMNS} FROM collection WHERE id = ?1"), [id], collection_row)
            .optional()?
            .ok_or(StoreError::Missing)
    }

    /// Records what the server has now. New collections are added (shown
    /// and synced unless the user says otherwise), ones that are gone are
    /// removed with their items, the rest keep the user's choices.
    pub fn set_collections(&self, found: &[Collection]) -> Result<()> {
        let existing = self.collections()?;
        let gone: Vec<i64> = existing
            .iter()
            .filter(|c| !found.iter().any(|f| super::client::same_path(&f.href, &c.href)))
            .map(|c| c.id)
            .collect();
        {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            for c in found {
                let (events, tasks) = match c.kind {
                    Kind::Calendar { events, tasks } => (events, tasks),
                    Kind::AddressBook => (false, false),
                };
                let known = existing.iter().find(|e| super::client::same_path(&e.href, &c.href));
                match known {
                    Some(known) => tx.execute(
                        "UPDATE collection SET name = ?1, kind = ?2, events = ?3, tasks = ?4, server_color = ?5 WHERE id = ?6",
                        params![c.name, kind_word(c.kind), events, tasks, c.color, known.id],
                    )?,
                    None => tx.execute(
                        "INSERT INTO collection (href, name, kind, events, tasks, server_color) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![c.href, c.name, kind_word(c.kind), events, tasks, c.color],
                    )?,
                };
            }
            for id in &gone {
                tx.execute("DELETE FROM collection WHERE id = ?1", [id])?;
            }
            tx.commit()?;
        }
        for id in gone {
            let _ = std::fs::remove_dir_all(self.folder(id));
        }
        Ok(())
    }

    /// The user's choices for a collection: whether it is used, and its
    /// colour (`None` goes back to the server's).
    pub fn choose(&self, id: i64, enabled: bool, color: Option<&str>) -> Result<()> {
        let changed = self.conn().execute(
            // Switching a collection back on must make the next sync read it.
            "UPDATE collection SET enabled = ?1, color = ?2, ctag = CASE WHEN ?1 AND NOT enabled THEN NULL ELSE ctag END WHERE id = ?3",
            params![enabled, color, id],
        )?;
        if changed == 0 {
            return Err(StoreError::Missing);
        }
        Ok(())
    }

    pub fn set_ctag(&self, id: i64, ctag: Option<&str>) -> Result<()> {
        self.conn().execute("UPDATE collection SET ctag = ?1 WHERE id = ?2", params![ctag, id])?;
        Ok(())
    }

    // ---- what sync does ----------------------------------------------------

    pub fn known(&self, collection: i64) -> Result<Vec<Known>> {
        let conn = self.conn();
        let mut statement = conn.prepare("SELECT id, href, etag, dirty FROM item WHERE collection_id = ?1")?;
        let rows = statement
            .query_map([collection], |r| Ok(Known { id: r.get(0)?, href: r.get(1)?, etag: r.get(2)?, dirty: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    fn write_file(&self, collection: i64, file: &str, data: &str) -> Result<()> {
        let folder = self.folder(collection);
        std::fs::create_dir_all(&folder)?;
        // Written beside and moved over, so a file is never half-written.
        let temporary = folder.join(format!("{file}.new"));
        std::fs::write(&temporary, data)?;
        std::fs::rename(&temporary, folder.join(file))?;
        Ok(())
    }

    /// Stores an item as the server has it. An item with a change waiting
    /// to be sent is left alone: the change is sent first.
    pub fn put_remote(&self, collection: i64, href: &str, etag: &str, data: &str) -> Result<()> {
        let kind = self.collection(collection)?.kind;
        let fields = self.fields(kind, data)?;
        let existing: Option<(i64, String, u8)> = self
            .conn()
            .query_row("SELECT id, file, dirty FROM item WHERE collection_id = ?1 AND href = ?2", params![collection, href], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        match existing {
            Some((_, _, dirty)) if dirty != 0 => Ok(()),
            Some((id, file, _)) => {
                self.write_file(collection, &file, data)?;
                self.update_fields(id, &fields, Some(etag), 0)
            }
            None => {
                let id = self.insert(collection, href, etag, &fields, 0, kind)?;
                let file = file_name(id, kind);
                self.write_file(collection, &file, data)
            }
        }
    }

    fn insert(&self, collection: i64, href: &str, etag: &str, f: &Fields, dirty: u8, kind: Kind) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO item (collection_id, href, etag, file, uid, kind, summary, start, end, all_day, recurring, completed, detail, search, dirty)
             VALUES (?1, ?2, ?3, '', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![collection, href, etag, f.uid, f.kind, f.summary, f.start, f.end, f.all_day, f.recurring, f.completed, f.detail, f.search, dirty],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute("UPDATE item SET file = ?1 WHERE id = ?2", params![file_name(id, kind), id])?;
        Ok(id)
    }

    fn update_fields(&self, id: i64, f: &Fields, etag: Option<&str>, dirty: u8) -> Result<()> {
        self.conn().execute(
            "UPDATE item SET uid = ?1, kind = ?2, summary = ?3, start = ?4, end = ?5, all_day = ?6, recurring = ?7, completed = ?8,
                             detail = ?9, search = ?10, etag = coalesce(?11, etag), dirty = ?12 WHERE id = ?13",
            params![f.uid, f.kind, f.summary, f.start, f.end, f.all_day, f.recurring, f.completed, f.detail, f.search, etag, dirty, id],
        )?;
        Ok(())
    }

    /// Removes items that are gone from the server: index entries and files.
    pub fn remove(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            let found: Option<(i64, String)> = self
                .conn()
                .query_row("SELECT collection_id, file FROM item WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((collection, file)) = found else { continue };
            self.conn().execute("DELETE FROM item WHERE id = ?1", [id])?;
            let _ = std::fs::remove_file(self.folder(collection).join(file));
        }
        Ok(())
    }

    /// What is waiting to be sent, oldest change first.
    pub fn pending(&self) -> Result<Vec<Pending>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT i.id, i.collection_id, i.href, i.etag, i.file, i.summary, i.dirty FROM item i
             JOIN collection c ON c.id = i.collection_id WHERE i.dirty != 0 AND c.enabled ORDER BY i.id",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok(Pending {
                    id: r.get(0)?,
                    collection: r.get(1)?,
                    href: r.get(2)?,
                    etag: r.get(3)?,
                    file: r.get(4)?,
                    summary: r.get(5)?,
                    delete: r.get::<_, u8>(6)? == 2,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// The server took a change: the item is now as on the server, at this
    /// address and version. If it was changed again here in the meantime
    /// (`data` is not what is in the file now), it stays waiting.
    pub fn sent(&self, id: i64, href: &str, etag: &str, data: &str) -> Result<()> {
        let unchanged = self.data(id).map(|now| now == data).unwrap_or(false);
        self.conn().execute(
            "UPDATE item SET href = ?1, etag = ?2, dirty = CASE WHEN ?3 THEN 0 ELSE dirty END WHERE id = ?4",
            params![href, etag, unchanged, id],
        )?;
        Ok(())
    }

    /// The local change to an item is given up: it will be read again from
    /// the server (or, if it was never there, it is removed).
    pub fn give_up(&self, id: i64) -> Result<()> {
        let href: Option<String> = self.conn().query_row("SELECT href FROM item WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        match href.as_deref() {
            None => Ok(()),
            Some("") => self.remove(&[id]),
            // An empty version never matches the server's, so the next
            // listing fetches the item again.
            Some(_) => {
                self.conn().execute("UPDATE item SET dirty = 0, etag = '' WHERE id = ?1", [id])?;
                Ok(())
            }
        }
    }

    // ---- what the user does ------------------------------------------------

    /// The file of an item, as text.
    pub fn data(&self, id: i64) -> Result<String> {
        let (collection, file): (i64, String) = self
            .conn()
            .query_row("SELECT collection_id, file FROM item WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or(StoreError::Missing)?;
        Ok(std::fs::read_to_string(self.folder(collection).join(file))?)
    }

    pub fn collection_of(&self, id: i64) -> Result<i64> {
        self.conn().query_row("SELECT collection_id FROM item WHERE id = ?1", [id], |r| r.get(0)).optional()?.ok_or(StoreError::Missing)
    }

    /// Saves an item the user made (`id` is `None`) or changed. It shows at
    /// once and waits to be sent. Returns its id.
    pub fn save_local(&self, collection: i64, id: Option<i64>, data: &str) -> Result<i64> {
        let kind = self.collection(collection)?.kind;
        let fields = self.fields(kind, data)?;
        match id {
            Some(id) => {
                let (in_collection, file): (i64, String) = self
                    .conn()
                    .query_row("SELECT collection_id, file FROM item WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?
                    .ok_or(StoreError::Missing)?;
                self.write_file(in_collection, &file, data)?;
                self.update_fields(id, &fields, None, 1)?;
                Ok(id)
            }
            None => {
                let id = self.insert(collection, "", "", &fields, 1, kind)?;
                self.write_file(collection, &file_name(id, kind), data)?;
                Ok(id)
            }
        }
    }

    /// Deletes an item here; it is deleted on the server at the next sync.
    /// One that was never sent is simply removed.
    pub fn delete_local(&self, id: i64) -> Result<()> {
        let href: String = self.conn().query_row("SELECT href FROM item WHERE id = ?1", [id], |r| r.get(0)).optional()?.ok_or(StoreError::Missing)?;
        if href.is_empty() {
            return self.remove(&[id]);
        }
        self.conn().execute("UPDATE item SET dirty = 2 WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---- lists -------------------------------------------------------------

    /// Every occurrence of every event that touches `from..to` (unix
    /// seconds), in the collections the user has switched on, earliest
    /// first.
    pub fn occurrences(&self, from: i64, to: i64) -> Result<Vec<Occurrence>> {
        struct Row {
            id: i64,
            collection: i64,
            file: String,
            summary: String,
            location: String,
            start: i64,
            end: i64,
            all_day: bool,
            recurring: bool,
        }
        let rows: Vec<Row> = {
            let conn = self.conn();
            let mut statement = conn.prepare(
                "SELECT i.id, i.collection_id, i.file, i.summary, i.detail, i.start, i.end, i.all_day, i.recurring
                 FROM item i JOIN collection c ON c.id = i.collection_id
                 WHERE i.kind = 'event' AND i.dirty != 2 AND c.enabled
                   AND i.start < ?2 AND (i.recurring OR i.end > ?1 OR (i.end = i.start AND i.start >= ?1))",
            )?;
            let rows = statement
                .query_map(params![from, to], |r| {
                    Ok(Row {
                        id: r.get(0)?,
                        collection: r.get(1)?,
                        file: r.get(2)?,
                        summary: r.get(3)?,
                        location: r.get(4)?,
                        start: r.get(5)?,
                        end: r.get(6)?,
                        all_day: r.get(7)?,
                        recurring: r.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let one = |start, end, all_day| Occurrence {
                id: row.id,
                collection: row.collection,
                summary: row.summary.clone(),
                location: row.location.clone(),
                start,
                end,
                all_day,
                recurring: row.recurring,
            };
            if !row.recurring {
                out.push(one(row.start, row.end, row.all_day));
                continue;
            }
            // A repeating event: its file says when.
            let Ok(data) = std::fs::read_to_string(self.folder(row.collection).join(&row.file)) else { continue };
            for (start, end, all_day) in item::occurrences(&data, &self.tz, from, to) {
                out.push(one(start, end, all_day));
            }
        }
        out.sort_by(|a, b| (a.start, !a.all_day, a.end, a.id).cmp(&(b.start, !b.all_day, b.end, b.id)));
        Ok(out)
    }

    /// Tasks: unfinished first, by due date (those without one last), then
    /// finished ones.
    pub fn tasks(&self) -> Result<Vec<TaskRow>> {
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT i.id, i.collection_id, i.summary, i.start, i.all_day, i.completed
             FROM item i JOIN collection c ON c.id = i.collection_id
             WHERE i.kind = 'task' AND i.dirty != 2 AND c.enabled
             ORDER BY i.completed, i.start IS NULL, i.start, lower(i.summary), i.id",
        )?;
        let rows = statement
            .query_map([], |r| {
                Ok(TaskRow { id: r.get(0)?, collection: r.get(1)?, summary: r.get(2)?, due: r.get(3)?, due_all_day: r.get(4)?, completed: r.get(5)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Contacts whose name, addresses, numbers or organisation contain
    /// every word of `query`, by name. An empty query lists everyone.
    pub fn contacts(&self, query: &str, limit: u32) -> Result<Vec<ContactRow>> {
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(|w| w.to_string()).collect();
        let conn = self.conn();
        let mut statement = conn.prepare(
            "SELECT i.id, i.collection_id, i.summary, i.detail, i.search
             FROM item i JOIN collection c ON c.id = i.collection_id
             WHERE i.kind = 'contact' AND i.dirty != 2 AND c.enabled
             ORDER BY lower(i.summary), i.id",
        )?;
        let rows = statement.query_map([], |r| {
            Ok((ContactRow { id: r.get(0)?, collection: r.get(1)?, name: r.get(2)?, emails: lines_of(&r.get::<_, String>(3)?) }, r.get::<_, String>(4)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (contact, search) = row?;
            if words.iter().all(|w| search.contains(w.as_str())) {
                out.push(contact);
                if out.len() as u32 >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Where this account is kept.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn lines_of(text: &str) -> Vec<String> {
    text.lines().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect()
}

fn file_name(id: i64, kind: Kind) -> String {
    match kind {
        Kind::Calendar { .. } => format!("{id}.ics"),
        Kind::AddressBook => format!("{id}.vcf"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dav::item::{new_event, new_task, write_card, ContactEdit, EventEdit, TaskEdit};

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "outpost-davstore-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn found(href: &str, name: &str, kind: Kind) -> Collection {
        Collection { href: href.into(), name: name.into(), kind, color: Some("#112233".into()), ctag: None }
    }

    const CAL: Kind = Kind::Calendar { events: true, tasks: true };
    const DAY: i64 = 86_400;

    fn event(uid: &str, summary: &str, start: i64, hours: i64) -> String {
        new_event(uid, &EventEdit { summary: summary.into(), start, end: start + hours * 3600, ..EventEdit::default() }, 0)
    }

    #[test]
    fn collections_keep_the_users_choices_and_go_with_their_items() {
        let store = DavStore::open(temp(), "UTC").unwrap();
        store.set_collections(&[found("/ann/work/", "Work", CAL), found("/ann/people/", "People", Kind::AddressBook)]).unwrap();
        let all = store.collections().unwrap();
        assert_eq!(all.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Work", "People"], "calendars first");
        let work = all[0].id;
        assert_eq!((all[0].color.as_deref(), all[0].enabled), (Some("#112233"), true));

        store.choose(work, false, Some("#ff0000")).unwrap();
        store.put_remote(work, "/ann/work/a.ics", "\"1\"", &event("a", "Planning", 10 * DAY, 1)).unwrap();
        // The server lists it again, renamed, written differently, plus a
        // new one; the address book is gone.
        store.set_collections(&[found("/ann/work", "Work (renamed)", CAL), found("/ann/home/", "Home", CAL)]).unwrap();
        let all = store.collections().unwrap();
        assert_eq!(all.len(), 2);
        let again = all.iter().find(|c| c.id == work).unwrap();
        assert_eq!((again.name.as_str(), again.color.as_deref(), again.enabled), ("Work (renamed)", Some("#ff0000"), false));
        assert_eq!(store.known(work).unwrap().len(), 1, "its items are untouched");
        // Back to the server's colour.
        store.choose(work, true, None).unwrap();
        assert_eq!(store.collection(work).unwrap().color.as_deref(), Some("#112233"));

        store.set_collections(&[found("/ann/home/", "Home", CAL)]).unwrap();
        assert!(matches!(store.collection(work), Err(StoreError::Missing)));
        assert!(matches!(store.data(1), Err(StoreError::Missing)));
        assert!(!store.dir().join("items").join(work.to_string()).exists(), "its files went with it");
    }

    #[test]
    fn events_are_listed_by_when_they_happen() {
        let store = DavStore::open(temp(), "UTC").unwrap();
        store.set_collections(&[found("/c/", "Cal", CAL), found("/off/", "Hidden", CAL)]).unwrap();
        let (cal, off) = (store.collections().unwrap()[0].id, store.collections().unwrap()[1].id);
        store.put_remote(cal, "/c/a.ics", "1", &event("a", "Before", 9 * DAY, 2)).unwrap();
        store.put_remote(cal, "/c/b.ics", "1", &event("b", "Spans the start", 10 * DAY - 3600, 2)).unwrap();
        store.put_remote(cal, "/c/c.ics", "1", &event("c", "Inside", 10 * DAY + 3600, 1)).unwrap();
        store.put_remote(cal, "/c/d.ics", "1", &event("d", "After", 11 * DAY, 1)).unwrap();
        store.put_remote(cal, "/c/e.ics", "1", &event("e", "A moment", 10 * DAY + 7200, 0)).unwrap();
        let all_day = new_event("f", &EventEdit { summary: "All day".into(), start: 10 * DAY, end: 10 * DAY, all_day: true, ..EventEdit::default() }, 0);
        store.put_remote(cal, "/c/f.ics", "1", &all_day).unwrap();
        let daily = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:g\r\nDTSTART:19700105T120000Z\r\nDTEND:19700105T123000Z\r\nRRULE:FREQ=DAILY\r\nSUMMARY:Daily\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        store.put_remote(cal, "/c/g.ics", "1", daily).unwrap();
        store.put_remote(off, "/off/h.ics", "1", &event("h", "In a hidden calendar", 10 * DAY + 3600, 1)).unwrap();
        store.choose(off, false, None).unwrap();

        let day: Vec<(String, i64)> = store.occurrences(10 * DAY, 11 * DAY).unwrap().into_iter().map(|o| (o.summary, o.start - 10 * DAY)).collect();
        assert_eq!(
            day,
            [
                ("Spans the start".to_string(), -3600),
                ("All day".to_string(), 0),
                ("Inside".to_string(), 3600),
                ("A moment".to_string(), 7200),
                ("Daily".to_string(), 12 * 3600),
            ]
        );
        // A week has seven of the daily one.
        let week = store.occurrences(10 * DAY, 17 * DAY).unwrap();
        assert_eq!(week.iter().filter(|o| o.summary == "Daily").count(), 7);
        assert!(week.iter().all(|o| o.summary != "In a hidden calendar"));

        // A file that is not a calendar is refused, not stored.
        assert!(matches!(store.put_remote(cal, "/c/bad.ics", "1", "hello"), Err(StoreError::Unreadable)));
        assert_eq!(store.known(cal).unwrap().len(), 7);
    }

    #[test]
    fn what_the_user_changes_waits_to_be_sent() {
        let store = DavStore::open(temp(), "UTC").unwrap();
        store.set_collections(&[found("/c/", "Cal", CAL)]).unwrap();
        let cal = store.collections().unwrap()[0].id;

        // A new event shows at once and is waiting.
        let text = event("n", "New here", 10 * DAY, 1);
        let id = store.save_local(cal, None, &text).unwrap();
        assert_eq!(store.occurrences(10 * DAY, 11 * DAY).unwrap()[0].summary, "New here");
        let waiting = store.pending().unwrap();
        assert_eq!((waiting.len(), waiting[0].id, waiting[0].href.as_str(), waiting[0].delete), (1, id, "", false));
        assert_eq!(store.data(id).unwrap(), text);

        // Changed again before the first save got through: when that first
        // save is confirmed, the item is still waiting, now with an address.
        let later = event("n", "New here, renamed", 10 * DAY, 1);
        store.save_local(cal, Some(id), &later).unwrap();
        store.sent(id, "/c/n.ics", "\"v1\"", &text).unwrap();
        let waiting = store.pending().unwrap();
        assert_eq!((waiting.len(), waiting[0].href.as_str(), waiting[0].etag.as_str()), (1, "/c/n.ics", "\"v1\""));
        store.sent(id, "/c/n.ics", "\"v2\"", &later).unwrap();
        assert_eq!(store.pending().unwrap(), []);

        // While a change waits, the server's copy does not overwrite it.
        store.save_local(cal, Some(id), &event("n", "Mine", 10 * DAY, 1)).unwrap();
        store.put_remote(cal, "/c/n.ics", "\"v3\"", &event("n", "Theirs", 10 * DAY, 1)).unwrap();
        assert_eq!(store.occurrences(10 * DAY, 11 * DAY).unwrap()[0].summary, "Mine");
        // Given up (the server refused it): the next sync reads it again.
        store.give_up(id).unwrap();
        assert_eq!(store.pending().unwrap(), []);
        assert_eq!(store.known(cal).unwrap()[0].etag, "", "no version matches, so it will be fetched");

        // Deleting something the server has waits too, and is hidden.
        store.delete_local(id).unwrap();
        assert_eq!(store.occurrences(0, 100 * DAY).unwrap(), []);
        assert!(store.pending().unwrap()[0].delete);
        // Deleting something never sent just removes it; giving up on
        // something never sent does too.
        let fresh = store.save_local(cal, None, &event("x", "Draft", DAY, 1)).unwrap();
        store.delete_local(fresh).unwrap();
        let other = store.save_local(cal, None, &event("y", "Draft 2", DAY, 1)).unwrap();
        store.give_up(other).unwrap();
        assert_eq!(store.known(cal).unwrap().len(), 1);
    }

    #[test]
    fn tasks_and_contacts_are_listed_and_searched() {
        let store = DavStore::open(temp(), "UTC").unwrap();
        store.set_collections(&[found("/c/", "Cal", CAL), found("/p/", "People", Kind::AddressBook)]).unwrap();
        let (cal, people) = (store.collections().unwrap()[0].id, store.collections().unwrap()[1].id);
        let task = |uid: &str, summary: &str, due: Option<i64>, completed| new_task(uid, &TaskEdit { summary: summary.into(), description: String::new(), due, completed }, 0);
        store.save_local(cal, None, &task("1", "No date", None, false)).unwrap();
        store.save_local(cal, None, &task("2", "Later", Some(20 * DAY), false)).unwrap();
        store.save_local(cal, None, &task("3", "Sooner", Some(5 * DAY), false)).unwrap();
        store.save_local(cal, None, &task("4", "Done long ago", Some(DAY), true)).unwrap();
        let order: Vec<String> = store.tasks().unwrap().into_iter().map(|t| t.summary).collect();
        assert_eq!(order, ["Sooner", "Later", "No date", "Done long ago"]);
        // Tasks are not events.
        assert_eq!(store.occurrences(0, 100 * DAY).unwrap(), []);

        let card = |uid: &str, name: &str, email: &str, org: &str| {
            write_card(None, uid, &ContactEdit { name: name.into(), emails: vec![email.into()], organisation: org.into(), ..ContactEdit::default() }).unwrap()
        };
        store.save_local(people, None, &card("a", "Dana Dubois", "dana@example.com", "Example Corp")).unwrap();
        store.save_local(people, None, &card("b", "Sam Okafor", "sam@elsewhere.example", "")).unwrap();
        store.save_local(people, None, &card("c", "alice abbott", "alice@example.com", "")).unwrap();
        let names = |q: &str| -> Vec<String> { store.contacts(q, 50).unwrap().into_iter().map(|c| c.name).collect() };
        assert_eq!(names(""), ["alice abbott", "Dana Dubois", "Sam Okafor"]);
        assert_eq!(names("EXAMPLE.com"), ["alice abbott", "Dana Dubois"]);
        assert_eq!(names("dana corp"), ["Dana Dubois"]);
        assert_eq!(names("nobody"), Vec::<String>::new());
        assert_eq!(store.contacts("", 2).unwrap().len(), 2);
        assert_eq!(store.contacts("sam", 5).unwrap()[0].emails, ["sam@elsewhere.example"]);
    }
}
