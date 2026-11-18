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
const SCHEMA_VERSION: i32 = 11;

const TABLES: &str = r#"
CREATE TABLE folder (
    id            INTEGER PRIMARY KEY,
    -- Directory under the mail root: encoded segments joined by '/'.
    path          TEXT NOT NULL UNIQUE,
    -- Display name of the last segment.
    name          TEXT NOT NULL,
    total         INTEGER NOT NULL DEFAULT 0,
    unread        INTEGER NOT NULL DEFAULT 0,
    -- What the server calls the folder, byte for byte. NULL for a folder
    -- that exists only on this machine.
    remote_name   BLOB,
    delimiter     TEXT,
    -- 0 for a name that only holds other folders.
    selectable    INTEGER NOT NULL DEFAULT 1,
    special_use   TEXT,
    -- Sync state. See store/sync_state.rs.
    uidvalidity   INTEGER NOT NULL DEFAULT 0,
    uidnext       INTEGER NOT NULL DEFAULT 0,
    highestmodseq INTEGER NOT NULL DEFAULT 0,
    -- 1 once every message's list data has been fetched at least once.
    headers_done  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE message (
    id             INTEGER PRIMARY KEY,
    folder_id      INTEGER NOT NULL REFERENCES folder(id) ON DELETE CASCADE,
    -- File name without the flag section. Never changes once set. Empty
    -- while only the message's list data has been fetched from the server.
    base           TEXT NOT NULL,
    -- The flags currently in the file's name. With `base`, locates the file.
    file_flags     INTEGER NOT NULL,
    -- The flags the user sees. The authority while the app runs; file names
    -- are brought into line in the background.
    flags          INTEGER NOT NULL,
    uid            INTEGER,
    modseq         INTEGER,
    date           INTEGER NOT NULL,
    -- Orders messages newest-first: the date in the high bits and a
    -- tie-breaker in the low bits (see `sort_key`). Unique. It is also the
    -- message's rowid in message_fts, which is what lets search return its
    -- newest matches first without sorting them.
    sort_key       INTEGER NOT NULL,
    size           INTEGER NOT NULL,
    has_attachment INTEGER NOT NULL,
    from_name      TEXT NOT NULL,
    from_addr      TEXT NOT NULL,
    recipients     TEXT NOT NULL,
    subject        TEXT NOT NULL,
    message_id     TEXT NOT NULL,
    in_reply_to    TEXT NOT NULL,
    refs           TEXT NOT NULL,
    -- Conversations. See store/thread.rs. A conversation is named by the
    -- sort key of one of its messages.
    thread_id      INTEGER,
    -- The sort key of the message this one replies to, if it is here.
    parent_id      INTEGER,
    -- 1 if that link was inferred from the subject rather than headers.
    link_guessed   INTEGER NOT NULL DEFAULT 0,
    -- The subject without reply and forward prefixes, lower-cased.
    subject_key    TEXT NOT NULL DEFAULT '',
    -- 1 for the newest message of its conversation within its folder: the
    -- one that stands for the conversation in the folder's list. Kept up
    -- to date by triggers (see TRIGGERS).
    is_head        INTEGER NOT NULL DEFAULT 0,
    preview        TEXT NOT NULL,
    preview_len    INTEGER NOT NULL,
    -- What the topmost Authentication-Results header adds up to (a
    -- verify::State number), and the server that wrote it. The verdict
    -- counts only when that server is the one the account trusts.
    verdict        INTEGER NOT NULL DEFAULT 0,
    auth_server    TEXT NOT NULL DEFAULT ''
);

-- How many messages there are from each sender domain: the domains the
-- user hears from, against which a new sender's domain is compared for
-- lookalikes. Kept by triggers.
CREATE TABLE sender_domain (
    domain   TEXT PRIMARY KEY,
    messages INTEGER NOT NULL
) WITHOUT ROWID;

-- Messages that failed their sender checks and that the user has chosen
-- to trust anyway, by Message-ID.
CREATE TABLE trusted_message (
    message_id TEXT PRIMARY KEY
) WITHOUT ROWID;

-- Which Message-IDs each message cites (its In-Reply-To and References),
-- so that a message arriving after its replies can find them.
CREATE TABLE reference_to (
    ref     TEXT NOT NULL,
    message INTEGER NOT NULL
);
CREATE INDEX reference_ref ON reference_to(ref);
CREATE INDEX reference_message ON reference_to(message);

-- What the user has done that the server has not been told yet, in order.
-- See store/actions.rs.
CREATE TABLE action (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    payload    TEXT NOT NULL,
    -- Unix time before which the action is not to be tried: the pause
    -- before a message is sent, and the wait after a failed attempt.
    not_before INTEGER NOT NULL DEFAULT 0,
    attempts   INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
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
    ("message_list", "CREATE INDEX IF NOT EXISTS message_list ON message(folder_id, sort_key DESC, preview_len)"),
    ("message_sort", "CREATE UNIQUE INDEX IF NOT EXISTS message_sort ON message(sort_key)"),
    ("message_base", "CREATE UNIQUE INDEX IF NOT EXISTS message_base ON message(folder_id, base) WHERE base != ''"),
    ("message_uid", "CREATE UNIQUE INDEX IF NOT EXISTS message_uid ON message(folder_id, uid) WHERE uid IS NOT NULL"),
    ("message_msgid", "CREATE INDEX IF NOT EXISTS message_msgid ON message(message_id) WHERE message_id != ''"),
    ("message_unread", "CREATE INDEX IF NOT EXISTS message_unread ON message(folder_id) WHERE (flags & 1) = 0"),
    ("message_rename", "CREATE INDEX IF NOT EXISTS message_rename ON message(id) WHERE flags != file_flags AND base != ''"),
    // Serves the conversation list exactly as message_list serves the
    // message list: one entry per conversation in the folder.
    ("message_heads", "CREATE INDEX IF NOT EXISTS message_heads ON message(folder_id, sort_key DESC, preview_len) WHERE is_head = 1"),
    ("message_folder_thread", "CREATE INDEX IF NOT EXISTS message_folder_thread ON message(folder_id, thread_id, sort_key)"),
    ("message_thread", "CREATE INDEX IF NOT EXISTS message_thread ON message(thread_id)"),
    ("message_subject", "CREATE INDEX IF NOT EXISTS message_subject ON message(subject_key, date) WHERE subject_key != ''"),
    // Finds earlier mail under the same display name.
    ("message_from_name", "CREATE INDEX IF NOT EXISTS message_from_name ON message(from_name COLLATE NOCASE, date) WHERE from_name != ''"),
    ("message_nobody", "CREATE INDEX IF NOT EXISTS message_nobody ON message(folder_id, sort_key DESC) WHERE base = ''"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub total: u32,
    pub unread: u32,
    /// What the server says the folder is for (`sent`, `trash`, …), if it says.
    pub special_use: Option<String>,
}

/// Keeps `is_head` true for exactly the newest message of each conversation
/// in each folder, whatever happens to a message: it arrives, is linked into
/// a conversation, moves to another folder, or goes. Dropped during a bulk
/// load, which sets every head at once at the end.
const TRIGGERS: &[(&str, &str)] = &[
    (
        "message_moved",
        "CREATE TRIGGER IF NOT EXISTS message_moved AFTER UPDATE OF thread_id, folder_id ON message BEGIN
            UPDATE message SET is_head = (sort_key = (SELECT max(sort_key) FROM message
                WHERE folder_id = OLD.folder_id AND thread_id IS OLD.thread_id))
              WHERE folder_id = OLD.folder_id AND thread_id IS OLD.thread_id;
            UPDATE message SET is_head = (sort_key = (SELECT max(sort_key) FROM message
                WHERE folder_id = NEW.folder_id AND thread_id IS NEW.thread_id))
              WHERE folder_id = NEW.folder_id AND thread_id IS NEW.thread_id;
         END",
    ),
    (
        "message_added",
        "CREATE TRIGGER IF NOT EXISTS message_added AFTER INSERT ON message WHEN instr(NEW.from_addr, '@') > 0 BEGIN
            INSERT OR IGNORE INTO sender_domain (domain, messages)
              VALUES (lower(substr(NEW.from_addr, instr(NEW.from_addr, '@') + 1)), 0);
            UPDATE sender_domain SET messages = messages + 1
              WHERE domain = lower(substr(NEW.from_addr, instr(NEW.from_addr, '@') + 1));
         END",
    ),
    (
        "message_gone",
        "CREATE TRIGGER IF NOT EXISTS message_gone AFTER DELETE ON message BEGIN
            DELETE FROM reference_to WHERE message = OLD.sort_key;
            UPDATE sender_domain SET messages = messages - 1
              WHERE domain = lower(substr(OLD.from_addr, instr(OLD.from_addr, '@') + 1));
            UPDATE message SET is_head = (sort_key = (SELECT max(sort_key) FROM message
                WHERE folder_id = OLD.folder_id AND thread_id IS OLD.thread_id))
              WHERE folder_id = OLD.folder_id AND thread_id IS OLD.thread_id;
         END",
    ),
];

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
    /// A `verify::State` number, as recorded by `auth_server`.
    pub verdict: u8,
    pub auth_server: String,
}

const LIST_COLUMNS: &str = "m.id, m.folder_id, m.from_name, m.from_addr, m.recipients, m.date, \
                            m.subject, m.preview, m.flags, m.has_attachment, m.verdict, m.auth_server";

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
        verdict: row.get(10)?,
        auth_server: row.get(11)?,
    })
}

