//! The sync engine: makes an account's local store mirror its IMAP server.
//!
//! In this milestone it only reads. It opens folders with EXAMINE and
//! fetches with PEEK, so it cannot change anything on the server.
//!
//! One pass ([`sync_once`]) does, in order:
//!
//! 1. **Folders.** The server's folder list is mirrored locally.
//! 2. **List data**, folder by folder, inbox first: what was removed or
//!    re-flagged on the server is applied, then headers of messages not yet
//!    known are fetched, newest first.
//! 3. **Bodies**, folder by folder, newest first.
//!
//! Every step commits as it goes and records only what is true, so a pass
//! that is cut off at any point is resumed by simply running another: the
//! index says which messages are known and which still lack a body.
//!
//! [`run`] repeats passes for as long as the account is open: it waits in
//! IDLE on the inbox for push, checks the other folders on a timer, and
//! reconnects with backoff when the connection fails.

mod actions;

use crate::imap::session::{RemoteFolder, SpecialUse};
use crate::imap::{ImapError, Server, Session};
use crate::store::maildir::{encode_segment, Flags};
use crate::store::sync_state::{HeaderOnly, KnownMessage, RemoteFolderInfo, SyncFolder, SyncState};
use crate::store::{Store, StoreError};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Account {
    pub server: Server,
    pub username: String,
}

/// The outgoing server for an account. It is signed in to with the same
/// password as the incoming one.
#[derive(Debug, Clone)]
pub struct Sending {
    pub server: crate::smtp::SmtpServer,
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Headers fetched per request.
    pub header_batch: usize,
    /// Use QRESYNC when the server offers it. Off only in tests, to exercise
    /// the path used with servers that lack it.
    pub fast_resync: bool,
    /// How long to stay in IDLE before checking the other folders.
    pub poll_interval: Duration,
    /// During a long download in another folder, how often to stop and
    /// bring the inbox up to date.
    pub inbox_check: Duration,
    /// How to send mail. `None` until the account has an outgoing server.
    pub sending: Option<Sending>,
    /// Folders the server marks as "all mail" hold a second copy of every
    /// message (Gmail). Skipped unless this is set.
    pub include_all_mail: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            header_batch: 200,
            fast_resync: true,
            poll_interval: Duration::from_secs(10 * 60),
            inbox_check: Duration::from_secs(30),
            sending: None,
            include_all_mail: false,
        }
    }
}

#[derive(Debug)]
pub enum SyncError {
    Imap(ImapError),
    Store(StoreError),
    /// Stopped on request.
    Stopped,
    /// An action could not be carried out for now and should be tried
    /// again later, without holding up the rest of the queue.
    Deferred(String),
    /// An action was refused for good.
    Refused(String),
    /// A message was handed to the outgoing server and the connection was
    /// lost before it answered: it may or may not have been sent.
    Uncertain(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Imap(e) => e.fmt(f),
            SyncError::Store(e) => e.fmt(f),
            SyncError::Stopped => write!(f, "sync was stopped"),
            SyncError::Deferred(e) | SyncError::Refused(e) | SyncError::Uncertain(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<ImapError> for SyncError {
    fn from(e: ImapError) -> Self {
        SyncError::Imap(e)
    }
}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> Self {
        SyncError::Store(e)
    }
}

type Result<T> = std::result::Result<T, SyncError>;

/// Progress and outcomes, for the status display. Sync is never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Connecting,
    /// The folder list was mirrored.
    Folders { count: usize },
    /// List data is being fetched for a folder.
    Headers { folder: String, done: usize, total: usize },
    /// A folder's contents changed locally.
    FolderChanged { folder_id: i64 },
    /// Bodies are being downloaded for a folder.
    Bodies { folder: String, done: usize, total: usize },
    /// Everything is up to date; waiting for changes.
    Idle,
    /// The connection failed; another attempt follows after `retry_in`.
    Offline { error: String, retry_in: Duration },
    /// The server refused the sign-in. No retry until credentials change.
    SignInFailed { error: String },
    /// The server would not carry out something the user did. It has been
    /// undone here where that was possible.
    ActionFailed { what: String, error: String },
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub folders: usize,
    pub headers_fetched: usize,
    pub bodies_fetched: usize,
    pub removed: usize,
    pub flags_changed: usize,
    /// Folders the server had renumbered, which were synced from scratch.
    pub folders_reset: usize,
    /// Things the user did that were carried out on the server.
    pub actions_replayed: usize,
}

/// How the rest of the app prods a running sync.
#[derive(Default)]
pub struct Signals {
    /// Do a full pass now.
    pub now: tokio::sync::Notify,
    /// The user did something: replay the action queue.
    pub changes: tokio::sync::Notify,
}

/// Lets a caller stop a running sync between steps.
#[derive(Clone)]
pub struct Stop(tokio::sync::watch::Receiver<bool>);

pub struct StopHandle(tokio::sync::watch::Sender<bool>);

pub fn stop_signal() -> (StopHandle, Stop) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (StopHandle(tx), Stop(rx))
}

