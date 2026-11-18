//! The core running accounts by itself: setup, stored passwords, sync
//! status, and events as mail arrives. Against a real Dovecot.

mod common;

use common::{message, TempDir, TestServer, PASSWORD};
use outpost_core::accounts::{MemorySecrets, Secrets};
use outpost_core::api::{Command, ErrorCode, Event, FolderInfo, Response, Secret, SyncState};
use outpost_core::{Config, Core};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn start(data: &TempDir, secrets: Arc<MemorySecrets>) -> (Core, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let config = Config { data_dir: Some(data.path().to_path_buf()) };
    let core = Core::start_with_secrets(config, secrets, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (core, rx)
}

/// Waits for an event that `wanted` accepts, skipping others.
fn wait_for<T>(events: &Receiver<Event>, what: &str, mut wanted: impl FnMut(&Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(event) => {
                if let Some(found) = wanted(&event) {
                    return found;
                }
            }
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

fn wait_for_state(events: &Receiver<Event>, state: SyncState) -> String {
    wait_for(events, &format!("{state:?}"), |e| match e {
        Event::AccountStatus { status, .. } if status.state == state => Some(status.detail.clone()),
        _ => None,
    })
}

fn add(core: &Core, server: &TestServer, user: &str, password: &str, remember: bool) {
    let reply = core
        .handle(Command::AddAccount {
            host: "127.0.0.1".into(),
            port: server.port,
            encrypted: false,
            username: user.into(),
            password: Secret(password.into()),
            remember,
        })
        .unwrap();
    assert_eq!(reply, Response::Accepted);
}

fn folders(core: &Core) -> Vec<FolderInfo> {
    match core.handle(Command::ListFolders).unwrap() {
        Response::Folders { folders } => folders,
        other => panic!("{other:?}"),
    }
}

fn subjects(core: &Core, folder: &str) -> Vec<String> {
    match core.handle(Command::ListMessages { folder: folder.into(), offset: 0, limit: 100 }).unwrap() {
        Response::Messages { rows, .. } => rows.into_iter().map(|r| r.subject).collect(),
        other => panic!("{other:?}"),
    }
}

fn state(core: &Core) -> SyncState {
    match core.handle(Command::ListAccounts).unwrap() {
        Response::Accounts { accounts } => accounts[0].status.state,
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_account_is_added_synced_and_kept_up_to_date() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("ann");
    other.cmd("CREATE Lists");
    other.append("INBOX", "", &message(1, "first", "hello"));
    let data = TempDir::new();
    let secrets = Arc::new(MemorySecrets::default());
    let (core, events) = start(&data, secrets.clone());
    assert!(folders(&core).iter().any(|f| f.id == "demo/inbox"), "an empty installation shows the synthetic account");

    // A wrong password adds nothing.
    add(&core, &server, "ann", "not the password", true);
    let error = wait_for(&events, "setup failure", |e| match e {
        Event::AccountSetupFailed { error } => Some(error.clone()),
        _ => None,
    });
    assert!(error.to_lowercase().contains("authentication") || error.contains("refused"), "{error}");
    assert!(std::fs::read_dir(data.path().join("accounts")).unwrap().next().is_none(), "nothing was saved");

    // The right one adds the account and syncs it.
    add(&core, &server, "ann", PASSWORD, true);
    let account = wait_for(&events, "the account to be added", |e| match e {
        Event::AccountAdded { account } => Some(account.clone()),
        _ => None,
    });
    assert_eq!(account, "acct/ann");
    wait_for_state(&events, SyncState::UpToDate);
    let tree = folders(&core);
    assert!(!tree.iter().any(|f| f.id.starts_with("demo")), "the synthetic account is gone");
    let inbox = tree.iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
    let lists = tree.iter().find(|f| f.name == "Lists").unwrap().id.clone();
    assert_eq!(subjects(&core, &inbox), ["first"]);
    assert_eq!(state(&core), SyncState::UpToDate);

    // The password is in the credential store and nowhere on disk.
    assert_eq!(secrets.get("ann:ann@127.0.0.1").as_deref(), Some(PASSWORD));
    let saved = std::fs::read_to_string(data.path().join("accounts/ann/account.json")).unwrap();
    assert!(saved.contains("127.0.0.1") && !saved.contains(PASSWORD), "{saved}");

    // The same account cannot be added twice.
    add(&core, &server, "ann", PASSWORD, true);
    wait_for(&events, "duplicate refusal", |e| matches!(e, Event::AccountSetupFailed { .. }).then_some(()));

    // New mail in the inbox is pushed, announced, and listed.
    other.append("INBOX", "", &message(2, "pushed", "hello"));
    wait_for(&events, "the pushed message", |e| match e {
        Event::FolderChanged { folder } if *folder == inbox && subjects(&core, &inbox).contains(&"pushed".to_string()) => Some(()),
        _ => None,
    });
    let Response::Messages { rows, .. } =
        core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 10 }).unwrap()
    else {
        panic!()
    };
    let first = rows.iter().find(|r| r.subject == "first").unwrap();
    let Response::Located { index } =
        core.handle(Command::LocateMessage { folder: inbox.clone(), id: first.id }).unwrap()
    else {
        panic!()
    };
    assert_eq!(index, Some(rows.iter().position(|r| r.subject == "first").unwrap() as u32));

    // Mail in another folder waits for the ten-minute poll, unless asked.
    other.append("Lists", "", &message(3, "listed", "hello"));
    std::thread::sleep(Duration::from_millis(300));
    assert!(subjects(&core, &lists).is_empty());
    core.handle(Command::SyncNow { account: account.clone() }).unwrap();
    wait_for(&events, "the polled message", |e| match e {
        Event::FolderChanged { folder } if *folder == lists && !subjects(&core, &lists).is_empty() => Some(()),
        _ => None,
    });

    // Closing and reopening: the stored password starts sync by itself.
    drop(core);
    other.append("INBOX", "", &message(4, "while closed", "hello"));
    let (core, events) = start(&data, secrets.clone());
    wait_for_state(&events, SyncState::UpToDate);
    assert!(subjects(&core, &inbox).contains(&"while closed".to_string()));
    drop(core);

    // With no stored password, the app asks instead of failing.
    let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
    assert_eq!(state(&core), SyncState::NeedsPassword);
    assert!(subjects(&core, &inbox).contains(&"first".to_string()), "mail already here can be read meanwhile");
    core.handle(Command::ProvidePassword { account: account.clone(), password: Secret("wrong".into()), remember: false }).unwrap();
    wait_for_state(&events, SyncState::SignInFailed);
    core.handle(Command::ProvidePassword { account, password: Secret(PASSWORD.into()), remember: false }).unwrap();
    wait_for_state(&events, SyncState::UpToDate);
}

#[test]
fn adding_an_account_needs_somewhere_to_keep_it() {
    let core = Core::start(|_| {});
    let refused = core
        .handle(Command::AddAccount {
            host: "mail.example.com".into(),
            port: 993,
            encrypted: true,
            username: "u".into(),
            password: Secret("p".into()),
            remember: false,
        })
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotPossible);
}
