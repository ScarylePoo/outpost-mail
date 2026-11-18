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

pub mod accounts;
pub mod api;
pub mod imap;
pub mod render;
pub mod store;
pub mod sync;
pub mod synthetic;

use accounts::{AccountConfig, OsSecrets, Secrets};
use api::{
    AccountInfo, AttachmentInfo, Command, CoreError, ErrorCode, Event, FolderId, FolderInfo,
    MessageRow, MessageView, Response, SyncState, SyncStatus, MAX_PAGE,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use store::index::ListRow;
use store::maildir::Flags;
use store::{Store, StoreError};
use sync::{Progress, StopHandle};

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
    /// account. `None` means no stored mail: the synthetic account is shown
    /// and accounts cannot be added.
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
    /// The directory name, which doubles as the display name.
    name: String,
    store: Arc<Store>,
    /// `None` for mail that is only browsed (no server details saved).
    config: Option<AccountConfig>,
    live: Mutex<Live>,
    /// Asks the running sync for a full pass now.
    now: Arc<tokio::sync::Notify>,
}

/// The parts of an account that change while the app runs.
struct Live {
    /// Held in memory only. See `accounts.rs`.
    password: Option<String>,
    /// Stops the running sync when dropped or told to.
    running: Option<StopHandle>,
    status: SyncStatus,
}

/// What a folder id refers to.
enum Target<'a> {
    Synthetic(&'a SyntheticFolder),
    Account(Arc<Account>),
    Folder(Arc<Account>, i64),
}

/// A handle to the running core. Cheap to clone and safe to share.
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    sink: Box<dyn EventSink>,
    data_dir: Option<PathBuf>,
    accounts: RwLock<Vec<Arc<Account>>>,
    synthetic: Vec<SyntheticFolder>,
    secrets: Arc<dyn Secrets>,
    /// Runs the sync engine. Commands never wait on it.
    runtime: tokio::runtime::Runtime,
}

fn status(state: SyncState, detail: impl Into<String>) -> SyncStatus {
    SyncStatus {
        state,
        detail: detail.into(),
    }
}

impl Core {
    /// Starts a core with no stored mail, showing the synthetic account.
    pub fn start(sink: impl EventSink) -> Self {
        Self::start_with(Config::default(), sink).expect("a core without stored mail cannot fail to start")
    }

    /// Starts a core, keeping passwords in the operating system's
    /// credential store. Opens each account's index (never scanning mail
    /// folders) and starts syncing every account whose password is stored.
    pub fn start_with(config: Config, sink: impl EventSink) -> Result<Self, CoreError> {
        Self::start_with_secrets(config, Arc::new(OsSecrets), sink)
    }