/// What earlier mail says about a message's sender.
#[derive(Debug, Clone, PartialEq)]
pub struct SenderHistory {
    /// An earlier message was from, or to, this address.
    pub known_address: bool,
    /// Another address that earlier mail under the same display name came
    /// from.
    pub name_used_by: Option<String>,
    /// The sender's domain, and how many messages here are from it.
    pub domain: String,
    pub domain_messages: u32,
    /// Every sender domain and its message count.
    pub domains: Vec<(String, u32)>,
}

/// A conversation as a folder's list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationRow {
    /// Its newest message in the folder.
    pub head: ListRow,
    /// How many messages the whole conversation has, in every folder.
    pub count: u32,
    /// How many of them are unread.
    pub unread: u32,
    /// Who wrote in it, earliest first: (name, address).
    pub senders: Vec<(String, String)>,
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
    /// The file held an index from an older version, which was discarded.
    discarded_old: bool,
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
        let discarded_old = migrate(&writer)?;
        Ok(Index {
            path,
            discarded_old,
            writer: Mutex::new(writer),
            readers: Mutex::new(Vec::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True if opening found an index from an older version and replaced
    /// it with an empty one. The mail files are untouched; a rebuild
    /// restores everything they hold.
    pub fn discarded_old(&self) -> bool {
        self.discarded_old
    }

    pub(super) fn writer(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on a read-only connection from the pool.
    pub(super) fn read<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
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
            conn.prepare_cached("SELECT id, path, name, total, unread, special_use FROM folder ORDER BY path")?
                .query_map([], |r| {
                    Ok(FolderRow {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        name: r.get(2)?,
                        total: r.get(3)?,
                        unread: r.get(4)?,
                        special_use: r.get(5)?,
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

    /// How many messages a folder holds. 0 for a folder that does not exist.
    pub fn folder_total(&self, folder_id: i64) -> Result<u32> {
        self.read(|conn| {
            conn.prepare_cached("SELECT total FROM folder WHERE id = ?1")?
                .query_row([folder_id], |r| r.get(0))
                .optional()
        })
        .map(Option::unwrap_or_default)
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
        let mut tie: u64 = tx.query_row("SELECT coalesce(max(id), 0) + 1 FROM message", [], |r| {
            r.get::<_, i64>(0)
        })? as u64;
        for message in messages {
            let id = insert_unique(&tx, "main.message", folder_id, message, sort_key(message.date, tie))?;
            super::thread::link(&tx, id)?;
            ids.push(id);
            tie += 1;
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
                 WHERE m.flags != m.file_flags AND m.base != '' ORDER BY m.id LIMIT ?1",
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
                 ORDER BY m.sort_key DESC LIMIT ?2 OFFSET ?3"
            ))?
            .query_map(params![folder_id, limit, offset], list_row)?
            .collect()
        })
    }

    /// Where a message is in its folder's list (0 is the top), or `None` if
    /// it is not in that folder.
    pub fn position(&self, folder_id: i64, id: i64) -> Result<Option<u32>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT (SELECT count(*) FROM message WHERE folder_id = ?1 AND sort_key > m.sort_key)
                 FROM message m WHERE m.id = ?2 AND m.folder_id = ?1",
            )?
            .query_row(params![folder_id, id], |r| r.get(0))
            .optional()
        })
    }

    // ---- conversations ----------------------------------------------------

    /// The preview length of the message standing for each conversation in
    /// a folder, in list order.
    pub fn conversation_lens(&self, folder_id: i64) -> Result<Vec<u16>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT preview_len FROM message WHERE folder_id = ?1 AND is_head = 1 ORDER BY sort_key DESC",
            )?
            .query_map([folder_id], |r| r.get(0))?
            .collect()
        })
    }

    /// How many conversations a folder holds.
    pub fn conversation_total(&self, folder_id: i64) -> Result<u32> {
        self.read(|conn| {
            conn.prepare_cached("SELECT count(*) FROM message WHERE folder_id = ?1 AND is_head = 1")?
                .query_row([folder_id], |r| r.get(0))
        })
    }

    /// One page of a folder's conversations, newest first: for each, its
    /// newest message in the folder, with what the list shows about the
    /// conversation as a whole.
    pub fn conversation_page(&self, folder_id: i64, offset: u32, limit: u32) -> Result<Vec<ConversationRow>> {
        self.read(|conn| {
            let heads: Vec<(ListRow, Option<i64>)> = conn
                .prepare_cached(&format!(
                    "SELECT {LIST_COLUMNS}, m.thread_id FROM message m
                     WHERE m.folder_id = ?1 AND m.is_head = 1
                     ORDER BY m.sort_key DESC LIMIT ?2 OFFSET ?3"
                ))?
                .query_map(params![folder_id, limit, offset], |r| Ok((list_row(r)?, r.get(12)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let mut counts = conn.prepare_cached(
                // A copy of a message kept in another folder counts once.
                "SELECT count(DISTINCT CASE WHEN message_id = '' THEN id ELSE message_id END),
                        count(DISTINCT CASE WHEN (flags & 1) = 0 THEN
                              CASE WHEN message_id = '' THEN id ELSE message_id END END)
                 FROM message WHERE thread_id = ?1",
            )?;
            let mut people = conn.prepare_cached(
                "SELECT from_name, from_addr FROM message WHERE thread_id = ?1 ORDER BY date, id LIMIT 60",
            )?;
            let mut out = Vec::with_capacity(heads.len());
            for (head, thread) in heads {
                let Some(thread) = thread else {
                    out.push(ConversationRow { senders: vec![(head.from_name.clone(), head.from_addr.clone())], head, count: 1, unread: 0 });
                    continue;
                };
                let (count, unread): (u32, u32) = counts.query_row([thread], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let mut senders: Vec<(String, String)> = Vec::new();
                if count > 1 {
                    let all = people.query_map([thread], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                    for sender in all {
                        let sender = sender?;
                        if !senders.iter().any(|s| s.1.eq_ignore_ascii_case(&sender.1)) {
                            senders.push(sender);
                        }
                    }
                } else {
                    senders.push((head.from_name.clone(), head.from_addr.clone()));
                }
                out.push(ConversationRow { head, count, unread, senders });
            }
            Ok(out)
        })
    }

    /// Where the conversation a message belongs to is in its folder's
    /// conversation list (0 is the top).
    pub fn conversation_position(&self, folder_id: i64, id: i64) -> Result<Option<u32>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT (SELECT count(*) FROM message WHERE folder_id = ?1 AND is_head = 1 AND sort_key > h.sort_key)
                 FROM message m JOIN message h
                   ON h.folder_id = ?1 AND h.thread_id IS m.thread_id AND h.is_head = 1
                 WHERE m.id = ?2 LIMIT 1",
            )?
            .query_row(params![folder_id, id], |r| r.get(0))
            .optional()
        })
    }

    /// Whether the user has chosen to trust this message despite its
    /// failed checks.
    pub fn is_trusted(&self, id: i64) -> Result<bool> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM trusted_message t JOIN message m ON m.message_id = t.message_id
                                WHERE m.id = ?1 AND m.message_id != '')",
            )?
            .query_row([id], |r| r.get(0))
        })
    }

    /// Records, or withdraws, the user's trust in a message. False if the
    /// message cannot be marked (it is gone, or has no Message-ID).
    pub fn set_trusted(&self, id: i64, trusted: bool) -> Result<bool> {
        let conn = self.writer();
        let sql = if trusted {
            "INSERT OR IGNORE INTO trusted_message (message_id) SELECT message_id FROM message WHERE id = ?1 AND message_id != ''"
        } else {
            "DELETE FROM trusted_message WHERE message_id = (SELECT message_id FROM message WHERE id = ?1)"
        };
        conn.execute(sql, [id])?;
        let known: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM message WHERE id = ?1 AND message_id != '')",
            [id],
            |r| r.get(0),
        )?;
        Ok(known)
    }

    /// The highest message number in use. Messages added later get higher
    /// ones.
    pub fn newest_id(&self) -> Result<i64> {
        self.read(|conn| conn.query_row("SELECT coalesce(max(id), 0) FROM message", [], |r| r.get(0)))
    }

    /// Unread messages in a folder that were added after message number
    /// `after`: how many, and the newest one's sender and subject.
    pub fn unread_since(&self, folder_id: i64, after: i64) -> Result<(u32, String, String)> {
        self.read(|conn| {
            // The `+` keeps this on the primary key: only the new rows are
            // looked at, however large the folder.
            let rows: Vec<(String, String, String)> = conn
                .prepare_cached(
                    "SELECT from_name, from_addr, subject FROM message
                     WHERE id > ?2 AND +folder_id = ?1 AND (flags & 1) = 0 ORDER BY id DESC LIMIT 500",
                )?
                .query_map(params![folder_id, after], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let newest = rows.first().cloned().unwrap_or_default();
            Ok((rows.len() as u32, if newest.0.is_empty() { newest.1 } else { newest.0 }, newest.2))
        })
    }

    /// The newest messages in a folder from any of these addresses.
    pub fn recent_from(&self, folder_id: i64, addresses: &[String], newest: u32) -> Result<Vec<i64>> {
        if addresses.is_empty() {
            return Ok(Vec::new());
        }
        self.read(|conn| {
            let rows: Vec<(i64, String)> = conn
                .prepare_cached("SELECT id, from_addr FROM message WHERE folder_id = ?1 ORDER BY sort_key DESC LIMIT ?2")?
                .query_map(params![folder_id, newest], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows
                .into_iter()
                .filter(|(_, from)| addresses.iter().any(|a| a.eq_ignore_ascii_case(from)))
                .map(|(id, _)| id)
                .collect())
        })
    }

    /// What earlier mail says about a message's sender. "Earlier" is by
    /// date: mail dated before this message.
    pub fn sender_history(&self, id: i64) -> Result<Option<SenderHistory>> {
        self.read(|conn| {
            let found: Option<(String, String, i64)> = conn
                .prepare_cached("SELECT from_name, from_addr, date FROM message WHERE id = ?1")?
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .optional()?;
            let Some((name, address, date)) = found else { return Ok(None) };
            let domain = crate::verify::domain_of(&address);

            // Known if any earlier message has this address among its
            // sender and recipients: mail from them, or mail to them.
            let words: Vec<String> = address
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(|w| w.to_string())
                .collect();
            let known_address = !words.is_empty()
                && conn
                    .prepare_cached("SELECT 1 FROM message_fts WHERE message_fts MATCH ?1 AND rowid < ?2 LIMIT 1")?
                    .query_row(params![format!("participants : \"{}\"", words.join(" ")), sort_key(date, 0)], |_| Ok(()))
                    .optional()?
                    .is_some();

            // The address that most earlier mail under this name came from.
            let name_used_by: Option<String> = if name.trim().is_empty() || name.eq_ignore_ascii_case(&address) {
                None
            } else {
                conn.prepare_cached(
                    "SELECT from_addr FROM
                       (SELECT from_addr FROM message
                        WHERE from_name = ?1 COLLATE NOCASE AND date < ?2 AND from_name != ''
                        ORDER BY date DESC LIMIT 200)
                     WHERE from_addr != ?3 COLLATE NOCASE
                     GROUP BY lower(from_addr) ORDER BY count(*) DESC LIMIT 1",
                )?
                .query_row(params![name, date, address], |r| r.get(0))
                .optional()?
            };

            let domains: Vec<(String, u32)> = conn
                .prepare_cached("SELECT domain, messages FROM sender_domain WHERE messages > 0")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let domain_messages = domains.iter().find(|d| d.0 == domain).map_or(0, |d| d.1);
            Ok(Some(SenderHistory { known_address, name_used_by, domain, domain_messages, domains }))
        })
    }

    /// The server that wrote the sender checks on most of the newest mail:
    /// the account's own receiving server, in all likelihood. `None` unless
    /// one name clearly leads.
    pub fn likely_checking_server(&self) -> Result<Option<String>> {
        self.read(|conn| {
            let counts: Vec<(String, u32)> = conn
                .prepare_cached(
                    "SELECT auth_server, count(*) FROM
                       (SELECT auth_server FROM message ORDER BY sort_key DESC LIMIT 500)
                     WHERE auth_server != '' GROUP BY 1 ORDER BY 2 DESC",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let all: u32 = counts.iter().map(|c| c.1).sum();
            Ok(counts.into_iter().next().filter(|(_, n)| *n >= 3 && *n * 5 >= all * 3).map(|(name, _)| name))
        })
    }

    /// The preview length of every message in a folder, in list order.
    pub fn preview_lens(&self, folder_id: i64) -> Result<Vec<u16>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT preview_len FROM message WHERE folder_id = ?1 ORDER BY sort_key DESC",
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
        .and_then(|at: Location| {
            if at.base.is_empty() {
                Err(StoreError::NotDownloaded(id))
            } else {
                Ok(at)
            }
        })
    }

    /// Full-text search, newest first. `folder_id` narrows it to one folder.
    /// Every word must match; the last word matches as a prefix, so results
    /// can be shown while the user is still typing.
    pub fn search(&self, text: &str, folder_id: Option<i64>, limit: u32) -> Result<Vec<ListRow>> {
        let Some(query) = fts_query(text) else {
            return Ok(Vec::new());
        };
        // The search index's rowids are sort keys, so walking it backwards
        // yields matches newest first and stops after `limit` of them,
        // however many there are in total. CROSS JOIN fixes that join order.
        self.read(|conn| match folder_id {
            Some(folder_id) => conn
                .prepare_cached(&format!(
                    "SELECT {LIST_COLUMNS}
                     FROM message_fts CROSS JOIN message m ON m.sort_key = message_fts.rowid
                     WHERE message_fts MATCH ?1 AND m.folder_id = ?2
                     ORDER BY message_fts.rowid DESC LIMIT ?3"
                ))?
                .query_map(params![query, folder_id, limit], list_row)?
                .collect(),
            None => conn
                .prepare_cached(&format!(
                    "SELECT {LIST_COLUMNS}
                     FROM message_fts CROSS JOIN message m ON m.sort_key = message_fts.rowid
                     WHERE message_fts MATCH ?1
                     ORDER BY message_fts.rowid DESC LIMIT ?2"
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
        for (name, _) in TRIGGERS {
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}"))?;
        }
        // Rows are loaded into a staging table in a scratch file, in whatever
        // order they arrive, and copied into place in list order at the end.
        let staging = staging_path(&self.path);
        let _ = std::fs::remove_file(&staging);
        conn.execute("ATTACH DATABASE ?1 AS load", [staging.to_string_lossy()])?;
        conn.execute_batch(&format!(
            "PRAGMA load.journal_mode = OFF; PRAGMA load.synchronous = OFF;
             CREATE TABLE load.message AS SELECT {MESSAGE_COLUMNS} FROM main.message WHERE 0;
             DELETE FROM main.message; DELETE FROM folder; DELETE FROM reference_to;
             DELETE FROM sender_domain;
             INSERT INTO message_fts(message_fts) VALUES('delete-all');
             BEGIN"
        ))?;
        Ok(Bulk { conn, staging, in_batch: 0, count: 0, finished: false })
    }
}

/// A bulk load in progress. See [`Index::begin_bulk`].
pub(super) struct Bulk<'a> {
    conn: MutexGuard<'a, Connection>,
    staging: PathBuf,
    in_batch: u32,
    /// Messages added so far. Also the next sort key's tie-breaker.
    count: u64,
    finished: bool,
}

impl Drop for Bulk<'_> {
    /// A load that was abandoned (an error part-way) must not leave the
    /// shared writer inside a transaction or without its indexes. What is
    /// left is a valid, emptier index; rebuilding again completes it.
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.conn.execute_batch("ROLLBACK");
        let _ = self.conn.execute_batch("DETACH DATABASE load");
        let _ = std::fs::remove_file(&self.staging);
        for (_, sql) in INDEXES.iter().chain(TRIGGERS) {
            let _ = self.conn.execute_batch(sql);
        }
    }
}

