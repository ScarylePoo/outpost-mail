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
//! Mail on disk is served from each account's index (see [`store`]). Until
//! the sync engine exists, an installation with no accounts shows a
//! synthetic one so the interface has something to display.

pub mod api;
pub mod imap;
pub mod render;
pub mod store;
pub mod sync;
pub mod synthetic;

use api::{
    AttachmentInfo, Command, CoreError, ErrorCode, Event, FolderId, FolderInfo, MessageRow,
    MessageView, Response, MAX_PAGE,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use store::index::ListRow;
use store::maildir::Flags;
use store::{Store, StoreError};

/// Where the core sends events. A shell adapter supplies one.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: Event);
}

impl<F: Fn(Event) + Send + Sync + 'static> EventSink for F {
    fn emit(&self, event: Event) {
        self(event)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The data directory. Each subdirectory of `<data_dir>/accounts` is one
    /// account. `None` means no stored mail: the synthetic account is shown.
    pub data_dir: Option<PathBuf>,
}

struct SyntheticFolder {
    info: FolderInfo,
    seed: u64,
}

/// One account on disk. Its id is `acct/<directory name>` and its folders'
/// ids are `acct/<directory name>/<folder number in the index>`.
struct Account {
    id: FolderId,
    name: String,
    store: Store,
}

