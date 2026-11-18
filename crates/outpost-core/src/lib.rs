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
pub mod compose;
pub mod dav;
pub mod imap;
pub mod recheck;
pub mod render;
pub mod settings;
pub mod smtp;
pub mod verify;
pub mod store;
pub mod sync;
pub mod synthetic;

use accounts::{AccountConfig, OsSecrets, Secrets};
use api::{
    AccountInfo, AttachmentInfo, CheckLine, Command, CoreError, ErrorCode, Event, FolderId, FolderInfo,
    MessageRow, MessageView, Response, SenderCheck, SyncState, SyncStatus, ThreadMessage, ThreadView, MAX_PAGE,
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
    /// Where saved attachments go. `None` means attachments cannot be saved.
    pub downloads_dir: Option<PathBuf>,
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
    config: RwLock<Option<AccountConfig>>,
    live: Mutex<Live>,
    /// Prods the running sync: a full pass now, or replay what the user did.
    signals: Arc<sync::Signals>,
}

/// The parts of an account that change while the app runs.
struct Live {
    /// Held in memory only. See `accounts.rs`.
    password: Option<String>,
    /// Stops the running sync when dropped or told to.
    running: Option<StopHandle>,
    status: SyncStatus,
    /// The highest message number there was when new mail was last
    /// announced. `None` until the first sync of this run has finished:
    /// mail that came while the app was closed is not announced.
    announced: Option<i64>,
}

impl Account {
    fn config(&self) -> Option<AccountConfig> {
        self.config.read().unwrap().clone()
    }

    /// The server whose sender checks are believed for this account.
    fn checking_server(&self) -> String {
        self.config.read().unwrap().as_ref().map(|c| c.checking_server.clone()).unwrap_or_default()
    }

    /// Where the account's settings file is.
    fn dir(&self) -> PathBuf {
        self.store.mail_root().parent().map(Path::to_path_buf).unwrap_or_default()
    }
}

/// The sending half of an account's settings, as the interface sees it.
fn sending_settings(config: &AccountConfig) -> Option<api::SendingSettings> {
    let smtp = config.smtp.as_ref()?;
    let identity = config.identity()?;
    Some(api::SendingSettings {
        name: identity.name,
        address: identity.address,
        host: smtp.host.clone(),
        port: smtp.port,
        security: smtp.security.clone(),
    })
}