impl Bulk<'_> {
    const BATCH: u32 = 20_000;

    /// `uid_validity` is what the folder's marker file says, or 0.
    pub fn add_folder(&self, path: &str, name: &str, uid_validity: u32) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO folder (path, name, uidvalidity) VALUES (?1, ?2, ?3)",
            params![path, name, uid_validity],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add(&mut self, folder_id: i64, message: &NewMessage) -> Result<()> {
        // The first 2^TIE_BITS messages all get different tie-breakers, so
        // their sort keys cannot collide and need no checking. A load bigger
        // than that turns the uniqueness index on and checks from then on.
        let key = sort_key(message.date, self.count);
        if self.count < 1 << TIE_BITS {
            insert_one(&self.conn, "load.message", folder_id, message, key)?;
        } else {
            if self.count == 1 << TIE_BITS {
                self.conn
                    .execute_batch("CREATE UNIQUE INDEX load.message_sort ON message(sort_key)")?;
            }
            insert_unique(&self.conn, "load.message", folder_id, message, key)?;
        }
        self.count += 1;
        self.in_batch += 1;
        if self.in_batch >= Self::BATCH {
            self.conn.execute_batch("COMMIT; BEGIN")?;
            self.in_batch = 0;
        }
        Ok(())
    }

    /// Commits, rebuilds the indexes and tidies up.
    pub fn finish(mut self, optimize_search: bool) -> Result<()> {
        // Copy into place folder by folder, newest last, so that the rows of
        // any one page of the list sit next to each other in the file. A cold
        // open of a folder then reads a few adjacent pages instead of one
        // scattered page per row.
        // Conversations first, in the staging table, so that every row is
        // complete before it is copied and nothing is rewritten afterwards
        // (rewriting rows in place would scatter them again).
        self.conn.execute_batch("COMMIT")?;
        super::thread::link_all(&self.conn, "load.message", |_, _| {})?;
        self.conn.execute_batch(
            "CREATE INDEX load.message_threads ON message(folder_id, thread_id, sort_key);
             UPDATE load.message SET is_head = 0;
             UPDATE load.message SET is_head = 1 WHERE sort_key IN
               (SELECT max(sort_key) FROM load.message GROUP BY folder_id, thread_id)",
        )?;
        self.conn.execute_batch(&format!(
            "CREATE INDEX load.message_order ON message(folder_id, sort_key);
             INSERT INTO main.message ({MESSAGE_COLUMNS})
               SELECT {MESSAGE_COLUMNS} FROM load.message INDEXED BY message_order
               ORDER BY folder_id, sort_key;
             DETACH DATABASE load"
        ))?;
        let _ = std::fs::remove_file(&self.staging);
        for (_, sql) in INDEXES {
            self.conn.execute_batch(sql)?;
        }
        self.conn.execute_batch(
            "INSERT INTO sender_domain (domain, messages)
               SELECT lower(substr(message.from_addr, instr(message.from_addr, '@') + 1)), count(*) FROM message
               WHERE instr(from_addr, '@') > 0 GROUP BY 1",
        )?;
        for (_, sql) in TRIGGERS {
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
        // A rebuild frees and reuses pages all over the file, which leaves
        // even freshly built indexes scattered and slow to read cold.
        // Rewriting the file lays every table and index out in one piece.
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM")?;
        self.conn.execute_batch("ANALYZE; PRAGMA wal_checkpoint(TRUNCATE)")?;
        self.finished = true;
        Ok(())
    }
}

