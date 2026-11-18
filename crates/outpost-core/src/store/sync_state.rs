//! What the store keeps for the sync engine: which server folder each local
//! folder mirrors, how far each has been synced, and messages whose list data
//! has arrived but whose body has not.
//!
//! Everything here is local bookkeeping. Nothing in this file talks to a
//! server.

use super::index::{Index, NewMessage};
use super::maildir::{Flags, Maildir};
use super::parse::{self, Parsed};
use super::{Result, Store, StoreError};
use rusqlite::{params, OptionalExtension};

/// What a folder is for: from the server's marker if it gave one, otherwise
/// from the names mail servers and clients commonly use.
pub fn role_of(path: &str, name: &str, special_use: Option<&str>) -> Option<&'static str> {
    if path.eq_ignore_ascii_case("INBOX") {
        return Some("inbox");
    }
    if path == super::actions::OUTBOX {
        return Some("outbox");
    }
    match special_use {
        Some("sent") => return Some("sent"),
        Some("drafts") => return Some("drafts"),
        Some("trash") => return Some("trash"),
        Some("junk") => return Some("junk"),
        Some("archive") | Some("all") => return Some("archive"),
        _ => {}
    }
    match name.to_lowercase().as_str() {
        "sent" | "sent items" | "sent mail" | "sent messages" => Some("sent"),
        "drafts" | "draft" => Some("drafts"),
        "trash" | "deleted items" | "deleted messages" | "bin" => Some("trash"),
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => Some("junk"),
        "archive" | "archives" => Some("archive"),
        _ => None,
    }
}

/// A folder as the server describes it, ready to be recorded locally.
#[derive(Debug, Clone)]
pub struct RemoteFolderInfo<'a> {
    /// Directory under the mail root: encoded segments joined by '/'.
    pub path: &'a str,
    /// Display name of the last level.
    pub name: &'a str,
    pub remote_name: &'a [u8],
    pub delimiter: Option<char>,
    pub selectable: bool,
    pub special_use: Option<&'a str>,
}

/// How far a folder has been synced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncState {
    /// 0 means the folder has never been synced.
    pub uid_validity: u32,
    /// The server's UIDNEXT when the folder was last synced.
    pub uid_next: u32,
    /// The server's modification sequence then. 0 if the server keeps none.
    pub highest_modseq: u64,
    /// Whether list data for every message has been fetched at least once.
    /// Until then a resync compares complete UID lists.
    pub headers_done: bool,
}

/// A local folder that mirrors a server folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFolder {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub remote_name: Vec<u8>,
    pub selectable: bool,
    pub special_use: Option<String>,
    pub total: u32,
    pub state: SyncState,
}

/// A message known to the index in one folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownMessage {
    pub uid: u32,
    pub id: i64,
    pub flags: Flags,
    /// Size in bytes, as the server reported it.
    pub size: u64,
}

impl Index {
    /// Records a server folder, creating or updating the local folder at
    /// `path`. Sync state is kept if the folder was already known.
    pub fn upsert_remote_folder(&self, f: &RemoteFolderInfo<'_>) -> Result<i64> {
        let conn = self.writer();
        conn.execute(
            "INSERT INTO folder (path, name, remote_name, delimiter, selectable, special_use)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name, remote_name = excluded.remote_name,
               delimiter = excluded.delimiter, selectable = excluded.selectable,
               special_use = excluded.special_use",
            params![
                f.path,
                f.name,
                f.remote_name,
                f.delimiter.map(String::from),
                f.selectable,
                f.special_use
            ],
        )?;
        Ok(conn.query_row("SELECT id FROM folder WHERE path = ?1", [f.path], |r| r.get(0))?)
    }

