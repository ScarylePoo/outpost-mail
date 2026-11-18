//! The wire types of the command-and-event interface.
//!
//! Everything the interface can ask of the core is a [`Command`]; every answer
//! is a [`Response`] or a [`CoreError`]; everything the core says unprompted is
//! an [`Event`]. All of them serialise to JSON with a `type` tag and camelCase
//! fields. `ui/src/core/api.ts` mirrors these types by hand and
//! `tests/wire_format.rs` pins the JSON shape, so a change here that the
//! interface would not understand fails a test.
//!
//! Nothing in this module may depend on a shell (Tauri, Electron).

use serde::{Deserialize, Serialize};

/// Identifies a folder. Opaque to the interface.
pub type FolderId = String;

/// The largest page `ListMessages` will return. Larger requests are clamped.
pub const MAX_PAGE: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    /// Liveness and version check.
    Ping,
    /// The folder tree, flattened, in display order.
    ListFolders,
    /// What the list needs to lay out a whole folder without loading it:
    /// the row count and each row's preview length in characters.
    FolderLayout { folder: FolderId },
    /// One page of rows, in the folder's sort order.
    ListMessages {
        folder: FolderId,
        offset: u32,
        limit: u32,
    },
    /// A message, prepared for reading. `folder` is any folder of the
    /// message's account.
    OpenMessage { folder: FolderId, id: i64 },
    /// The whole conversation a message belongs to, across folders, each
    /// message reduced to what its sender wrote.
    OpenThread { folder: FolderId, id: i64 },
    /// Full-text search, newest first. Every word must match; the last word
    /// matches as a prefix unless the text ends with a space.
    Search {
        /// What to search: an account's id for all of its mail, or a
        /// folder's id for that folder alone.
        scope: FolderId,
        text: String,
        limit: u32,
    },
    /// Marks messages read or unread, flagged or not. `None` leaves that
    /// mark as it is. Shown at once; the server is told in the background.
    /// `folder` is any folder of the messages' account.
    ChangeFlags {
        folder: FolderId,
        ids: Vec<i64>,
        seen: Option<bool>,
        flagged: Option<bool>,
    },
    /// Moves messages to another folder of the same account.
    MoveMessages { ids: Vec<i64>, to: FolderId },
    /// Moves messages to the account's archive folder.
    ArchiveMessages { folder: FolderId, ids: Vec<i64> },
    /// Deletes messages. Normally that moves them to the trash folder.
    /// With `permanently`, they are removed for good: the interface asks
    /// the user before sending that.
    DeleteMessages {
        folder: FolderId,
        ids: Vec<i64>,
        permanently: bool,
    },
    /// Creates a folder. `parent` is the folder to create it in, or an
    /// account's id for the top level.
    CreateFolder { parent: FolderId, name: String },
    /// Deletes a folder and the mail in it. The interface asks first.
    DeleteFolder { folder: FolderId },
    /// Writes one of a message's attachments to the downloads folder,
    /// under a name that does not replace anything there.
    SaveAttachment { folder: FolderId, id: i64, index: u32 },
    /// Where a message now is in a folder's list, so the list can keep its
    /// place when the folder changes underneath it.
    LocateMessage { folder: FolderId, id: i64 },
    /// The accounts and what each one's sync is doing.
    ListAccounts,
    /// Signs in to check the details, then adds the account and starts
    /// syncing it. Answers `Accepted` at once; the outcome arrives as an
    /// `AccountAdded` or `AccountSetupFailed` event.
    AddAccount {
        host: String,
        port: u16,
        /// False only when the user has explicitly chosen an unencrypted
        /// connection for this account.
        encrypted: bool,
        username: String,
        password: Secret,
        /// Keep the password in the operating system's credential store.
        remember: bool,
        /// How to send mail from the account. Can be left out and set later.
        #[serde(default)]
        sending: Option<SendingSettings>,
    },
    /// Sets or changes how an account sends mail.
    SetSending { account: FolderId, sending: SendingSettings },
    /// The starting point for a reply to, or a forward of, a message.
    /// `mode` is `reply`, `replyAll` or `forward`.
    ComposeReply { folder: FolderId, id: i64, mode: String },
    /// A stored draft (or a message left in the outbox), for editing.
    EditDraft { folder: FolderId, id: i64 },
    /// Saves what the composer holds as a draft, here and on the server.
    SaveDraft { account: FolderId, draft: DraftContent },
    /// Puts a message in the outbox. It is sent after `delay_seconds`,
    /// during which `CancelSend` takes it back.
    SendMessage {
        account: FolderId,
        draft: DraftContent,
        delay_seconds: u32,
    },
    /// Takes a message back out of the outbox and makes it a draft again.
    CancelSend { account: FolderId, id: i64 },
    /// Supplies the password for an account that has none stored, or whose
    /// stored one was refused, and starts syncing it.
    ProvidePassword {
        account: FolderId,
        password: Secret,
        remember: bool,
    },
    /// Checks every folder of an account now instead of at the next poll.
    SyncNow { account: FolderId },
}

