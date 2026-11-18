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

/// The known-result set: messages whose right answer is known, as the
/// account's own server (`mx.home.example`) and others would mark them.
/// (subject, From address, extra headers, the state that must be shown)
const KNOWN_RESULTS: &[(&str, &str, &str, &str)] = &[
    ("dmarc pass", "alice@bank.example", "Authentication-Results: mx.home.example; spf=pass smtp.mailfrom=bounce@bank.example; dkim=pass header.d=bank.example; dmarc=pass header.from=bank.example\r\n", "verified"),
    ("signed by its own domain", "news@shop.example", "Authentication-Results: mx.home.example;\r\n\tdkim=pass (2048-bit key) header.d=shop.example header.i=@shop.example;\r\n\tspf=softfail smtp.mailfrom=bounce@mailer.example\r\n", "verified"),
    ("signed only by a mailing service", "alice@bank.example", "Authentication-Results: mx.home.example; spf=pass smtp.mailfrom=bounce@mailer.example; dkim=pass header.d=mailer.example\r\n", "unverified"),
    ("dmarc fail", "alice@bank.example", "Authentication-Results: mx.home.example; spf=pass smtp.mailfrom=x@evil.example; dkim=pass header.d=evil.example; dmarc=fail header.from=bank.example\r\n", "failed"),
    ("spf fail", "alice@bank.example", "Authentication-Results: mx.home.example; spf=fail smtp.mailfrom=alice@bank.example; dkim=none\r\n", "failed"),
    ("nothing recorded", "carol@plain.example", "", "notChecked"),
    ("results recorded as none", "carol@plain.example", "Authentication-Results: mx.home.example; none\r\n", "notChecked"),
    ("forged under another server's name", "alice@bank.example", "Authentication-Results: mx.evil.example; dkim=pass header.d=bank.example; dmarc=pass header.from=bank.example\r\n", "notChecked"),
    ("forged below the real result", "alice@bank.example", "Authentication-Results: mx.home.example; spf=softfail smtp.mailfrom=alice@bank.example; dkim=none; dmarc=none header.from=bank.example\r\nAuthentication-Results: mx.home.example; dkim=pass header.d=bank.example; dmarc=pass header.from=bank.example\r\n", "unverified"),
    ("forged below a real failure", "alice@bank.example", "Authentication-Results: mx.home.example; dmarc=fail header.from=bank.example\r\nAuthentication-Results: mx.home.example; dmarc=pass header.from=bank.example\r\n", "failed"),
    ("passes for a lookalike", "alice@bank.example", "Authentication-Results: mx.home.example; dkim=pass header.d=bank.example.evil.example; spf=pass smtp.mailfrom=x@bank-example.example\r\n", "unverified"),
    ("two senders named", "alice@bank.example", "Authentication-Results: mx.home.example; dmarc=pass header.from=bank.example\r\nFrom: boss@victim.example\r\n", "unverified"),
];

