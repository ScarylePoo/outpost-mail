//! The action queue: what the user has done that the server does not know
//! about yet.
//!
//! Every change the user makes (marking, moving, deleting, saving a draft)
//! is applied to the local store at once, so the interface never waits on
//! the network, and recorded here in the same transaction. The sync engine
//! replays the queue to the server in order (`sync/actions.rs`). Offline,
//! the queue simply grows.
//!
//! A message that has been moved or created locally has no server UID until
//! its action has been replayed. Actions refer to such a message by its row
//! in the index, and get its UID filled in when the server reports it.

use super::index::Index;
use super::maildir::{encode_segment, Flags, Maildir};
use super::{Result, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

/// A message as an action refers to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// The message's row in the index. Stays valid as a name for the
    /// message even after the row is deleted.
    pub message: i64,
    /// Its UID in the folder the action applies to. `None` until an
    /// earlier action that puts it there has been replayed.
    pub uid: Option<u32>,
    /// Its flags when the action was queued.
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Add and remove flags.
    Flags { folder: i64, items: Vec<Item>, add: u8, remove: u8 },
    /// Move messages from one folder to another.
    Move { from: i64, to: i64, items: Vec<Item> },
    /// Remove messages for good.
    Expunge { folder: i64, items: Vec<Item> },
    /// Upload a message that so far exists only here.
    Append { folder: i64, message: i64 },
    /// Send a message waiting in the outbox.
    Send { message: i64 },
    /// Create, on the server, a folder that was made here.
    CreateFolder { folder: i64 },
    /// Delete a folder from the server. It is already gone here.
    DeleteFolder { remote_name: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub id: i64,
    pub action: Action,
    pub attempts: u32,
}

fn enqueue(conn: &Connection, action: &Action) -> rusqlite::Result<()> {
    enqueue_at(conn, action, 0)
}

fn enqueue_at(conn: &Connection, action: &Action, not_before: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO action (payload, not_before) VALUES (?1, ?2)",
        params![serde_json::to_string(action).expect("actions serialise"), not_before],
    )?;
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The directory of the outbox: a folder that exists only on this computer,
/// holding messages on their way out. The name is one no mail server uses.
pub const OUTBOX: &str = "Outpost Outbox";

fn recount(conn: &Connection, folder_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE folder SET
           total  = (SELECT count(*) FROM message WHERE folder_id = ?1),
           unread = (SELECT count(*) FROM message WHERE folder_id = ?1 AND (flags & 1) = 0)
         WHERE id = ?1",
        [folder_id],
    )?;
    Ok(())
}

struct Row {
    id: i64,
    folder: i64,
    uid: Option<u32>,
    flags: u8,
    base: String,
    file_flags: u8,
}

fn rows(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<Row>> {
    let mut find = conn.prepare_cached(
        "SELECT id, folder_id, uid, flags, base, file_flags FROM message WHERE id = ?1",
    )?;
    let mut out = Vec::new();
    for id in ids {
        let row = find
            .query_row([id], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    folder: r.get(1)?,
                    uid: r.get(2)?,
                    flags: r.get(3)?,
                    base: r.get(4)?,
                    file_flags: r.get(5)?,
                })
            })
            .optional()?;
        // A message that has gone in the meantime is simply skipped.
        out.extend(row);
    }
    Ok(out)
}

fn item(row: &Row) -> Item {
    Item {
        message: row.id,
        uid: row.uid,
        flags: row.flags,
    }
}

/// The few facts about a message that replaying an action needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    pub folder: i64,
    pub uid: Option<u32>,
    pub flags: Flags,
    pub message_id: String,
}

impl Index {
    /// `None` if the message is no longer in the index.
    pub fn brief(&self, message: i64) -> Result<Option<Brief>> {
        self.read(|conn| {
            conn.prepare_cached("SELECT folder_id, uid, flags, message_id FROM message WHERE id = ?1")?
                .query_row([message], |r| {
                    Ok(Brief {
                        folder: r.get(0)?,
                        uid: r.get(1)?,
                        flags: Flags(r.get(2)?),
                        message_id: r.get(3)?,
                    })
                })
                .optional()
        })
    }