/// What a folder id refers to.
enum Target<'a> {
    Synthetic(&'a SyntheticFolder),
    Account(&'a Account),
    Folder(&'a Account, i64),
}

/// A handle to the running core. Cheap to clone and safe to share.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    sink: Box<dyn EventSink>,
    accounts: Vec<Account>,
    synthetic: Vec<SyntheticFolder>,
}

impl Core {
    /// Starts a core with no stored mail, showing the synthetic account.
    pub fn start(sink: impl EventSink) -> Self {
        Self::start_with(Config::default(), sink).expect("a core without stored mail cannot fail to start")
    }

    /// Starts a core. Opens each account's index; never scans mail folders.
    pub fn start_with(config: Config, sink: impl EventSink) -> Result<Self, CoreError> {
        let accounts = match &config.data_dir {
            Some(dir) => open_accounts(dir).map_err(internal)?,
            None => Vec::new(),
        };
        let synthetic = if accounts.is_empty() {
            synthetic_folders()
        } else {
            Vec::new()
        };
        Ok(Self {
            inner: Arc::new(Inner {
                sink: Box::new(sink),
                accounts,
                synthetic,
            }),
        })
    }

    /// Handles one command.
    pub fn handle(&self, command: Command) -> Result<Response, CoreError> {
        match command {
            Command::Ping => Ok(Response::Pong {
                core_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            Command::ListFolders => self.list_folders(),
            Command::FolderLayout { folder } => {
                let preview_lens = match self.target(&folder)? {
                    Target::Synthetic(f) => (0..f.info.total)
                        .map(|i| synthetic::preview_len(f.seed, i))
                        .collect(),
                    Target::Folder(account, id) => {
                        account.store.index().preview_lens(id).map_err(internal)?
                    }
                    Target::Account(_) => Vec::new(),
                };
                Ok(Response::FolderLayout {
                    folder,
                    total: preview_lens.len() as u32,
                    preview_lens,
                })
            }
            Command::ListMessages {
                folder,
                offset,
                limit,
            } => {
                let limit = limit.min(MAX_PAGE);
                let (offset, total, rows) = match self.target(&folder)? {
                    Target::Synthetic(f) => {
                        let total = f.info.total;
                        let start = offset.min(total);
                        let end = start.saturating_add(limit).min(total);
                        (start, total, (start..end).map(|i| synthetic::row(f.seed, i)).collect())
                    }
                    Target::Folder(account, id) => {
                        let index = account.store.index();
                        let total = index.folder_total(id).map_err(internal)?;
                        let start = offset.min(total);
                        let rows = index.page(id, start, limit).map_err(internal)?;
                        (start, total, message_rows(rows, start))
                    }
                    Target::Account(_) => (0, 0, Vec::new()),
                };
                Ok(Response::Messages {
                    folder,
                    offset,
                    total,
                    rows,
                })
            }
            Command::OpenMessage { folder, id } => {
                let view = match self.target(&folder)? {
                    Target::Synthetic(f) => {
                        let row = synthetic::row(f.seed, id.clamp(0, u32::MAX as i64) as u32);
                        MessageView {
                            id,
                            subject: row.subject,
                            sender_name: row.sender_name,
                            sender_address: row.sender_address,
                            to: "Synthetic recipient <you@example.com>".into(),
                            cc: String::new(),
                            date: row.date,
                            html: None,
                            text: Some(row.preview),
                            attachments: Vec::new(),
                            remote_content_blocked: false,
                            downloaded: true,
                        }
                    }
                    Target::Folder(account, _) | Target::Account(account) => {
                        open_message(&account.store, id)?
                    }
                };
                Ok(Response::Message { message: view })
            }
            Command::Search { scope, text, limit } => {
                let limit = limit.min(MAX_PAGE);
                let rows = match self.target(&scope)? {
                    // The synthetic account has no text to search.
                    Target::Synthetic(_) => Vec::new(),
                    Target::Account(account) => {
                        account.store.index().search(&text, None, limit).map_err(internal)?
                    }
                    Target::Folder(account, id) => {
                        account.store.index().search(&text, Some(id), limit).map_err(internal)?
                    }
                };
                Ok(Response::SearchResults {
                    rows: message_rows(rows, 0),
                })
            }
            Command::StartDemoSync { folder } => {
                let total = match self.target(&folder)? {
                    Target::Synthetic(f) => f.info.total,
                    Target::Folder(account, id) => {
                        account.store.index().folder_total(id).map_err(internal)?
                    }
                    Target::Account(_) => 0,
                };
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

    fn list_folders(&self) -> Result<Response, CoreError> {
        let mut folders: Vec<FolderInfo> =
            self.inner.synthetic.iter().map(|f| f.info.clone()).collect();
        for account in &self.inner.accounts {
            folders.push(FolderInfo {
                id: account.id.clone(),
                name: account.name.clone(),
                depth: 0,
                total: 0,
                unread: 0,
                role: None,
            });
            folders.extend(folder_tree(&account.id, account.store.index().folders().map_err(internal)?));
        }
        Ok(Response::Folders { folders })
    }

    fn target(&self, id: &FolderId) -> Result<Target<'_>, CoreError> {
        let unknown = || CoreError::new(ErrorCode::UnknownFolder, format!("no folder {id:?}"));
        if let Some(f) = self.inner.synthetic.iter().find(|f| &f.info.id == id) {
            return Ok(Target::Synthetic(f));
        }
        for account in &self.inner.accounts {
            if id == &account.id {
                return Ok(Target::Account(account));
            }
            let folder = id
                .strip_prefix(account.id.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
                .and_then(|n| n.parse::<i64>().ok());
            if let Some(folder) = folder {
                return match account.store.index().folder_path(folder).map_err(internal)? {
                    Some(_) => Ok(Target::Folder(account, folder)),
                    None => Err(unknown()),
                };
            }
        }
        Err(unknown())
    }
}

/// What a folder is for: from the server's marker if it gave one, otherwise
/// from the names mail servers and clients commonly use.
fn role_of(folder: &store::index::FolderRow) -> Option<&'static str> {
    if folder.path.eq_ignore_ascii_case("INBOX") {
        return Some("inbox");
    }
    match folder.special_use.as_deref() {
        Some("sent") => return Some("sent"),
        Some("drafts") => return Some("drafts"),
        Some("trash") => return Some("trash"),
        Some("junk") => return Some("junk"),
        Some("archive") | Some("all") => return Some("archive"),
        _ => {}
    }
    match folder.name.to_lowercase().as_str() {
        "sent" | "sent items" | "sent mail" | "sent messages" => Some("sent"),
        "drafts" | "draft" => Some("drafts"),
        "trash" | "deleted items" | "deleted messages" | "bin" => Some("trash"),
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" => Some("junk"),
        "archive" | "archives" => Some("archive"),
        _ => None,
    }
}

/// Lays an account's folders out as the tree shows them: the inbox first,
/// then drafts, sent, archive, junk and trash, then everything else by name.
/// On servers that keep every folder inside the inbox, those folders are
/// shown beside it rather than under it.
fn folder_tree(account_id: &str, rows: Vec<store::index::FolderRow>) -> Vec<FolderInfo> {
    fn rank(role: Option<&str>) -> u8 {
        match role {
            Some("inbox") => 0,
            Some("drafts") => 1,
            Some("sent") => 2,
            Some("archive") => 3,
            Some("junk") => 4,
            Some("trash") => 5,
            _ => 6,
        }
    }
    fn parent(path: &str) -> &str {
        path.rsplit_once('/').map_or("", |(p, _)| p)
    }
    fn place(
        account_id: &str,
        rows: &[store::index::FolderRow],
        under: &str,
        depth: u8,
        hoist_inbox_children: bool,
        out: &mut Vec<FolderInfo>,
    ) {
        let mut here: Vec<&store::index::FolderRow> =
            rows.iter().filter(|r| parent(&r.path) == under).collect();
        if hoist_inbox_children {
            let inbox = rows.iter().find(|r| r.path.eq_ignore_ascii_case("INBOX"));
            if let Some(inbox) = inbox {
                here.extend(rows.iter().filter(|r| parent(&r.path) == inbox.path));
            }
        }
        here.sort_by_key(|r| (rank(role_of(r)), r.name.to_lowercase()));
        for row in here {
            let role = role_of(row);
            out.push(FolderInfo {
                id: format!("{account_id}/{}", row.id),
                name: if role == Some("inbox") { "Inbox".into() } else { row.name.clone() },
                depth,
                total: row.total,
                unread: row.unread,
                role: role.map(String::from),
            });
            if !(hoist_inbox_children && role == Some("inbox")) {
                place(account_id, rows, &row.path, depth.saturating_add(1), false, out);
            }
        }
    }
    let top: Vec<&store::index::FolderRow> = rows.iter().filter(|r| parent(&r.path).is_empty()).collect();
    let only_inbox = top.len() == 1 && top[0].path.eq_ignore_ascii_case("INBOX") && rows.len() > 1;
    let mut out = Vec::with_capacity(rows.len());
    place(account_id, &rows, "", 1, only_inbox, &mut out);
    out
}

fn internal(e: StoreError) -> CoreError {
    CoreError::new(ErrorCode::Internal, e.to_string())
}

/// Reads a message from its file and prepares it for the reading pane.
fn open_message(store: &Store, id: i64) -> Result<MessageView, CoreError> {
    let raw = match store.read_message(id) {
        Ok(raw) => raw,
        Err(StoreError::NotDownloaded(_)) => {
            return Ok(MessageView {
                id,
                downloaded: false,
                ..MessageView::default()
            })
        }
        Err(StoreError::NoSuchMessage(_)) => {
            return Err(CoreError::new(ErrorCode::UnknownMessage, format!("no message {id}")))
        }
        Err(e) => return Err(internal(e)),
    };
    let shown = store::parse::display(&raw);
    let safe = shown.html.as_deref().map(render::safe_html);
    Ok(MessageView {
        id,
        subject: shown.subject,
        sender_name: shown.from_name,
        sender_address: shown.from_addr,
        to: shown.to,
        cc: shown.cc,
        date: shown.date.unwrap_or(0),
        remote_content_blocked: safe.as_ref().is_some_and(|s| s.remote_content_blocked),
        html: safe.map(|s| s.html),
        text: shown.text,
        attachments: shown
            .attachments
            .into_iter()
            .map(|a| AttachmentInfo {
                name: a.name,
                mime_type: a.mime_type,
                size: a.size,
            })
            .collect(),
        downloaded: true,
    })
}

fn message_rows(rows: Vec<ListRow>, first_index: u32) -> Vec<MessageRow> {
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| MessageRow {
            id: r.id,
            index: first_index + i as u32,
            sender_name: if r.from_name.is_empty() { r.from_addr.clone() } else { r.from_name },
            sender_address: r.from_addr,
            date: r.date,
            subject: r.subject,
            preview: r.preview,
            unread: !r.flags.contains(Flags::SEEN),
            has_attachment: r.has_attachment,
        })
        .collect()
}

fn open_accounts(data_dir: &Path) -> Result<Vec<Account>, StoreError> {
    let root = data_dir.join("accounts");
    std::fs::create_dir_all(&root)?;
    let mut names: Vec<String> = std::fs::read_dir(&root)?
        .filter_map(|entry| entry.ok())
        // `is_dir` on the path follows links, so an account may be a link to
        // a directory on another disk.
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            Ok(Account {
                id: format!("acct/{name}"),
                store: Store::open(root.join(&name))?,
                name,
            })
        })
        .collect()
}

fn synthetic_folders() -> Vec<SyntheticFolder> {
    let folder = |id: &str, name: &str, depth: u8, total: u32, seed: u64| SyntheticFolder {
        info: FolderInfo {
            id: id.to_string(),
            name: name.to_string(),
            depth,
            total,
            // Matches synthetic::row, which marks 12% unread.
            unread: total / 100 * 12,
            role: (name == "Inbox").then(|| "inbox".to_string()),
        },
        seed,
    };
    vec![
        folder("demo", "Synthetic account", 0, 0, 0),
        folder("demo/inbox", "Inbox", 1, 100_000, 1),
        folder("demo/archive", "Archive", 1, 200_000, 2),
        folder("demo/sent", "Sent", 1, 20_000, 3),
        folder("demo/small", "Small folder", 1, 40, 4),
        folder("demo/empty", "Empty folder", 1, 0, 5),
    ]
}
