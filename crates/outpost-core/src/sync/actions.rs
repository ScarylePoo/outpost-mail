//! Replays the action queue to the server: the half of the sync engine that
//! changes things.
//!
//! Actions are carried out strictly in the order the user made them. Each is
//! safe to repeat, because a connection can drop between the server doing
//! something and this client hearing that it did:
//!
//! - setting or clearing flags again changes nothing;
//! - moving or expunging a UID that is no longer there does nothing;
//! - before a message is uploaded a second time, the folder is searched for
//!   its Message-ID, so a retry cannot create a duplicate.
//!
//! An action the server refuses is not retried. Its local effect is undone
//! where that is possible, the user is told, and the queue moves on.

use super::{Progress, Result, Summary, SyncError};
use crate::imap::{ImapError, Session};
use crate::store::actions::{Action, Item};
use crate::store::maildir::Flags;
use crate::store::sync_state::SyncFolder;
use crate::store::Store;
use std::collections::{HashMap, HashSet};

/// Replays everything in the queue. Returns early, leaving the rest queued,
/// only if the connection fails.
pub(super) async fn replay(
    session: &mut Session,
    store: &Store,
    progress: &mut (dyn FnMut(Progress) + Send),
    summary: &mut Summary,
) -> Result<()> {
    loop {
        // One at a time, read fresh each time: carrying out an action can
        // fill in UIDs that the actions queued behind it were waiting for.
        let queued = store.index().queued_actions(1)?;
        if queued.is_empty() {
            return Ok(());
        }
        let folders: HashMap<i64, SyncFolder> =
            store.index().remote_folders()?.into_iter().map(|f| (f.id, f)).collect();
        for entry in queued {
            match carry_out(session, store, &folders, &entry.action).await {
                Ok(touched) => {
                    store.index().finish_action(entry.id)?;
                    summary.actions_replayed += 1;
                    for folder_id in touched {
                        progress(Progress::FolderChanged { folder_id });
                    }
                }
                Err(SyncError::Imap(e)) if e.is_transient() => return Err(SyncError::Imap(e)),
                Err(SyncError::Stopped) => return Err(SyncError::Stopped),
                Err(e) => {
                    // Refused, or impossible. Undo what can be undone and
                    // say so; do not block everything queued behind it.
                    let touched = undo(store, &entry.action)?;
                    store.index().finish_action(entry.id)?;
                    progress(Progress::ActionFailed {
                        what: describe(&entry.action),
                        error: e.to_string(),
                    });
                    for folder_id in touched {
                        progress(Progress::FolderChanged { folder_id });
                    }
                }
            }
        }
    }
}

fn describe(action: &Action) -> String {
    let count = |items: &Vec<Item>| match items.len() {
        1 => "a message".to_string(),
        n => format!("{n} messages"),
    };
    match action {
        Action::Flags { items, .. } => format!("Marking {}", count(items)),
        Action::Move { items, .. } => format!("Moving {}", count(items)),
        Action::Expunge { items, .. } => format!("Deleting {}", count(items)),
        Action::Append { .. } => "Saving a message to the server".to_string(),
    }
}

/// Undoes the local side of an action the server would not carry out.
/// Returns the folders that changed.
fn undo(store: &Store, action: &Action) -> Result<Vec<i64>> {
    Ok(match action {
        Action::Move { from, to, items } => {
            store.revert_move(*from, *to, items)?;
            vec![*from, *to]
        }
        // The next sync shows these as the server has them.
        Action::Flags { folder, .. } | Action::Expunge { folder, .. } => vec![*folder],
        // The message stays here, where the user can still see it.
        Action::Append { .. } => Vec::new(),
    })
}

fn folder<'a>(folders: &'a HashMap<i64, SyncFolder>, id: i64) -> Result<&'a SyncFolder> {
    folders.get(&id).ok_or_else(|| {
        SyncError::Imap(ImapError::Protocol("the folder no longer exists on the server".into()))
    })
}

fn uids(items: &[Item]) -> Vec<u32> {
    items.iter().filter_map(|i| i.uid).collect()
}