    /// The oldest actions that are due, in the order they must be replayed.
    pub fn queued_actions(&self, limit: u32) -> Result<Vec<Queued>> {
        self.actions_where("not_before <= ?2", limit)
    }

    /// Every queued action, due or not.
    pub fn all_actions(&self) -> Result<Vec<Queued>> {
        self.actions_where("?2 = ?2", u32::MAX)
    }

    /// When the next action that is not yet due becomes due.
    pub fn next_action_due(&self) -> Result<Option<i64>> {
        self.read(|conn| {
            conn.query_row("SELECT min(not_before) FROM action WHERE not_before > ?1", [now()], |r| r.get(0))
        })
    }

    /// Makes every action that was put off after a failure due now. For
    /// when the user asks for a sync: they expect waiting mail to be tried.
    pub fn retry_deferred(&self) -> Result<()> {
        self.writer().execute("UPDATE action SET not_before = 0 WHERE attempts > 0", [])?;
        Ok(())
    }

    /// Puts an action off until `until`, noting why.
    pub fn defer_action(&self, id: i64, until: i64, error: &str) -> Result<()> {
        self.writer().execute(
            "UPDATE action SET not_before = ?2, attempts = attempts + 1, last_error = ?3 WHERE id = ?1",
            params![id, until, error],
        )?;
        Ok(())
    }

