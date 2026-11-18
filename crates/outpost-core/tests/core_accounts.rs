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
    let config = Config { data_dir: Some(data.path().to_path_buf()), downloads_dir: None };
    let core = Core::start_with_secrets(config, secrets, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (core, rx)
}

/// Waits for an event that `wanted` accepts, skipping others.
fn wait_for<T>(events: &Receiver<Event>, what: &str, mut wanted: impl FnMut(&Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
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
            sending: None,
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
    match core.handle(Command::ListMessages { folder: folder.into(), offset: 0, limit: 100, conversations: false }).unwrap() {
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
        core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 10, conversations: false }).unwrap()
    else {
        panic!()
    };
    let first = rows.iter().find(|r| r.subject == "first").unwrap();
    let Response::Located { index } =
        core.handle(Command::LocateMessage { folder: inbox.clone(), id: first.id, conversations: false }).unwrap()
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
            sending: None,
        })
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotPossible);
}

#[test]
fn an_out_of_date_index_is_rebuilt_from_the_mail_files_by_itself() {
    use outpost_core::store::maildir::Flags;
    use outpost_core::store::Store;
    let data = TempDir::new();
    let dir = data.path().join("accounts/old");
    {
        let store = Store::open(&dir).unwrap();
        let inbox = store.create_folder(&["INBOX"]).unwrap();
        for n in 0..25 {
            store.add_message(inbox, &message(n, &format!("kept {n}"), "still here"), Flags::SEEN, Some(n + 1)).unwrap();
        }
    }
    // Make the index look as if an older Outpost wrote it.
    for file in ["index.sqlite-wal", "index.sqlite-shm"] {
        let _ = std::fs::remove_file(dir.join(file));
    }
    {
        use std::io::{Seek, SeekFrom, Write};
        // SQLite keeps user_version as a big-endian number at byte 60.
        let mut file = std::fs::OpenOptions::new().write(true).open(dir.join("index.sqlite")).unwrap();
        file.seek(SeekFrom::Start(60)).unwrap();
        file.write_all(&1u32.to_be_bytes()).unwrap();
    }

    let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
    wait_for(&events, "the rebuild to finish", |e| matches!(e, Event::FoldersChanged { .. }).then_some(()));
    wait_for_state(&events, SyncState::NotSyncing);
    let tree = folders(&core);
    let inbox = tree.iter().find(|f| f.role.as_deref() == Some("inbox")).expect("the inbox is back");
    assert_eq!(inbox.total, 25);
    assert_eq!(subjects(&core, &inbox.id).len(), 25);
}

#[test]
fn what_the_user_does_in_the_app_reaches_the_server() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("zoe");
    for n in 1..=4 {
        other.append("INBOX", "", &message(n, &format!("m{n}"), "hello"));
    }
    let data = TempDir::new();
    let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
    add(&core, &server, "zoe", PASSWORD, false);
    wait_for_state(&events, SyncState::UpToDate);
    let tree = folders(&core);
    let folder = |role: &str| tree.iter().find(|f| f.role.as_deref() == Some(role)).unwrap().id.clone();
    let (inbox, trash, archive) = (folder("inbox"), folder("trash"), folder("archive"));
    let id = |subject: &str| -> i64 {
        match core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 100, conversations: false }).unwrap() {
            Response::Messages { rows, .. } => rows.into_iter().find(|r| r.subject == subject).unwrap().id,
            other => panic!("{other:?}"),
        }
    };
    let (m1, m2, m3, m4) = (id("m1"), id("m2"), id("m3"), id("m4"));

    core.handle(Command::ChangeFlags { folder: inbox.clone(), ids: vec![m1], seen: Some(true), flagged: Some(true) }).unwrap();
    core.handle(Command::DeleteMessages { folder: inbox.clone(), ids: vec![m2], permanently: false }).unwrap();
    core.handle(Command::ArchiveMessages { folder: inbox.clone(), ids: vec![m3] }).unwrap();
    core.handle(Command::DeleteMessages { folder: inbox.clone(), ids: vec![m4], permanently: true }).unwrap();

    // Here, at once.
    assert_eq!(subjects(&core, &inbox), ["m1"]);
    assert_eq!(subjects(&core, &trash), ["m2"]);
    assert_eq!(subjects(&core, &archive), ["m3"]);
    let Response::Messages { rows, .. } =
        core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 10, conversations: false }).unwrap()
    else {
        panic!()
    };
    assert!(!rows[0].unread && rows[0].flagged);

    // On the server, shortly, without anyone asking for a sync.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (i, t, a) = (other.subjects_in("INBOX"), other.subjects_in("Trash"), other.subjects_in("Archive"));
        if i == [("m1".to_string(), "\\Flagged \\Seen".to_string())] && t.len() == 1 && a.len() == 1 {
            assert_eq!((t[0].0.as_str(), a[0].0.as_str()), ("m2", "m3"));
            break;
        }
        assert!(Instant::now() < deadline, "the server never caught up: {i:?} {t:?} {a:?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    // And nothing bounces back afterwards.
    core.handle(Command::SyncNow { account: "acct/zoe".into() }).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(subjects(&core, &inbox), ["m1"]);
    assert_eq!(subjects(&core, &trash), ["m2"]);
    assert_eq!(subjects(&core, &archive), ["m3"]);
}