/// Carries out one action. Returns the folders whose local contents
/// changed as a result.
async fn carry_out(
    session: &mut Session,
    store: &Store,
    folders: &HashMap<i64, SyncFolder>,
    action: &Action,
) -> Result<Vec<i64>> {
    let index = store.index();
    match action {
        Action::Flags { folder: id, items, add, remove } => {
            let uids = uids(items);
            if uids.is_empty() {
                return Ok(Vec::new());
            }
            session.select(&folder(folders, *id)?.remote_name).await?;
            session.store_flags(&uids, Flags(*add), true).await?;
            session.store_flags(&uids, Flags(*remove), false).await?;
            Ok(Vec::new())
        }
        Action::Expunge { folder: id, items } => {
            let uids = uids(items);
            if uids.is_empty() {
                return Ok(Vec::new());
            }
            session.select(&folder(folders, *id)?.remote_name).await?;
            if !session.expunge(&uids).await? {
                // The server cannot remove single messages. Mark them
                // deleted, which is as far as this client will go: sync
                // treats messages so marked as gone.
                session.store_flags(&uids, Flags::DELETED, true).await?;
            }
            Ok(Vec::new())
        }
        Action::Move { from, to, items } => {
            let (source, destination) = (folder(folders, *from)?, folder(folders, *to)?);
            let moving = uids(items);
            let mut new_uid: HashMap<u32, u32> = HashMap::new();
            if !moving.is_empty() {
                session.select(&source.remote_name).await?;
                new_uid = session
                    .move_messages(&moving, &destination.remote_name)
                    .await?
                    .into_iter()
                    .collect();
            }
            // Give each message its UID in the new folder. Where the server
            // did not report it, look the message up by its Message-ID.
            let mut unresolved: Vec<&Item> = Vec::new();
            for item in items {
                match item.uid.and_then(|old| new_uid.get(&old)) {
                    Some(uid) => index.resolve_uid(item.message, *to, *uid)?,
                    None => unresolved.push(item),
                }
            }
            let mut touched = Vec::new();
            if !unresolved.is_empty() {
                session.examine(&destination.remote_name, None).await?;
                let known: HashSet<u32> =
                    index.known_messages(*to)?.into_iter().map(|m| m.uid).collect();
                for item in unresolved {
                    let Some(brief) = index.brief(item.message)? else { continue };
                    if brief.folder != *to || brief.uid.is_some() {
                        continue;
                    }
                    let found = match brief.message_id.is_empty() {
                        true => Vec::new(),
                        false => session.find_by_message_id(&brief.message_id).await?,
                    };
                    match found.into_iter().filter(|uid| !known.contains(uid)).max() {
                        Some(uid) => index.resolve_uid(item.message, *to, uid)?,
                        // Not in the destination and not movable from the
                        // source: it no longer exists on the server.
                        None if !brief.message_id.is_empty() => {
                            store.drop_local_message(item.message)?;
                            touched.push(*to);
                        }
                        // No way to tell. Leave it; a later sync of the
                        // folder will list whatever is really there.
                        None => {}
                    }
                }
            }
            Ok(touched)
        }
        Action::Append { folder: id, message } => {
            let Some(brief) = index.brief(*message)? else {
                return Ok(Vec::new());
            };
            if brief.folder != *id || brief.uid.is_some() {
                return Ok(Vec::new());
            }
            let destination = folder(folders, *id)?;
            let raw = store.read_message(*message)?;
            // If an earlier attempt got through without our hearing of it,
            // the message is already there. Never upload it twice.
            if !brief.message_id.is_empty() {
                session.examine(&destination.remote_name, None).await?;
                let known: HashSet<u32> =
                    index.known_messages(*id)?.into_iter().map(|m| m.uid).collect();
                let already = session.find_by_message_id(&brief.message_id).await?;
                if let Some(uid) = already.into_iter().filter(|u| !known.contains(u)).max() {
                    index.resolve_uid(*message, *id, uid)?;
                    return Ok(Vec::new());
                }
            }
            let uid = session.append(&destination.remote_name, brief.flags, &raw).await?;
            let uid = match uid {
                Some(uid) => Some(uid),
                None if !brief.message_id.is_empty() => {
                    session.examine(&destination.remote_name, None).await?;
                    session.find_by_message_id(&brief.message_id).await?.into_iter().max()
                }
                None => None,
            };
            if let Some(uid) = uid {
                index.resolve_uid(*message, *id, uid)?;
            }
            Ok(Vec::new())
        }
    }
}
