//! Calendar-and-contacts accounts as the core runs them: the list of
//! accounts, their background sync, and the commands the interface sends.
//!
//! An account is a directory under `dav/` in the data folder holding
//! `account.json` (the server's address and the user name, never the
//! password) and the store. The password is kept exactly as a mail
//! account's is: in the operating system's credential store, or in memory
//! only where there is none.

use super::client::{Dav, DavError, Kind};
use super::http::Url;
use super::item::{self, ContactEdit, EventEdit, Item, TaskEdit};
use super::store::{DavStore, StoreError};
use super::sync::{self, SyncError};
use crate::api::{
    Command, ContactContent, ContactRow, CoreError, DavAccountInfo, DavCollectionInfo, DavItemView, ErrorCode, Event,
    EventContent, EventRow, FolderId, Response, SyncState, SyncStatus, TaskContent, TaskRow,
};
use crate::Core;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often an account is checked when nothing prompts it sooner.
const INTERVAL: Duration = Duration::from_secs(5 * 60);
const CONFIG_FILE: &str = "account.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DavConfig {
    pub url: String,
    pub username: String,
}

impl DavConfig {
    fn secret_key(&self, name: &str) -> String {
        format!("dav:{name}:{}@{}", self.username, self.url)
    }
}

pub struct DavAccount {
    pub id: FolderId,
    name: String,
    dir: PathBuf,
    config: DavConfig,
    store: DavStore,
    live: Mutex<Live>,
    /// Prods the sync loop: sync now.
    wake: tokio::sync::Notify,
}

struct Live {
    password: Option<String>,
    status: SyncStatus,
    /// Set when the account is removed or the app is closing.
    stopped: bool,
    /// A sync loop is running.
    running: bool,
}

impl DavAccount {
    /// Ends the account's sync loop.
    pub(crate) fn stop(&self) {
        self.live.lock().unwrap().stopped = true;
        self.wake.notify_one();
    }
}

/// The time zone of this computer, for times written without one.
pub fn local_zone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string())
}

fn status(state: SyncState, detail: impl Into<String>) -> SyncStatus {
    SyncStatus { state, detail: detail.into() }
}

fn failed(e: impl std::fmt::Display) -> CoreError {
    CoreError::new(ErrorCode::Internal, e.to_string())
}

fn store_error(e: StoreError) -> CoreError {
    match e {
        StoreError::Missing => CoreError::new(ErrorCode::UnknownMessage, "it is no longer there"),
        StoreError::Unreadable => CoreError::new(ErrorCode::NotPossible, e.to_string()),
        other => failed(other),
    }
}

/// A directory name for an account, from its user name and server.
fn directory_name(config: &DavConfig, taken: impl Fn(&str) -> bool) -> String {
    let host = Url::parse(&config.url).map(|u| u.host).unwrap_or_default();
    let base: String = format!("{}-{host}", config.username.split('@').next().unwrap_or(""))
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let base = match base.trim_matches(|c| c == '.' || c == '-' || c == '_') {
        "" => "calendar".to_string(),
        b => b.chars().take(60).collect(),
    };
    (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|name| !taken(name)).expect("some number is free")
}

fn open_account(root: &Path, name: &str) -> Option<Arc<DavAccount>> {
    let dir = root.join(name);
    let config: DavConfig = serde_json::from_slice(&std::fs::read(dir.join(CONFIG_FILE)).ok()?).ok()?;
    let store = DavStore::open(&dir, &local_zone()).ok()?;
    Some(Arc::new(DavAccount {
        id: format!("dav/{name}"),
        name: name.to_string(),
        dir,
        config,
        store,
        live: Mutex::new(Live { password: None, status: status(SyncState::NotSyncing, ""), stopped: false, running: false }),
        wake: tokio::sync::Notify::new(),
    }))
}

/// The accounts kept in `data_dir`. A directory marked as removed is
/// deleted instead (see `remove`).
pub fn open_accounts(data_dir: &Path) -> Vec<Arc<DavAccount>> {
    let root = data_dir.join("dav");
    let Ok(entries) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| {
            let removed = root.join(name).join(crate::REMOVED_MARK).exists();
            if removed {
                let _ = std::fs::remove_dir_all(root.join(name));
            }
            !removed
        })
        .collect();
    names.sort();
    names.iter().filter_map(|name| open_account(&root, name)).collect()
}

impl Core {
    fn dav_accounts(&self) -> Vec<Arc<DavAccount>> {
        self.inner.dav.read().unwrap().clone()
    }