#[test]
fn mail_that_is_only_browsed_cannot_be_changed() {
    use outpost_core::store::maildir::Flags;
    use outpost_core::store::Store;
    let data = TempDir::new();
    let id = {
        let store = Store::open(data.path().join("accounts/local")).unwrap();
        let inbox = store.create_folder(&["INBOX"]).unwrap();
        store.add_message(inbox, &message(1, "kept", "x"), Flags::default(), None).unwrap()
    };
    let (core, _events) = start(&data, Arc::new(MemorySecrets::default()));
    let inbox = folders(&core).into_iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id;
    for command in [
        Command::ChangeFlags { folder: inbox.clone(), ids: vec![id], seen: Some(true), flagged: None },
        Command::DeleteMessages { folder: inbox.clone(), ids: vec![id], permanently: true },
        Command::MoveMessages { ids: vec![id], to: inbox.clone() },
    ] {
        assert_eq!(core.handle(command).unwrap_err().code, ErrorCode::NotPossible);
    }
    assert_eq!(subjects(&core, &inbox), ["kept"]);
}

mod sending {
    use super::*;
    use common::smtp::{Behaviour, SmtpSink};
    use outpost_core::api::{DraftContent, SendingSettings};

    fn until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !check() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn folder_named(core: &Core, role_or_name: &str) -> Option<String> {
        folders(core)
            .into_iter()
            .find(|f| f.role.as_deref() == Some(role_or_name) || f.name == role_or_name)
            .map(|f| f.id)
    }

    fn send(core: &Core, draft: DraftContent, delay_seconds: u32) -> i64 {
        match core.handle(Command::SendMessage { account: "acct/sam".into(), draft, delay_seconds }).unwrap() {
            Response::SendQueued { id } => id,
            other => panic!("{other:?}"),
        }
    }

    fn to(address: &str, subject: &str) -> DraftContent {
        DraftContent {
            to: address.into(),
            subject: subject.into(),
            body: format!("Body of {subject}.\n"),
            ..DraftContent::default()
        }
    }