fn checks(core: &Core, folder: &str) -> std::collections::BTreeMap<String, String> {
    match core.handle(Command::ListMessages { folder: folder.into(), offset: 0, limit: 100, conversations: false }).unwrap() {
        Response::Messages { rows, .. } => rows.into_iter().map(|r| (r.subject, r.check)).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_known_result_set_shows_the_right_state_every_time() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("ann");
    for (i, (subject, from, headers, _)) in KNOWN_RESULTS.iter().enumerate() {
        let raw = format!(
            "Received: from elsewhere by mx.home.example; Mon, 16 Nov 2026 10:{i:02}:00 +0000\r\n{headers}From: Somebody <{from}>\r\nReply-To: Collector <collect@elsewhere.example>\r\nTo: ann@example.com\r\nSubject: {subject}\r\nDate: Mon, 16 Nov 2026 10:{i:02}:00 +0000\r\nMessage-ID: <known-{i}@test>\r\n\r\nHello.\r\n"
        );
        other.append("INBOX", "", raw.as_bytes());
    }
    let data = TempDir::new();
    let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
    add(&core, &server, "ann", PASSWORD, true);
    wait_for_state(&events, SyncState::UpToDate);
    let inbox = folders(&core).iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();

    // The server to believe was worked out from the mail, and is marked as
    // a guess until the user confirms it.
    let account = || match core.handle(Command::ListAccounts).unwrap() {
        Response::Accounts { accounts } => accounts[0].clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!((account().checking_server.as_str(), account().checking_server_confirmed), ("mx.home.example", false));

    let expected: std::collections::BTreeMap<String, String> =
        KNOWN_RESULTS.iter().map(|(subject, _, _, state)| (subject.to_string(), state.to_string())).collect();
    assert_eq!(checks(&core, &inbox), expected);

    // Opening a message gives the same state as its row, every time, with
    // each check spelled out.
    let Response::Messages { rows, .. } =
        core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 100, conversations: false }).unwrap()
    else {
        panic!()
    };
    for row in &rows {
        let Response::Message { message } = core.handle(Command::OpenMessage { folder: inbox.clone(), id: row.id }).unwrap() else {
            panic!()
        };
        assert_eq!(message.sender_check.state, row.check, "{}", row.subject);
        assert_eq!(message.sender_check.state == "verified", expected[&row.subject] == "verified", "{}", row.subject);
        assert!(message.sender_check.reply_elsewhere, "{}", row.subject);
        assert_eq!(message.sender_check.reply_to, "collect@elsewhere.example");
        if row.subject == "dmarc fail" {
            let c = &message.sender_check;
            assert_eq!(c.server, "mx.home.example");
            let lines: Vec<(&str, &str, bool)> = c.checks.iter().map(|l| (l.name.as_str(), l.outcome.as_str(), l.aligned)).collect();
            assert_eq!(lines, [("SPF", "pass", false), ("DKIM", "pass", false), ("DMARC", "fail", true)]);
            assert!(c.checks[2].sentence.contains("bank.example"), "{}", c.checks[2].sentence);
        }
        if row.subject == "two senders named" {
            assert!(message.sender_check.ambiguous_from);
        }
    }

    // The user names a different server: nothing the old one said counts,
    // and what the newly named one "said" now does.
    core.handle(Command::SetCheckingServer { account: "acct/ann".into(), server: " MX.Evil.Example. ".into() }).unwrap();
    assert_eq!((account().checking_server.as_str(), account().checking_server_confirmed), ("mx.evil.example", true));
    let now = checks(&core, &inbox);
    assert_eq!(now["dmarc pass"], "notChecked");
    assert_eq!(now["dmarc fail"], "notChecked");
    assert_eq!(now["forged under another server's name"], "verified");

    // No server named: nothing is believed. A confirmed choice is not
    // replaced by a guess on the next sync.
    core.handle(Command::SetCheckingServer { account: "acct/ann".into(), server: String::new() }).unwrap();
    assert!(checks(&core, &inbox).values().all(|state| state == "notChecked"));
    other.append("INBOX", "", b"Authentication-Results: mx.home.example; dmarc=pass header.from=bank.example\r\nFrom: alice@bank.example\r\nSubject: later\r\n\r\nHello.\r\n");
    wait_for(&events, "the later message", |e| match e {
        Event::FolderChanged { folder } if *folder == inbox && checks(&core, &inbox).contains_key("later") => Some(()),
        _ => None,
    });
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(account().checking_server, "");
    assert_eq!(checks(&core, &inbox)["later"], "notChecked");
}

#[test]
fn a_new_sender_is_compared_with_what_came_before() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("ann");
    let mail = |minute: u32, name: &str, address: &str, to: &str, subject: &str| {
        format!(
            "From: {name} <{address}>\r\nTo: {to}\r\nSubject: {subject}\r\nDate: Mon, 16 Nov 2026 10:{minute:02}:00 +0000\r\nMessage-ID: <{minute}@history>\r\n\r\nHello.\r\n"
        )
    };
    // Mail the user is used to getting from the bank, and one message the
    // user wrote to a friend.
    for i in 0..4 {
        other.append("INBOX", "", mail(i, "First National", "service@firstnational.example", "ann@example.com", &format!("statement {i}")).as_bytes());
    }
    other.append("Sent", "", mail(5, "Ann", "ann@example.com", "Old Friend <friend@elsewhere.example>", "hello friend").as_bytes());
    // Then: the bank again, the friend answering, a stranger, the bank's
    // name on a new address at a lookalike domain, and Ann herself.
    other.append("INBOX", "", mail(10, "First National", "service@firstnational.example", "ann@example.com", "the bank again").as_bytes());
    other.append("INBOX", "", mail(11, "Old Friend", "friend@elsewhere.example", "ann@example.com", "the friend answers").as_bytes());
    other.append("INBOX", "", mail(12, "Stranger", "someone@new.example", "ann@example.com", "a stranger").as_bytes());
    other.append("INBOX", "", mail(13, "First National", "service@firstnationa1.example", "ann@example.com", "an impostor").as_bytes());
    other.append("INBOX", "", mail(14, "Ann", "ann@example.com", "ann@example.com", "a note to self").as_bytes());

    let data = TempDir::new();
    let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
    add(&core, &server, "ann", PASSWORD, true);
    wait_for_state(&events, SyncState::UpToDate);
    core.handle(Command::SetSending {
        account: "acct/ann".into(),
        sending: outpost_core::api::SendingSettings {
            name: "Ann".into(),
            address: "ann@example.com".into(),
            host: "127.0.0.1".into(),
            port: 1,
            security: "plaintext".into(),
            ..Default::default()
        },
    })
    .unwrap();
    let inbox = folders(&core).iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
    let Response::Messages { rows, .. } =
        core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 100, conversations: false }).unwrap()
    else {
        panic!()
    };
    let open = |subject: &str| {
        let id = rows.iter().find(|r| r.subject == subject).unwrap_or_else(|| panic!("no {subject}")).id;
        match core.handle(Command::OpenMessage { folder: inbox.clone(), id }).unwrap() {
            Response::Message { message } => message.sender_check,
            other => panic!("{other:?}"),
        }
    };
    let seen = |subject: &str| {
        let c = open(subject);
        (c.first_time, c.name_seen_from, c.lookalike_of)
    };
    assert_eq!(seen("statement 0"), (true, String::new(), String::new()), "the very first message from the bank");
    assert_eq!(seen("the bank again"), (false, String::new(), String::new()));
    assert_eq!(seen("the friend answers"), (false, String::new(), String::new()), "someone the user wrote to is no stranger");
    assert_eq!(seen("a stranger"), (true, String::new(), String::new()));
    assert_eq!(
        seen("an impostor"),
        (true, "service@firstnational.example".to_string(), "firstnational.example".to_string())
    );
    assert_eq!(seen("a note to self"), (false, String::new(), String::new()), "the user's own mail is never warned about");
}