    /// As [`Core::start_with`], with passwords kept somewhere else.
    pub fn start_with_secrets(
        config: Config,
        secrets: Arc<dyn Secrets>,
        sink: impl EventSink,
    ) -> Result<Self, CoreError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("outpost-sync")
            .enable_all()
            .build()
            .map_err(|e| CoreError::new(ErrorCode::Internal, e.to_string()))?;
        let accounts = match &config.data_dir {
            Some(dir) => open_accounts(dir).map_err(internal)?,
            None => Vec::new(),
        };
        let core = Self {
            inner: Arc::new(Inner {
                sink: Box::new(sink),
                data_dir: config.data_dir,
                accounts: RwLock::new(accounts),
                synthetic: synthetic_folders(),
                secrets,
                runtime,
            }),
        };
        for account in core.accounts() {
            let Some(config) = &account.config else { continue };
            match core.inner.secrets.get(&config.secret_key(&account.name)) {
                Some(password) => {
                    account.live.lock().unwrap().password = Some(password);
                    core.start_sync(&account);
                }
                None => core.set_status(&account, status(SyncState::NeedsPassword, "")),
            }
        }
        Ok(core)
    }

    fn accounts(&self) -> Vec<Arc<Account>> {
        self.inner.accounts.read().unwrap().clone()
    }

    fn account(&self, id: &FolderId) -> Result<Arc<Account>, CoreError> {
        match self.target(id)? {
            Target::Account(account) | Target::Folder(account, _) => Ok(account),
            Target::Synthetic(_) => Err(CoreError::new(
                ErrorCode::NotPossible,
                "the synthetic account has no server",
            )),
        }
    }

    fn set_status(&self, account: &Account, new: SyncStatus) {
        {
            let mut live = account.live.lock().unwrap();
            if live.status == new {
                return;
            }
            live.status = new.clone();
        }
        self.inner.sink.emit(Event::AccountStatus {
            account: account.id.clone(),
            status: new,
        });
    }

    /// Starts (or restarts) an account's sync with the password it holds.
    fn start_sync(&self, account: &Arc<Account>) {
        let Some(config) = account.config.clone() else { return };
        let (handle, stop) = sync::stop_signal();
        let password = {
            let mut live = account.live.lock().unwrap();
            let Some(password) = live.password.clone() else { return };
            // Replacing the handle stops whichever sync was running.
            if let Some(old) = live.running.replace(handle) {
                old.stop();
            }
            password
        };
        let core = self.clone();
        let account = account.clone();
        self.inner.runtime.spawn(async move {
            let server_account = sync::Account {
                server: config.server(),
                username: config.username.clone(),
            };
            let report = {
                let (core, account) = (core.clone(), account.clone());
                move |progress: Progress| core.report(&account, progress)
            };
            sync::run(
                &account.store,
                &server_account,
                &password,
                &sync::Options::default(),
                stop,
                &account.now,
                report,
            )
            .await;
        });
    }

    /// Turns the sync engine's progress into status and events.
    fn report(&self, account: &Account, progress: Progress) {
        let count = |n: usize| n.to_string();
        match progress {
            Progress::Connecting => self.set_status(account, status(SyncState::Connecting, "")),
            Progress::Folders { .. } => self.inner.sink.emit(Event::FoldersChanged {
                account: account.id.clone(),
            }),
            Progress::Headers { folder, done, total } => self.set_status(
                account,
                status(
                    SyncState::Syncing,
                    format!("{folder}: message list {} of {}", count(done), count(total)),
                ),
            ),
            Progress::Bodies { folder, done, total } => self.set_status(
                account,
                status(
                    SyncState::Syncing,
                    format!("{folder}: downloading {} of {}", count(done), count(total)),
                ),
            ),
            Progress::FolderChanged { folder_id } => self.inner.sink.emit(Event::FolderChanged {
                folder: format!("{}/{folder_id}", account.id),
            }),
            Progress::Idle => self.set_status(account, status(SyncState::UpToDate, "")),
            Progress::Offline { error, retry_in } => self.set_status(
                account,
                status(
                    SyncState::Offline,
                    format!("{error}. Trying again in {} s.", retry_in.as_secs()),
                ),
            ),
            Progress::SignInFailed { error } => {
                account.live.lock().unwrap().running = None;
                self.set_status(account, status(SyncState::SignInFailed, error));
            }
        }
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
            Command::LocateMessage { folder, id } => {
                let index = match self.target(&folder)? {
                    Target::Synthetic(f) => (id >= 0 && id < f.info.total as i64).then_some(id as u32),
                    Target::Folder(account, folder_id) => {
                        account.store.index().position(folder_id, id).map_err(internal)?
                    }
                    Target::Account(_) => None,
                };
                Ok(Response::Located { index })
            }
            Command::ListAccounts => Ok(Response::Accounts {
                accounts: self
                    .accounts()
                    .iter()
                    .map(|a| AccountInfo {
                        id: a.id.clone(),
                        name: a.name.clone(),
                        host: a.config.as_ref().map(|c| c.host.clone()).unwrap_or_default(),
                        username: a.config.as_ref().map(|c| c.username.clone()).unwrap_or_default(),
                        status: a.live.lock().unwrap().status.clone(),
                    })
                    .collect(),
            }),
            Command::AddAccount {
                host,
                port,
                encrypted,
                username,
                password,
                remember,
            } => {
                let Some(data_dir) = self.inner.data_dir.clone() else {
                    return Err(CoreError::new(
                        ErrorCode::NotPossible,
                        "there is no data directory to keep an account in",
                    ));
                };
                let config = AccountConfig {
                    host: host.trim().to_string(),
                    port,
                    encrypted,
                    username: username.trim().to_string(),
                };
                if config.host.is_empty() || config.username.is_empty() || password.0.is_empty() {
                    return Err(CoreError::new(
                        ErrorCode::NotPossible,
                        "the server, the user name and the password are all needed",
                    ));
                }
                let core = self.clone();
                self.inner.runtime.spawn(async move {
                    let outcome = core.add_account(data_dir, config, password.0, remember).await;
                    core.inner.sink.emit(match outcome {
                        Ok(account) => Event::AccountAdded { account },
                        Err(error) => Event::AccountSetupFailed { error },
                    });
                });
                Ok(Response::Accepted)
            }
            Command::ProvidePassword {
                account,
                password,
                remember,
            } => {
                let account = self.account(&account)?;
                let Some(config) = &account.config else {
                    return Err(CoreError::new(ErrorCode::NotPossible, "this account has no server"));
                };
                if remember {
                    // Not being able to remember it is not a reason to
                    // refuse to sync; it will be asked for again next time.
                    let _ = self.inner.secrets.set(&config.secret_key(&account.name), &password.0);
                }
                account.live.lock().unwrap().password = Some(password.0);
                self.start_sync(&account);
                Ok(Response::Accepted)
            }
            Command::SyncNow { account } => {
                let account = self.account(&account)?;
                if account.config.is_none() {
                    return Err(CoreError::new(ErrorCode::NotPossible, "this account has no server"));
                }
                let (running, has_password) = {
                    let live = account.live.lock().unwrap();
                    (live.running.is_some(), live.password.is_some())
                };
                if !has_password {
                    return Err(CoreError::new(ErrorCode::NotPossible, "the account needs its password first"));
                }
                if running {
                    account.now.notify_one();
                } else {
                    self.start_sync(&account);
                }
                Ok(Response::Accepted)
            }
        }
    }

    /// Checks the details by signing in, then saves the account and starts
    /// syncing it. Returns the new account's id, or what went wrong in words
    /// for the user. Nothing is saved unless the sign-in worked.
    async fn add_account(
        &self,
        data_dir: PathBuf,
        config: AccountConfig,
        password: String,
        remember: bool,
    ) -> Result<FolderId, String> {
        let duplicate = self.accounts().iter().any(|a| {
            a.config.as_ref().is_some_and(|c| {
                c.host.eq_ignore_ascii_case(&config.host) && c.username.eq_ignore_ascii_case(&config.username)
            })
        });
        if duplicate {
            return Err("This account has already been added.".into());
        }
        let session = imap::Session::connect(&config.server(), &config.username, &password)
            .await
            .map_err(|e| e.to_string())?;
        session.logout().await;

        let core = self.clone();
        tokio::task::spawn_blocking(move || {
            let root = data_dir.join("accounts");
            let name = accounts::directory_name(&config.username, |n| root.join(n).exists());
            let dir = root.join(&name);
            let store = Store::open(&dir).map_err(|e| e.to_string())?;
            config.save(&dir).map_err(|e| format!("could not save the account: {e}"))?;
            if remember {
                // If there is no credential store, the password simply is
                // not remembered: it stays in memory until the app closes.
                let _ = core.inner.secrets.set(&config.secret_key(&name), &password);
            }
            let account = Arc::new(Account {
                id: format!("acct/{name}"),
                name,
                store: Arc::new(store),
                config: Some(config),
                live: Mutex::new(Live {
                    password: Some(password),
                    running: None,
                    status: status(SyncState::Connecting, ""),
                }),
                now: Arc::default(),
            });
            core.inner.accounts.write().unwrap().push(account.clone());
            core.start_sync(&account);
            Ok(account.id.clone())
        })
        .await
        .map_err(|e| e.to_string())?
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
        let accounts = self.accounts();
        // The synthetic account is only there to fill an empty window.
        let mut folders: Vec<FolderInfo> = if accounts.is_empty() {
            self.inner.synthetic.iter().map(|f| f.info.clone()).collect()
        } else {
            Vec::new()
        };
        for account in &accounts {
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
        for account in self.accounts() {
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

impl Drop for Inner {
    fn drop(&mut self) {
        // Ask every sync to stop; the runtime then winds down without
        // waiting on a server.
        for account in self.accounts.read().unwrap().iter() {
            if let Some(running) = account.live.lock().unwrap().running.take() {
                running.stop();
            }
        }
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

fn open_accounts(data_dir: &Path) -> Result<Vec<Arc<Account>>, StoreError> {
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
            let dir = root.join(&name);
            Ok(Arc::new(Account {
                id: format!("acct/{name}"),
                store: Arc::new(Store::open(&dir)?),
                config: AccountConfig::load(&dir),
                name,
                live: Mutex::new(Live {
                    password: None,
                    running: None,
                    status: status(SyncState::NotSyncing, ""),
                }),
                now: Arc::default(),
            }))
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
