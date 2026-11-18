//! What Outpost asks of an IMAP server, in Outpost's own terms.
//!
//! In this milestone every operation is read-only: folders are opened with
//! EXAMINE and bodies are fetched with PEEK, so nothing here can change a
//! flag or a message on the server.

use super::conn::{Connection, ImapError, Result, Server, Untagged};
use super::utf7;
use crate::store::maildir::Flags;
use imap_codec::imap_types::command::{CommandBody, FetchModifier, SelectParameter};
use imap_codec::imap_types::core::{AString, Vec1};
use imap_codec::imap_types::fetch::{MessageDataItem, MessageDataItemName, Section};
use imap_codec::imap_types::flag::{Flag, FlagFetch, FlagNameAttribute};
use imap_codec::imap_types::mailbox::Mailbox;
use imap_codec::imap_types::response::{Code, Data};
use imap_codec::imap_types::sequence::{SeqOrUid, Sequence, SequenceSet};
use imap_codec::imap_types::status::{StatusDataItem, StatusDataItemName};
use std::num::{NonZeroU32, NonZeroU64};
use std::time::Duration;

/// Header fields fetched for the message list, before any body is downloaded.
const LIST_HEADERS: &[&str] = &[
    "From", "To", "Cc", "Subject", "Date", "Message-ID", "In-Reply-To", "References",
    "Content-Type",
];

/// Bodies are fetched in pieces of this size, so one large attachment cannot
/// stall a connection or need a huge buffer.
pub const BODY_PIECE: u32 = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialUse {
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    /// Gmail's "All Mail": every message, again.
    All,
    Flagged,
}

impl SpecialUse {
    pub fn as_str(self) -> &'static str {
        match self {
            SpecialUse::Sent => "sent",
            SpecialUse::Drafts => "drafts",
            SpecialUse::Trash => "trash",
            SpecialUse::Junk => "junk",
            SpecialUse::Archive => "archive",
            SpecialUse::All => "all",
            SpecialUse::Flagged => "flagged",
        }
    }
}

/// A folder as the server lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolder {
    /// The name exactly as the server sent it. Used to address the folder.
    pub raw_name: Vec<u8>,
    /// The name decoded for people, levels separated by `delimiter`.
    pub name: String,
    pub delimiter: Option<char>,
    /// False for a name that only exists to hold other folders.
    pub selectable: bool,
    pub special_use: Option<SpecialUse>,
}

impl RemoteFolder {
    /// The folder's place in the tree, outermost level first.
    pub fn levels(&self) -> Vec<&str> {
        match self.delimiter {
            Some(d) => self.name.split(d).filter(|s| !s.is_empty()).collect(),
            None => vec![self.name.as_str()],
        }
    }
}

/// What the server reports when a folder is opened.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opened {
    pub uid_validity: u32,
    pub uid_next: Option<u32>,
    pub exists: u32,
    /// `None` if the server keeps no modification sequences for this folder.
    pub highest_modseq: Option<u64>,
    /// With QRESYNC: UID ranges removed since the state we presented.
    pub vanished: Vec<(u32, u32)>,
    /// With QRESYNC: messages whose flags changed since that state.
    pub changed: Vec<FlagState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlagState {
    pub uid: u32,
    pub flags: Flags,
    pub modseq: Option<u64>,
}

/// A message's list data, fetched without its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMessage {
    pub uid: u32,
    pub flags: Flags,
    pub size: u32,
    /// When the server received it, unix seconds.
    pub internal_date: i64,
    pub modseq: Option<u64>,
    /// The header fields in `LIST_HEADERS`, as a small message of its own.
    pub header: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderStatus {
    pub messages: u32,
    pub uid_next: u32,
    pub uid_validity: u32,
    pub highest_modseq: Option<u64>,
}

pub struct Session {
    pub(super) conn: Connection,
    pub(super) qresync: bool,
}