    #[test]
    fn mail_is_sent_filed_and_never_lost_or_doubled() {
        let Some(server) = TestServer::start() else { return };
        let sink = SmtpSink::start();
        let mut other = server.other_client("sam");
        other.append("INBOX", "", &message(1, "a question", "Can you make Friday?"));
        let data = TempDir::new();
        let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
        let sending = SendingSettings {
            name: "Sam Sender".into(),
            address: "sam@example.com".into(),
            host: "127.0.0.1".into(),
            port: sink.port,
            security: "plaintext".into(),
        };
        core.handle(Command::AddAccount {
            host: "127.0.0.1".into(),
            port: server.port,
            encrypted: false,
            username: "sam".into(),
            password: Secret(PASSWORD.into()),
            remember: false,
            sending: Some(sending.clone()),
        })
        .unwrap();
        wait_for_state(&events, SyncState::UpToDate);
        let Response::Accounts { accounts } = core.handle(Command::ListAccounts).unwrap() else { panic!() };
        assert_eq!(accounts[0].sending, Some(sending));
        let inbox = folder_named(&core, "inbox").unwrap();
        let sent = folder_named(&core, "sent").unwrap();
        let drafts = folder_named(&core, "drafts").unwrap();

        // 1. A new message, with a blind copy.
        let draft = DraftContent { cc: "Carol <carol@example.com>".into(), bcc: "secret@example.org".into(), ..to("Alice <alice@example.com>", "hello") };
        send(&core, draft, 0);
        until("the message to be sent", || sink.messages().len() == 1);
        let got = &sink.messages()[0];
        assert_eq!(got.from, "sam@example.com");
        assert_eq!(got.recipients, ["alice@example.com", "carol@example.com", "secret@example.org"]);
        let wire = String::from_utf8_lossy(&got.message).into_owned();
        assert!(wire.contains("Subject: hello") && wire.contains("Sam Sender") && wire.contains("Body of hello."));
        assert!(!wire.contains("secret@example.org"), "the blind copy is not revealed to recipients");
        until("the copy in Sent, here and on the server", || {
            subjects(&core, &sent) == ["hello"] && other.subjects_in("Sent").len() == 1
        });
        assert!(folder_named(&core, "Outbox").is_none(), "the outbox empties and disappears");

        // 2. A reply: threaded, and the original is marked answered.
        let question = match core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 10, conversations: false }).unwrap() {
            Response::Messages { rows, .. } => rows[0].id,
            other => panic!("{other:?}"),
        };
        let Response::Draft { draft } =
            core.handle(Command::ComposeReply { folder: inbox.clone(), id: question, mode: "reply".into() }).unwrap()
        else {
            panic!()
        };
        assert_eq!((draft.to.as_str(), draft.subject.as_str()), ("Sender 1 <sender1@example.com>", "Re: a question"));
        assert!(draft.body.contains("> Can you make Friday?"));
        send(&core, DraftContent { body: format!("Yes.{}", draft.body), ..draft }, 0);
        until("the reply to be sent", || sink.messages().len() == 2);
        let wire = String::from_utf8_lossy(&sink.messages()[1].message).into_owned();
        assert!(wire.contains("In-Reply-To: <1.10@example.com>"), "{wire}");
        until("the original to be marked answered on the server", || {
            other.subjects_in("INBOX") == [("a question".to_string(), "\\Answered".to_string())]
        });

        // 3. Undo: taken back before it goes, it is a draft again.
        let pending = send(&core, to("alice@example.com", "second thoughts"), 60);
        assert_eq!(subjects(&core, &folder_named(&core, "Outbox").unwrap()), ["second thoughts"]);
        let Response::SendCancelled { cancelled } =
            core.handle(Command::CancelSend { account: "acct/sam".into(), id: pending }).unwrap()
        else {
            panic!()
        };
        assert!(cancelled);
        assert_eq!(subjects(&core, &drafts), ["second thoughts"]);
        until("the draft to reach the server", || other.subjects_in("Drafts").len() == 1);
        assert_eq!(sink.messages().len(), 2, "it was not sent");

        // 4. A recipient the server refuses: back to Drafts, with a notice.
        send(&core, to("nobody@example.com", "undeliverable"), 0);
        let notice = wait_for(&events, "a notice about the refusal", |e| match e {
            Event::Notice { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert!(notice.contains("nobody@example.com") && notice.contains("Drafts"), "{notice}");
        until("it to be back in Drafts", || subjects(&core, &drafts).contains(&"undeliverable".to_string()));
        assert_eq!(sink.messages().len(), 2);

        // 5. The outgoing server is busy: the message waits, and other
        //    things the user does are not held up behind it.
        sink.behave(Behaviour::Busy);
        send(&core, to("alice@example.com", "patient"), 0);
        std::thread::sleep(Duration::from_millis(800));
        core.handle(Command::ChangeFlags { folder: inbox.clone(), ids: vec![question], seen: Some(true), flagged: Some(true) }).unwrap();
        until("the flag to reach the server despite the stuck message", || {
            other.subjects_in("INBOX") == [("a question".to_string(), "\\Answered \\Flagged \\Seen".to_string())]
        });
        assert_eq!(sink.messages().len(), 2);
        assert_eq!(subjects(&core, &folder_named(&core, "Outbox").unwrap()), ["patient"]);
        sink.behave(Behaviour::Normal);
        core.handle(Command::SyncNow { account: "acct/sam".into() }).unwrap();
        until("the waiting message to go once the server recovers", || sink.messages().len() == 3);
        until("it to be filed in Sent", || subjects(&core, &sent).contains(&"patient".to_string()));

        // 6. The connection is lost after the message was handed over. It
        //    may have gone (here it did), so it is never sent again by
        //    itself; it waits in the outbox for the user.
        sink.behave(Behaviour::VanishAfterMessage);
        send(&core, to("alice@example.com", "maybe"), 0);
        let notice = wait_for(&events, "a notice about the uncertain send", |e| match e {
            Event::Notice { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert!(notice.contains("Outbox"), "{notice}");
        assert_eq!(sink.messages().len(), 4);
        sink.behave(Behaviour::Normal);
        core.handle(Command::SyncNow { account: "acct/sam".into() }).unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(sink.messages().len(), 4, "not sent a second time");
        assert_eq!(subjects(&core, &folder_named(&core, "Outbox").unwrap()), ["maybe"]);

        // Nothing was doubled on the way.
        let mut in_sent = subjects(&core, &sent);
        in_sent.sort();
        assert_eq!(in_sent, ["Re: a question", "hello", "patient"]);
        until("Sent on the server to match", || other.subjects_in("Sent").len() == 3);
    }

    #[test]
    fn drafts_are_saved_reopened_and_replaced() {
        let Some(server) = TestServer::start() else { return };
        let sink = SmtpSink::start();
        let mut other = server.other_client("sam");
        let data = TempDir::new();
        let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
        add(&core, &server, "sam", PASSWORD, false);
        wait_for_state(&events, SyncState::UpToDate);
        let account: String = "acct/sam".into();

        // Without sending set up, nothing can be written or sent.
        let refused = core.handle(Command::SaveDraft { account: account.clone(), draft: to("a@example.com", "x") });
        assert_eq!(refused.unwrap_err().code, ErrorCode::NotPossible, "the account has no address yet");
        let bad = SendingSettings { name: "Sam".into(), address: "not an address".into(), host: "h".into(), port: 465, security: "tls".into() };
        assert_eq!(
            core.handle(Command::SetSending { account: account.clone(), sending: bad }).unwrap_err().code,
            ErrorCode::NotPossible
        );
        let sending = SendingSettings {
            name: "Sam Sender".into(),
            address: "sam@example.com".into(),
            host: "127.0.0.1".into(),
            port: sink.port,
            security: "plaintext".into(),
        };
        core.handle(Command::SetSending { account: account.clone(), sending }).unwrap();
        let saved = std::fs::read_to_string(data.path().join("accounts/sam/account.json")).unwrap();
        assert!(saved.contains("sam@example.com") && !saved.contains(PASSWORD), "{saved}");

        let drafts = folder_named(&core, "drafts").unwrap();
        let Response::DraftSaved { id } =
            core.handle(Command::SaveDraft { account: account.clone(), draft: to("alice@example.com", "first version") }).unwrap()
        else {
            panic!()
        };
        until("the draft on the server", || other.subjects_in("Drafts").len() == 1);

        // Reopen, change, save again: the new one replaces the old.
        let Response::Draft { draft } = core.handle(Command::EditDraft { folder: drafts.clone(), id }).unwrap() else { panic!() };
        assert_eq!((draft.to.as_str(), draft.subject.as_str(), draft.replaces), ("alice@example.com", "first version", Some(id)));
        let changed = DraftContent { subject: "second version".into(), ..draft };
        let Response::DraftSaved { id: second } =
            core.handle(Command::SaveDraft { account: account.clone(), draft: changed }).unwrap()
        else {
            panic!()
        };
        assert_eq!(subjects(&core, &drafts), ["second version"]);
        until("only the new draft on the server", || {
            other.subjects_in("Drafts").iter().map(|d| d.0.clone()).collect::<Vec<_>>() == ["second version"]
        });

        // Sending a draft removes it from Drafts.
        let Response::Draft { draft } = core.handle(Command::EditDraft { folder: drafts.clone(), id: second }).unwrap() else { panic!() };
        send(&core, draft, 0);
        until("it to be sent", || sink.messages().len() == 1);
        until("Drafts to be empty, here and there", || {
            subjects(&core, &drafts).is_empty() && other.subjects_in("Drafts").is_empty()
        });

        // A message with nobody to go to, or a bad address, is not accepted.
        for bad in [to("", "nobody"), to("not-an-address", "bad")] {
            let refused = core.handle(Command::SendMessage { account: account.clone(), draft: bad, delay_seconds: 0 });
            assert_eq!(refused.unwrap_err().code, ErrorCode::NotPossible);
        }
        assert_eq!(sink.messages().len(), 1);
    }
}
