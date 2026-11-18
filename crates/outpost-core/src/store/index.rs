//! The SQLite index for one account.
//!
//! It answers every question the interface asks without touching a mail file,
//! and can be rebuilt from the mail files at any time (see `rebuild.rs`).
//!
//! One connection writes; a small pool of others read. In write-ahead mode
//! readers never block the writer or each other.

use super::maildir::Flags;
use super::parse::Parsed;
use super::StoreError;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

type Result<T> = std::result::Result<T, StoreError>;

/// Bump when the schema changes, and add a step to `migrate`.
const SCHEMA_VERSION: i32 = 1;

const TABLES: &str = r#"
CREATE TABLE folder (
    id            INTEGER PRIMARY KEY,
    -- Directory under the mail root: encoded segments joined by '/'.
    path          TEXT NOT NULL UNIQUE,
    -- Display name of the last segment.
    name          TEXT NOT NULL,
    total         INTEGER NOT NULL DEFAULT 0,
    unread        INTEGER NOT NULL DEFAULT 0,
    -- Sync state, filled in by the sync engine (milestone 2).
    uidvalidity   INTEGER,
    uidnext       INTEGER,
    highestmodseq INTEGER,
    special_use   TEXT
);

CREATE TABLE message (
    id             INTEGER PRIMARY KEY,
    folder_id      INTEGER NOT NULL REFERENCES folder(id) ON DELETE CASCADE,
    -- File name without the flag section. Never changes.
    base           TEXT NOT NULL,
    -- The flags currently in the file's name. With `base`, locates the file.
    file_flags     INTEGER NOT NULL,
    -- The flags the user sees. The authority while the app runs; file names
    -- are brought into line in the background.
    flags          INTEGER NOT NULL,
    uid            INTEGER,
    modseq         INTEGER,
    date           INTEGER NOT NULL,
    size           INTEGER NOT NULL,
    has_attachment INTEGER NOT NULL,
    from_name      TEXT NOT NULL,
    from_addr      TEXT NOT NULL,
    recipients     TEXT NOT NULL,
    subject        TEXT NOT NULL,
    message_id     TEXT NOT NULL,
    in_reply_to    TEXT NOT NULL,
    refs           TEXT NOT NULL,
    thread_id      INTEGER,
    preview        TEXT NOT NULL,
    preview_len    INTEGER NOT NULL,
    -- Sender-check verdict (milestone 6). 0 = not checked.
    verdict        INTEGER NOT NULL DEFAULT 0
);

-- Contentless: the words are indexed, the text itself is not stored again.
CREATE VIRTUAL TABLE message_fts USING fts5(
    subject, participants, body,
    content = '',
    contentless_delete = 1,
    tokenize = "unicode61 remove_diacritics 2"
);
"#;

/// Kept apart from the tables so a rebuild can load rows first and build
/// these afterwards, which is several times faster.
const INDEXES: &[(&str, &str)] = &[
    // Serves the message list. Includes preview_len so that laying out a
    // whole folder reads this index only.
    ("message_list", "CREATE INDEX message_list ON message(folder_id, date DESC, id DESC, preview_len)"),
    ("message_base", "CREATE UNIQUE INDEX message_base ON message(folder_id, base)"),
    ("message_uid", "CREATE UNIQUE INDEX message_uid ON message(folder_id, uid) WHERE uid IS NOT NULL"),
    ("message_msgid", "CREATE INDEX message_msgid ON message(message_id) WHERE message_id != ''"),
    ("message_unread", "CREATE INDEX message_unread ON message(folder_id) WHERE (flags & 1) = 0"),
    ("message_rename", "CREATE INDEX message_rename ON message(id) WHERE flags != file_flags"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub total: u32,
    pub unread: u32,
}

/// A message to add to the index.
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub base: String,
    /// The flags in the file's name right now.
    pub file_flags: Flags,
    pub flags: Flags,
    pub uid: Option<u32>,
    /// Sort date, unix seconds.
    pub date: i64,
    pub size: u64,
    pub parsed: Parsed,
}

/// What a row of the message list shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ListRow {
    pub id: i64,
    pub folder_id: i64,
    pub from_name: String,
    pub from_addr: String,
    pub recipients: String,
    pub date: i64,
    pub subject: String,
    pub preview: String,
    pub flags: Flags,
    pub has_attachment: bool,
}

const LIST_COLUMNS: &str = "m.id, m.folder_id, m.from_name, m.from_addr, m.recipients, m.date, \
                            m.subject, m.preview, m.flags, m.has_attachment";