/// Checks sending settings and puts them into an account's configuration.
fn with_sending(mut config: AccountConfig, sending: api::SendingSettings) -> Result<AccountConfig, CoreError> {
    let refuse = |why: &str| Err(CoreError::new(ErrorCode::NotPossible, why));
    let address = sending.address.trim();
    if compose::parse_recipients(address).map(|r| r.len()).unwrap_or(0) != 1 {
        return refuse("the address to send from is not an email address");
    }
    if sending.host.trim().is_empty() || sending.port == 0 {
        return refuse("the outgoing server and its port are both needed");
    }
    if !["tls", "starttls", "plaintext"].contains(&sending.security.as_str()) {
        return refuse("the outgoing server's security must be tls, starttls or plaintext");
    }
    config.name = sending.name.trim().to_string();
    config.address = address.to_string();
    config.smtp = Some(accounts::SmtpSettings {
        host: sending.host.trim().to_string(),
        port: sending.port,
        security: sending.security,
    });
    Ok(config)
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
    downloads_dir: Option<PathBuf>,
    accounts: RwLock<Vec<Arc<Account>>>,
    /// Calendar-and-contacts accounts (phase two). See `dav/service.rs`.
    dav: RwLock<Vec<Arc<dav::service::DavAccount>>>,
    synthetic: Vec<SyntheticFolder>,
    secrets: Arc<dyn Secrets>,
    /// Re-checks signatures, keeping the keys it has fetched.
    rechecker: Arc<recheck::Rechecker>,
    settings: settings::Settings,
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
                settings: settings::Settings::load(config.data_dir.as_deref()),
                downloads_dir: config.downloads_dir,
                accounts: RwLock::new(accounts),
                dav: RwLock::new(config.data_dir.as_deref().map(dav::service::open_accounts).unwrap_or_default()),
                data_dir: config.data_dir,
                synthetic: synthetic_folders(),
                secrets,
                rechecker: Arc::new(recheck::Rechecker::new()),
                runtime,
            }),
        };
        for account in core.accounts() {
            if let Some(config) = account.config() {
                let stored = core.inner.secrets.get(&config.secret_key(&account.name));
                account.live.lock().unwrap().password = stored;
            }
            // Also covers an index that is simply empty while there is mail
            // on disk: a rebuild that was interrupted, or mail copied in.
            let index = account.store.index();
            let empty_but_mail = index.folders().map(|f| f.is_empty()).unwrap_or(false)
                && std::fs::read_dir(account.store.mail_root()).is_ok_and(|mut d| d.next().is_some());
            if index.discarded_old() || empty_but_mail {
                // The index was from an older version and is now empty. The
                // mail is all still on disk: read it back in before anything
                // else, so the account is never shown empty.
                core.reindex_then_sync(&account);
            } else {
                core.begin(&account);
            }
        }
        core.begin_dav();
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

    /// Starts an account's sync if it can sync, or says what it is waiting
    /// for.
    fn begin(&self, account: &Arc<Account>) {
        if account.config().is_none() {
            self.set_status(account, status(SyncState::NotSyncing, ""));
        } else if account.live.lock().unwrap().password.is_none() {
            self.set_status(account, status(SyncState::NeedsPassword, ""));
        } else {
            self.start_sync(account);
        }
    }

    /// Rebuilds an account's index from its mail files in the background,
    /// reporting progress, and then starts its sync.
    fn reindex_then_sync(&self, account: &Arc<Account>) {
        self.set_status(account, status(SyncState::Indexing, "Reading the mail on this computer…"));
        let (core, account) = (self.clone(), account.clone());
        self.inner.runtime.spawn_blocking(move || {
            let result = account.store.rebuild(true, |progress| {
                if let store::rebuild::Progress::Loading { done, total } = progress {
                    core.set_status(
                        &account,
                        status(SyncState::Indexing, format!("Reading the mail on this computer: {done} of {total}")),
                    );
                }
            });
            core.inner.sink.emit(Event::FoldersChanged {
                account: account.id.clone(),
            });
            match result {
                Ok(_) => core.begin(&account),
                Err(e) => core.set_status(
                    &account,
                    status(SyncState::Offline, format!("Could not read the mail on this computer: {e}")),
                ),
            }
        });
    }

    /// Starts (or restarts) an account's sync with the password it holds.
    fn start_sync(&self, account: &Arc<Account>) {
        let Some(config) = account.config() else { return };
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
            let options = sync::Options {
                sending: config.smtp_server().map(|server| sync::Sending {
                    server,
                    username: config.username.clone(),
                }),
                ..sync::Options::default()
            };
            sync::run(
                &account.store,
                &server_account,
                &password,
                &options,
                stop,
                &account.signals,
                report,
            )
            .await;
        });
    }

    /// Moves mail from blocked senders out of the inbox into the junk
    /// folder. Only the newest mail is looked at, and only if the account
    /// has both folders and can be changed.
    fn move_blocked(&self, account: &Account) {
        let blocked = self.inner.settings.protection().blocked_senders;
        if blocked.is_empty() || account.config().is_none() {
            return;
        }
        let (Ok(Some(inbox)), Ok(Some(junk))) =
            (self.folder_with_role(account, "inbox"), self.folder_with_role(account, "junk"))
        else {
            return;
        };
        let Ok(ids) = account.store.index().recent_from(inbox, &blocked, 200) else { return };
        if ids.is_empty() {
            return;
        }
        if let Ok(touched) = account.store.move_messages(&ids, junk) {
            self.acted(account, touched);
        }
    }

    /// Tells the interface about unread mail that has arrived in the inbox
    /// since the last time this was called.
    fn announce_new_mail(&self, account: &Account) {
        let index = account.store.index();
        let Ok(newest) = index.newest_id() else { return };
        let before = account.live.lock().unwrap().announced.replace(newest);
        let Some(before) = before else { return };
        if newest <= before {
            return;
        }
        let Ok(Some(inbox)) = self.folder_with_role(account, "inbox") else { return };
        let Ok((count, sender, subject)) = index.unread_since(inbox, before) else { return };
        if count > 0 {
            self.inner.sink.emit(Event::NewMail {
                account: account.id.clone(),
                folder: format!("{}/{inbox}", account.id),
                count,
                sender,
                subject,
            });
        }
    }

    /// Until the user has confirmed which server's sender checks to believe,
    /// it is taken to be the one that wrote them on most of the newest mail.
    fn guess_checking_server(&self, account: &Account) {
        let Some(mut config) = account.config() else { return };
        if config.checking_server_confirmed {
            return;
        }
        let Ok(Some(likely)) = account.store.index().likely_checking_server() else { return };
        if likely == config.checking_server {
            return;
        }
        config.checking_server = likely;
        if config.save(&account.dir()).is_ok() {
            *account.config.write().unwrap() = Some(config);
            // Lists show the checks, so they are read again.
            self.inner.sink.emit(Event::FoldersChanged { account: account.id.clone() });
        }
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
            Progress::FolderChanged { folder_id } => {
                self.inner.sink.emit(Event::FolderChanged { folder: format!("{}/{folder_id}", account.id) });
                self.move_blocked(account);
            }
            Progress::Idle => {
                self.guess_checking_server(account);
                self.announce_new_mail(account);
                self.set_status(account, status(SyncState::UpToDate, ""))
            }
            Progress::Offline { error, retry_in } => self.set_status(
                account,
                status(
                    SyncState::Offline,
                    format!("{error}. Trying again in {} s.", retry_in.as_secs()),
                ),
            ),
            Progress::ActionFailed { what, error } => self.inner.sink.emit(Event::Notice {
                account: account.id.clone(),
                text: format!("{what} did not work: {error}"),
            }),
            Progress::SignInFailed { error } => {
                account.live.lock().unwrap().running = None;
                self.set_status(account, status(SyncState::SignInFailed, error));
            }
        }
    }

    /// Handles one command.
    pub fn handle(&self, command: Command) -> Result<Response, CoreError> {
        // Calendars, tasks and contacts are handled in `dav/service.rs`.
        let command = match self.handle_dav(command) {
            Ok(answer) => return answer,
            Err(command) => command,
        };
        match command {
            Command::Ping => Ok(Response::Pong {
                core_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            Command::ListFolders => self.list_folders(),
            Command::FolderLayout { folder, conversations } => {
                let preview_lens = match self.target(&folder)? {
                    Target::Synthetic(f) => (0..f.info.total)
                        .map(|i| synthetic::preview_len(f.seed, i))
                        .collect(),
                    Target::Folder(account, id) if conversations => {
                        account.store.index().conversation_lens(id).map_err(internal)?
                    }
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
                conversations,
            } => {
                let limit = limit.min(MAX_PAGE);
                let (offset, total, rows) = match self.target(&folder)? {
                    Target::Synthetic(f) => {
                        let total = f.info.total;
                        let start = offset.min(total);
                        let end = start.saturating_add(limit).min(total);
                        (start, total, (start..end).map(|i| synthetic::row(f.seed, i)).collect())
                    }
                    Target::Folder(account, id) if conversations => {
                        let index = account.store.index();
                        let total = index.conversation_total(id).map_err(internal)?;
                        let start = offset.min(total);
                        let rows = index.conversation_page(id, start, limit).map_err(internal)?;
                        let own: Vec<String> = account
                            .config()
                            .map(|c| [c.identity().map(|i| i.address), Some(c.username)].into_iter().flatten().collect())
                            .unwrap_or_default();
                        (start, total, conversation_rows(rows, start, &own, &account.checking_server()))
                    }
                    Target::Folder(account, id) => {
                        let index = account.store.index();
                        let total = index.folder_total(id).map_err(internal)?;
                        let start = offset.min(total);
                        let rows = index.page(id, start, limit).map_err(internal)?;
                        (start, total, message_rows(rows, start, &account.checking_server()))
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
                            sender_check: SenderCheck { state: "notChecked".into(), ..SenderCheck::default() },
                            ..MessageView::default()
                        }
                    }
                    Target::Folder(account, _) | Target::Account(account) => {
                        let protection = self.inner.settings.protection();
                        let mut view = open_message(&account.store, id, &account.checking_server(), &protection)?;
                        let own: Vec<String> = account
                            .config()
                            .map(|c| [c.identity().map(|i| i.address), Some(c.username)].into_iter().flatten().collect())
                            .unwrap_or_default();
                        let mine = own.iter().any(|o| o.eq_ignore_ascii_case(&view.sender_address));
                        if view.downloaded && !mine {
                            view.invitation = self.invitation_in(&account, id);
                            if let Some(history) = account.store.index().sender_history(id).map_err(internal)? {
                                let check = &mut view.sender_check;
                                let first_time = !history.known_address;
                                check.first_time = first_time && protection.first_time_banner;
                                // A familiar name on an unfamiliar address.
                                if first_time {
                                    check.name_seen_from = history.name_used_by.unwrap_or_default();
                                }
                                if protection.lookalike_warning {
                                    check.lookalike_of =
                                        verify::lookalike(&history.domain, &history.domains).unwrap_or_default();
                                }
                            }
                        }
                        view
                    }
                };
                Ok(Response::Message { message: view })
            }
            Command::OpenThread { folder, id } => {
                let thread = match self.target(&folder)? {
                    Target::Synthetic(f) => {
                        let row = synthetic::row(f.seed, id.clamp(0, u32::MAX as i64) as u32);
                        ThreadView {
                            subject: row.subject.clone(),
                            messages: vec![ThreadMessage {
                                id,
                                folder: folder.clone(),
                                sender_name: row.sender_name,
                                sender_address: row.sender_address,
                                date: row.date,
                                unread: row.unread,
                                downloaded: true,
                                text: row.preview,
                                ..ThreadMessage::default()
                            }],
                        }
                    }
                    Target::Folder(account, _) | Target::Account(account) => open_thread(&account, id)?,
                };
                Ok(Response::Thread { thread })
            }
            Command::Search { scope, text, limit } => {
                let limit = limit.min(MAX_PAGE);
                let rows = match self.target(&scope)? {
                    // The synthetic account has no text to search.
                    Target::Synthetic(_) => Vec::new(),
                    Target::Account(account) => {
                        let rows = account.store.index().search(&text, None, limit).map_err(internal)?;
                        message_rows(rows, 0, &account.checking_server())
                    }
                    Target::Folder(account, id) => {
                        let rows = account.store.index().search(&text, Some(id), limit).map_err(internal)?;
                        message_rows(rows, 0, &account.checking_server())
                    }
                };
                Ok(Response::SearchResults { rows })
            }
            Command::ChangeFlags {
                folder,
                ids,
                seen,
                flagged,
            } => {
                let account = self.syncing_account(&folder)?;
                let mut add = Flags::default();
                let mut remove = Flags::default();
                for (wanted, flag) in [(seen, Flags::SEEN), (flagged, Flags::FLAGGED)] {
                    match wanted {
                        Some(true) => add = add.with(flag),
                        Some(false) => remove = remove.with(flag),
                        None => {}
                    }
                }
                let touched = account.store.change_flags(&ids, add, remove).map_err(internal)?;
                self.acted(&account, touched);
                Ok(Response::Accepted)
            }
            Command::MoveMessages { ids, to } => {
                let Target::Folder(account, destination) = self.target(&to)? else {
                    return Err(CoreError::new(ErrorCode::NotPossible, "messages can only be moved into a folder"));
                };
                self.must_sync(&account)?;
                let touched = account.store.move_messages(&ids, destination).map_err(internal)?;
                self.acted(&account, touched);
                Ok(Response::Accepted)
            }
            Command::ArchiveMessages { folder, ids } => {
                let account = self.syncing_account(&folder)?;
                let archive = self.folder_with_role(&account, "archive")?.ok_or_else(|| {
                    CoreError::new(ErrorCode::NotPossible, "this account has no archive folder")
                })?;
                let touched = account.store.move_messages(&ids, archive).map_err(internal)?;
                self.acted(&account, touched);
                Ok(Response::Accepted)
            }
            Command::DeleteMessages {
                folder,
                ids,
                permanently,
            } => {
                let account = self.syncing_account(&folder)?;
                let touched = if permanently {
                    account.store.expunge_messages(&ids).map_err(internal)?
                } else {
                    let trash = self.folder_with_role(&account, "trash")?.ok_or_else(|| {
                        CoreError::new(ErrorCode::NotPossible, "this account has no trash folder")
                    })?;
                    account.store.move_messages(&ids, trash).map_err(internal)?
                };
                self.acted(&account, touched);
                Ok(Response::Accepted)
            }
            Command::CreateFolder { parent, name } => {
                let (account, parent) = match self.target(&parent)? {
                    Target::Account(account) => (account, None),
                    Target::Folder(account, id) => (account, Some(id)),
                    Target::Synthetic(_) => return Err(CoreError::new(ErrorCode::NotPossible, "not a real account")),
                };
                self.must_sync(&account)?;
                let id = account
                    .store
                    .create_remote_folder(parent, &name)
                    .map_err(internal)?
                    .map_err(|refusal| CoreError::new(ErrorCode::NotPossible, refusal.0))?;
                self.inner.sink.emit(Event::FoldersChanged { account: account.id.clone() });
                account.signals.changes.notify_one();
                Ok(Response::FolderCreated { folder: format!("{}/{id}", account.id) })
            }
            Command::DeleteFolder { folder } => {
                let Target::Folder(account, id) = self.target(&folder)? else {
                    return Err(CoreError::new(ErrorCode::NotPossible, "that is not a folder"));
                };
                self.must_sync(&account)?;
                account
                    .store
                    .delete_remote_folder(id)
                    .map_err(internal)?
                    .map_err(|refusal| CoreError::new(ErrorCode::NotPossible, refusal.0))?;
                self.inner.sink.emit(Event::FoldersChanged { account: account.id.clone() });
                account.signals.changes.notify_one();
                Ok(Response::Accepted)
            }
            Command::SaveAttachment { folder, id, index } => {
                let account = self.account(&folder)?;
                let Some(downloads) = &self.inner.downloads_dir else {
                    return Err(CoreError::new(ErrorCode::NotPossible, "there is no downloads folder to save to"));
                };
                let raw = account.store.read_message(id).map_err(|e| match e {
                    StoreError::NoSuchMessage(_) | StoreError::NotDownloaded(_) => {
                        CoreError::new(ErrorCode::UnknownMessage, e.to_string())
                    }
                    other => internal(other),
                })?;
                let protection = self.inner.settings.protection();
                if protection.disable_failed && !account.store.index().is_trusted(id).map_err(internal)? {
                    let from = store::parse::display(&raw).from_addr;
                    if sender_check(&raw, &from, &account.checking_server()).state == "failed" {
                        return Err(CoreError::new(
                            ErrorCode::NotPossible,
                            "this message failed its sender checks, so its attachments are switched off",
                        ));
                    }
                }
                let (name, data) = store::parse::attachment(&raw, index as usize)
                    .ok_or_else(|| CoreError::new(ErrorCode::UnknownMessage, "the message has no such attachment"))?;
                let (path, name) = save_download(downloads, &name, &data)
                    .map_err(|e| CoreError::new(ErrorCode::Internal, format!("could not save the file: {e}")))?;
                Ok(Response::AttachmentSaved {
                    path: path.to_string_lossy().into_owned(),
                    name,
                })
            }
            Command::LocateMessage { folder, id, conversations } => {
                let index = match self.target(&folder)? {
                    Target::Synthetic(f) => (id >= 0 && id < f.info.total as i64).then_some(id as u32),
                    Target::Folder(account, folder_id) if conversations => {
                        account.store.index().conversation_position(folder_id, id).map_err(internal)?
                    }
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
                        host: a.config().map(|c| c.host).unwrap_or_default(),
                        username: a.config().map(|c| c.username).unwrap_or_default(),
                        status: a.live.lock().unwrap().status.clone(),
                        sending: a.config().and_then(|c| sending_settings(&c)),
                        port: a.config().map(|c| c.port).unwrap_or_default(),
                        encrypted: a.config().is_none_or(|c| c.encrypted),
                        signature: a.config().map(|c| c.signature).unwrap_or_default(),
                        checking_server: a.checking_server(),
                        checking_server_confirmed: a.config().is_some_and(|c| c.checking_server_confirmed),
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
                sending,
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
                    name: String::new(),
                    address: String::new(),
                    smtp: None,
                    checking_server: String::new(),
                    checking_server_confirmed: false,
                    signature: String::new(),
                };
                let config = match sending {
                    Some(sending) => with_sending(config, sending)?,
                    None => config,
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
            Command::SetSending { account, sending } => {
                let account = self.syncing_account(&account)?;
                let config = with_sending(account.config().expect("checked above"), sending)?;
                let dir = account.store.mail_root().parent().map(Path::to_path_buf).unwrap_or_default();
                config
                    .save(&dir)
                    .map_err(|e| CoreError::new(ErrorCode::Internal, format!("could not save the settings: {e}")))?;
                *account.config.write().unwrap() = Some(config);
                // The running sync was started with the old settings.
                self.start_sync(&account);
                Ok(Response::Accepted)
            }
            // Answered above, in `handle_dav`.
            Command::ListDavAccounts
            | Command::AddDavAccount { .. }
            | Command::ProvideDavPassword { .. }
            | Command::RemoveDavAccount { .. }
            | Command::SetDavCollection { .. }
            | Command::SyncDav { .. }
            | Command::ListEvents { .. }
            | Command::ListTasks
            | Command::ListContacts { .. }
            | Command::OpenDavItem { .. }
            | Command::SaveEvent { .. }
            | Command::SaveTask { .. }
            | Command::SaveContact { .. }
            | Command::AnswerInvitation { .. }
            | Command::DeleteDavItem { .. } => Err(CoreError::new(ErrorCode::Internal, "a calendar command was not handled")),
            Command::GetGeneral => Ok(Response::General { general: self.inner.settings.general() }),
            Command::SetGeneral { general, password } => {
                self.inner
                    .settings
                    .set_general(general, password.as_ref().map(|p| p.0.as_str()))
                    .map_err(denied)?;
                Ok(Response::General { general: self.inner.settings.general() })
            }
            Command::UpdateAccount { account, host, port, encrypted, signature, password } => {
                let account = self.syncing_account(&account)?;
                if !self.inner.settings.allows(password.as_ref().map(|p| p.0.as_str())) {
                    return Err(denied(settings::Denied::WrongPassword));
                }
                let host = host.trim().to_string();
                if host.is_empty() || port == 0 {
                    return Err(CoreError::new(ErrorCode::NotPossible, "the server and its port are both needed"));
                }
                let old = account.config().expect("checked above");
                let mut config = old.clone();
                config.host = host;
                config.port = port;
                config.encrypted = encrypted;
                config.signature = signature.trim_end().to_string();
                config
                    .save(&account.dir())
                    .map_err(|e| CoreError::new(ErrorCode::Internal, format!("could not save the settings: {e}")))?;
                // The stored password is filed under the server's name.
                if config.host != old.host {
                    if let Some(secret) = self.inner.secrets.get(&old.secret_key(&account.name)) {
                        if self.inner.secrets.set(&config.secret_key(&account.name), &secret).is_ok() {
                            self.inner.secrets.delete(&old.secret_key(&account.name));
                        }
                    }
                }
                let reconnect = (&config.host, config.port, config.encrypted) != (&old.host, old.port, old.encrypted);
                *account.config.write().unwrap() = Some(config);
                if reconnect {
                    self.start_sync(&account);
                }
                Ok(Response::Accepted)
            }
            Command::RemoveAccount { account, password } => {
                let account = self.syncing_account(&account)?;
                if !self.inner.settings.allows(password.as_ref().map(|p| p.0.as_str())) {
                    return Err(denied(settings::Denied::WrongPassword));
                }
                let config = account.config().expect("checked above");
                if let Some(running) = account.live.lock().unwrap().running.take() {
                    running.stop();
                }
                let dir = account.dir();
                // Marked first: if the files cannot all be deleted now (on
                // Windows, while they are open), the rest go at next start.
                std::fs::write(dir.join(REMOVED_MARK), b"")
                    .map_err(|e| CoreError::new(ErrorCode::Internal, format!("could not remove the account: {e}")))?;
                self.inner.accounts.write().unwrap().retain(|a| a.id != account.id);
                self.inner.secrets.delete(&config.secret_key(&account.name));
                let _ = std::fs::remove_dir_all(&dir);
                self.inner.sink.emit(Event::AccountRemoved { account: account.id.clone() });
                Ok(Response::Accepted)
            }
            Command::ExportMessage { folder, id } => {
                let account = self.account(&folder)?;
                let Some(downloads) = &self.inner.downloads_dir else {
                    return Err(CoreError::new(ErrorCode::NotPossible, "there is no downloads folder to save to"));
                };
                let raw = self.read(&account, id)?;
                let subject = store::parse::display(&raw).subject;
                let name = format!("{}.eml", if subject.trim().is_empty() { "message" } else { subject.trim() });
                let (path, name) = save_download(downloads, &name, &raw)
                    .map_err(|e| CoreError::new(ErrorCode::Internal, format!("could not save the file: {e}")))?;
                Ok(Response::AttachmentSaved { path: path.to_string_lossy().into_owned(), name })
            }
            Command::GetProtection => Ok(Response::Protection {
                protection: self.inner.settings.protection(),
                password_set: self.inner.settings.password_set(),
            }),
            Command::SetProtection { protection, password } => {
                self.inner
                    .settings
                    .set_protection(protection, password.as_ref().map(|p| p.0.as_str()))
                    .map_err(denied)?;
                // Newly blocked senders are dealt with now, not only when
                // their next message arrives.
                for account in self.accounts() {
                    self.move_blocked(&account);
                }
                Ok(Response::Protection {
                    protection: self.inner.settings.protection(),
                    password_set: self.inner.settings.password_set(),
                })
            }
            Command::SetProtectionPassword { current, new } => {
                self.inner
                    .settings
                    .set_password(current.as_ref().map(|p| p.0.as_str()), new.as_ref().map(|p| p.0.as_str()))
                    .map_err(denied)?;
                Ok(Response::Protection {
                    protection: self.inner.settings.protection(),
                    password_set: self.inner.settings.password_set(),
                })
            }
            Command::TrustMessage { folder, id, trusted, password } => {
                let account = self.account(&folder)?;
                // Withdrawing trust is always allowed; giving it overrides
                // a warning.
                if trusted && !self.inner.settings.allows(password.as_ref().map(|p| p.0.as_str())) {
                    return Err(denied(settings::Denied::WrongPassword));
                }
                if !account.store.index().set_trusted(id, trusted).map_err(internal)? {
                    return Err(CoreError::new(ErrorCode::NotPossible, "this message cannot be marked as trusted"));
                }
                Ok(Response::Accepted)
            }
            Command::RecheckSignature { folder, id } => {
                if !self.inner.settings.protection().recheck_signatures {
                    return Err(CoreError::new(ErrorCode::NotPossible, "re-checking signatures is switched off"));
                }
                let account = self.account(&folder)?;
                let raw = self.read(&account, id)?;
                let from = store::parse::display(&raw).from_addr;
                // The lookups run on the core's own runtime; this thread
                // only waits for them.
                let (done, result) = std::sync::mpsc::channel();
                let rechecker = self.inner.rechecker.clone();
                let for_verdict = raw.clone();
                self.inner.runtime.spawn(async move {
                    let _ = done.send(rechecker.check(&raw).await);
                });
                let checks = result.recv().unwrap_or_default();
                let from_domain = verify::domain_of(&from);
                let server = match account.checking_server().trim() {
                    "" => verify::assess(b"", &from, Some("none")),
                    name => verify::assess(&for_verdict, &from, Some(name)),
                };
                Ok(Response::SignatureRechecked {
                    verified: recheck::vouches(&checks, &from, &server),
                    signatures: checks
                        .into_iter()
                        .map(|c| api::SignatureLine {
                            aligned: verify::related(&c.domain, &from_domain),
                            domain: c.domain,
                            outcome: match c.outcome {
                                recheck::Outcome::Valid => "valid",
                                recheck::Outcome::Mismatch => "mismatch",
                                recheck::Outcome::Unknown => "unknown",
                            }
                            .into(),
                            sentence: c.sentence,
                        })
                        .collect(),
                })
            }
            Command::SetCheckingServer { account, server, password } => {
                let account = self.syncing_account(&account)?;
                // Naming a different server changes which messages show as
                // verified, so it is guarded like any protection setting.
                if !self.inner.settings.allows(password.as_ref().map(|p| p.0.as_str())) {
                    return Err(denied(settings::Denied::WrongPassword));
                }
                let mut config = account.config().expect("checked above");
                config.checking_server = server.trim().trim_end_matches('.').to_ascii_lowercase();
                config.checking_server_confirmed = true;
                config
                    .save(&account.dir())
                    .map_err(|e| CoreError::new(ErrorCode::Internal, format!("could not save the settings: {e}")))?;
                *account.config.write().unwrap() = Some(config);
                self.inner.sink.emit(Event::FoldersChanged { account: account.id.clone() });
                Ok(Response::Accepted)
            }
            Command::ComposeReply { folder, id, mode } => {
                let account = self.account(&folder)?;
                let mode = match mode.as_str() {
                    "reply" => compose::ReplyMode::Reply,
                    "replyAll" => compose::ReplyMode::ReplyAll,
                    "forward" => compose::ReplyMode::Forward,
                    other => return Err(CoreError::new(ErrorCode::BadCommand, format!("unknown mode {other:?}"))),
                };
                let raw = self.read(&account, id)?;
                let own: Vec<String> = account
                    .config()
                    .map(|c| [c.identity().map(|i| i.address), Some(c.username)].into_iter().flatten().collect())
                    .unwrap_or_default();
                let mine = |address: &str| own.iter().any(|o| o.eq_ignore_ascii_case(address));
                let draft = compose::respond(&raw, id, mode, &mine)
                    .ok_or_else(|| CoreError::new(ErrorCode::Internal, "the message could not be read"))?;
                Ok(Response::Draft { draft })
            }
            Command::EditDraft { folder, id } => {
                let account = self.account(&folder)?;
                let raw = self.read(&account, id)?;
                let draft = compose::reopen(&raw, id)
                    .ok_or_else(|| CoreError::new(ErrorCode::Internal, "the draft could not be read"))?;
                Ok(Response::Draft { draft })
            }
            Command::SaveDraft { account, draft } => {
                let account = self.syncing_account(&account)?;
                let built = self.build(&account, &draft)?;
                let drafts = self.folder_with_role(&account, "drafts")?.ok_or_else(|| {
                    CoreError::new(ErrorCode::NotPossible, "this account has no drafts folder")
                })?;
                let id = account
                    .store
                    .add_local_message(drafts, &built, Flags::DRAFT.with(Flags::SEEN))
                    .map_err(internal)?;
                let mut touched = vec![drafts];
                touched.extend(self.discard_replaced(&account, &draft)?);
                self.acted(&account, touched);
                Ok(Response::DraftSaved { id })
            }
            Command::SendMessage {
                account,
                draft,
                delay_seconds,
            } => {
                let account = self.syncing_account(&account)?;
                if account.config().and_then(|c| c.smtp_server()).is_none() {
                    return Err(CoreError::new(
                        ErrorCode::NotPossible,
                        "no outgoing mail server has been set up for this account",
                    ));
                }
                let recipients = [&draft.to, &draft.cc, &draft.bcc]
                    .iter()
                    .map(|list| compose::parse_recipients(list).map(|r| r.len()))
                    .sum::<Result<usize, String>>()
                    .map_err(|e| CoreError::new(ErrorCode::NotPossible, e))?;
                if recipients == 0 {
                    return Err(CoreError::new(ErrorCode::NotPossible, "the message has nobody to go to"));
                }
                let built = self.build(&account, &draft)?;
                let send_at = store::actions::now() + delay_seconds.min(3600) as i64;
                let id = account.store.queue_for_sending(&built, send_at).map_err(internal)?;
                let mut touched: Vec<i64> = account.store.index().brief(id).map_err(internal)?.map(|b| b.folder).into_iter().collect();
                touched.extend(self.discard_replaced(&account, &draft)?);
                // The message being answered or passed on is marked as such.
                for (original, flag) in [(draft.reply_to_message, Flags::ANSWERED), (draft.forward_of, Flags::FORWARDED)] {
                    if let Some(original) = original {
                        touched.extend(account.store.change_flags(&[original], flag, Flags::default()).map_err(internal)?);
                    }
                }
                self.acted(&account, touched);
                Ok(Response::SendQueued { id })
            }
            Command::CancelSend { account, id } => {
                let account = self.syncing_account(&account)?;
                let outbox = account.store.index().brief(id).map_err(internal)?.map(|b| b.folder);
                let cancelled = account.store.cancel_sending(id).map_err(internal)?;
                if cancelled {
                    let drafts = self.folder_with_role(&account, "drafts")?;
                    self.acted(&account, outbox.into_iter().chain(drafts));
                }
                Ok(Response::SendCancelled { cancelled })
            }
            Command::ProvidePassword {
                account,
                password,
                remember,
            } => {
                let account = self.account(&account)?;
                let Some(config) = account.config() else {
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
                if account.config().is_none() {
                    return Err(CoreError::new(ErrorCode::NotPossible, "this account has no server"));
                }
                let (running, has_password) = {
                    let live = account.live.lock().unwrap();
                    (live.running.is_some(), live.password.is_some())
                };
                if !has_password {
                    return Err(CoreError::new(ErrorCode::NotPossible, "the account needs its password first"));
                }
                account.store.index().retry_deferred().map_err(internal)?;
                if running {
                    account.signals.now.notify_one();
                } else {
                    self.start_sync(&account);
                }
                Ok(Response::Accepted)
            }
        }
    }

    /// The account a folder belongs to, if it is one that syncs. Changes
    /// are only accepted for those: on any other they would wait for ever.
    fn syncing_account(&self, folder: &FolderId) -> Result<Arc<Account>, CoreError> {
        let account = self.account(folder)?;
        self.must_sync(&account)?;
        Ok(account)
    }

    fn must_sync(&self, account: &Account) -> Result<(), CoreError> {
        if account.config().is_none() {
            return Err(CoreError::new(
                ErrorCode::NotPossible,
                "this mail is only stored here, with no server to change it on",
            ));
        }
        Ok(())
    }

    fn read(&self, account: &Account, id: i64) -> Result<Vec<u8>, CoreError> {
        account.store.read_message(id).map_err(|e| match e {
            StoreError::NoSuchMessage(_) | StoreError::NotDownloaded(_) => {
                CoreError::new(ErrorCode::UnknownMessage, e.to_string())
            }
            other => internal(other),
        })
    }

    /// Builds the message a draft describes, from this account's identity,
    /// carrying the attachments of a message being forwarded.
    /// The meeting invitation a stored message carries, if any.
    fn invitation_in(&self, account: &Account, id: i64) -> Option<api::InvitationView> {
        let raw = account.store.read_message(id).ok()?;
        let text = store::parse::calendar_part(&raw)?;
        let invitation = dav::item::read_invitation(&text, &dav::service::local_zone())?;
        let kind = match invitation.method.as_str() {
            "REQUEST" => "request",
            "CANCEL" => "cancel",
            _ => return None,
        };
        Some(api::InvitationView {
            kind: kind.into(),
            in_calendar: self.dav_has_uid(&invitation.event.uid),
            event: api::EventContent {
                summary: invitation.event.summary,
                location: invitation.event.location,
                description: invitation.event.description,
                start: invitation.event.start,
                end: invitation.event.end,
                all_day: invitation.event.all_day,
            },
            recurring: invitation.event.recurring,
            organizer: invitation.organizer,
            organizer_name: invitation.organizer_name,
        })
    }

    /// Queues the reply to an invitation for sending, from the account the
    /// invitation arrived in. Used by `dav/service.rs`.
    pub(crate) fn send_invitation_reply(
        &self,
        folder: &FolderId,
        message: i64,
        answer: dav::item::Answer,
    ) -> Result<(String, dav::item::Invitation), CoreError> {
        let account = self.syncing_account(folder)?;
        let raw = self.read(&account, message)?;
        let not_one = || CoreError::new(ErrorCode::NotPossible, "this message carries no meeting invitation");
        let text = store::parse::calendar_part(&raw).ok_or_else(not_one)?;
        let invitation = dav::item::read_invitation(&text, &dav::service::local_zone()).ok_or_else(not_one)?;
        let identity = account
            .config()
            .filter(|c| c.smtp_server().is_some())
            .and_then(|c| c.identity())
            .ok_or_else(|| CoreError::new(ErrorCode::NotPossible, "set up sending for this account first, so the organiser can be answered"))?;
        // Answer as whichever of the user's addresses was invited.
        let own: Vec<String> = [Some(identity.address.clone()), account.config().map(|c| c.username)].into_iter().flatten().collect();
        let me = invitation.attendees.iter().find(|a| own.iter().any(|o| o.eq_ignore_ascii_case(a))).cloned().unwrap_or(identity.address.clone());
        let (copy, reply) = dav::item::answer_invitation(&text, &me, &identity.name, answer, store::actions::now()).ok_or_else(not_one)?;
        if invitation.organizer.is_empty() {
            return Err(CoreError::new(ErrorCode::NotPossible, "the invitation does not say who organised it"));
        }
        let (word, verb) = match answer {
            dav::item::Answer::Accepted => ("Accepted", "has accepted"),
            dav::item::Answer::Tentative => ("Tentative", "has tentatively accepted"),
            dav::item::Answer::Declined => ("Declined", "has declined"),
        };
        let who = if identity.name.is_empty() { identity.address.clone() } else { identity.name.clone() };
        let built = compose::build_invitation_reply(
            &identity,
            &invitation.organizer,
            &format!("{word}: {}", invitation.event.summary),
            &format!("{who} {verb} the invitation to \"{}\".\n", invitation.event.summary),
            &reply,
        )
        .map_err(|e| CoreError::new(ErrorCode::NotPossible, e))?;
        let id = account.store.queue_for_sending(&built.raw, store::actions::now()).map_err(internal)?;
        let mut touched: Vec<i64> = account.store.index().brief(id).map_err(internal)?.map(|b| b.folder).into_iter().collect();
        touched.extend(account.store.change_flags(&[message], Flags::ANSWERED, Flags::default()).map_err(internal)?);
        self.acted(&account, touched);
        Ok((copy, invitation))
    }

    /// The calendar file a stored message carries, read as an invitation.
    pub(crate) fn stored_invitation(&self, folder: &FolderId, message: i64) -> Result<dav::item::Invitation, CoreError> {
        let account = self.account(folder)?;
        let raw = self.read(&account, message)?;
        store::parse::calendar_part(&raw)
            .and_then(|text| dav::item::read_invitation(&text, &dav::service::local_zone()))
            .ok_or_else(|| CoreError::new(ErrorCode::NotPossible, "this message carries no meeting invitation"))
    }

    fn build(&self, account: &Account, draft: &api::DraftContent) -> Result<Vec<u8>, CoreError> {
        let identity = account.config().and_then(|c| c.identity()).ok_or_else(|| {
            CoreError::new(ErrorCode::NotPossible, "this account has no address to send from yet")
        })?;
        let mut carried = Vec::new();
        if let Some(original) = draft.forward_of {
            let raw = self.read(account, original)?;
            let mut index = 0;
            while let Some((name, data)) = store::parse::attachment(&raw, index) {
                let mime_type = store::parse::display(&raw)
                    .attachments
                    .get(index)
                    .map(|a| a.mime_type.clone())
                    .unwrap_or_default();
                carried.push((name, mime_type, data));
                index += 1;
            }
        }
        compose::build(&identity, draft, &carried)
            .map(|built| built.raw)
            .map_err(|e| CoreError::new(ErrorCode::NotPossible, e))
    }

    /// Removes the stored draft that a saved or sent message replaces.
    /// Only a draft or an outbox message can be replaced this way.
    fn discard_replaced(&self, account: &Account, draft: &api::DraftContent) -> Result<Vec<i64>, CoreError> {
        let Some(old) = draft.replaces else { return Ok(Vec::new()) };
        let Some(brief) = account.store.index().brief(old).map_err(internal)? else {
            return Ok(Vec::new());
        };
        let folders = account.store.index().folders().map_err(internal)?;
        let role = folders.iter().find(|f| f.id == brief.folder).and_then(role_of);
        if !matches!(role, Some("drafts") | Some("outbox")) {
            return Ok(Vec::new());
        }
        // A message still waiting in the outbox must not go as well.
        account.store.cancel_pending_send(old).map_err(internal)?;
        Ok(account.store.expunge_messages(&[old]).map_err(internal)?.into_iter().collect())
    }

    /// The account's folder with the given role, if it has one.
    fn folder_with_role(&self, account: &Account, role: &str) -> Result<Option<i64>, CoreError> {
        let folders = account.store.index().folders().map_err(internal)?;
        Ok(folders.iter().find(|f| role_of(f) == Some(role)).map(|f| f.id))
    }

    /// After the user changed something: tell the interface which folders
    /// look different, and tell sync there is something to send.
    fn acted(&self, account: &Account, touched: impl IntoIterator<Item = i64>) {
        for folder_id in touched {
            self.inner.sink.emit(Event::FolderChanged {
                folder: format!("{}/{folder_id}", account.id),
            });
        }
        account.signals.changes.notify_one();
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
            a.config().is_some_and(|c| {
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
                config: RwLock::new(Some(config)),
                live: Mutex::new(Live {
                    password: Some(password),
                    running: None,
                    status: status(SyncState::Connecting, ""),
                    announced: None,
                }),
                signals: Arc::default(),
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
        }        for account in self.dav.read().unwrap().iter() {
            account.stop();
        }
    }
}

fn role_of(folder: &store::index::FolderRow) -> Option<&'static str> {
    store::sync_state::role_of(&folder.path, &folder.name, folder.special_use.as_deref())
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
            Some("outbox") => 2,
            Some("sent") => 3,
            Some("archive") => 4,
            Some("junk") => 5,
            Some("trash") => 6,
            _ => 7,
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
                name: match role {
                    Some("inbox") => "Inbox".into(),
                    Some("outbox") => "Outbox".into(),
                    _ => row.name.clone(),
                },
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
    // The outbox is shown only while something is in it, and does not
    // count as one of the server's folders.
    let rows: Vec<store::index::FolderRow> = rows
        .into_iter()
        .filter(|r| role_of(r) != Some("outbox") || r.total > 0)
        .collect();
    let top: Vec<&store::index::FolderRow> = rows
        .iter()
        .filter(|r| parent(&r.path).is_empty() && role_of(r) != Some("outbox"))
        .collect();
    let only_inbox = top.len() == 1 && top[0].path.eq_ignore_ascii_case("INBOX") && rows.len() > 1;
    let mut out = Vec::with_capacity(rows.len());
    place(account_id, &rows, "", 1, only_inbox, &mut out);
    out
}

fn internal(e: StoreError) -> CoreError {
    CoreError::new(ErrorCode::Internal, e.to_string())
}

/// The conversation a message belongs to, each message reduced to what its
/// sender wrote, with who joined and left along the way.
fn open_thread(account: &Account, id: i64) -> Result<ThreadView, CoreError> {
    use std::collections::{BTreeSet, HashMap};
    use store::reduce;
    let index = account.store.index();
    let rows = index.thread_of(id).map_err(internal)?;
    if rows.is_empty() {
        return Err(CoreError::new(ErrorCode::UnknownMessage, format!("no message {id}")));
    }
    let folders: HashMap<i64, String> = index
        .folders()
        .map_err(internal)?
        .into_iter()
        .map(|f| {
            let name = match role_of(&f) {
                Some("inbox") => "Inbox".to_string(),
                Some("outbox") => "Outbox".to_string(),
                _ => f.name.clone(),
            };
            (f.id, name)
        })
        .collect();

    let own: Vec<String> = account
        .config()
        .map(|c| [c.identity().map(|i| i.address), Some(c.username)].into_iter().flatten().collect())
        .unwrap_or_default();
    let checking_server = account.checking_server();
    let mut messages: Vec<ThreadMessage> = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut message = ThreadMessage {
            id: row.id,
            folder: format!("{}/{}", account.id, row.folder_id),
            folder_name: folders.get(&row.folder_id).cloned().unwrap_or_default(),
            parent: row.parent,
            guessed: row.guessed,
            sender_name: if row.from_name.is_empty() { row.from_addr.clone() } else { row.from_name.clone() },
            sender_address: row.from_addr.clone(),
            mine: own.iter().any(|o| o.eq_ignore_ascii_case(&row.from_addr)),
            check: recorded_check(row.verdict, &row.auth_server, &checking_server).into(),
            recipients: row.recipients.clone(),
            date: row.date,
            unread: !row.flags.contains(Flags::SEEN),
            flagged: row.flags.contains(Flags::FLAGGED),
            downloaded: row.downloaded,
            text: row.preview.clone(),
            ..ThreadMessage::default()
        };
        if row.downloaded {
            if let Ok(raw) = account.store.read_message(row.id) {
                let shown = store::parse::display(&raw);
                let reduced = reduce::reduce_reply(shown.text.as_deref().unwrap_or_default(), row.parent.is_some());
                message.text = reduced.text;
                message.quoted = reduced.quoted;
                message.quoted_lines = reduced.quoted_lines as u32;
                message.signature = reduced.signature;
                message.attachments = shown
                    .attachments
                    .into_iter()
                    .map(|a| AttachmentInfo { name: a.name, mime_type: a.mime_type, size: a.size })
                    .collect();
            }
        }
        messages.push(message);
    }

    // Signatures that only repetition reveals: a block that ends every
    // message a sender wrote in this conversation.
    let mut by_sender: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, m) in messages.iter().enumerate() {
        if m.downloaded && !m.text.is_empty() {
            by_sender.entry(m.sender_address.to_lowercase()).or_default().push(i);
        }
    }
    for indices in by_sender.values() {
        let texts: Vec<&str> = indices.iter().map(|i| messages[*i].text.as_str()).collect();
        let shared = reduce::shared_ending(&texts);
        if shared == 0 {
            continue;
        }
        for i in indices {
            let (text, ending) = reduce::split_ending(&messages[*i].text, shared);
            messages[*i].text = text;
            messages[*i].signature = match messages[*i].signature.is_empty() {
                true => ending,
                false => format!("{ending}\n{}", messages[*i].signature),
            };
        }
    }

    // Who each message added to, and dropped from, the conversation.
    let people = |m: &ThreadMessage| -> Vec<(String, String)> {
        let mut all = vec![(m.sender_address.to_lowercase(), m.sender_name.clone())];
        for r in compose::parse_recipients(&m.recipients).unwrap_or_default() {
            let name = if r.name.is_empty() { r.address.clone() } else { r.name };
            all.push((r.address.to_lowercase(), name));
        }
        all
    };
    let everyone: HashMap<i64, Vec<(String, String)>> = messages.iter().map(|m| (m.id, people(m))).collect();
    for message in &mut messages {
        let Some(before) = message.parent.and_then(|p| everyone.get(&p)) else { continue };
        let now = &everyone[&message.id];
        let had: BTreeSet<&str> = before.iter().map(|(a, _)| a.as_str()).collect();
        let has: BTreeSet<&str> = now.iter().map(|(a, _)| a.as_str()).collect();
        message.added = now.iter().filter(|(a, _)| !had.contains(a.as_str())).map(|(_, n)| n.clone()).collect();
        message.dropped = before.iter().filter(|(a, _)| !has.contains(a.as_str())).map(|(_, n)| n.clone()).collect();
    }

    Ok(ThreadView {
        subject: rows[0].subject.clone(),
        messages,
    })
}

/// Reads a message from its file and prepares it for the reading pane.
fn open_message(
    store: &Store,
    id: i64,
    checking_server: &str,
    protection: &settings::Protection,
) -> Result<MessageView, CoreError> {
    let raw = match store.read_message(id) {
        Ok(raw) => raw,
        Err(StoreError::NotDownloaded(_)) => {
            return Ok(MessageView {
                id,
                downloaded: false,
                sender_check: SenderCheck { state: "notChecked".into(), ..SenderCheck::default() },
                ..MessageView::default()
            })
        }
        Err(StoreError::NoSuchMessage(_)) => {
            return Err(CoreError::new(ErrorCode::UnknownMessage, format!("no message {id}")))
        }
        Err(e) => return Err(internal(e)),
    };
    let shown = store::parse::display(&raw);
    let mut sender_check = sender_check(&raw, &shown.from_addr, checking_server);
    if !protection.reply_warning {
        sender_check.reply_elsewhere = false;
    }
    let trusted = store.index().is_trusted(id).map_err(internal)?;
    let restricted = sender_check.state == "failed" && protection.disable_failed && !trusted;
    let allow = render::Allow {
        remote_images: !restricted && protection.images_allowed_from(&shown.from_addr),
        links: !restricted,
    };
    let images: Vec<(String, String, Vec<u8>)> = shown
        .inline_images
        .into_iter()
        .map(|i| (i.content_id, i.mime_type, i.data))
        .collect();
    let safe = shown
        .html
        .as_deref()
        .map(|html| render::safe_html_with(&render::embed_inline_images(html, &images), allow));
    Ok(MessageView {
        id,
        subject: shown.subject,
        sender_name: shown.from_name,
        sender_address: shown.from_addr,
        to: shown.to,
        cc: shown.cc,
        date: shown.date.unwrap_or(0),
        remote_content_blocked: safe.as_ref().is_some_and(|s| s.remote_content_blocked),
        remote_images_shown: safe.as_ref().is_some_and(|s| s.remote_images_kept),
        invitation: None,
        restricted,
        trusted,
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
        sender_check,
    })
}

/// What the account's own server recorded about a message's sender.
/// With no server to believe, nothing is: the message is "not checked".
fn sender_check(raw: &[u8], from_addr: &str, checking_server: &str) -> SenderCheck {
    let none = verify::Assessment {
        state: verify::State::NotChecked,
        server: String::new(),
        checks: Vec::new(),
        ambiguous_from: false,
    };
    let found = match checking_server.trim() {
        "" => none,
        server => verify::assess(raw, from_addr, Some(server)),
    };
    let reply_to = mail_parser::MessageParser::default()
        .parse_headers(raw)
        .and_then(|m| {
            m.reply_to().and_then(|list| list.first().and_then(|a| a.address().map(|s| s.to_string())))
        })
        .filter(|r| !r.eq_ignore_ascii_case(from_addr))
        .unwrap_or_default();
    let reply_elsewhere = !reply_to.is_empty()
        && !verify::related(&verify::domain_of(&reply_to), &verify::domain_of(from_addr));
    SenderCheck {
        state: found.state.word().into(),
        server: found.server,
        checks: found
            .checks
            .iter()
            .map(|c| CheckLine {
                name: c.name().into(),
                outcome: match c.outcome {
                    verify::Outcome::Pass => "pass",
                    verify::Outcome::Fail | verify::Outcome::SoftFail => "fail",
                    _ => "neutral",
                }
                .into(),
                word: c.word.clone(),
                sentence: c.sentence(),
                aligned: c.aligned,
            })
            .collect(),
        reply_to,
        reply_elsewhere,
        ambiguous_from: found.ambiguous_from,
        ..SenderCheck::default()
    }
}

/// A file name from a message, made safe to create: no directories, no
/// control characters, nothing Windows reserves, not hidden, not too long.
/// The extension is kept, because it decides what opens the file.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let mut cleaned = cleaned.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).to_string();
    if cleaned.chars().count() > 120 {
        let extension: String = match cleaned.rsplit_once('.') {
            Some((_, e)) if e.chars().count() <= 10 => format!(".{e}"),
            _ => String::new(),
        };
        let keep = 120 - extension.chars().count();
        cleaned = cleaned.chars().take(keep).collect::<String>() + &extension;
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_lowercase();
    let reserved = ["con", "prn", "aux", "nul"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("com") || stem.starts_with("lpt")) && stem.ends_with(|c: char| c.is_ascii_digit()));
    if cleaned.is_empty() || reserved {
        cleaned = format!("attachment {cleaned}").trim().to_string();
    }
    cleaned
}

/// Writes `data` into `dir` as `name`, or as `name (2)`, `name (3)`… if that
/// exists. Never replaces a file. Returns the path and the name used.
fn save_download(dir: &Path, name: &str, data: &[u8]) -> std::io::Result<(PathBuf, String)> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let name = safe_file_name(name);
    let (stem, extension) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    for n in 1..10_000 {
        let candidate = if n == 1 { name.clone() } else { format!("{stem} ({n}){extension}") };
        let path = dir.join(&candidate);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(data)?;
                return Ok((path, candidate));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many files with that name"))
}

pub(crate) fn denied(why: settings::Denied) -> CoreError {
    match why {
        settings::Denied::WrongPassword => {
            CoreError::new(ErrorCode::WrongPassword, "the settings password is needed, and that was not it")
        }
        settings::Denied::NotSaved(e) => CoreError::new(ErrorCode::Internal, format!("could not save the settings: {e}")),
    }
}

/// The word for a verdict the index holds, which counts only if it was
/// recorded by the server the account believes.
fn recorded_check(verdict: u8, recorded_by: &str, checking_server: &str) -> &'static str {
    if !checking_server.trim().is_empty() && recorded_by.eq_ignore_ascii_case(checking_server.trim()) {
        verify::State::from_number(verdict as i64)
    } else {
        verify::State::NotChecked
    }
    .word()
}