/// Bits of a sort key given to the tie-breaker: room for eight million
/// messages dated the same second. Under test it is tiny, so that the
/// handling of collisions is exercised by ordinary-sized tests.
const TIE_BITS: u32 = if cfg!(test) { 4 } else { 23 };

/// A key that sorts by date, then by `tie`. Dates are clamped to what fits
/// (the years 1970 to 19391), so a nonsense Date header cannot overflow it.
fn sort_key(date: i64, tie: u64) -> i64 {
    (date.clamp(0, (1 << 39) - 1) << TIE_BITS) | (tie & ((1 << TIE_BITS) - 1)) as i64
}

/// Inserts a message, moving to the next free sort key if `key` is taken.
/// Needs the `message_sort` index to exist.
fn insert_unique(
    conn: &Connection,
    table: &str,
    folder_id: i64,
    m: &NewMessage,
    mut key: i64,
) -> rusqlite::Result<i64> {
    loop {
        match insert_one(conn, table, folder_id, m, key) {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::ConstraintViolation
                    && key_taken(conn, table, key)? =>
            {
                key += 1;
            }
            other => return other,
        }
    }
}

fn key_taken(conn: &Connection, table: &str, key: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached(&format!("SELECT 1 FROM {table} WHERE sort_key = ?1"))?
        .exists([key])
}