    fn dav_account(&self, id: &str) -> Result<Arc<DavAccount>, CoreError> {
        self.dav_accounts()
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| CoreError::new(ErrorCode::UnknownFolder, format!("no calendar account {id:?}")))
    }

    fn dav_changed(&self, account: &DavAccount) {
        self.inner.sink.emit(Event::DavChanged { account: account.id.clone() });
    }

    fn dav_status(&self, account: &DavAccount, new: SyncStatus) {
        {
            let mut live = account.live.lock().unwrap();
            if live.status == new {
                return;
            }
            live.status = new;
        }
        self.dav_changed(account);
    }

    /// Starts every stored account: with its stored password if there is
    /// one, otherwise waiting to be given one.
    pub(crate) fn begin_dav(&self) {
        for account in self.dav_accounts() {
            let stored = self.inner.secrets.get(&account.config.secret_key(&account.name));
            match stored {
                Some(password) => {
                    account.live.lock().unwrap().password = Some(password);
                    self.start_dav_sync(&account);
                }
                None => self.dav_status(&account, status(SyncState::NeedsPassword, "")),
            }
        }
    }

    /// Runs the account's sync loop, if it is not running already: a pass
    /// now, then one every few minutes and whenever something is changed
    /// here or the user asks.
    fn start_dav_sync(&self, account: &Arc<DavAccount>) {
        {
            let mut live = account.live.lock().unwrap();
            if live.running || live.stopped || live.password.is_none() {
                drop(live);
                account.wake.notify_one();
                return;
            }
            live.running = true;
        }
        let (core, account) = (self.clone(), account.clone());
        self.inner.runtime.spawn(async move {
            loop {
                let password = {
                    let live = account.live.lock().unwrap();
                    if live.stopped {
                        break;
                    }
                    live.password.clone()
                };
                let Some(password) = password else { break };
                let Some(url) = Url::parse(&account.config.url) else { break };
                core.dav_status(&account, status(SyncState::Syncing, ""));
                let dav = Dav::new(url, &account.config.username, &password);
                match sync::sync(&dav, &account.store).await {
                    Ok(outcome) => {
                        for text in outcome.notices {
                            core.inner.sink.emit(Event::Notice { account: account.id.clone(), text });
                        }
                        if outcome.changed {
                            core.dav_changed(&account);
                        }
                        core.dav_status(&account, status(SyncState::UpToDate, ""));
                    }
                    Err(SyncError::Dav(DavError::SignIn)) => {
                        // Not retried with a password that was refused.
                        account.live.lock().unwrap().password = None;
                        core.dav_status(&account, status(SyncState::SignInFailed, DavError::SignIn.to_string()));
                        break;
                    }
                    Err(e) => core.dav_status(&account, status(SyncState::Offline, e.to_string())),
                }
                tokio::select! {
                    _ = tokio::time::sleep(INTERVAL) => {}
                    _ = account.wake.notified() => {}
                }
            }
            account.live.lock().unwrap().running = false;
        });
    }

    fn dav_info(&self, account: &DavAccount) -> DavAccountInfo {
        let collections = account
            .store
            .collections()
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let (events, tasks, contacts) = match c.kind {
                    Kind::Calendar { events, tasks } => (events, tasks, false),
                    Kind::AddressBook => (false, false, true),
                };
                DavCollectionInfo { id: c.id, name: c.name, events, tasks, contacts, color: c.color, enabled: c.enabled }
            })
            .collect();
        DavAccountInfo {
            id: account.id.clone(),
            name: account.name.clone(),
            url: account.config.url.clone(),
            username: account.config.username.clone(),
            status: account.live.lock().unwrap().status.clone(),
            collections,
        }
    }

    /// Checks the details by asking the server for the user's collections,
    /// then saves the account and starts syncing it.
    async fn add_dav_account(&self, data_dir: PathBuf, config: DavConfig, password: String, remember: bool) -> Result<FolderId, String> {
        let url = Url::parse(&config.url).ok_or("that is not a web address")?;
        let dav = Dav::new(url, &config.username, &password);
        dav.discover().await.map_err(|e| e.to_string())?;
        let root = data_dir.join("dav");
        let core = self.clone();
        tokio::task::spawn_blocking(move || -> Result<FolderId, String> {
            if core.dav_accounts().iter().any(|a| a.config == config) {
                return Err("that account has already been added".into());
            }
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let name = directory_name(&config, |name| root.join(name).exists());
            let dir = root.join(&name);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let text = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(CONFIG_FILE), text).map_err(|e| e.to_string())?;
            if remember {
                // Not being able to store it is not a reason to refuse the
                // account: it is asked for again at the next start.
                let _ = core.inner.secrets.set(&config.secret_key(&name), &password);
            }
            let account = open_account(&root, &name).ok_or("the account could not be opened after saving it")?;
            account.live.lock().unwrap().password = Some(password);
            core.inner.dav.write().unwrap().push(account.clone());
            core.start_dav_sync(&account);
            Ok(account.id.clone())
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Handles the calendar, task and contact commands. `Err(command)`
    /// hands back a command that is not one of them.
    pub(crate) fn handle_dav(&self, command: Command) -> Result<Result<Response, CoreError>, Command> {
        Ok(match command {
            Command::ListDavAccounts => {
                Ok(Response::DavAccounts { accounts: self.dav_accounts().iter().map(|a| self.dav_info(a)).collect() })
            }
            Command::AddDavAccount { url, username, password, remember } => {
                let Some(data_dir) = self.inner.data_dir.clone() else {
                    return Ok(Err(CoreError::new(ErrorCode::NotPossible, "there is nowhere to keep an account")));
                };
                let config = DavConfig { url: url.trim().to_string(), username: username.trim().to_string() };
                if config.url.is_empty() || config.username.is_empty() || password.0.is_empty() {
                    return Ok(Err(CoreError::new(ErrorCode::NotPossible, "the address, user name and password are all needed")));
                }
                let core = self.clone();
                self.inner.runtime.spawn(async move {
                    let event = match core.add_dav_account(data_dir, config, password.0, remember).await {
                        Ok(account) => Event::DavAccountAdded { account },
                        Err(error) => Event::DavAccountSetupFailed { error },
                    };
                    core.inner.sink.emit(event);
                });
                Ok(Response::Accepted)
            }
            Command::ProvideDavPassword { account, password, remember } => self.dav_account(&account).map(|account| {
                if remember {
                    let _ = self.inner.secrets.set(&account.config.secret_key(&account.name), &password.0);
                }
                account.live.lock().unwrap().password = Some(password.0);
                self.start_dav_sync(&account);
                Response::Accepted
            }),
            Command::RemoveDavAccount { account, password } => self.dav_account(&account).and_then(|account| {
                if !self.inner.settings.allows(password.as_ref().map(|p| p.0.as_str())) {
                    return Err(crate::denied(crate::settings::Denied::WrongPassword));
                }
                account.stop();
                std::fs::write(account.dir.join(crate::REMOVED_MARK), b"").map_err(failed)?;
                self.inner.dav.write().unwrap().retain(|a| a.id != account.id);
                self.inner.secrets.delete(&account.config.secret_key(&account.name));
                let _ = std::fs::remove_dir_all(&account.dir);
                self.dav_changed(&account);
                Ok(Response::Accepted)
            }),
            Command::SetDavCollection { account, collection, enabled, color } => self.dav_account(&account).and_then(|account| {
                let color = color.filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|d| d.is_ascii_hexdigit()));
                account.store.choose(collection, enabled, color.as_deref()).map_err(store_error)?;
                self.dav_changed(&account);
                self.start_dav_sync(&account);
                Ok(Response::Accepted)
            }),
            Command::SyncDav { account } => self.dav_account(&account).map(|account| {
                self.start_dav_sync(&account);
                Response::Accepted
            }),
            Command::ListEvents { from, to } => (|| {
                let mut events = Vec::new();
                for account in self.dav_accounts() {
                    let colors = colors_of(&account)?;
                    for o in account.store.occurrences(from, to).map_err(store_error)? {
                        events.push(EventRow {
                            account: account.id.clone(),
                            id: o.id,
                            collection: o.collection,
                            color: colors.iter().find(|c| c.0 == o.collection).and_then(|c| c.1.clone()),
                            summary: o.summary,
                            location: o.location,
                            start: o.start,
                            end: o.end,
                            all_day: o.all_day,
                            recurring: o.recurring,
                        });
                    }
                }
                events.sort_by(|a, b| (a.start, !a.all_day, a.end).cmp(&(b.start, !b.all_day, b.end)));
                Ok(Response::Events { events })
            })(),
            Command::ListTasks => (|| {
                let mut tasks = Vec::new();
                for account in self.dav_accounts() {
                    let colors = colors_of(&account)?;
                    for t in account.store.tasks().map_err(store_error)? {
                        tasks.push(TaskRow {
                            account: account.id.clone(),
                            id: t.id,
                            collection: t.collection,
                            color: colors.iter().find(|c| c.0 == t.collection).and_then(|c| c.1.clone()),
                            summary: t.summary,
                            due: t.due,
                            due_all_day: t.due_all_day,
                            completed: t.completed,
                        });
                    }
                }
                tasks.sort_by(|a, b| {
                    (a.completed, a.due.is_none(), a.due, a.summary.to_lowercase()).cmp(&(b.completed, b.due.is_none(), b.due, b.summary.to_lowercase()))
                });
                Ok(Response::Tasks { tasks })
            })(),
            Command::ListContacts { query, limit } => (|| {
                let limit = limit.clamp(1, 5_000);
                let mut contacts = Vec::new();
                for account in self.dav_accounts() {
                    for c in account.store.contacts(&query, limit).map_err(store_error)? {
                        contacts.push(ContactRow { account: account.id.clone(), id: c.id, collection: c.collection, name: c.name, emails: c.emails });
                    }
                }
                contacts.sort_by_key(|c| c.name.to_lowercase());
                contacts.truncate(limit as usize);
                Ok(Response::Contacts { contacts })
            })(),
            Command::OpenDavItem { account, id } => self.dav_account(&account).and_then(|account| {
                let data = account.store.data(id).map_err(store_error)?;
                let collection = account.store.collection_of(id).map_err(store_error)?;
                let mut view = DavItemView { id, collection, ..DavItemView::default() };
                if account.store.collection(collection).map_err(store_error)?.kind == Kind::AddressBook {
                    let c = item::read_card(&data).ok_or_else(|| store_error(StoreError::Unreadable))?;
                    view.contact = Some(ContactContent { name: c.name, emails: c.emails, phones: c.phones, organisation: c.organisation, note: c.note });
                } else {
                    match item::read_calendar(&data, account.store.tz()).ok_or_else(|| store_error(StoreError::Unreadable))? {
                        Item::Event(e) => {
                            view.recurring = e.recurring;
                            view.event = Some(EventContent {
                                summary: e.summary,
                                location: e.location,
                                description: e.description,
                                start: e.start,
                                end: e.end,
                                all_day: e.all_day,
                            });
                        }
                        Item::Task(t) => {
                            view.task = Some(TaskContent { summary: t.summary, description: t.description, due: t.due, completed: t.completed })
                        }
                        Item::Contact(_) => {}
                    }
                }
                Ok(Response::DavItem { item: view })
            }),
            Command::SaveEvent { account, collection, id, event } => self.dav_account(&account).and_then(|account| {
                if event.end < event.start {
                    return Err(CoreError::new(ErrorCode::NotPossible, "the event ends before it starts"));
                }
                let edit = EventEdit {
                    summary: event.summary,
                    location: event.location,
                    description: event.description,
                    start: event.start,
                    end: event.end,
                    all_day: event.all_day,
                };
                let now = now();
                let data = match id {
                    Some(id) => item::edit_event(&account.store.data(id).map_err(store_error)?, &edit, now)
                        .ok_or_else(|| store_error(StoreError::Unreadable))?,
                    None => item::new_event(&format!("{}@outpost", sync::new_id()), &edit, now),
                };
                self.save_dav(&account, collection, id, &data)
            }),
            Command::SaveTask { account, collection, id, task } => self.dav_account(&account).and_then(|account| {
                let edit = TaskEdit { summary: task.summary, description: task.description, due: task.due, completed: task.completed };
                let now = now();
                let data = match id {
                    Some(id) => item::edit_task(&account.store.data(id).map_err(store_error)?, &edit, now, account.store.tz())
                        .ok_or_else(|| store_error(StoreError::Unreadable))?,
                    None => item::new_task(&format!("{}@outpost", sync::new_id()), &edit, now),
                };
                self.save_dav(&account, collection, id, &data)
            }),
            Command::SaveContact { account, collection, id, contact } => self.dav_account(&account).and_then(|account| {
                if contact.name.trim().is_empty() {
                    return Err(CoreError::new(ErrorCode::NotPossible, "a contact needs a name"));
                }
                let edit = ContactEdit {
                    name: contact.name,
                    emails: contact.emails,
                    phones: contact.phones,
                    organisation: contact.organisation,
                    note: contact.note,
                };
                let existing = match id {
                    Some(id) => Some(account.store.data(id).map_err(store_error)?),
                    None => None,
                };
                let data = item::write_card(existing.as_deref(), &sync::new_id(), &edit).ok_or_else(|| store_error(StoreError::Unreadable))?;
                self.save_dav(&account, collection, id, &data)
            }),
            Command::DeleteDavItem { account, id } => self.dav_account(&account).and_then(|account| {
                account.store.delete_local(id).map_err(store_error)?;
                self.dav_changed(&account);
                self.start_dav_sync(&account);
                Ok(Response::Accepted)
            }),
            other => return Err(other),
        })
    }

    fn save_dav(&self, account: &Arc<DavAccount>, collection: i64, id: Option<i64>, data: &str) -> Result<Response, CoreError> {
        let id = account.store.save_local(collection, id, data).map_err(store_error)?;
        self.dav_changed(account);
        self.start_dav_sync(account);
        Ok(Response::DavSaved { id })
    }
}

fn colors_of(account: &DavAccount) -> Result<Vec<(i64, Option<String>)>, CoreError> {
    Ok(account.store.collections().map_err(store_error)?.into_iter().map(|c| (c.id, c.color)).collect())
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}