/// `checking_server` is the server whose sender checks the account
/// believes; a verdict recorded by any other counts for nothing.
fn message_rows(rows: Vec<ListRow>, first_index: u32, checking_server: &str) -> Vec<MessageRow> {
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| MessageRow {
            id: r.id,
            index: first_index + i as u32,
            sender_name: if r.from_name.is_empty() { r.from_addr.clone() } else { r.from_name },
            sender_address: r.from_addr,
            recipients: r.recipients,
            date: r.date,
            subject: r.subject,
            preview: r.preview,
            unread: !r.flags.contains(Flags::SEEN),
            flagged: r.flags.contains(Flags::FLAGGED),
            has_attachment: r.has_attachment,
            thread_count: 1,
            thread_unread: u32::from(!r.flags.contains(Flags::SEEN)),
            senders: String::new(),
            check: recorded_check(r.verdict, &r.auth_server, checking_server).into(),
        })
        .collect()
}

/// Rows for a conversation list. `own` are the user's addresses, shown as
/// "You" among the people in a conversation.
fn conversation_rows(
    rows: Vec<store::index::ConversationRow>,
    first_index: u32,
    own: &[String],
    checking_server: &str,
) -> Vec<MessageRow> {
    rows.into_iter()
        .enumerate()
        .map(|(i, c)| {
            let names: Vec<String> = c
                .senders
                .iter()
                .map(|(name, address)| {
                    if own.iter().any(|o| o.eq_ignore_ascii_case(address)) {
                        return "You".to_string();
                    }
                    let shown = if name.trim().is_empty() { address.split('@').next().unwrap_or(address) } else { name.trim() };
                    // In a list of several people, first names are enough.
                    match c.senders.len() {
                        1 => shown.to_string(),
                        _ => shown.split_whitespace().next().unwrap_or(shown).to_string(),
                    }
                })
                .collect();
            let senders = match names.len() {
                0..=4 => names.join(", "),
                n => format!("{}, {} and {} more", names[0], names[1], n - 2),
            };
            let mut row = message_rows(vec![c.head], first_index + i as u32, checking_server).remove(0);
            row.thread_count = c.count;
            row.thread_unread = c.unread;
            row.senders = senders;
            row
        })
        .collect()
}

/// A file that marks an account's directory as removed by the user.
pub(crate) const REMOVED_MARK: &str = "removed-by-user";

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
        // An account the user removed whose files could not all be deleted
        // at the time: finish the job, and do not show it.
        .filter(|name| {
            let dir = root.join(name);
            let removed = dir.join(REMOVED_MARK).exists();
            if removed {
                let _ = std::fs::remove_dir_all(&dir);
            }
            !removed
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let dir = root.join(&name);
            Ok(Arc::new(Account {
                id: format!("acct/{name}"),
                store: Arc::new(Store::open(&dir)?),
                config: RwLock::new(AccountConfig::load(&dir)),
                name,
                live: Mutex::new(Live {
                    password: None,
                    running: None,
                    status: status(SyncState::NotSyncing, ""),
                    announced: None,
                }),
                signals: Arc::default(),
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