/// The columns of `message` other than `id`, which SQLite assigns.
const MESSAGE_COLUMNS: &str = "folder_id, base, file_flags, flags, uid, modseq, date, sort_key, \
     size, has_attachment, from_name, from_addr, recipients, subject, message_id, in_reply_to, \
     refs, thread_id, parent_id, link_guessed, subject_key, is_head, preview, preview_len, verdict, auth_server";

fn staging_path(index: &Path) -> PathBuf {
    let mut name = index.file_name().unwrap_or_default().to_os_string();
    name.push(".rebuild");
    index.with_file_name(name)
}

/// Inserts into `table`, which is `main.message` or the bulk loader's
/// staging copy of it.
fn insert_one(
    conn: &Connection,
    table: &str,
    folder_id: i64,
    m: &NewMessage,
    key: i64,
) -> rusqlite::Result<i64> {
    let p = &m.parsed;
    conn.prepare_cached(&format!(
        "INSERT INTO {table} (folder_id, base, file_flags, flags, uid, date, size, has_attachment,
                              from_name, from_addr, recipients, subject, message_id, in_reply_to,
                              refs, preview, preview_len, sort_key, verdict, auth_server)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"
    ))?
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
        key,
        p.verdict,
        p.auth_server,
    ])?;
    let id = conn.last_insert_rowid();
    conn.prepare_cached(
        "INSERT INTO message_fts (rowid, subject, participants, body) VALUES (?1, ?2, ?3, ?4)",
    )?
    .execute(params![
        key,
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
    // 64 MB of page cache per connection. Memory-mapped reads are left off
    // on purpose: measured on a million messages they made no difference
    // warm and made the first, cold read of a folder five to ten times
    // slower (docs/performance-results.md).
    conn.pragma_update(None, "cache_size", -65_536)?;
    conn.set_prepared_statement_cache_capacity(32);
    Ok(())
}

/// Brings the file to the current schema. Returns true if that meant
/// throwing away an older index.
fn migrate(conn: &Connection) -> Result<bool> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    match version {
        0 => {
            conn.execute_batch("BEGIN")?;
            conn.execute_batch(TABLES)?;
            for (_, sql) in INDEXES.iter().chain(TRIGGERS) {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            conn.execute_batch("COMMIT")?;
            Ok(false)
        }
        SCHEMA_VERSION => Ok(false),
        // An index from an older Outpost. It holds nothing that cannot be
        // rebuilt, so replace it with an empty one of the current shape.
        older if older < SCHEMA_VERSION => {
            conn.execute_batch(
                "DROP TABLE IF EXISTS message_fts; DROP TABLE IF EXISTS message;
                 DROP TABLE IF EXISTS folder; DROP TABLE IF EXISTS action;
                 DROP TABLE IF EXISTS reference_to; DROP TABLE IF EXISTS sender_domain;
                 DROP TABLE IF EXISTS trusted_message;
                 PRAGMA user_version = 0",
            )?;
            migrate(conn)?;
            Ok(true)
        }
        newer => Err(StoreError::IndexVersion(newer)),
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
    fn sort_keys_order_by_date_then_tie_and_never_overflow() {
        assert!(sort_key(100, 5) < sort_key(101, 0));
        assert!(sort_key(100, 5) < sort_key(100, 6));
        assert_eq!(sort_key(-50, 1), sort_key(0, 1), "dates before 1970 clamp to 1970");
        assert!(sort_key(i64::MAX, u64::MAX) > sort_key(4_000_000_000, 0));
        assert!(sort_key(i64::MAX, u64::MAX) > 0);
        // The tie-breaker wraps instead of spilling into the date.
        assert_eq!(sort_key(7, 1 << TIE_BITS), sort_key(7, 0));
    }

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