fn list_row(row: &rusqlite::Row) -> rusqlite::Result<ListRow> {
    Ok(ListRow {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        from_name: row.get(2)?,
        from_addr: row.get(3)?,
        recipients: row.get(4)?,
        date: row.get(5)?,
        subject: row.get(6)?,
        preview: row.get(7)?,
        flags: Flags(row.get(8)?),
        has_attachment: row.get(9)?,
    })
}

/// Where a message's file is.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub folder_path: String,
    pub base: String,
    pub file_flags: Flags,
}

/// A file whose name does not yet show its message's current flags.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRename {
    pub id: i64,
    pub folder_path: String,
    pub base: String,
    pub from: Flags,
    pub to: Flags,
}

pub struct Index {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
}

impl Index {
    /// Opens the index at `path`, creating or upgrading it as needed.
    pub fn open(path: impl Into<PathBuf>) -> Result<Index> {
        let path = path.into();
        let writer = Connection::open(&path)?;
        configure(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "synchronous", "NORMAL")?;
        migrate(&writer)?;
        Ok(Index {
            path,
            writer: Mutex::new(writer),
            readers: Mutex::new(Vec::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on a read-only connection from the pool.
    fn read<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let pooled = self.readers.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let conn = match pooled {
            Some(conn) => conn,
            None => {
                let conn = Connection::open_with_flags(
                    &self.path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                configure(&conn)?;
                conn
            }
        };
        let result = f(&conn);
        self.readers.lock().unwrap_or_else(|e| e.into_inner()).push(conn);
        Ok(result?)
    }

    // ---- folders ----------------------------------------------------------

    /// Returns the folder's id, adding the folder if it is new.
    pub fn ensure_folder(&self, path: &str, name: &str) -> Result<i64> {
        let conn = self.writer();
        conn.execute(
            "INSERT INTO folder (path, name) VALUES (?1, ?2) ON CONFLICT(path) DO NOTHING",
            params![path, name],
        )?;
        Ok(conn.query_row("SELECT id FROM folder WHERE path = ?1", [path], |r| r.get(0))?)
    }

    /// All folders, parents before children.
    pub fn folders(&self) -> Result<Vec<FolderRow>> {
        self.read(|conn| {
            conn.prepare_cached("SELECT id, path, name, total, unread FROM folder ORDER BY path")?
                .query_map([], |r| {
                    Ok(FolderRow {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        name: r.get(2)?,
                        total: r.get(3)?,
                        unread: r.get(4)?,
                    })
                })?
                .collect()
        })
    }

    pub fn folder_path(&self, folder_id: i64) -> Result<Option<String>> {
        self.read(|conn| {
            conn.query_row("SELECT path FROM folder WHERE id = ?1", [folder_id], |r| r.get(0))
                .optional()
        })
    }

    /// Recounts every folder's totals from its messages.
    pub fn refresh_counts(&self) -> Result<()> {
        self.writer().execute_batch(
            "UPDATE folder SET
               total  = (SELECT count(*) FROM message WHERE folder_id = folder.id),
               unread = (SELECT count(*) FROM message WHERE folder_id = folder.id AND (flags & 1) = 0)",
        )?;
        Ok(())
    }

    // ---- writing messages -------------------------------------------------

    /// Adds messages to a folder in one transaction. Returns their ids.
    pub fn insert(&self, folder_id: i64, messages: &[NewMessage]) -> Result<Vec<i64>> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let mut ids = Vec::with_capacity(messages.len());
        for message in messages {
            ids.push(insert_one(&tx, folder_id, message)?);
        }
        let unread = messages.iter().filter(|m| !m.flags.contains(Flags::SEEN)).count();
        tx.execute(
            "UPDATE folder SET total = total + ?1, unread = unread + ?2 WHERE id = ?3",
            params![messages.len() as i64, unread as i64, folder_id],
        )?;
        tx.commit()?;
        Ok(ids)
    }

    /// Changes a message's flags. The file is renamed later; see
    /// [`Index::pending_renames`].
    pub fn set_flags(&self, id: i64, flags: Flags) -> Result<()> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let (folder_id, old): (i64, u8) = tx
            .query_row("SELECT folder_id, flags FROM message WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?
            .ok_or(StoreError::NoSuchMessage(id))?;
        tx.execute("UPDATE message SET flags = ?1 WHERE id = ?2", params![flags.0, id])?;
        let was_unread = !Flags(old).contains(Flags::SEEN);
        let is_unread = !flags.contains(Flags::SEEN);
        if was_unread != is_unread {
            tx.execute(
                "UPDATE folder SET unread = unread + ?1 WHERE id = ?2",
                params![if is_unread { 1 } else { -1 }, folder_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Messages whose file name is behind their flags, oldest change first.
    pub fn pending_renames(&self, limit: u32) -> Result<Vec<PendingRename>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT m.id, f.path, m.base, m.file_flags, m.flags
                 FROM message m JOIN folder f ON f.id = m.folder_id
                 WHERE m.flags != m.file_flags ORDER BY m.id LIMIT ?1",
            )?
            .query_map([limit], |r| {
                Ok(PendingRename {
                    id: r.get(0)?,
                    folder_path: r.get(1)?,
                    base: r.get(2)?,
                    from: Flags(r.get(3)?),
                    to: Flags(r.get(4)?),
                })
            })?
            .collect()
        })
    }

    /// Records that a message's file now carries `file_flags`.
    pub fn renamed(&self, id: i64, file_flags: Flags) -> Result<()> {
        self.writer().execute(
            "UPDATE message SET file_flags = ?1 WHERE id = ?2",
            params![file_flags.0, id],
        )?;
        Ok(())
    }

    // ---- reading ----------------------------------------------------------

    /// One page of a folder's list, newest first.
    pub fn page(&self, folder_id: i64, offset: u32, limit: u32) -> Result<Vec<ListRow>> {
        self.read(|conn| {
            conn.prepare_cached(&format!(
                "SELECT {LIST_COLUMNS} FROM message m WHERE m.folder_id = ?1
                 ORDER BY m.date DESC, m.id DESC LIMIT ?2 OFFSET ?3"
            ))?
            .query_map(params![folder_id, limit, offset], list_row)?
            .collect()
        })
    }

    /// The preview length of every message in a folder, in list order.
    pub fn preview_lens(&self, folder_id: i64) -> Result<Vec<u16>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT preview_len FROM message WHERE folder_id = ?1 ORDER BY date DESC, id DESC",
            )?
            .query_map([folder_id], |r| r.get(0))?
            .collect()
        })
    }

    pub fn locate(&self, id: i64) -> Result<Location> {
        self.read(|conn| {
            conn.query_row(
                "SELECT f.path, m.base, m.file_flags
                 FROM message m JOIN folder f ON f.id = m.folder_id WHERE m.id = ?1",
                [id],
                |r| {
                    Ok(Location {
                        folder_path: r.get(0)?,
                        base: r.get(1)?,
                        file_flags: Flags(r.get(2)?),
                    })
                },
            )
            .optional()
        })?
        .ok_or(StoreError::NoSuchMessage(id))
    }

    /// Full-text search, newest first. `folder_id` narrows it to one folder.
    /// Every word must match; the last word matches as a prefix, so results
    /// can be shown while the user is still typing.
    pub fn search(&self, text: &str, folder_id: Option<i64>, limit: u32) -> Result<Vec<ListRow>> {
        let Some(query) = fts_query(text) else {
            return Ok(Vec::new());
        };
        self.read(|conn| match folder_id {
            Some(folder_id) => conn
                .prepare_cached(&format!(
                    "SELECT {LIST_COLUMNS} FROM message_fts JOIN message m ON m.id = message_fts.rowid
                     WHERE message_fts MATCH ?1 AND m.folder_id = ?2
                     ORDER BY m.date DESC LIMIT ?3"
                ))?
                .query_map(params![query, folder_id, limit], list_row)?
                .collect(),
            None => conn
                .prepare_cached(&format!(
                    "SELECT {LIST_COLUMNS} FROM message_fts JOIN message m ON m.id = message_fts.rowid
                     WHERE message_fts MATCH ?1
                     ORDER BY m.date DESC LIMIT ?2"
                ))?
                .query_map(params![query, limit], list_row)?
                .collect(),
        })
    }

    pub fn message_count(&self) -> Result<u64> {
        self.read(|conn| conn.query_row("SELECT count(*) FROM message", [], |r| r.get::<_, i64>(0)))
            .map(|n| n as u64)
    }

    // ---- rebuild support --------------------------------------------------

    /// Empties the index and prepares it for loading in bulk. Hold the
    /// returned writer for the whole load, then call [`Bulk::finish`].
    pub(super) fn begin_bulk(&self) -> Result<Bulk<'_>> {
        let conn = self.writer();
        for (name, _) in INDEXES {
            conn.execute_batch(&format!("DROP INDEX IF EXISTS {name}"))?;
        }
        conn.execute_batch(
            "DELETE FROM message; DELETE FROM folder;
             INSERT INTO message_fts(message_fts) VALUES('delete-all');
             BEGIN",
        )?;
        Ok(Bulk { conn, in_batch: 0 })
    }
}