pub(super) fn mailbox(raw_name: &[u8]) -> Result<Mailbox<'static>> {
    Mailbox::try_from(raw_name.to_vec())
        .map_err(|_| ImapError::Protocol("a folder name the protocol cannot express".into()))
}

fn raw_name(mailbox: &Mailbox<'_>) -> Vec<u8> {
    match mailbox {
        Mailbox::Inbox => b"INBOX".to_vec(),
        Mailbox::Other(other) => other.as_ref().to_vec(),
    }
}

fn to_flags(fetched: &[FlagFetch<'_>]) -> Flags {
    let mut flags = Flags::default();
    for flag in fetched {
        let FlagFetch::Flag(flag) = flag else { continue };
        flags = flags.with(match flag {
            Flag::Seen => Flags::SEEN,
            Flag::Answered => Flags::ANSWERED,
            Flag::Flagged => Flags::FLAGGED,
            Flag::Deleted => Flags::DELETED,
            Flag::Draft => Flags::DRAFT,
            Flag::Keyword(k) if k.as_ref().eq_ignore_ascii_case("$Forwarded") => Flags::FORWARDED,
            _ => Flags::default(),
        });
    }
    flags
}

/// A UID set like `3:7,12` from sorted or unsorted UIDs.
pub(super) fn uid_set(uids: &[u32]) -> Option<SequenceSet> {
    let mut sorted: Vec<u32> = uids.iter().copied().filter(|u| *u > 0).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut ranges: Vec<Sequence> = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end + 1 {
            i += 1;
            end = sorted[i];
        }
        let value = |n: u32| SeqOrUid::Value(NonZeroU32::new(n).expect("filtered above"));
        ranges.push(if start == end {
            Sequence::Single(value(start))
        } else {
            Sequence::Range(value(start), value(end))
        });
        i += 1;
    }
    SequenceSet::try_from(ranges).ok()
}

fn from_uid(from: u32) -> SequenceSet {
    SequenceSet::try_from(vec![Sequence::Range(
        SeqOrUid::Value(NonZeroU32::new(from).unwrap_or(NonZeroU32::MIN)),
        SeqOrUid::Asterisk,
    )])
    .expect("one range is a valid set")
}

fn ranges_of(set: &SequenceSet) -> Vec<(u32, u32)> {
    let number = |v: &SeqOrUid| match v {
        SeqOrUid::Value(n) => n.get(),
        SeqOrUid::Asterisk => u32::MAX,
    };
    set.0
        .as_ref()
        .iter()
        .map(|s| match s {
            Sequence::Single(v) => (number(v), number(v)),
            Sequence::Range(a, b) => (number(a).min(number(b)), number(a).max(number(b))),
        })
        .collect()
}

impl Session {
    /// Connects, signs in with a password, and turns on fast resync if the
    /// server offers it.
    pub async fn connect(server: &Server, username: &str, password: &str) -> Result<Session> {
        let mut conn = Connection::open(server).await?;
        if !conn.capabilities_known() {
            conn.run(CommandBody::Capability, |_| {}).await?;
        }
        if !conn.greeting_was_preauth {
            if conn.has("LOGINDISABLED") {
                return Err(ImapError::Protocol(
                    "the server does not allow signing in on this connection".into(),
                ));
            }
            let login = CommandBody::login(username, password).map_err(|_| {
                ImapError::Protocol("the user name or password cannot be sent over IMAP".into())
            })?;
            conn.run(login, |_| {}).await?;
        }
        // Capabilities usually change after signing in. Ask unless the
        // server volunteered them in its answer.
        conn.run(CommandBody::Capability, |_| {}).await?;

        let mut qresync = false;
        if conn.has("ENABLE") && conn.has("QRESYNC") {
            let enable = CommandBody::enable(vec!["QRESYNC".try_into().expect("valid atom")])
                .expect("one capability is a valid list");
            // If the server refuses, carry on without it.
            qresync = conn.run(enable, |_| {}).await.is_ok();
        }
        Ok(Session { conn, qresync })
    }