/// A password in transit between the interface and the core. It prints as
/// `***` so that it cannot end up in a log by way of a debug message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Response {
    Pong {
        core_version: String,
    },
    Folders {
        folders: Vec<FolderInfo>,
    },
    FolderLayout {
        folder: FolderId,
        total: u32,
        preview_lens: Vec<u16>,
    },
    Messages {
        folder: FolderId,
        offset: u32,
        total: u32,
        rows: Vec<MessageRow>,
    },
    SearchResults {
        rows: Vec<MessageRow>,
    },
    Message {
        message: MessageView,
    },
    Thread {
        thread: ThreadView,
    },
    AttachmentSaved {
        /// Where it was written.
        path: String,
        /// The file name it was given.
        name: String,
    },
    Draft {
        draft: DraftContent,
    },
    DraftSaved {
        id: i64,
    },
    SendQueued {
        /// The message's row in the outbox.
        id: i64,
    },
    SendCancelled {
        /// False if the message had already gone.
        cancelled: bool,
    },
    FolderCreated {
        folder: FolderId,
    },
    Located {
        /// `None` if the message is no longer in the folder.
        index: Option<u32>,
    },
    Accounts {
        accounts: Vec<AccountInfo>,
    },
    /// The command was accepted; results arrive as events.
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    /// What an account's sync is doing changed.
    AccountStatus {
        account: FolderId,
        status: SyncStatus,
    },
    /// The contents of a folder changed: messages arrived, left, were
    /// re-flagged or gained their body.
    FolderChanged { folder: FolderId },
    /// An account's set of folders changed.
    FoldersChanged { account: FolderId },
    /// Something the user should be told, once: an action the server
    /// refused, for example.
    Notice { account: FolderId, text: String },
    /// `AddAccount` succeeded.
    AccountAdded { account: FolderId },
    /// `AddAccount` failed. Nothing was saved.
    AccountSetupFailed { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    /// The account is only being browsed; nothing syncs it.
    NotSyncing,
    /// The index is being rebuilt from the mail files on this computer.
    Indexing,
    /// A password is needed before sync can start.
    NeedsPassword,
    Connecting,
    Syncing,
    /// Up to date and waiting for changes.
    UpToDate,
    /// The connection failed; another attempt is scheduled.
    Offline,
    /// The server refused the password.
    SignInFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: SyncState,
    /// A sentence for the status bar, or empty.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub id: FolderId,
    pub name: String,
    /// Empty for an account that is only browsed.
    pub host: String,
    pub username: String,
    pub status: SyncStatus,
    /// How the account sends mail, if that has been set up.
    pub sending: Option<SendingSettings>,
}

/// Who mail is from and which server sends it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendingSettings {
    /// The name recipients see.
    pub name: String,
    /// The address mail is sent from.
    pub address: String,
    pub host: String,
    pub port: u16,
    /// `tls` (usually port 465), `starttls` (usually 587) or `plaintext`.
    pub security: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub id: FolderId,
    pub name: String,
    /// Nesting depth in the tree; 0 is an account.
    pub depth: u8,
    pub total: u32,
    pub unread: u32,
    /// What the folder is for, when known: `inbox`, `drafts`, `sent`,
    /// `archive`, `junk` or `trash`.
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRow {
    /// Identifies the message within its account.
    pub id: i64,
    /// Position in the folder's current sort order, or in the search results.
    pub index: u32,
    pub sender_name: String,
    pub sender_address: String,
    /// Unix time, seconds, UTC.
    pub date: i64,
    pub subject: String,
    /// The sender's own text only: no quoted history, no signature.
    pub preview: String,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachment: bool,
}

/// What the composer holds. The same shape is used for a new message, a
/// reply prepared by the core, and a draft reopened for editing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftContent {
    /// Recipients as typed: `Name <address>` or bare addresses, separated
    /// by commas or semicolons.
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    /// Plain text.
    pub body: String,
    /// The Message-ID being replied to, without angle brackets.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<DraftAttachment>,
    /// The message this is a reply to. It is marked answered on sending.
    pub reply_to_message: Option<i64>,
    /// The message being forwarded. Its attachments go with it, and it is
    /// marked forwarded on sending.
    pub forward_of: Option<i64>,
    /// The stored draft this replaces when saved or sent.
    pub replaces: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DraftAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
}

/// A message as the reading pane shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub id: i64,
    pub subject: String,
    pub sender_name: String,
    pub sender_address: String,
    /// Recipients as display text.
    pub to: String,
    pub cc: String,
    /// Unix time, seconds, UTC. 0 if the message has no usable date.
    pub date: i64,
    /// The HTML body, already made safe to display (see `render.rs`).
    /// `None` if the message has no HTML part.
    pub html: Option<String>,
    /// The plain-text body, if there is one.
    pub text: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    /// The message tried to load pictures from the network.
    pub remote_content_blocked: bool,
    /// False while only the message's list data has been synced.
    pub downloaded: bool,
}

/// A conversation, oldest message first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub subject: String,
    pub messages: Vec<ThreadMessage>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMessage {
    pub id: i64,
    /// The folder this copy is in, and its name for display.
    pub folder: FolderId,
    pub folder_name: String,
    /// The message this one replies to, if it is in the conversation.
    pub parent: Option<i64>,
    /// That link was inferred from the subject: the message had no reply
    /// headers.
    pub guessed: bool,
    pub sender_name: String,
    pub sender_address: String,
    pub recipients: String,
    pub date: i64,
    pub unread: bool,
    pub flagged: bool,
    /// False while only the message's list data has been synced; `text` is
    /// then just the preview.
    pub downloaded: bool,
    /// What the sender wrote, as plain text.
    pub text: String,
    /// The quoted history that was set aside, and how many lines it is.
    pub quoted: String,
    pub quoted_lines: u32,
    /// The signature or disclaimer that was set aside.
    pub signature: String,
    pub attachments: Vec<AttachmentInfo>,
    /// People this message brought into the conversation, compared with
    /// the message it replies to, and people it left out.
    pub added: Vec<String>,
    pub dropped: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    pub name: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The command could not be decoded.
    BadCommand,
    UnknownFolder,
    UnknownMessage,
    /// The command cannot be carried out as things stand.
    NotPossible,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for CoreError {}