/// A bulk load in progress. See [`Index::begin_bulk`].
pub(super) struct Bulk<'a> {
    conn: MutexGuard<'a, Connection>,
    in_batch: u32,
}

impl Bulk<'_> {
    const BATCH: u32 = 20_000;

    pub fn add_folder(&self, path: &str, name: &str) -> Result<i64> {
        self.conn
            .execute("INSERT INTO folder (path, name) VALUES (?1, ?2)", params![path, name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add(&mut self, folder_id: i64, message: &NewMessage) -> Result<()> {
        insert_one(&self.conn, folder_id, message)?;
        self.in_batch += 1;
        if self.in_batch >= Self::BATCH {
            self.conn.execute_batch("COMMIT; BEGIN")?;
            self.in_batch = 0;
        }
        Ok(())
    }

    /// Commits, rebuilds the indexes and tidies up.
    pub fn finish(self, optimize_search: bool) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        for (_, sql) in INDEXES {
            self.conn.execute_batch(sql)?;
        }
        self.conn.execute_batch(
            "UPDATE folder SET
               total  = (SELECT count(*) FROM message WHERE folder_id = folder.id),
               unread = (SELECT count(*) FROM message WHERE folder_id = folder.id AND (flags & 1) = 0)",
        )?;
        if optimize_search {
            self.conn
                .execute_batch("INSERT INTO message_fts(message_fts) VALUES('optimize')")?;
        }
        self.conn.execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(())
    }
}

fn insert_one(conn: &Connection, folder_id: i64, m: &NewMessage) -> rusqlite::Result<i64> {
    let p = &m.parsed;
    conn.prepare_cached(
        "INSERT INTO message (folder_id, base, file_flags, flags, uid, date, size, has_attachment,
                              from_name, from_addr, recipients, subject, message_id, in_reply_to,
                              refs, preview, preview_len)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )?
    .execute(params![
        folder_id,
        m.base,
        m.file_flags.0,
        m.flags.0,
        m.uid,
        m.date,
        m.size as i64,
        p.has_attachment,
        p.from_name,
        p.from_addr,
        p.recipients,
        p.subject,
        p.message_id,
        p.in_reply_to,
        p.references,
        p.preview,
        p.preview.chars().count().min(u16::MAX as usize) as i64,
    ])?;
    let id = conn.last_insert_rowid();
    conn.prepare_cached(
        "INSERT INTO message_fts (rowid, subject, participants, body) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        id,
        p.subject,
        format!("{} {} {}", p.from_name, p.from_addr, p.recipients),
        p.body,
    ])?;
    Ok(id)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // 64 MB of page cache per connection, and let the OS map up to 4 GB of
    // the file so warm reads skip a copy.
    conn.pragma_update(None, "cache_size", -65_536)?;
    conn.pragma_update(None, "mmap_size", 4_294_967_296_i64)?;
    conn.set_prepared_statement_cache_capacity(32);
    Ok(())
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    match version {
        0 => {
            conn.execute_batch("BEGIN")?;
            conn.execute_batch(TABLES)?;
            for (_, sql) in INDEXES {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        SCHEMA_VERSION => Ok(()),
        other => Err(StoreError::IndexVersion(other)),
    }
}

/// Turns what the user typed into an FTS5 query. Each word becomes a quoted
/// term, so nothing the user types is read as query syntax. `None` if there
/// is nothing to search for.
pub fn fts_query(text: &str) -> Option<String> {
    let still_typing = !text.ends_with(char::is_whitespace);
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.replace('"', ""))
        // A term with no letters or digits matches nothing and FTS5 rejects
        // a prefix query on it.
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    if words.is_empty() {
        return None;
    }
    let last = words.len() - 1;
    let terms: Vec<String> = words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i == last && still_typing {
                format!("\"{w}\"*")
            } else {
                format!("\"{w}\"")
            }
        })
        .collect();
    Some(terms.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_quoted_so_user_text_is_never_syntax() {
        assert_eq!(fts_query("roof repair").as_deref(), Some("\"roof\" \"repair\"*"));
        assert_eq!(fts_query("roof repair ").as_deref(), Some("\"roof\" \"repair\""));
        assert_eq!(fts_query("  ").as_deref(), None);
        assert_eq!(fts_query("").as_deref(), None);
        assert_eq!(fts_query("\"quoted\" OR NOT").as_deref(), Some("\"quoted\" \"OR\" \"NOT\"*"));
        assert_eq!(fts_query("a* -b (c)").as_deref(), Some("\"a*\" \"-b\" \"(c)\"*"));
        assert_eq!(fts_query("- * \"").as_deref(), None);
        assert_eq!(fts_query("subject:x").as_deref(), Some("\"subject:x\"*"));
    }
}