    pub fn has(&self, capability: &str) -> bool {
        self.conn.has(capability)
    }

    /// What the server advertises now that we are signed in.
    pub fn capabilities(&self) -> Vec<String> {
        self.conn.capabilities()
    }

    /// Connects without signing in and returns what the server advertises.
    /// A way to check an address, port and certificate before asking for a
    /// password.
    pub async fn probe(server: &Server) -> Result<Vec<String>> {
        let mut conn = Connection::open(server).await?;
        if !conn.capabilities_known() {
            conn.run(CommandBody::Capability, |_| {}).await?;
        }
        let capabilities = conn.capabilities();
        let _ = conn.run(CommandBody::Logout, |_| {}).await;
        Ok(capabilities)
    }

    /// Whether QRESYNC was offered and switched on.
    pub fn qresync(&self) -> bool {
        self.qresync
    }

    /// Every folder in the account.
    pub async fn list(&mut self) -> Result<Vec<RemoteFolder>> {
        let mut folders = Vec::new();
        let command = CommandBody::list("", "*")
            .map_err(|_| ImapError::Protocol("could not build LIST".into()))?;
        self.conn
            .run(command, |untagged| {
                let Untagged::Data(Data::List { items, delimiter, mailbox }) = untagged else {
                    return;
                };
                let raw = raw_name(&mailbox);
                let mut selectable = true;
                let mut special_use = None;
                for item in &items {
                    match item {
                        FlagNameAttribute::Noselect => selectable = false,
                        FlagNameAttribute::Extension(_) => {
                            let name = item.to_string().to_ascii_lowercase();
                            match name.as_str() {
                                "\\nonexistent" => selectable = false,
                                "\\sent" => special_use = Some(SpecialUse::Sent),
                                "\\drafts" => special_use = Some(SpecialUse::Drafts),
                                "\\trash" => special_use = Some(SpecialUse::Trash),
                                "\\junk" => special_use = Some(SpecialUse::Junk),
                                "\\archive" => special_use = Some(SpecialUse::Archive),
                                "\\all" => special_use = Some(SpecialUse::All),
                                "\\flagged" => special_use = Some(SpecialUse::Flagged),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                folders.push(RemoteFolder {
                    name: utf7::decode(&raw),
                    raw_name: raw,
                    delimiter: delimiter.map(|d| d.inner()),
                    selectable,
                    special_use,
                });
            })
            .await?;
        Ok(folders)
    }

    /// A folder's counters, without opening it.
    pub async fn status(&mut self, raw_name: &[u8]) -> Result<FolderStatus> {
        let mut names = vec![
            StatusDataItemName::Messages,
            StatusDataItemName::UidNext,
            StatusDataItemName::UidValidity,
        ];
        if self.conn.has("CONDSTORE") {
            names.push(StatusDataItemName::HighestModSeq);
        }
        let mut status = FolderStatus::default();
        let command = CommandBody::status(mailbox(raw_name)?, names)
            .map_err(|_| ImapError::Protocol("could not build STATUS".into()))?;
        self.conn
            .run(command, |untagged| {
                let Untagged::Data(Data::Status { items, .. }) = untagged else { return };
                for item in items.iter() {
                    match item {
                        StatusDataItem::Messages(n) => status.messages = *n,
                        StatusDataItem::UidNext(n) => status.uid_next = n.get(),
                        StatusDataItem::UidValidity(n) => status.uid_validity = n.get(),
                        StatusDataItem::HighestModSeq(n) => status.highest_modseq = Some(*n),
                        _ => {}
                    }
                }
            })
            .await?;
        Ok(status)
    }

    /// Opens a folder read-only. With `known` (the UIDVALIDITY and
    /// modification sequence from the last sync) and QRESYNC, the server
    /// also reports what has been removed or re-flagged since then.
    pub async fn examine(&mut self, raw_name: &[u8], known: Option<(u32, u64)>) -> Result<Opened> {
        self.open(raw_name, known, true).await
    }

    pub(super) async fn open(&mut self, raw_name: &[u8], known: Option<(u32, u64)>, read_only: bool) -> Result<Opened> {
        let mut parameters = Vec::new();
        if let (true, Some((validity, modseq))) = (self.qresync, known) {
            if let (Some(uid_validity), Some(mod_sequence_value)) =
                (NonZeroU32::new(validity), NonZeroU64::new(modseq))
            {
                parameters.push(SelectParameter::QResync {
                    uid_validity,
                    mod_sequence_value,
                    known_uids: None,
                    seq_match_data: None,
                });
            }
        }
        let command = if read_only {
            CommandBody::Examine {
                mailbox: mailbox(raw_name)?,
                parameters,
            }
        } else {
            CommandBody::Select {
                mailbox: mailbox(raw_name)?,
                parameters,
            }
        };
        let mut opened = Opened::default();
        let mut no_modseq = false;
        let done = self
            .conn
            .run(command, |untagged| match untagged {
                Untagged::Data(Data::Exists(n)) => opened.exists = n,
                Untagged::Data(Data::Vanished { known_uids, .. }) => {
                    opened.vanished.extend(ranges_of(&known_uids))
                }
                Untagged::Data(Data::Fetch { items, .. }) => {
                    if let Some(state) = flag_state(items.as_ref()) {
                        opened.changed.push(state);
                    }
                }
                Untagged::Status(body) => match body.code {
                    Some(Code::UidValidity(n)) => opened.uid_validity = n.get(),
                    Some(Code::UidNext(n)) => opened.uid_next = Some(n.get()),
                    Some(Code::HighestModSeq(n)) => opened.highest_modseq = Some(n.get()),
                    Some(Code::NoModSeq) => no_modseq = true,
                    _ => {}
                },
                _ => {}
            })
            .await?;
        // Some servers put a code on the final answer instead.
        match done.code {
            Some(Code::HighestModSeq(n)) => opened.highest_modseq = Some(n.get()),
            Some(Code::NoModSeq) => no_modseq = true,
            _ => {}
        }
        if no_modseq {
            opened.highest_modseq = None;
        }
        if opened.uid_validity == 0 {
            return Err(ImapError::Protocol("the server gave no UIDVALIDITY for the folder".into()));
        }
        Ok(opened)
    }

    /// UID and flags of every message in the open folder with a UID of
    /// `from` or above; with `changed_since`, only those changed after that
    /// modification sequence.
    pub async fn fetch_flags(&mut self, from: u32, changed_since: Option<u64>) -> Result<Vec<FlagState>> {
        let mut modifiers = Vec::new();
        if let Some(modseq) = changed_since.and_then(NonZeroU64::new) {
            modifiers.push(FetchModifier::ChangedSince(modseq));
        }
        let command = CommandBody::Fetch {
            sequence_set: from_uid(from),
            macro_or_item_names: vec![MessageDataItemName::Uid, MessageDataItemName::Flags].into(),
            uid: true,
            modifiers,
        };
        let mut states = Vec::new();
        self.conn
            .run(command, |untagged| {
                if let Untagged::Data(Data::Fetch { items, .. }) = untagged {
                    // `n:*` always includes the last message, even when its
                    // UID is below n.
                    states.extend(flag_state(items.as_ref()).filter(|s| s.uid >= from));
                }
            })
            .await?;
        Ok(states)
    }

    /// List data for the given UIDs in the open folder. Bodies are not
    /// touched and nothing is marked read.
    pub async fn fetch_headers(&mut self, uids: &[u32]) -> Result<Vec<RemoteMessage>> {
        let Some(sequence_set) = uid_set(uids) else {
            return Ok(Vec::new());
        };
        let fields: Vec<AString> = LIST_HEADERS
            .iter()
            .map(|f| AString::try_from(*f).expect("header names are atoms"))
            .collect();
        let mut names = vec![
            MessageDataItemName::Uid,
            MessageDataItemName::Flags,
            MessageDataItemName::Rfc822Size,
            MessageDataItemName::InternalDate,
            MessageDataItemName::BodyExt {
                section: Some(Section::HeaderFields(
                    None,
                    Vec1::try_from(fields).expect("the list is not empty"),
                )),
                partial: None,
                peek: true,
            },
        ];
        if self.conn.has("CONDSTORE") {
            names.push(MessageDataItemName::ModSeq);
        }
        let command = CommandBody::Fetch {
            sequence_set,
            macro_or_item_names: names.into(),
            uid: true,
            modifiers: Vec::new(),
        };
        let mut messages = Vec::new();
        self.conn
            .run(command, |untagged| {
                let Untagged::Data(Data::Fetch { items, .. }) = untagged else { return };
                let mut message = RemoteMessage {
                    uid: 0,
                    flags: Flags::default(),
                    size: 0,
                    internal_date: 0,
                    modseq: None,
                    header: Vec::new(),
                };
                let mut has_header = false;
                for item in items.into_inner() {
                    match item {
                        MessageDataItem::Uid(uid) => message.uid = uid.get(),
                        MessageDataItem::Flags(flags) => message.flags = to_flags(&flags),
                        MessageDataItem::Rfc822Size(size) => message.size = size,
                        MessageDataItem::InternalDate(date) => {
                            message.internal_date = date.as_ref().timestamp()
                        }
                        MessageDataItem::ModSeq(modseq) => message.modseq = Some(modseq.get()),
                        MessageDataItem::BodyExt { data, .. } => {
                            has_header = true;
                            if let Some(data) = data.0 {
                                message.header = data.into_inner().into_owned();
                            }
                        }
                        _ => {}
                    }
                }
                // A flag-only update for some other message can arrive in
                // the middle of the answers; it is not one of ours.
                if message.uid != 0 && has_header {
                    messages.push(message);
                }
            })
            .await?;
        Ok(messages)
    }

    /// One piece of a message's raw bytes, starting at `offset`. Fewer than
    /// `length` bytes back means the end of the message was reached.
    /// `Ok(None)` means the message is no longer in the folder.
    pub async fn fetch_piece(&mut self, uid: u32, offset: u32, length: u32) -> Result<Option<Vec<u8>>> {
        let (Some(set), Some(length)) = (uid_set(&[uid]), NonZeroU32::new(length)) else {
            return Ok(None);
        };
        let command = CommandBody::Fetch {
            sequence_set: set,
            macro_or_item_names: vec![
                MessageDataItemName::Uid,
                MessageDataItemName::BodyExt {
                    section: None,
                    partial: Some((offset, length)),
                    peek: true,
                },
            ]
            .into(),
            uid: true,
            modifiers: Vec::new(),
        };
        let mut piece = None;
        self.conn
            .run(command, |untagged| {
                let Untagged::Data(Data::Fetch { items, .. }) = untagged else { return };
                let mut this_uid = 0;
                let mut bytes = None;
                for item in items.into_inner() {
                    match item {
                        MessageDataItem::Uid(n) => this_uid = n.get(),
                        MessageDataItem::BodyExt { data, .. } => {
                            bytes = Some(data.0.map(|d| d.into_inner().into_owned()).unwrap_or_default())
                        }
                        _ => {}
                    }
                }
                if this_uid == uid {
                    if let Some(bytes) = bytes {
                        piece = Some(bytes);
                    }
                }
            })
            .await?;
        Ok(piece)
    }

    /// Several whole messages in one request. Meant for small messages,
    /// where a round trip each would dominate. UIDs that no longer exist are
    /// simply absent from the result.
    pub async fn fetch_whole(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<u8>)>> {
        let Some(sequence_set) = uid_set(uids) else {
            return Ok(Vec::new());
        };
        let command = CommandBody::Fetch {
            sequence_set,
            macro_or_item_names: vec![
                MessageDataItemName::Uid,
                MessageDataItemName::BodyExt {
                    section: None,
                    partial: None,
                    peek: true,
                },
            ]
            .into(),
            uid: true,
            modifiers: Vec::new(),
        };
        let mut messages = Vec::new();
        self.conn
            .run(command, |untagged| {
                let Untagged::Data(Data::Fetch { items, .. }) = untagged else { return };
                let mut uid = 0;
                let mut bytes = None;
                for item in items.into_inner() {
                    match item {
                        MessageDataItem::Uid(n) => uid = n.get(),
                        MessageDataItem::BodyExt { data, .. } => {
                            bytes = Some(data.0.map(|d| d.into_inner().into_owned()).unwrap_or_default())
                        }
                        _ => {}
                    }
                }
                if let (true, Some(bytes)) = (uid != 0, bytes) {
                    messages.push((uid, bytes));
                }
            })
            .await?;
        Ok(messages)
    }

    /// A whole message, fetched in pieces. `Ok(None)` if it is gone.
    pub async fn fetch_message(&mut self, uid: u32) -> Result<Option<Vec<u8>>> {
        let mut message = Vec::new();
        loop {
            let Some(piece) = self.fetch_piece(uid, message.len() as u32, BODY_PIECE).await? else {
                return Ok(None);
            };
            let last = (piece.len() as u32) < BODY_PIECE;
            message.extend_from_slice(&piece);
            if last {
                return Ok(Some(message));
            }
        }
    }

    /// Waits for the server to report a change in the open folder, for at
    /// most `limit`, or until `wake` resolves. Returns whether the server
    /// reported anything.
    pub async fn idle(
        &mut self,
        limit: Duration,
        wake: impl std::future::Future<Output = ()> + Send,
    ) -> Result<bool> {
        let seen = self.conn.idle(limit, wake).await?;
        Ok(seen.iter().any(|u| {
            matches!(
                u,
                Untagged::Data(
                    Data::Exists(_) | Data::Expunge(_) | Data::Fetch { .. } | Data::Vanished { .. }
                )
            )
        }))
    }

    pub async fn noop(&mut self) -> Result<()> {
        self.conn.run(CommandBody::Noop, |_| {}).await.map(|_| ())
    }

    /// Says goodbye. Errors are ignored: the connection is going away.
    pub async fn logout(mut self) {
        let _ = self.conn.run(CommandBody::Logout, |_| {}).await;
    }
}

fn flag_state(items: &[MessageDataItem<'_>]) -> Option<FlagState> {
    let mut uid = None;
    let mut flags = None;
    let mut modseq = None;
    for item in items {
        match item {
            MessageDataItem::Uid(n) => uid = Some(n.get()),
            MessageDataItem::Flags(f) => flags = Some(to_flags(f)),
            MessageDataItem::ModSeq(n) => modseq = Some(n.get()),
            _ => {}
        }
    }
    Some(FlagState {
        uid: uid?,
        flags: flags?,
        modseq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_sets_are_compact() {
        let text = |uids: &[u32]| {
            uid_set(uids).map(|set| {
                ranges_of(&set)
                    .iter()
                    .map(|(a, b)| if a == b { a.to_string() } else { format!("{a}:{b}") })
                    .collect::<Vec<_>>()
                    .join(",")
            })
        };
        assert_eq!(text(&[5, 3, 4, 9, 1, 10, 10]).as_deref(), Some("1,3:5,9:10"));
        assert_eq!(text(&[7]).as_deref(), Some("7"));
        assert_eq!(text(&[]), None);
        assert_eq!(text(&[0]), None, "0 is not a UID");
    }
}
