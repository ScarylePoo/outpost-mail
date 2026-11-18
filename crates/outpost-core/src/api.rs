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
    /// Full-text search, newest first. Every word must match; the last word
    /// matches as a prefix unless the text ends with a space.
    Search {
        /// What to search: an account's id for all of its mail, or a
        /// folder's id for that folder alone.
        scope: FolderId,
        text: String,
        limit: u32,
    },
    /// Milestone 0 only: makes the core push a burst of progress events, to
    /// prove the event path end to end.
    StartDemoSync { folder: FolderId },
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
    /// The command was accepted; results arrive as events.
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    SyncProgress {
        folder: FolderId,
        done: u32,
        total: u32,
    },
    SyncFinished {
        folder: FolderId,
    },
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
    pub has_attachment: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The command could not be decoded.
    BadCommand,
    UnknownFolder,
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
