//! Outpost Mail core.
//!
//! The core owns all mail work. The interface talks to it only through
//! [`Core::handle`] (commands in, responses out) and an [`EventSink`] (events
//! pushed by the core). A shell adapter is the few lines that connect those
//! two to whatever the shell uses for messaging; see `crates/shell-tauri`.
//!
//! Rules for this crate:
//! - No dependency on any shell.
//! - `handle` returns quickly. Anything slow is started in the background and
//!   reports through events.
//!
//! In milestone 0 the mailbox behind the interface is synthetic.

pub mod api;
pub mod store;
pub mod synthetic;

use api::{
    Command, CoreError, ErrorCode, Event, FolderId, FolderInfo, Response, MAX_PAGE,
};
use std::sync::Arc;
use std::time::Duration;

/// Where the core sends events. A shell adapter supplies one.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: Event);
}

impl<F: Fn(Event) + Send + Sync + 'static> EventSink for F {
    fn emit(&self, event: Event) {
        self(event)
    }
}

struct StubFolder {
    info: FolderInfo,
    seed: u64,
}

/// A handle to the running core. Cheap to clone and safe to share.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    sink: Box<dyn EventSink>,
    folders: Vec<StubFolder>,
}

impl Core {
    pub fn start(sink: impl EventSink) -> Self {
        let folder = |id: &str, name: &str, depth: u8, total: u32, seed: u64| StubFolder {
            info: FolderInfo {
                id: id.to_string(),
                name: name.to_string(),
                depth,
                total,
                // Matches synthetic::row, which marks 12% unread.
                unread: total / 100 * 12,
            },
            seed,
        };
        Self {
            inner: Arc::new(Inner {
                sink: Box::new(sink),
                folders: vec![
                    folder("demo", "Synthetic account", 0, 0, 0),
                    folder("demo/inbox", "Inbox", 1, 100_000, 1),
                    folder("demo/archive", "Archive", 1, 200_000, 2),
                    folder("demo/sent", "Sent", 1, 20_000, 3),
                    folder("demo/small", "Small folder", 1, 40, 4),
                    folder("demo/empty", "Empty folder", 1, 0, 5),
                ],
            }),
        }
    }

    /// Handles one command.
    pub fn handle(&self, command: Command) -> Result<Response, CoreError> {
        match command {
            Command::Ping => Ok(Response::Pong {
                core_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            Command::ListFolders => Ok(Response::Folders {
                folders: self.inner.folders.iter().map(|f| f.info.clone()).collect(),
            }),
            Command::FolderLayout { folder } => {
                let f = self.folder(&folder)?;
                Ok(Response::FolderLayout {
                    total: f.info.total,
                    preview_lens: (0..f.info.total)
                        .map(|i| synthetic::preview_len(f.seed, i))
                        .collect(),
                    folder,
                })
            }
            Command::ListMessages {
                folder,
                offset,
                limit,
            } => {
                let f = self.folder(&folder)?;
                let total = f.info.total;
                let start = offset.min(total);
                let end = start.saturating_add(limit.min(MAX_PAGE)).min(total);
                Ok(Response::Messages {
                    rows: (start..end).map(|i| synthetic::row(f.seed, i)).collect(),
                    folder,
                    offset: start,
                    total,
                })
            }
            Command::StartDemoSync { folder } => {
                let total = self.folder(&folder)?.info.total;
                let core = self.clone();
                std::thread::spawn(move || {
                    const STEPS: u32 = 20;
                    for step in 1..=STEPS {
                        std::thread::sleep(Duration::from_millis(100));
                        core.inner.sink.emit(Event::SyncProgress {
                            folder: folder.clone(),
                            done: (total as u64 * step as u64 / STEPS as u64) as u32,
                            total,
                        });
                    }
                    core.inner.sink.emit(Event::SyncFinished { folder });
                });
                Ok(Response::Accepted)
            }
        }
    }

    /// The same interface as [`Core::handle`], as JSON text in and out, for
    /// shells that can only pass strings. The reply is
    /// `{"ok": <Response>}` or `{"error": <CoreError>}`.
    pub fn handle_json(&self, command: &str) -> String {
        let result = serde_json::from_str::<Command>(command)
            .map_err(|e| CoreError::new(ErrorCode::BadCommand, e.to_string()))
            .and_then(|c| self.handle(c));
        let reply = match result {
            Ok(r) => serde_json::json!({ "ok": r }),
            Err(e) => serde_json::json!({ "error": e }),
        };
        reply.to_string()
    }

    fn folder(&self, id: &FolderId) -> Result<&StubFolder, CoreError> {
        self.inner
            .folders
            .iter()
            .find(|f| &f.info.id == id)
            .ok_or_else(|| {
                CoreError::new(ErrorCode::UnknownFolder, format!("no folder {id:?}"))
            })
    }
}
