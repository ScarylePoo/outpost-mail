//! The commands that change mail on the server.
//!
//! Kept apart from `session.rs` so that it is obvious which code can alter a
//! mailbox. Everything here is called only from the action queue
//! (`sync/actions.rs`), which replays what the user did.
//!
//! One rule from the handoff doc is enforced here and nowhere else: messages
//! are only ever expunged by UID. There is no folder-wide EXPUNGE in this
//! client, because that would also remove messages another client had marked
//! for deletion.

use super::conn::{ImapError, Result, Untagged};
use super::session::{mailbox, uid_set, Opened, Session};
use crate::store::maildir::Flags;
use imap_codec::imap_types::command::CommandBody;
use imap_codec::imap_types::core::{AString, Atom, Vec1};
use imap_codec::imap_types::extensions::uidplus::{UidElement, UidSet};
use imap_codec::imap_types::flag::{Flag, StoreResponse, StoreType};
use imap_codec::imap_types::response::{Code, Data};
use imap_codec::imap_types::search::SearchKey;

fn to_imap(flags: Flags) -> Vec<Flag<'static>> {
    let mut out = Vec::new();
    for (flag, imap) in [
        (Flags::SEEN, Flag::Seen),
        (Flags::ANSWERED, Flag::Answered),
        (Flags::FLAGGED, Flag::Flagged),
        (Flags::DELETED, Flag::Deleted),
        (Flags::DRAFT, Flag::Draft),
    ] {
        if flags.contains(flag) {
            out.push(imap);
        }
    }
    if flags.contains(Flags::FORWARDED) {
        out.push(Flag::Keyword(Atom::try_from("$Forwarded").expect("valid atom")));
    }
    out
}

fn expand(set: &UidSet) -> Vec<u32> {
    let mut out = Vec::new();
    for element in set.0.as_ref() {
        match element {
            UidElement::Single(n) => out.push(n.get()),
            UidElement::Range(a, b) => {
                let (a, b) = (a.get(), b.get());
                if a <= b {
                    out.extend(a..=b);
                } else {
                    out.extend((b..=a).rev());
                }
            }
        }
    }
    out
}

impl Session {
    /// Opens a folder for changes.
    pub async fn select(&mut self, raw_name: &[u8]) -> Result<Opened> {
        self.open(raw_name, None, false).await
    }

    /// Adds or removes flags on messages in the open folder.
    pub async fn store_flags(&mut self, uids: &[u32], flags: Flags, add: bool) -> Result<()> {
        let (Some(set), imap) = (uid_set(uids), to_imap(flags)) else {
            return Ok(());
        };
        if imap.is_empty() {
            return Ok(());
        }
        let kind = if add { StoreType::Add } else { StoreType::Remove };
        let command = CommandBody::store(set, kind, StoreResponse::Silent, imap, true)
            .map_err(|_| ImapError::Protocol("could not build STORE".into()))?;
        self.conn.run(command, |_| {}).await.map(|_| ())
    }

    /// Removes exactly these messages from the open folder for good.
    /// Returns false, having removed nothing, if the server cannot expunge
    /// by UID: this client never issues a folder-wide expunge.
    pub async fn expunge(&mut self, uids: &[u32]) -> Result<bool> {
        if !self.conn.has("UIDPLUS") {
            return Ok(false);
        }
        let Some(set) = uid_set(uids) else { return Ok(true) };
        self.store_flags(uids, Flags::DELETED, true).await?;
        self.conn
            .run(CommandBody::ExpungeUid { sequence_set: set }, |_| {})
            .await?;
        Ok(true)
    }

    /// Moves messages from the open folder to another. Returns the UIDs the
    /// messages have in the destination, as (old, new) pairs, when the
    /// server reports them.
    ///
    /// Uses MOVE when offered. Otherwise copies, then removes exactly those
    /// messages from the source; if the server cannot do that by UID either,
    /// the originals are left marked as deleted.
    pub async fn move_messages(&mut self, uids: &[u32], destination: &[u8]) -> Result<Vec<(u32, u32)>> {
        let Some(set) = uid_set(uids) else { return Ok(Vec::new()) };
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        let mut note = |code: &Option<Code<'_>>| {
            if let Some(Code::CopyUid { source, destination, .. }) = code {
                pairs.extend(expand(source).into_iter().zip(expand(destination)));
            }
        };
        if self.conn.has("MOVE") {
            let command = CommandBody::Move {
                sequence_set: set,
                mailbox: mailbox(destination)?,
                uid: true,
            };
            let mut seen = Vec::new();
            let done = self
                .conn
                .run(command, |u| {
                    if let Untagged::Status(body) = u {
                        seen.push(body.code);
                    }
                })
                .await?;
            // MOVE reports the new UIDs on an untagged OK; some servers put
            // them on the final answer.
            seen.iter().for_each(&mut note);
            note(&done.code);
        } else {
            let command = CommandBody::copy(set, mailbox(destination)?, true)
                .map_err(|_| ImapError::Protocol("could not build COPY".into()))?;
            let done = self.conn.run(command, |_| {}).await?;
            note(&done.code);
            if !self.expunge(uids).await? {
                self.store_flags(uids, Flags::DELETED, true).await?;
            }
        }
        Ok(pairs)
    }

    /// Adds a message to a folder. Returns its UID there when the server
    /// reports it.
    pub async fn append(&mut self, folder: &[u8], flags: Flags, message: &[u8]) -> Result<Option<u32>> {
        let command = CommandBody::append(mailbox(folder)?, to_imap(flags), None, message.to_vec())
            .map_err(|_| ImapError::Protocol("the message cannot be sent over IMAP as it is".into()))?;
        let done = self.conn.run(command, |_| {}).await?;
        Ok(match done.code {
            Some(Code::AppendUid { uid, .. }) => Some(uid.get()),
            _ => None,
        })
    }

    /// UIDs of the messages in the open folder with this Message-ID.
    pub async fn find_by_message_id(&mut self, message_id: &str) -> Result<Vec<u32>> {
        let value = format!("<{}>", message_id.trim_matches(['<', '>']));
        let (Ok(field), Ok(value)) = (AString::try_from("Message-ID"), AString::try_from(value)) else {
            return Ok(Vec::new());
        };
        let command = CommandBody::search(None, Vec1::from(SearchKey::Header(field, value)), true);
        let mut found = Vec::new();
        self.conn
            .run(command, |u| {
                if let Untagged::Data(Data::Search(uids, _)) = u {
                    found.extend(uids.into_iter().map(|n| n.get()));
                }
            })
            .await?;
        Ok(found)
    }

    pub async fn create_folder(&mut self, raw_name: &[u8]) -> Result<()> {
        let command = CommandBody::Create { mailbox: mailbox(raw_name)? };
        self.conn.run(command, |_| {}).await?;
        let subscribe = CommandBody::Subscribe { mailbox: mailbox(raw_name)? };
        // Subscribing is a courtesy to other clients; not all servers have it.
        let _ = self.conn.run(subscribe, |_| {}).await;
        Ok(())
    }

    pub async fn rename_folder(&mut self, from: &[u8], to: &[u8]) -> Result<()> {
        let command = CommandBody::Rename {
            from: mailbox(from)?,
            to: mailbox(to)?,
        };
        self.conn.run(command, |_| {}).await.map(|_| ())
    }

    /// Deletes a folder and the mail in it. The caller is responsible for
    /// having asked the user.
    pub async fn delete_folder(&mut self, raw_name: &[u8]) -> Result<()> {
        let command = CommandBody::Delete { mailbox: mailbox(raw_name)? };
        self.conn.run(command, |_| {}).await.map(|_| ())
    }
}