    /// Every folder that mirrors a server folder, parents before children.
    pub fn remote_folders(&self) -> Result<Vec<SyncFolder>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT id, path, name, remote_name, selectable, special_use, total,
                        uidvalidity, uidnext, highestmodseq, headers_done
                 FROM folder WHERE remote_name IS NOT NULL ORDER BY path",
            )?
            .query_map([], |r| {
                Ok(SyncFolder {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    name: r.get(2)?,
                    remote_name: r.get(3)?,
                    selectable: r.get(4)?,
                    special_use: r.get(5)?,
                    total: r.get(6)?,
                    state: SyncState {
                        uid_validity: r.get(7)?,
                        uid_next: r.get(8)?,
                        highest_modseq: r.get::<_, i64>(9)? as u64,
                        headers_done: r.get(10)?,
                    },
                })
            })?
            .collect()
        })
    }

    /// The character the server puts between the levels of a folder name.
    pub fn delimiter(&self) -> Result<Option<char>> {
        self.read(|conn| {
            conn.query_row(
                "SELECT delimiter FROM folder WHERE delimiter IS NOT NULL AND delimiter != '' LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .map(|d| d.and_then(|d| d.chars().next()))
    }

    pub fn set_sync_state(&self, folder_id: i64, state: &SyncState) -> Result<()> {
        self.writer().execute(
            "UPDATE folder SET uidvalidity = ?1, uidnext = ?2, highestmodseq = ?3, headers_done = ?4
             WHERE id = ?5",
            params![
                state.uid_validity,
                state.uid_next,
                state.highest_modseq as i64,
                state.headers_done,
                folder_id
            ],
        )?;
        Ok(())
    }

    /// Every message in a folder that has a server UID.
    pub fn known_messages(&self, folder_id: i64) -> Result<Vec<KnownMessage>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT uid, id, flags, size FROM message WHERE folder_id = ?1 AND uid IS NOT NULL",
            )?
            .query_map([folder_id], |r| {
                Ok(KnownMessage {
                    uid: r.get(0)?,
                    id: r.get(1)?,
                    flags: Flags(r.get(2)?),
                    size: r.get::<_, i64>(3)? as u64,
                })
            })?
            .collect()
        })
    }

    /// Sets messages' flags to what the server says. Returns how many
    /// actually changed.
    pub fn apply_server_flags(&self, folder_id: i64, flags: &[(u32, Flags)]) -> Result<u32> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let mut changed = 0;
        {
            let mut update = tx.prepare_cached(
                "UPDATE message SET flags = ?1 WHERE folder_id = ?2 AND uid = ?3 AND flags != ?1",
            )?;
            for (uid, f) in flags {
                changed += update.execute(params![f.0, folder_id, uid])? as u32;
            }
        }
        if changed > 0 {
            recount(&tx, folder_id)?;
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Removes messages from the index. Returns where their files are (for
    /// those that had one), so the caller can delete them.
    pub fn remove_messages(&self, folder_id: i64, ids: &[i64]) -> Result<Vec<(String, Flags)>> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let mut files = Vec::new();
        {
            let mut find = tx.prepare_cached(
                "SELECT base, file_flags, sort_key FROM message WHERE id = ?1 AND folder_id = ?2",
            )?;
            let mut unindex = tx.prepare_cached("DELETE FROM message_fts WHERE rowid = ?1")?;
            let mut delete = tx.prepare_cached("DELETE FROM message WHERE id = ?1")?;
            for id in ids {
                let found: Option<(String, u8, i64)> = find
                    .query_row(params![id, folder_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()?;
                let Some((base, file_flags, sort_key)) = found else { continue };
                unindex.execute([sort_key])?;
                delete.execute([id])?;
                if !base.is_empty() {
                    files.push((base, Flags(file_flags)));
                }
            }
        }
        recount(&tx, folder_id)?;
        tx.commit()?;
        Ok(files)
    }

    /// Removes every message of a folder from the index, and forgets how
    /// far it was synced.
    pub fn clear_folder(&self, folder_id: i64) -> Result<()> {
        self.writer().execute_batch(&format!(
            "BEGIN;
             DELETE FROM message_fts WHERE rowid IN (SELECT sort_key FROM message WHERE folder_id = {folder_id});
             DELETE FROM message WHERE folder_id = {folder_id};
             UPDATE folder SET total = 0, unread = 0, uidvalidity = 0, uidnext = 0,
                               highestmodseq = 0, headers_done = 0 WHERE id = {folder_id};
             COMMIT"
        ))?;
        Ok(())
    }

    /// Removes a folder and its messages from the index.
    pub fn delete_folder(&self, folder_id: i64) -> Result<()> {
        self.clear_folder(folder_id)?;
        self.writer().execute("DELETE FROM folder WHERE id = ?1", [folder_id])?;
        Ok(())
    }

    /// Detaches a folder's messages from their server UIDs, keeping the
    /// messages. For mail whose UIDs cannot be trusted to mean anything to
    /// the server the folder is now syncing with.
    pub fn forget_uids(&self, folder_id: i64) -> Result<()> {
        self.writer()
            .execute("UPDATE message SET uid = NULL WHERE folder_id = ?1", [folder_id])?;
        Ok(())
    }

    /// Gives `uid` to a message in the folder that has this Message-ID and
    /// no UID yet. Returns whether there was one.
    pub fn adopt_uid(&self, folder_id: i64, message_id: &str, uid: u32) -> Result<bool> {
        let adopted = self.writer().execute(
            "UPDATE message SET uid = ?1 WHERE id = (
               SELECT id FROM message WHERE folder_id = ?2 AND uid IS NULL AND message_id = ?3 LIMIT 1)
             AND NOT EXISTS (SELECT 1 FROM message WHERE folder_id = ?2 AND uid = ?1)",
            params![uid, folder_id, message_id],
        )?;
        Ok(adopted > 0)
    }

    /// Messages whose body has not been downloaded, newest first.
    pub fn missing_bodies(&self, folder_id: i64, limit: u32) -> Result<Vec<KnownMessage>> {
        self.read(|conn| {
            conn.prepare_cached(
                "SELECT uid, id, flags, size FROM message
                 WHERE folder_id = ?1 AND base = '' AND uid IS NOT NULL
                 ORDER BY sort_key DESC LIMIT ?2",
            )?
            .query_map(params![folder_id, limit], |r| {
                Ok(KnownMessage {
                    uid: r.get(0)?,
                    id: r.get(1)?,
                    flags: Flags(r.get(2)?),
                    size: r.get::<_, i64>(3)? as u64,
                })
            })?
            .collect()
        })
    }

    /// How many messages in one folder still need their body.
    pub fn missing_bodies_in(&self, folder_id: i64) -> Result<u64> {
        self.read(|conn| {
            conn.prepare_cached("SELECT count(*) FROM message WHERE folder_id = ?1 AND base = ''")?
                .query_row([folder_id], |r| r.get::<_, i64>(0))
        })
        .map(|n| n as u64)
    }

    /// How many messages in the account still need their body.
    pub fn missing_body_count(&self) -> Result<u64> {
        self.read(|conn| {
            conn.query_row("SELECT count(*) FROM message WHERE base = ''", [], |r| r.get::<_, i64>(0))
        })
        .map(|n| n as u64)
    }

    /// Records that a message's body is now in a file. `file_flags` are the
    /// flags the file was named with. Returns false if the message is no
    /// longer in the index (it was removed while downloading).
    fn attach_body(&self, id: i64, base: &str, file_flags: Flags, size: u64, p: &Parsed) -> Result<bool> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        let found: Option<(i64, i64)> = tx
            .query_row("SELECT sort_key, folder_id FROM message WHERE id = ?1 AND base = ''", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((sort_key, _folder)) = found else {
            return Ok(false);
        };
        // The full message is a better source for these than the handful of
        // header fields fetched for the list. The date, and so the sort key,
        // is left alone: a row must not move in the list when its body lands.
        tx.execute(
            "UPDATE message SET base = ?1, file_flags = ?2, size = ?3, has_attachment = ?4,
                                from_name = ?5, from_addr = ?6, recipients = ?7, subject = ?8,
                                message_id = ?9, in_reply_to = ?10, refs = ?11,
                                preview = ?12, preview_len = ?13
             WHERE id = ?14",
            params![
                base,
                file_flags.0,
                size as i64,
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
                id
            ],
        )?;
        tx.execute("DELETE FROM message_fts WHERE rowid = ?1", [sort_key])?;
        tx.execute(
            "INSERT INTO message_fts (rowid, subject, participants, body) VALUES (?1, ?2, ?3, ?4)",
            params![
                sort_key,
                p.subject,
                format!("{} {} {}", p.from_name, p.from_addr, p.recipients),
                p.body
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

fn recount(conn: &rusqlite::Connection, folder_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE folder SET
           total  = (SELECT count(*) FROM message WHERE folder_id = ?1),
           unread = (SELECT count(*) FROM message WHERE folder_id = ?1 AND (flags & 1) = 0)
         WHERE id = ?1",
        [folder_id],
    )?;
    Ok(())
}

/// Each synced folder keeps its UIDVALIDITY in a small file beside its mail,
/// so that UIDs recorded in file names can still be trusted (or known to be
/// stale) after the index has been rebuilt.
const UID_VALIDITY_FILE: &str = "outpost-uidvalidity";

pub(super) fn read_uid_validity(folder_dir: &std::path::Path) -> u32 {
    std::fs::read_to_string(folder_dir.join(UID_VALIDITY_FILE))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// A message's list data from the server, before its body is downloaded.
#[derive(Debug, Clone)]
pub struct HeaderOnly<'a> {
    pub uid: u32,
    pub flags: Flags,
    pub size: u32,
    /// When the server received it. Used as the date if the header has none.
    pub internal_date: i64,
    /// The fetched header fields.
    pub header: &'a [u8],
}

impl Store {
    fn folder_dir(&self, folder_id: i64) -> Result<Maildir> {
        let path = self
            .index()
            .folder_path(folder_id)?
            .ok_or(StoreError::NoSuchFolder(folder_id))?;
        Ok(Maildir::at(self.mail_root().join(path)))
    }

    /// Records how far a folder has been synced, in the index and (for the
    /// UIDVALIDITY) beside the mail files.
    pub fn set_sync_state(&self, folder_id: i64, state: &SyncState) -> Result<()> {
        let dir = self.folder_dir(folder_id)?;
        if read_uid_validity(dir.path()) != state.uid_validity {
            std::fs::write(dir.path().join(UID_VALIDITY_FILE), state.uid_validity.to_string())?;
        }
        self.index().set_sync_state(folder_id, state)
    }

    /// Creates the local folder for a server folder, on disk and in the index.
    pub fn add_remote_folder(&self, f: &RemoteFolderInfo<'_>) -> Result<i64> {
        Maildir::create(self.mail_root().join(f.path))?;
        self.index().upsert_remote_folder(f)
    }

    /// Adds messages known only by their list data. They appear in the
    /// folder at once; `store_body` completes each one later.
    pub fn add_headers(&self, folder_id: i64, headers: &[HeaderOnly<'_>]) -> Result<()> {
        let mut messages: Vec<NewMessage> = Vec::new();
        for h in headers {
            let parsed = parse::parse(h.header);
            // A message we put in this folder ourselves (moved here, or
            // saved here) is already present, waiting to learn its UID.
            // Recognise it instead of adding it a second time.
            if !parsed.message_id.is_empty() && self.index().adopt_uid(folder_id, &parsed.message_id, h.uid)? {
                continue;
            }
            messages.push({
                NewMessage {
                    base: String::new(),
                    file_flags: h.flags,
                    flags: h.flags,
                    uid: Some(h.uid),
                    date: parsed.date.unwrap_or(h.internal_date),
                    size: h.size as u64,
                    parsed,
                }
            });
        }
        self.index().insert(folder_id, &messages)?;
        Ok(())
    }

    /// Stores a downloaded message as a file and completes its index entry.
    /// If the message was removed from the index in the meantime, the file
    /// is not kept.
    pub fn store_body(&self, folder_id: i64, known: &KnownMessage, raw: &[u8]) -> Result<()> {
        let dir = self.folder_dir(folder_id)?;
        let base = dir.deliver(raw, known.flags, Some(known.uid), true)?;
        let parsed = parse::parse(raw);
        if !self.index().attach_body(known.id, &base, known.flags, raw.len() as u64, &parsed)? {
            dir.remove(&base, known.flags)?;
        }
        Ok(())
    }

    /// Removes messages that no longer exist on the server: index entries
    /// and files.
    pub fn remove_messages(&self, folder_id: i64, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let dir = self.folder_dir(folder_id)?;
        for (base, file_flags) in self.index().remove_messages(folder_id, ids)? {
            match dir.remove(&base, file_flags) {
                // Already gone is the outcome we wanted.
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Empties a folder locally (index and files) so it can be synced from
    /// scratch. Used when the server has renumbered the folder's messages.
    pub fn reset_folder(&self, folder_id: i64) -> Result<()> {
        let dir = self.folder_dir(folder_id)?;
        self.index().clear_folder(folder_id)?;
        for name in dir.file_names()? {
            std::fs::remove_file(dir.path().join("cur").join(name))?;
        }
        Ok(())
    }

    /// Removes a folder that no longer exists on the server, with its mail.
    pub fn remove_folder(&self, folder_id: i64) -> Result<()> {
        let dir = self.folder_dir(folder_id)?;
        self.index().delete_folder(folder_id)?;
        // Only this folder's own mail. A child folder is its own directory
        // and is removed when the server stops listing it too.
        for sub in ["cur", "new", "tmp"] {
            match std::fs::remove_dir_all(dir.path().join(sub)) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        let _ = std::fs::remove_file(dir.path().join(UID_VALIDITY_FILE));
        // Succeeds only if nothing else is inside, which is what we want.
        let _ = std::fs::remove_dir(dir.path());
        Ok(())
    }
}