    fn actions_where(&self, condition: &str, limit: u32) -> Result<Vec<Queued>> {
        self.read(|conn| {
            conn.prepare_cached(&format!(
                "SELECT id, payload, attempts FROM action WHERE {condition} ORDER BY id LIMIT ?1"
            ))?
                .query_map(params![limit, now()], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, u32>(2)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map(|rows| {
            rows.into_iter()
                .filter_map(|(id, payload, attempts)| {
                    Some(Queued {
                        id,
                        action: serde_json::from_str(&payload).ok()?,
                        attempts,
                    })
                })
                .collect()
        })
    }

    pub fn queued_action_count(&self) -> Result<u64> {
        self.read(|conn| conn.query_row("SELECT count(*) FROM action", [], |r| r.get::<_, i64>(0)))
            .map(|n| n as u64)
    }

    /// Removes an action that has been carried out (or given up on).
    pub fn finish_action(&self, id: i64) -> Result<()> {
        self.writer().execute("DELETE FROM action WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Notes a failed attempt, so that a doomed action is not retried for ever.
    pub fn note_action_failure(&self, id: i64, error: &str) -> Result<()> {
        self.writer().execute(
            "UPDATE action SET attempts = attempts + 1, last_error = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    /// Records the UID a message has turned out to have in `folder`.
    /// The row gets it if the message is still there without one, and so do
    /// queued actions that were waiting to learn it.
    pub fn resolve_uid(&self, message: i64, folder: i64, uid: u32) -> Result<()> {
        let mut conn = self.writer();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE message SET uid = ?1 WHERE id = ?2 AND folder_id = ?3 AND uid IS NULL
               AND NOT EXISTS (SELECT 1 FROM message WHERE folder_id = ?3 AND uid = ?1)",
            params![uid, message, folder],
        )?;
        let queued: Vec<(i64, String)> = tx
            .prepare("SELECT id, payload FROM action ORDER BY id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, payload) in queued {
            let Ok(mut action) = serde_json::from_str::<Action>(&payload) else { continue };
            let (in_folder, items) = match &mut action {
                Action::Flags { folder, items, .. } => (*folder, Some(items)),
                Action::Move { from, items, .. } => (*from, Some(items)),
                Action::Expunge { folder, items } => (*folder, Some(items)),
                _ => (0, None),
            };
            let Some(items) = items else { continue };
            let mut changed = false;
            if in_folder == folder {
                for item in items.iter_mut().filter(|i| i.message == message && i.uid.is_none()) {
                    item.uid = Some(uid);
                    changed = true;
                }
            }
            if changed {
                tx.execute(
                    "UPDATE action SET payload = ?1 WHERE id = ?2",
                    params![serde_json::to_string(&action).expect("actions serialise"), id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// UIDs in a folder that queued actions are about to change, move away
    /// or remove. Sync must not undo the local side of those by copying the
    /// server's (not yet updated) view over it.
    pub fn pending_uids(&self, folder_id: i64) -> Result<HashSet<u32>> {
        let mut pending = HashSet::new();
        for queued in self.all_actions()? {
            let (folder, items) = match &queued.action {
                Action::Flags { folder, items, .. } => (*folder, items),
                Action::Move { from, items, .. } => (*from, items),
                Action::Expunge { folder, items } => (*folder, items),
                _ => continue,
            };
            if folder == folder_id {
                pending.extend(items.iter().filter_map(|i| i.uid));
            }
        }
        Ok(pending)
    }
}

impl Store {
    fn dir_of(&self, folder_id: i64) -> Result<Maildir> {
        let path = self
            .index()
            .folder_path(folder_id)?
            .ok_or(StoreError::NoSuchFolder(folder_id))?;
        Ok(Maildir::at(self.mail_root().join(path)))
    }

    /// Adds and removes flags on messages, here and (queued) on the server.
    /// Returns the folders whose contents changed.
    pub fn change_flags(&self, ids: &[i64], add: Flags, remove: Flags) -> Result<BTreeSet<i64>> {
        let mut conn = self.index().writer();
        let tx = conn.transaction()?;
        let mut touched = BTreeSet::new();
        let mut by_folder: std::collections::BTreeMap<i64, Vec<Item>> = Default::default();
        for row in rows(&tx, ids)? {
            let new = Flags(row.flags).with(add).without(remove);
            if new.0 == row.flags {
                continue;
            }
            tx.execute("UPDATE message SET flags = ?1 WHERE id = ?2", params![new.0, row.id])?;
            touched.insert(row.folder);
            by_folder.entry(row.folder).or_default().push(item(&row));
        }
        for (folder, items) in by_folder {
            recount(&tx, folder)?;
            enqueue(&tx, &Action::Flags { folder, items, add: add.0, remove: remove.0 })?;
        }
        tx.commit()?;
        Ok(touched)
    }

    /// Moves messages to another folder, here and (queued) on the server.
    /// Returns the folders whose contents changed.
    pub fn move_messages(&self, ids: &[i64], to: i64) -> Result<BTreeSet<i64>> {
        let destination = self.dir_of(to)?;
        let mut conn = self.index().writer();
        let tx = conn.transaction()?;
        let mut touched = BTreeSet::new();
        let mut by_source: std::collections::BTreeMap<i64, Vec<Item>> = Default::default();
        let mut moved_files: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
        let result = (|| -> Result<()> {
            for row in rows(&tx, ids)? {
                if row.folder == to {
                    continue;
                }
                if !row.base.is_empty() {
                    let source = Maildir::at(
                        self.mail_root().join(
                            tx.query_row("SELECT path FROM folder WHERE id = ?1", [row.folder], |r| {
                                r.get::<_, String>(0)
                            })?,
                        ),
                    );
                    let from = source.file_path(&row.base, Flags(row.file_flags));
                    let into = destination.file_path(&row.base, Flags(row.file_flags));
                    std::fs::rename(&from, &into)?;
                    moved_files.push((into, from));
                }
                // In its new folder the message has no UID until the server
                // has moved it and said what it is.
                tx.execute(
                    "UPDATE message SET folder_id = ?1, uid = NULL, modseq = NULL WHERE id = ?2",
                    params![to, row.id],
                )?;
                touched.insert(row.folder);
                by_source.entry(row.folder).or_default().push(item(&row));
            }
            for (from, items) in std::mem::take(&mut by_source) {
                recount(&tx, from)?;
                enqueue(&tx, &Action::Move { from, to, items })?;
            }
            if !touched.is_empty() {
                recount(&tx, to)?;
                touched.insert(to);
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                tx.commit()?;
                Ok(touched)
            }
            Err(e) => {
                // Put the files back where the index still says they are.
                drop(tx);
                for (now, before) in moved_files {
                    let _ = std::fs::rename(now, before);
                }
                Err(e)
            }
        }
    }

    /// Removes messages for good, here and (queued) on the server.
    /// Returns the folders whose contents changed.
    pub fn expunge_messages(&self, ids: &[i64]) -> Result<BTreeSet<i64>> {
        let mut touched = BTreeSet::new();
        let mut files = Vec::new();
        {
            let mut conn = self.index().writer();
            let tx = conn.transaction()?;
            let mut by_folder: std::collections::BTreeMap<i64, Vec<Item>> = Default::default();
            for row in rows(&tx, ids)? {
                tx.execute(
                    "DELETE FROM message_fts WHERE rowid = (SELECT sort_key FROM message WHERE id = ?1)",
                    [row.id],
                )?;
                tx.execute("DELETE FROM message WHERE id = ?1", [row.id])?;
                if !row.base.is_empty() {
                    files.push((row.folder, row.base.clone(), Flags(row.file_flags)));
                }
                touched.insert(row.folder);
                by_folder.entry(row.folder).or_default().push(item(&row));
            }
            for (folder, items) in by_folder {
                recount(&tx, folder)?;
                enqueue(&tx, &Action::Expunge { folder, items })?;
            }
            tx.commit()?;
        }
        for (folder, base, flags) in files {
            if let Ok(dir) = self.dir_of(folder) {
                let _ = dir.remove(&base, flags);
            }
        }
        Ok(touched)
    }

    /// Stores a message that exists only here (a draft, a sent copy) and
    /// queues its upload. Returns its row.
    pub fn add_local_message(&self, folder_id: i64, raw: &[u8], flags: Flags) -> Result<i64> {
        let id = self.add_message(folder_id, raw, flags, None)?;
        let conn = self.index().writer();
        enqueue(&conn, &Action::Append { folder: folder_id, message: id })?;
        Ok(id)
    }

    /// Undoes the local side of a move the server refused: the messages go
    /// back to the folder they came from, with the UIDs they had there.
    pub fn revert_move(&self, from: i64, to: i64, items: &[Item]) -> Result<()> {
        let (source, destination) = (self.dir_of(from)?, self.dir_of(to)?);
        let mut conn = self.index().writer();
        let tx = conn.transaction()?;
        for item in items {
            let found: Option<(String, u8)> = tx
                .query_row(
                    "SELECT base, file_flags FROM message WHERE id = ?1 AND folder_id = ?2 AND uid IS NULL",
                    params![item.message, to],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((base, file_flags)) = found else { continue };
            if !base.is_empty() {
                let _ = std::fs::rename(
                    destination.file_path(&base, Flags(file_flags)),
                    source.file_path(&base, Flags(file_flags)),
                );
            }
            tx.execute(
                "UPDATE message SET folder_id = ?1, uid = ?2 WHERE id = ?3",
                params![from, item.uid, item.message],
            )?;
        }
        recount(&tx, from)?;
        recount(&tx, to)?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a local message whose queued action turned out to have
    /// nothing to act on (it no longer exists on the server).
    pub fn drop_local_message(&self, message: i64) -> Result<()> {
        let folder: Option<i64> = self
            .index()
            .read(|c| c.query_row("SELECT folder_id FROM message WHERE id = ?1", [message], |r| r.get(0)).optional())?;
        if let Some(folder) = folder {
            self.remove_messages(folder, &[message])?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The outbox
// ---------------------------------------------------------------------------

impl Store {
    fn folder_with_role(&self, role: &str) -> Result<Option<i64>> {
        Ok(self
            .index()
            .folders()?
            .iter()
            .find(|f| super::sync_state::role_of(&f.path, &f.name, f.special_use.as_deref()) == Some(role))
            .map(|f| f.id))
    }

    /// Puts a finished message in the outbox, to be sent no sooner than
    /// `send_at` (unix time). Returns its row.
    pub fn queue_for_sending(&self, raw: &[u8], send_at: i64) -> Result<i64> {
        let outbox = self.create_folder(&[OUTBOX])?;
        let id = self.add_message(outbox, raw, Flags::SEEN, None)?;
        let conn = self.index().writer();
        enqueue_at(&conn, &Action::Send { message: id }, send_at)?;
        Ok(id)
    }

    /// Takes a message back out of the outbox before it is sent, and makes
    /// it a draft again. Returns false if it is too late.
    pub fn cancel_sending(&self, message: i64) -> Result<bool> {
        let wanted = serde_json::to_string(&Action::Send { message }).expect("actions serialise");
        let cancelled = self.index().writer().execute("DELETE FROM action WHERE payload = ?1", [wanted])?;
        if cancelled == 0 {
            return Ok(false);
        }
        self.back_to_drafts(message)?;
        Ok(true)
    }

    /// Removes a message's queued send, if it has one, leaving the message
    /// where it is. Returns whether there was one.
    pub fn cancel_pending_send(&self, message: i64) -> Result<bool> {
        let wanted = serde_json::to_string(&Action::Send { message }).expect("actions serialise");
        Ok(self.index().writer().execute("DELETE FROM action WHERE payload = ?1", [wanted])? > 0)
    }

    /// Moves a message between folders on this computer only, queuing
    /// nothing. Returns false if the message is not there.
    fn relocate(&self, message: i64, to: i64) -> Result<bool> {
        let destination = self.dir_of(to)?;
        let mut conn = self.index().writer();
        let tx = conn.transaction()?;
        let found: Option<(i64, String, u8, String)> = tx
            .query_row(
                "SELECT m.folder_id, m.base, m.file_flags, f.path FROM message m JOIN folder f ON f.id = m.folder_id WHERE m.id = ?1",
                [message],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((from, base, file_flags, from_path)) = found else { return Ok(false) };
        if !base.is_empty() {
            let source = Maildir::at(self.mail_root().join(from_path));
            std::fs::rename(source.file_path(&base, Flags(file_flags)), destination.file_path(&base, Flags(file_flags)))?;
        }
        tx.execute("UPDATE message SET folder_id = ?1, uid = NULL WHERE id = ?2", params![to, message])?;
        recount(&tx, from)?;
        recount(&tx, to)?;
        tx.commit()?;
        Ok(true)
    }

    /// After a message has been sent: it leaves the outbox for the Sent
    /// folder. With `upload`, the copy is queued for upload there; without
    /// (servers that file sent mail themselves), the local copy is dropped
    /// and the server's own copy arrives by sync. Returns the folders that
    /// changed.
    pub fn after_sent(&self, message: i64, upload: bool) -> Result<Vec<i64>> {
        let outbox = self.index().brief(message)?.map(|b| b.folder);
        let sent = self.folder_with_role("sent")?;
        match (sent, upload) {
            (Some(sent), true) => {
                if self.relocate(message, sent)? {
                    let conn = self.index().writer();
                    enqueue(&conn, &Action::Append { folder: sent, message })?;
                }
                Ok(outbox.into_iter().chain([sent]).collect())
            }
            _ => {
                self.drop_local_message(message)?;
                Ok(outbox.into_iter().collect())
            }
        }
    }

    /// A message that could not be sent goes back to Drafts, where the user
    /// will find it. Without a Drafts folder it stays in the outbox.
    pub fn back_to_drafts(&self, message: i64) -> Result<Vec<i64>> {
        let outbox = self.index().brief(message)?.map(|b| b.folder);
        let Some(drafts) = self.folder_with_role("drafts")? else {
            return Ok(Vec::new());
        };
        if self.relocate(message, drafts)? {
            self.index().writer().execute(
                "UPDATE message SET flags = flags | ?1 WHERE id = ?2",
                params![Flags::DRAFT.0, message],
            )?;
            let conn = self.index().writer();
            enqueue(&conn, &Action::Append { folder: drafts, message })?;
        }
        Ok(outbox.into_iter().chain([drafts]).collect())
    }
}

// ---------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------

/// Why a folder could not be created or deleted, in words for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRefusal(pub String);

impl Store {
    /// Creates a folder here and (queued) on the server. `parent` is the
    /// folder to put it in, or `None` for the top level of the account.
    pub fn create_remote_folder(
        &self,
        parent: Option<i64>,
        name: &str,
    ) -> Result<std::result::Result<i64, FolderRefusal>> {
        let refuse = |why: &str| Ok(Err(FolderRefusal(why.to_string())));
        let name = name.trim();
        let remote = self.index().remote_folders()?;
        let delimiter = self.index().delimiter()?.unwrap_or('/');
        if name.is_empty() {
            return refuse("The folder needs a name.");
        }
        if name.contains(delimiter) || name.chars().any(char::is_control) {
            return refuse(&format!("A folder name cannot contain \"{delimiter}\"."));
        }
        // On servers that keep every folder inside the inbox, "the top
        // level" is inside the inbox too.
        let inbox = remote.iter().find(|f| f.remote_name.eq_ignore_ascii_case(b"INBOX"));
        let inside_inbox = inbox.is_some()
            && remote.len() > 1
            && remote.iter().all(|f| f.path.eq_ignore_ascii_case("INBOX") || f.path.to_ascii_uppercase().starts_with("INBOX/"));
        let parent = match parent {
            Some(id) => match remote.iter().find(|f| f.id == id) {
                Some(found) => Some(found),
                None => return refuse("That folder cannot hold other folders."),
            },
            None if inside_inbox => inbox,
            None => None,
        };
        let segment = encode_segment(name);
        let encoded = crate::imap::utf7::encode(name);
        let (path, remote_name) = match parent {
            Some(p) => {
                let mut raw = p.remote_name.clone();
                raw.extend_from_slice(delimiter.to_string().as_bytes());
                raw.extend_from_slice(&encoded);
                (format!("{}/{segment}", p.path), raw)
            }
            None => (segment, encoded),
        };
        if self.index().folders()?.iter().any(|f| f.path.eq_ignore_ascii_case(&path)) {
            return refuse("There is already a folder with that name there.");
        }
        let id = self.add_remote_folder(&super::sync_state::RemoteFolderInfo {
            path: &path,
            name,
            remote_name: &remote_name,
            delimiter: Some(delimiter),
            selectable: true,
            special_use: None,
        })?;
        let conn = self.index().writer();
        enqueue(&conn, &Action::CreateFolder { folder: id })?;
        Ok(Ok(id))
    }

    /// Deletes a folder and the mail in it, here and (queued) on the
    /// server. Only an ordinary folder with no folders inside it.
    pub fn delete_remote_folder(&self, id: i64) -> Result<std::result::Result<(), FolderRefusal>> {
        let refuse = |why: &str| Ok(Err(FolderRefusal(why.to_string())));
        let folders = self.index().folders()?;
        let Some(folder) = folders.iter().find(|f| f.id == id) else {
            return refuse("That folder no longer exists.");
        };
        if super::sync_state::role_of(&folder.path, &folder.name, folder.special_use.as_deref()).is_some() {
            return refuse("This folder is one the account needs and cannot be deleted.");
        }
        let inside = format!("{}/", folder.path);
        if folders.iter().any(|f| f.path.starts_with(&inside)) {
            return refuse("Delete the folders inside this one first.");
        }
        let remote = self.index().remote_folders()?;
        let Some(remote_name) = remote.iter().find(|f| f.id == id).map(|f| f.remote_name.clone()) else {
            return refuse("This folder exists only on this computer.");
        };
        self.remove_folder(id)?;
        let conn = self.index().writer();
        enqueue(&conn, &Action::DeleteFolder { remote_name })?;
        Ok(Ok(()))
    }
}