#[test]
fn a_failed_message_is_held_at_arms_length_until_trusted() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("ann");
    other.cmd("CREATE Junk");
    let html_mail = |minute: u32, from: &str, results: &str, subject: &str| {
        format!(
            "Authentication-Results: mx.home.example; {results}\r\nFrom: First National <{from}>\r\nTo: ann@example.com\r\nSubject: {subject}\r\nDate: Mon, 16 Nov 2026 11:{minute:02}:00 +0000\r\nMessage-ID: <{minute}@protect>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>Please <a href=\"https://evil.example/login\">sign in</a>.</p><img src=\"https://track.example/p.png\">\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"invoice.pdf\"\r\n\r\nPDFDATA\r\n--b--\r\n"
        )
    };
    let pass = "dmarc=pass header.from=firstnational.example";
    let fail = "dmarc=fail header.from=firstnational.example";
    for i in 0..3 {
        other.append("INBOX", "", html_mail(i, "service@firstnational.example", pass, &format!("genuine {i}")).as_bytes());
    }
    other.append("INBOX", "", html_mail(10, "service@firstnational.example", fail, "forged").as_bytes());
    other.append("INBOX", "", html_mail(11, "pest@spam.example", "dmarc=pass header.from=spam.example", "pest one").as_bytes());

    let data = TempDir::new();
    let downloads = TempDir::new();
    let (tx, events) = mpsc::channel();
    let config = Config { data_dir: Some(data.path().to_path_buf()), downloads_dir: Some(downloads.path().to_path_buf()) };
    let core = Core::start_with_secrets(config, Arc::new(MemorySecrets::default()), move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    add(&core, &server, "ann", PASSWORD, true);
    wait_for_state(&events, SyncState::UpToDate);
    let inbox = folders(&core).iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
    let id_of = |subject: &str| {
        let Response::Messages { rows, .. } =
            core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 100, conversations: false }).unwrap()
        else {
            panic!()
        };
        rows.iter().find(|r| r.subject == subject).unwrap_or_else(|| panic!("no {subject}")).id
    };
    let open = |subject: &str| match core.handle(Command::OpenMessage { folder: inbox.clone(), id: id_of(subject) }).unwrap() {
        Response::Message { message } => message,
        other => panic!("{other:?}"),
    };
    let save = |subject: &str| core.handle(Command::SaveAttachment { folder: inbox.clone(), id: id_of(subject), index: 0 });
    let protection = || match core.handle(Command::GetProtection).unwrap() {
        Response::Protection { protection, password_set } => (protection, password_set),
        other => panic!("{other:?}"),
    };

    // A genuine message: links work, pictures are not fetched, the
    // attachment can be saved.
    let genuine = open("genuine 0");
    assert_eq!((genuine.sender_check.state.as_str(), genuine.restricted), ("verified", false));
    let html = genuine.html.unwrap();
    assert!(html.contains("href=\"https://evil.example/login\"") && !html.contains("track.example"), "{html}");
    assert!(genuine.remote_content_blocked && !genuine.remote_images_shown);
    assert!(save("genuine 0").is_ok());

    // The forged one: no links (the destination is written out), no
    // pictures, no saving.
    let forged = open("forged");
    assert_eq!((forged.sender_check.state.as_str(), forged.restricted, forged.trusted), ("failed", true, false));
    let html = forged.html.unwrap();
    assert!(!html.contains("href") && html.contains("[https://evil.example/login]") && !html.contains("track.example"), "{html}");
    let refused = save("forged").unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotPossible);
    assert!(refused.message.contains("failed its sender checks"), "{}", refused.message);

    // With a password set, neither the settings nor the warning give way
    // without it.
    core.handle(Command::SetProtectionPassword { current: None, new: Some(Secret("sesame".into())) }).unwrap();
    assert!(protection().1);
    let mut loose = protection().0;
    loose.disable_failed = false;
    let no = core.handle(Command::SetProtection { protection: loose.clone(), password: None }).unwrap_err();
    assert_eq!(no.code, ErrorCode::WrongPassword);
    let trust = |password: Option<&str>| {
        core.handle(Command::TrustMessage {
            folder: inbox.clone(),
            id: id_of("forged"),
            trusted: true,
            password: password.map(|p| Secret(p.into())),
        })
    };
    assert_eq!(trust(None).unwrap_err().code, ErrorCode::WrongPassword);
    assert_eq!(trust(Some("guess")).unwrap_err().code, ErrorCode::WrongPassword);
    assert!(open("forged").restricted);

    // With it, the one message is trusted: still shown as failed, but its
    // links and attachment work. Other messages are unaffected.
    trust(Some("sesame")).unwrap();
    let trusted = open("forged");
    assert_eq!((trusted.sender_check.state.as_str(), trusted.restricted, trusted.trusted), ("failed", false, true));
    assert!(trusted.html.unwrap().contains("href=\"https://evil.example/login\""));
    assert!(save("forged").is_ok());
    // Trust can be withdrawn without the password.
    core.handle(Command::TrustMessage { folder: inbox.clone(), id: id_of("forged"), trusted: false, password: None }).unwrap();
    assert!(open("forged").restricted);

    // Pictures for one sender; and a sender blocked: their mail goes to
    // Junk, now and when more arrives.
    let mut changed = protection().0;
    changed.allow_images_from.push("Service@FirstNational.example".into());
    changed.blocked_senders.push("pest@spam.example".into());
    core.handle(Command::SetProtection { protection: changed, password: Some(Secret("sesame".into())) }).unwrap();
    let genuine = open("genuine 1");
    assert!(genuine.remote_images_shown && genuine.html.unwrap().contains("https://track.example/p.png"));
    let forged = open("forged");
    assert!(!forged.remote_images_shown && !forged.html.unwrap().contains("track.example"), "never for a failed message");

    let junk = folders(&core).iter().find(|f| f.role.as_deref() == Some("junk")).unwrap().id.clone();
    assert_eq!(subjects(&core, &junk), ["pest one"]);
    assert!(!subjects(&core, &inbox).contains(&"pest one".to_string()));
    other.append("INBOX", "", html_mail(20, "pest@spam.example", "dmarc=pass header.from=spam.example", "pest two").as_bytes());
    wait_for(&events, "the second message from the pest to reach Junk", |e| match e {
        Event::FolderChanged { .. } if subjects(&core, &junk).contains(&"pest two".to_string()) => Some(()),
        _ => None,
    });
    assert!(!subjects(&core, &inbox).contains(&"pest two".to_string()));
    // ...and on the server too, once the queue has been replayed.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let on_server: Vec<String> = other.subjects_in("Junk").into_iter().map(|(subject, _)| subject).collect();
        if on_server.contains(&"pest one".to_string()) && on_server.contains(&"pest two".to_string()) {
            break;
        }
        assert!(Instant::now() < deadline, "Junk on the server: {on_server:?}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Mail that arrives while the connection is busy carrying out what the
/// user did must not wait for the next poll (ten minutes) to be seen.
#[test]
fn mail_arriving_while_the_connection_is_busy_is_not_missed() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("ann");
    for i in 0..12 {
        other.append("INBOX", "", &message(i, &format!("old {i}"), "hello"));
    }
    let data = TempDir::new();
    let (core, events) = start(&data, Arc::new(MemorySecrets::default()));
    add(&core, &server, "ann", PASSWORD, true);
    wait_for_state(&events, SyncState::UpToDate);
    let inbox = folders(&core).iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
    let Response::Messages { rows, .. } =
        core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 100, conversations: false }).unwrap()
    else {
        panic!()
    };
    for round in 0..5 {
        // A burst of separate changes, and a delivery in the middle of it.
        for (n, row) in rows.iter().enumerate() {
            core.handle(Command::ChangeFlags { folder: inbox.clone(), ids: vec![row.id], seen: None, flagged: Some(round % 2 == 0) })
                .unwrap();
            if n == 3 {
                other.append("INBOX", "", &message(100 + round, &format!("raced {round}"), "hello"));
            }
        }
        let wanted = format!("raced {round}");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !subjects(&core, &inbox).contains(&wanted) {
            assert!(Instant::now() < deadline, "{wanted} was not noticed");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