impl StopHandle {
    pub fn stop(&self) {
        let _ = self.0.send(true);
    }
}

impl Stop {
    fn check(&self) -> Result<()> {
        if *self.0.borrow() {
            Err(SyncError::Stopped)
        } else {
            Ok(())
        }
    }

    /// Resolves when a stop is requested.
    async fn requested(&mut self) {
        while !*self.0.borrow() {
            if self.0.changed().await.is_err() {
                // The handle is gone: nobody can ask any more.
                std::future::pending::<()>().await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------

/// Where a server folder lives locally: its directory under the mail root
/// and the display name of its last level.
fn local_place(folder: &RemoteFolder) -> (String, String) {
    let levels = folder.levels();
    let path = levels.iter().map(|l| encode_segment(l)).collect::<Vec<_>>().join("/");
    let name = levels.last().map(|l| l.to_string()).unwrap_or_else(|| folder.name.clone());
    (path, name)
}

/// The order folders are synced in: the inbox, then ordinary folders, then
/// the ones people look at least.
fn priority(folder: &SyncFolder) -> u8 {
    if folder.remote_name.eq_ignore_ascii_case(b"INBOX") {
        return 0;
    }
    match folder.special_use.as_deref() {
        Some("sent") | Some("drafts") => 1,
        None | Some("flagged") => 2,
        Some("archive") | Some("all") => 3,
        Some("trash") => 4,
        Some("junk") => 5,
        Some(_) => 2,
    }
}

/// Mirrors the server's folder list. Returns the folders to sync, in order.
async fn sync_folders(
    session: &mut Session,
    store: &Store,
    options: &Options,
) -> Result<Vec<SyncFolder>> {
    let remote = session.list().await?;
    let mut listed: HashSet<String> = HashSet::new();
    for folder in &remote {
        let (path, name) = local_place(folder);
        if path.is_empty() {
            continue;
        }
        // Two server names that land on one local directory (which a
        // case-insensitive disk or an odd delimiter can cause) would mix
        // their mail. The first one wins and the other is left unsynced.
        if !listed.insert(path.to_lowercase()) {
            continue;
        }
        store.add_remote_folder(&RemoteFolderInfo {
            path: &path,
            name: &name,
            remote_name: &folder.raw_name,
            delimiter: folder.delimiter,
            selectable: folder.selectable,
            special_use: folder.special_use.map(SpecialUse::as_str),
        })?;
    }
    // A folder the server no longer lists was deleted or renamed elsewhere.
    for local in store.index().remote_folders()? {
        if !listed.contains(&local.path.to_lowercase()) {
            store.remove_folder(local.id)?;
        }
    }

    let mut folders: Vec<SyncFolder> = store
        .index()
        .remote_folders()?
        .into_iter()
        .filter(|f| f.selectable)
        .filter(|f| options.include_all_mail || f.special_use.as_deref() != Some("all"))
        .collect();
    folders.sort_by_key(|f| (priority(f), f.path.clone()));
    Ok(folders)
}

// ---------------------------------------------------------------------------
// One folder's list data
// ---------------------------------------------------------------------------

/// Brings one folder's message list, flags and removals up to date.
/// Bodies are not downloaded here.
async fn sync_folder(
    session: &mut Session,
    store: &Store,
    folder: &SyncFolder,
    options: &Options,
    stop: &Stop,
    progress: &mut (dyn FnMut(Progress) + Send),
    summary: &mut Summary,
) -> Result<()> {
    let index = store.index();
    let mut state = folder.state;
    let fast = options.fast_resync
        && session.qresync()
        && state.headers_done
        && state.uid_validity != 0
        && state.highest_modseq != 0;
    let opened = session
        .examine(
            &folder.remote_name,
            fast.then_some((state.uid_validity, state.highest_modseq)),
        )
        .await?;

    let mut changed = false;
    if state.uid_validity != opened.uid_validity {
        if state.uid_validity != 0 {
            // The server has renumbered this folder. Nothing local can be
            // matched to it any more, so the local copy is replaced.
            store.reset_folder(folder.id)?;
            summary.folders_reset += 1;
            changed = true;
        } else if !index.known_messages(folder.id)?.is_empty() {
            // Mail with UIDs but no record of which numbering they belong
            // to (imported, or the marker file was lost). Keep the mail, do
            // not pretend to know its UIDs.
            index.forget_uids(folder.id)?;
        }
        state = SyncState {
            uid_validity: opened.uid_validity,
            ..SyncState::default()
        };
        store.set_sync_state(folder.id, &state)?;
    }
    let fast = fast && state.headers_done;

    let known: HashMap<u32, KnownMessage> =
        index.known_messages(folder.id)?.into_iter().map(|m| (m.uid, m)).collect();
    // What the user has changed here that the server has not heard yet.
    // The server's view of those messages is out of date, not ours.
    let pending = index.pending_uids(folder.id)?;
    let mut gone: Vec<i64> = Vec::new();
    let mut reflagged: Vec<(u32, Flags)> = Vec::new();
    let mut wanted: Vec<u32> = Vec::new();

    if fast {
        // The server told us what changed since last time.
        for (first, last) in &opened.vanished {
            // A range can be far wider than what we hold: walk our side.
            gone.extend(known.values().filter(|m| m.uid >= *first && m.uid <= *last).map(|m| m.id));
        }
        for change in &opened.changed {
            if change.flags.contains(Flags::DELETED) {
                // Marked for deletion (here or elsewhere): as good as gone.
                gone.extend(known.get(&change.uid).map(|m| m.id));
            } else if pending.contains(&change.uid) {
                // Ours to change; the server will hear shortly.
            } else if known.contains_key(&change.uid) {
                reflagged.push((change.uid, change.flags));
            } else {
                // Changed, and not here: a message that had been marked
                // for deletion and no longer is.
                wanted.push(change.uid);
            }
        }
        // Anything new has a UID at or above the UIDNEXT we last saw.
        if opened.uid_next.is_none_or(|next| next > state.uid_next) {
            for current in session.fetch_flags(state.uid_next.max(1), None).await? {
                if current.uid >= state.uid_next
                    && !known.contains_key(&current.uid)
                    && !pending.contains(&current.uid)
                    && !current.flags.contains(Flags::DELETED)
                {
                    wanted.push(current.uid);
                }
            }
        }
    } else {
        // Compare complete lists.
        // A message marked for deletion is treated as already gone.
        let server: Vec<_> = session
            .fetch_flags(1, None)
            .await?
            .into_iter()
            .filter(|s| !s.flags.contains(Flags::DELETED))
            .collect();
        let on_server: HashSet<u32> = server.iter().map(|s| s.uid).collect();
        gone.extend(known.values().filter(|m| !on_server.contains(&m.uid)).map(|m| m.id));
        for current in &server {
            match known.get(&current.uid) {
                Some(local) if local.flags != current.flags && !pending.contains(&current.uid) => {
                    reflagged.push((current.uid, current.flags))
                }
                Some(_) => {}
                None if pending.contains(&current.uid) => {}
                None => wanted.push(current.uid),
            }
        }
    }

    if !gone.is_empty() {
        store.remove_messages(folder.id, &gone)?;
        summary.removed += gone.len();
        changed = true;
    }
    if !reflagged.is_empty() {
        let n = index.apply_server_flags(folder.id, &reflagged)?;
        summary.flags_changed += n as usize;
        changed |= n > 0;
    }
    if changed {
        progress(Progress::FolderChanged { folder_id: folder.id });
    }

    // Headers, newest first, a batch at a time. Each batch is committed, so
    // an interruption loses at most one batch of work.
    wanted.sort_unstable_by(|a, b| b.cmp(a));
    let total = wanted.len();
    let mut done = 0;
    for batch in wanted.chunks(options.header_batch.max(1)) {
        stop.check()?;
        let fetched = session.fetch_headers(batch).await?;
        let headers: Vec<HeaderOnly> = fetched
            .iter()
            .map(|m| HeaderOnly {
                uid: m.uid,
                flags: m.flags,
                size: m.size,
                internal_date: m.internal_date,
                header: &m.header,
            })
            .collect();
        store.add_headers(folder.id, &headers)?;
        summary.headers_fetched += headers.len();
        done += batch.len();
        progress(Progress::Headers {
            folder: folder.name.clone(),
            done,
            total,
        });
        progress(Progress::FolderChanged { folder_id: folder.id });
    }

    // Only now is it true that we have seen everything up to this point.
    state.uid_next = opened.uid_next.unwrap_or(state.uid_next);
    state.highest_modseq = opened.highest_modseq.unwrap_or(0);
    state.headers_done = true;
    store.set_sync_state(folder.id, &state)?;
    Ok(())
}

/// A message up to this size is fetched whole, along with others.
const SMALL_MESSAGE: u64 = 256 * 1024;
/// The most a group of small messages may add up to.
const GROUP_BYTES: u64 = 2 * 1024 * 1024;

/// Downloads the bodies a folder still lacks, newest first. The folder must
/// be the one currently open on `session`.
///
/// A big folder can take hours the first time. With `inbox` given, the
/// download pauses every `options.inbox_check` to bring the inbox up to
/// date, so new mail is not kept waiting behind it.
#[allow(clippy::too_many_arguments)]
async fn download_bodies(
    session: &mut Session,
    store: &Store,
    folder: &SyncFolder,
    inbox: Option<&SyncFolder>,
    options: &Options,
    stop: &Stop,
    progress: &mut (dyn FnMut(Progress) + Send),
    summary: &mut Summary,
) -> Result<()> {
    let mut run = BodyRun::default();
    loop {
        let until = inbox.map(|_| std::time::Instant::now() + options.inbox_check);
        if download_some(session, store, folder, until, &mut run, stop, progress, summary).await? {
            return Ok(());
        }
        let Some(inbox) = inbox else { continue };
        // Its sync position may have moved since this pass began.
        let current = store.index().remote_folders()?.into_iter().find(|f| f.id == inbox.id);
        if let Some(current) = current {
            sync_folder(session, store, &current, options, stop, progress, summary).await?;
            let mut inbox_run = BodyRun::default();
            download_some(session, store, &current, None, &mut inbox_run, stop, progress, summary).await?;
        }
        session.examine(&folder.remote_name, None).await?;
    }
}

#[derive(Default)]
struct BodyRun {
    done: usize,
    /// Messages that turned out to be gone from the server stay in the index
    /// until the next list sync removes them; do not ask for them twice.
    skipped: HashSet<i64>,
}

/// Downloads missing bodies until there are none (returns true) or until
/// the time `until` has passed (returns false).
#[allow(clippy::too_many_arguments)]
async fn download_some(
    session: &mut Session,
    store: &Store,
    folder: &SyncFolder,
    until: Option<std::time::Instant>,
    run: &mut BodyRun,
    stop: &Stop,
    progress: &mut (dyn FnMut(Progress) + Send),
    summary: &mut Summary,
) -> Result<bool> {
    let index = store.index();
    let BodyRun { done, skipped } = run;
    // However short the time allowed, each call fetches something, so a
    // download that keeps being interrupted still finishes.
    let mut fetched_any = false;
    loop {
        let batch: Vec<KnownMessage> = index
            .missing_bodies(folder.id, 50 + skipped.len() as u32)?
            .into_iter()
            .filter(|m| !skipped.contains(&m.id))
            .collect();
        if batch.is_empty() {
            return Ok(true);
        }
        // Small messages go several to a request, because a round trip per
        // message is what makes a first sync slow. Large ones go alone, in
        // pieces, so that no single reply is big.
        let mut i = 0;
        while i < batch.len() {
            stop.check()?;
            if fetched_any && until.is_some_and(|t| std::time::Instant::now() >= t) {
                progress(Progress::Bodies {
                    folder: folder.name.clone(),
                    done: *done,
                    total: *done + (index.missing_bodies_in(folder.id)? as usize).saturating_sub(skipped.len()),
                });
                progress(Progress::FolderChanged { folder_id: folder.id });
                return Ok(false);
            }
            fetched_any = true;
            let first = &batch[i];
            if first.size > SMALL_MESSAGE {
                match session.fetch_message(first.uid).await? {
                    Some(raw) => {
                        store.store_body(folder.id, first, &raw)?;
                        summary.bodies_fetched += 1;
                    }
                    None => {
                        skipped.insert(first.id);
                    }
                }
                *done += 1;
                i += 1;
                continue;
            }
            let mut group = vec![first];
            let mut bytes = first.size;
            while let Some(next) = batch.get(i + group.len()) {
                if next.size > SMALL_MESSAGE || bytes + next.size > GROUP_BYTES {
                    break;
                }
                bytes += next.size;
                group.push(next);
            }
            let uids: Vec<u32> = group.iter().map(|m| m.uid).collect();
            let mut fetched: HashMap<u32, Vec<u8>> =
                session.fetch_whole(&uids).await?.into_iter().collect();
            for message in &group {
                match fetched.remove(&message.uid) {
                    Some(raw) => {
                        store.store_body(folder.id, message, &raw)?;
                        summary.bodies_fetched += 1;
                    }
                    None => {
                        skipped.insert(message.id);
                    }
                }
            }
            *done += group.len();
            i += group.len();
        }
        progress(Progress::Bodies {
            folder: folder.name.clone(),
            done: *done,
            total: *done + (index.missing_bodies_in(folder.id)? as usize).saturating_sub(skipped.len()),
        });
        progress(Progress::FolderChanged { folder_id: folder.id });
    }
}

// ---------------------------------------------------------------------------
// Passes
// ---------------------------------------------------------------------------

async fn pass(
    session: &mut Session,
    store: &Store,
    options: &Options,
    password: &str,
    stop: &Stop,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Summary> {
    let mut summary = Summary::default();
    // What the user did comes first: the server must hear it before its
    // (older) view of those messages is read back.
    actions::replay(session, store, options, password, progress, &mut summary).await?;
    let folders = sync_folders(session, store, options).await?;
    summary.folders = folders.len();
    progress(Progress::Folders { count: folders.len() });

    // List data everywhere first, so every folder can be browsed early.
    for folder in &folders {
        stop.check()?;
        sync_folder(session, store, folder, options, stop, progress, &mut summary).await?;
    }
    // Then bodies.
    for folder in &folders {
        stop.check()?;
        if store.index().missing_bodies(folder.id, 1)?.is_empty() {
            continue;
        }
        session.examine(&folder.remote_name, None).await?;
        // While any folder but the inbox downloads, keep the inbox current.
        let is_inbox = |f: &SyncFolder| f.remote_name.eq_ignore_ascii_case(b"INBOX");
        let inbox = if is_inbox(folder) { None } else { folders.iter().find(|f| is_inbox(f)) };
        download_bodies(session, store, folder, inbox, options, stop, progress, &mut summary).await?;
    }
    Ok(summary)
}

/// Runs one complete pass and signs out.
pub async fn sync_once(
    store: &Store,
    account: &Account,
    password: &str,
    options: &Options,
    stop: &Stop,
    mut progress: impl FnMut(Progress) + Send,
) -> Result<Summary> {
    progress(Progress::Connecting);
    let mut session = Session::connect(&account.server, &account.username, password).await?;
    let result = pass(&mut session, store, options, password, stop, &mut progress).await;
    session.logout().await;
    result
}

/// Keeps an account in sync until stopped: a full pass, then IDLE on the
/// inbox for push, with the other folders checked every
/// `options.poll_interval`. Reconnects with backoff when the connection
/// fails. Returns only when stopped or when the server refuses the sign-in.
///
/// `signals.now` asks for a full pass straight away: it ends a wait between
/// passes and a wait before a retry. `signals.changes` says the user has
/// done something: the action queue is replayed at once.
pub async fn run(
    store: &Store,
    account: &Account,
    password: &str,
    options: &Options,
    mut stop: Stop,
    signals: &Signals,
    mut progress: impl FnMut(Progress) + Send,
) {
    let now = &signals.now;
    const FIRST_RETRY: Duration = Duration::from_secs(2);
    const LONGEST_RETRY: Duration = Duration::from_secs(5 * 60);
    let mut retry_in = FIRST_RETRY;
    loop {
        let outcome = connected(store, account, password, options, &mut stop, signals, &mut progress, &mut retry_in).await;
        match outcome {
            Err(SyncError::Stopped) | Ok(()) => return,
            Err(SyncError::Imap(e)) if !e.is_transient() && matches!(e, ImapError::Rejected { command: "LOGIN", .. }) => {
                progress(Progress::SignInFailed { error: e.to_string() });
                return;
            }
            Err(e) => {
                progress(Progress::Offline {
                    error: e.to_string(),
                    retry_in,
                });
                tokio::select! {
                    _ = tokio::time::sleep(retry_in) => retry_in = (retry_in * 2).min(LONGEST_RETRY),
                    _ = now.notified() => retry_in = FIRST_RETRY,
                    _ = stop.requested() => return,
                }
            }
        }
    }
}

/// One connection's worth of [`run`]. Returns `Ok` only when stopped.
async fn connected(
    store: &Store,
    account: &Account,
    password: &str,
    options: &Options,
    stop: &mut Stop,
    signals: &Signals,
    progress: &mut (dyn FnMut(Progress) + Send),
    retry_in: &mut Duration,
) -> Result<()> {
    let now = &signals.now;
    progress(Progress::Connecting);
    let mut session = Session::connect(&account.server, &account.username, password).await?;
    loop {
        pass(&mut session, store, options, password, stop, progress).await?;
        // A pass that completed means the connection is healthy again.
        *retry_in = Duration::from_secs(2);
        progress(Progress::Idle);

        // Wait on the inbox until the next poll is due, waking for push.
        let inbox = store
            .index()
            .remote_folders()?
            .into_iter()
            .find(|f| f.remote_name.eq_ignore_ascii_case(b"INBOX"));
        let poll_due = tokio::time::Instant::now() + options.poll_interval;
        // Set when the inbox was found to have moved on without us and was
        // synced for that reason, until a real wait has happened: so that a
        // server whose numbers never settle cannot keep us spinning.
        let mut caught_up = false;
        loop {
            stop.check()?;
            let remaining = poll_due.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let Some(inbox) = &inbox else {
                tokio::select! {
                    _ = tokio::time::sleep(remaining) => {}
                    _ = now.notified() => {}
                    _ = stop.requested() => return Ok(()),
                }
                break;
            };
            if !session.has("IDLE") {
                // No push: look at the inbox once a minute instead.
                tokio::select! {
                    _ = tokio::time::sleep(remaining.min(Duration::from_secs(60))) => {}
                    _ = now.notified() => break,
                    _ = stop.requested() => return Ok(()),
                }
            } else {
                let opened = session.examine(&inbox.remote_name, None).await?;
                // The server only pushes what changes while we are waiting.
                // Mail that arrived while this connection was busy (replaying
                // what the user did, downloading another folder) was reported
                // in the middle of that work, or not at all. So before
                // waiting, compare where the inbox is with where our last
                // sync of it ended.
                let synced = store.index().remote_folders()?.into_iter().find(|f| f.id == inbox.id).map(|f| f.state);
                let behind = !caught_up
                    && synced.is_some_and(|state| {
                        opened.uid_next.is_some_and(|next| next != state.uid_next)
                            || opened.highest_modseq.is_some_and(|modseq| modseq != state.highest_modseq)
                    });
                // Servers drop a connection that idles for 30 minutes.
                let limit = remaining.min(Duration::from_secs(25 * 60));
                let mut stopping = stop.clone();
                let asked = std::sync::atomic::AtomicBool::new(false);
                let acted = std::sync::atomic::AtomicBool::new(false);
                let wake = async {
                    tokio::select! {
                        _ = stopping.requested() => {}
                        _ = now.notified() => asked.store(true, std::sync::atomic::Ordering::Relaxed),
                        _ = signals.changes.notified() => acted.store(true, std::sync::atomic::Ordering::Relaxed),
                    }
                };
                // Anything the user did while we were busy goes first, and
                // the wait ends when a scheduled action (a message waiting
                // out its "undo send" pause, a retry) comes due.
                let limit = match store.index().next_action_due()? {
                    Some(due) => {
                        let wait = (due - crate::store::actions::now()).max(0) as u64 + 1;
                        limit.min(Duration::from_secs(wait))
                    }
                    None => limit,
                };
                let pushed = if !store.index().queued_actions(1)?.is_empty() {
                    acted.store(true, std::sync::atomic::Ordering::Relaxed);
                    false
                } else if behind {
                    caught_up = true;
                    true
                } else {
                    caught_up = false;
                    session.idle(limit, wake).await?
                };
                stop.check()?;
                if !store.index().queued_actions(1)?.is_empty() {
                    acted.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if acted.load(std::sync::atomic::Ordering::Relaxed) {
                    let mut summary = Summary::default();
                    actions::replay(&mut session, store, options, password, progress, &mut summary).await?;
                    if !pushed {
                        continue;
                    }
                }
                if asked.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if !pushed {
                    continue;
                }
            }
            // The inbox changed (or may have): bring just it up to date.
            let mut summary = Summary::default();
            let current = store
                .index()
                .remote_folders()?
                .into_iter()
                .find(|f| f.id == inbox.id);
            if let Some(current) = current {
                sync_folder(&mut session, store, &current, options, stop, progress, &mut summary).await?;
                session.examine(&current.remote_name, None).await?;
                download_bodies(&mut session, store, &current, None, options, stop, progress, &mut summary).await?;
                progress(Progress::Idle);
            }
        }
    }
}
