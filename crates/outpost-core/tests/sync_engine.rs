//! The sync engine against a real Dovecot, with a second client making
//! changes "from elsewhere" and a proxy that can cut the connection.

mod common;

use common::{message, FlakyProxy, OtherClient, TempDir, TestServer, PASSWORD};
use outpost_core::imap::Server;
use outpost_core::store::maildir::Flags;
use outpost_core::store::Store;
use outpost_core::sync::{self, Account, Options, Progress, Summary, SyncError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn account(server: Server, user: &str) -> Account {
    Account {
        server,
        username: user.into(),
    }
}

async fn sync(store: &Store, server: Server, user: &str, options: &Options) -> Result<Summary, SyncError> {
    let (_handle, stop) = sync::stop_signal();
    sync::sync_once(store, &account(server, user), PASSWORD, options, &stop, |_| {}).await
}

/// Everything the local store holds, in a form that can be compared:
/// (folder path, subject, flags, has a body).
fn local(store: &Store) -> Vec<(String, String, Flags, bool)> {
    let mut rows = Vec::new();
    for folder in store.index().folders().unwrap() {
        for row in store.index().page(folder.id, 0, 10_000).unwrap() {
            let has_body = store.read_message(row.id).is_ok();
            rows.push((folder.path.clone(), row.subject, row.flags, has_body));
        }
    }
    rows.sort();
    rows
}

fn files_in(store: &Store, folder: &str) -> usize {
    std::fs::read_dir(store.mail_root().join(folder).join("cur")).map(|d| d.count()).unwrap_or(0)
}

fn fill(other: &mut OtherClient) -> Vec<u8> {
    other.cmd("CREATE \"Projects/Alpha\"");
    other.cmd("CREATE \"Entw&APw-rfe alt\"");
    for n in 1..=10 {
        let flags = match n % 4 {
            0 => "\\Seen",
            1 => "\\Seen \\Flagged",
            2 => "\\Answered \\Seen",
            _ => "",
        };
        other.append("INBOX", flags, &message(n, &format!("inbox {n}"), &format!("inbox body {n} zebra")));
    }
    for n in 1..=3 {
        other.append("Projects/Alpha", "\\Seen", &message(n, &format!("alpha {n}"), "alpha body"));
    }
    other.append("Sent", "\\Seen", &message(1, "sent 1", "sent body"));
    other.append("Entw&APw-rfe alt", "", &message(1, "draft 1", "draft body"));
    let mut large = message(99, "large", "start of large").to_vec();
    while large.len() < 1_300_000 {
        large.extend_from_slice(format!("filler line {}\r\n", large.len()).as_bytes());
    }
    other.append("INBOX", "", &large);
    large
}

#[tokio::test(flavor = "multi_thread")]
async fn first_sync_mirrors_the_account_and_changes_nothing() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("alice");
    let large = fill(&mut other);
    let before: Vec<Vec<String>> =
        ["INBOX", "Projects/Alpha", "Sent"].iter().map(|f| other.flags_in(f)).collect();

    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    let summary = sync(&store, server.server(), "alice", &Options::default()).await.unwrap();
    assert_eq!((summary.headers_fetched, summary.bodies_fetched), (16, 16));
    assert_eq!((summary.removed, summary.flags_changed, summary.folders_reset), (0, 0, 0));

    // Folders, including nesting, special uses and a non-ASCII name.
    let folders = store.index().remote_folders().unwrap();
    let find = |path: &str| folders.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("no {path} in {folders:?}"));
    assert_eq!(find("INBOX").total, 11);
    assert_eq!(find("Projects/Alpha").total, 3);
    assert_eq!(find("Sent").special_use.as_deref(), Some("sent"));
    assert_eq!(find("Trash").special_use.as_deref(), Some("trash"));
    assert_eq!(find("Entwürfe alt").name, "Entwürfe alt");
    assert!(tmp.path().join("mail/Projects/Alpha/cur").is_dir());

    // Messages: flags as on the server, bodies byte for byte, searchable.
    let rows = local(&store);
    assert_eq!(rows.len(), 16);
    assert!(rows.iter().all(|r| r.3), "every body was downloaded");
    let flags_of = |subject: &str| rows.iter().find(|r| r.1 == subject).unwrap().2;
    assert_eq!(flags_of("inbox 1"), Flags::SEEN.with(Flags::FLAGGED));
    assert_eq!(flags_of("inbox 2"), Flags::SEEN.with(Flags::ANSWERED));
    assert_eq!(flags_of("inbox 3"), Flags::default());
    assert_eq!(flags_of("inbox 4"), Flags::SEEN);
    let inbox = find("INBOX").id;
    let page = store.index().page(inbox, 0, 100).unwrap();
    let three = page.iter().find(|r| r.subject == "inbox 3").unwrap();
    assert_eq!(store.read_message(three.id).unwrap(), message(3, "inbox 3", "inbox body 3 zebra"));
    assert_eq!(three.preview, "inbox body 3 zebra");
    let big = page.iter().find(|r| r.subject == "large").unwrap();
    assert_eq!(store.read_message(big.id).unwrap(), large);
    assert_eq!(store.index().search("zebra ", None, 100).unwrap().len(), 10);
    assert_eq!(store.index().missing_body_count().unwrap(), 0);

    // Nothing on the server was touched.
    let after: Vec<Vec<String>> =
        ["INBOX", "Projects/Alpha", "Sent"].iter().map(|f| other.flags_in(f)).collect();
    assert_eq!(after, before);

    // A second pass finds nothing to do.
    let again = sync(&store, server.server(), "alice", &Options::default()).await.unwrap();
    assert_eq!(again, Summary { folders: summary.folders, ..Summary::default() });
    assert_eq!(local(&store), rows);
}

async fn changes_made_elsewhere(fast_resync: bool) {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("bob");
    fill(&mut other);
    let options = Options { fast_resync, ..Options::default() };
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "bob", &options).await.unwrap();
    assert_eq!(files_in(&store, "INBOX"), 11);

    // Meanwhile, in another mail client:
    other.cmd("SELECT INBOX");
    other.cmd("UID STORE 2,5 +FLAGS (\\Deleted)");
    other.cmd("EXPUNGE"); // two messages deleted
    other.cmd("UID STORE 3 +FLAGS (\\Seen)"); // one read
    other.cmd("UID STORE 1 -FLAGS (\\Flagged)"); // one unflagged
    other.append("INBOX", "", &message(20, "arrived later", "new body"));
    other.append("Projects/Alpha", "", &message(21, "alpha later", "new body"));

    let summary = sync(&store, server.server(), "bob", &options).await.unwrap();
    assert_eq!(summary.removed, 2, "{summary:?}");
    assert_eq!(summary.flags_changed, 2, "{summary:?}");
    assert_eq!((summary.headers_fetched, summary.bodies_fetched), (2, 2), "{summary:?}");

    let rows = local(&store);
    let subjects: Vec<&str> = rows.iter().filter(|r| r.0 == "INBOX").map(|r| r.1.as_str()).collect();
    assert!(!subjects.contains(&"inbox 2") && !subjects.contains(&"inbox 5"));
    assert!(subjects.contains(&"arrived later"));
    let flags_of = |subject: &str| rows.iter().find(|r| r.1 == subject).unwrap().2;
    assert_eq!(flags_of("inbox 3"), Flags::SEEN);
    assert_eq!(flags_of("inbox 1"), Flags::SEEN);
    assert_eq!(files_in(&store, "INBOX"), 10, "files of deleted messages are removed");
    assert_eq!(store.index().search("zebra ", None, 100).unwrap().len(), 8, "and they leave the search index");

    // File names catch up with the new flags, and the mail is still readable.
    store.reconcile_flags(100).unwrap();
    assert!(local(&store).iter().all(|r| r.3));

    let again = sync(&store, server.server(), "bob", &options).await.unwrap();
    assert_eq!((again.removed, again.flags_changed, again.headers_fetched, again.bodies_fetched), (0, 0, 0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_made_elsewhere_arrive_with_fast_resync() {
    changes_made_elsewhere(true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_made_elsewhere_arrive_without_fast_resync() {
    changes_made_elsewhere(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sync_cut_off_anywhere_resumes_without_duplicates() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("carol");
    let body = "x".repeat(20_000);
    for n in 1..=40 {
        other.append("INBOX", "", &message(n, &format!("message {n}"), &body));
    }
    let proxy = FlakyProxy::start(server.port);
    let options = Options { header_batch: 5, ..Options::default() };
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();

    // Cut repeatedly, at points that fall in sign-in, the folder list, the
    // headers and the bodies. Every attempt must fail cleanly or finish.
    let mut attempts = 0;
    let mut bodies = 0;
    let mut headers = 0;
    for budget in [200, 1_500, 4_000, 9_000, 60_000, 250_000, 400_000, -1] {
        proxy.cut_after(budget);
        attempts += 1;
        match sync(&store, proxy.server(), "carol", &options).await {
            Ok(summary) => {
                headers += summary.headers_fetched;
                bodies += summary.bodies_fetched;
                break;
            }
            Err(SyncError::Imap(e)) => assert!(e.is_transient(), "attempt {attempts}: {e}"),
            Err(e) => panic!("attempt {attempts}: {e}"),
        }
        // What is there so far is consistent: no row twice, no stray files.
        let rows = local(&store);
        let mut subjects: Vec<&String> = rows.iter().map(|r| &r.1).collect();
        subjects.dedup();
        assert_eq!(subjects.len(), rows.len());
        assert_eq!(files_in(&store, "INBOX"), rows.iter().filter(|r| r.3).count());
        // Count what each failed attempt managed.
        headers = rows.len();
        bodies = rows.iter().filter(|r| r.3).count();
    }
    assert!(attempts >= 4, "the cuts were meant to interrupt several stages, got {attempts} attempts");
    let _ = (headers, bodies);

    let rows = local(&store);
    assert_eq!(rows.len(), 40);
    assert!(rows.iter().all(|r| r.3));
    assert_eq!(files_in(&store, "INBOX"), 40, "no message was stored twice");
    assert_eq!(store.index().search("message ", None, 100).unwrap().len(), 40);

    // And the final state needs nothing more.
    proxy.heal();
    let again = sync(&store, proxy.server(), "carol", &options).await.unwrap();
    assert_eq!((again.headers_fetched, again.bodies_fetched), (0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_renumbered_folder_is_synced_from_scratch() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("dave");
    other.cmd("CREATE Work");
    for n in 1..=4 {
        other.append("Work", "\\Seen", &message(n, &format!("old {n}"), "old body"));
    }
    other.append("INBOX", "", &message(1, "inbox stays", "b"));
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "dave", &Options::default()).await.unwrap();
    assert_eq!(files_in(&store, "Work"), 4);

    // The folder is deleted and a new one made with the same name: same
    // UIDs, different messages. Only UIDVALIDITY tells them apart.
    other.cmd("DELETE Work");
    std::thread::sleep(Duration::from_millis(1100)); // UIDVALIDITY is a timestamp
    other.cmd("CREATE Work");
    for n in 1..=3 {
        other.append("Work", "", &message(n, &format!("new {n}"), "new body"));
    }

    let summary = sync(&store, server.server(), "dave", &Options::default()).await.unwrap();
    assert_eq!(summary.folders_reset, 1);
    let rows = local(&store);
    let work: Vec<&str> = rows.iter().filter(|r| r.0 == "Work").map(|r| r.1.as_str()).collect();
    assert_eq!(work, ["new 1", "new 2", "new 3"]);
    assert!(rows.iter().all(|r| r.3));
    assert_eq!(files_in(&store, "Work"), 3, "the old messages' files are gone");
    assert!(rows.iter().any(|r| r.1 == "inbox stays"));
    assert!(store.index().search("old ", None, 10).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_created_and_deleted_elsewhere_are_noticed() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("erin");
    other.cmd("CREATE Temporary");
    other.append("Temporary", "", &message(1, "doomed", "b"));
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "erin", &Options::default()).await.unwrap();
    assert!(tmp.path().join("mail/Temporary/cur").is_dir());

    other.cmd("DELETE Temporary");
    other.cmd("CREATE \"Brand New/Child\"");
    other.append("Brand New/Child", "", &message(2, "fresh", "b"));
    sync(&store, server.server(), "erin", &Options::default()).await.unwrap();

    let paths: Vec<String> = store.index().folders().unwrap().into_iter().map(|f| f.path).collect();
    assert!(!paths.contains(&"Temporary".to_string()), "{paths:?}");
    assert!(paths.contains(&"Brand New/Child".to_string()), "{paths:?}");
    assert!(!tmp.path().join("mail/Temporary").exists());
    let rows = local(&store);
    assert!(rows.iter().any(|r| r.1 == "fresh" && r.3));
    assert!(!rows.iter().any(|r| r.1 == "doomed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rebuilt_index_does_not_download_anything_again() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("frank");
    fill(&mut other);
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "frank", &Options::default()).await.unwrap();
    let before = local(&store);

    // The index is lost; the mail files remain.
    drop(store);
    for file in ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"] {
        let _ = std::fs::remove_file(tmp.path().join(file));
    }
    let store = Store::open(tmp.path()).unwrap();
    store.rebuild(false, |_| {}).unwrap();
    assert_eq!(local(&store), before, "the files alone restore the mail");

    let summary = sync(&store, server.server(), "frank", &Options::default()).await.unwrap();
    assert_eq!((summary.headers_fetched, summary.bodies_fetched, summary.removed), (0, 0, 0), "{summary:?}");
    assert_eq!(summary.folders_reset, 0);
    assert_eq!(local(&store), before);

    // And it still notices changes afterwards.
    other.append("INBOX", "", &message(50, "after rebuild", "b"));
    let summary = sync(&store, server.server(), "frank", &Options::default()).await.unwrap();
    assert_eq!((summary.headers_fetched, summary.bodies_fetched), (1, 1));
}

/// Waits until `check` passes, failing with `what` after ten seconds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn running_sync_gets_push_polls_and_survives_a_dropped_connection() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("gina");
    other.cmd("CREATE Lists");
    other.append("INBOX", "", &message(1, "first", "b"));
    let proxy = FlakyProxy::start(server.port);
    let tmp = TempDir::new();
    let store = Arc::new(Store::open(tmp.path()).unwrap());
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();

    let (handle, stop) = sync::stop_signal();
    let task = {
        let (store, seen, server) = (store.clone(), seen.clone(), proxy.server());
        tokio::spawn(async move {
            let options = Options { poll_interval: Duration::from_secs(2), ..Options::default() };
            let account = account(server, "gina");
            let now = tokio::sync::Notify::new();
            sync::run(&store, &account, PASSWORD, &options, stop, &now, move |p| seen.lock().unwrap().push(p)).await;
        })
    };
    let subjects = |store: &Store| -> Vec<String> { local(store).into_iter().filter(|r| r.3).map(|r| r.1).collect() };
    let has = |subject: &str| subjects(&store).iter().any(|s| s == subject);

    eventually("the first pass", || has("first")).await;
    eventually("idle", || seen.lock().unwrap().contains(&Progress::Idle)).await;

    // Push: mail to the inbox arrives without waiting for a poll.
    let started = Instant::now();
    other.append("INBOX", "", &message(2, "pushed", "b"));
    eventually("pushed mail", || has("pushed")).await;
    assert!(started.elapsed() < Duration::from_millis(1900), "arrived by push, not by the 2 s poll ({:?})", started.elapsed());

    // Poll: other folders are checked on the timer.
    other.append("Lists", "", &message(3, "polled", "b"));
    eventually("polled mail", || has("polled")).await;

    // The connection drops. Sync says so, reconnects, and carries on.
    proxy.cut_after(0);
    other.append("INBOX", "", &message(4, "while offline", "b"));
    eventually("offline report", || seen.lock().unwrap().iter().any(|p| matches!(p, Progress::Offline { .. }))).await;
    proxy.heal();
    eventually("mail after reconnecting", || has("while offline")).await;

    handle.stop();
    tokio::time::timeout(Duration::from_secs(10), task).await.expect("run stops when asked").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_sign_in_is_reported_and_not_retried() {
    let Some(server) = TestServer::start() else { return };
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let (_handle, stop) = sync::stop_signal();
    let log = seen.clone();
    let (account, options) = (account(server.server(), "harry"), Options::default());
    let now = tokio::sync::Notify::new();
    let run = sync::run(&store, &account, "wrong password", &options, stop, &now, move |p| {
        log.lock().unwrap().push(p)
    });
    tokio::time::timeout(Duration::from_secs(10), run).await.expect("returns instead of retrying");
    let seen = seen.lock().unwrap();
    assert!(matches!(seen.last(), Some(Progress::SignInFailed { .. })), "{seen:?}");
    assert_eq!(seen.iter().filter(|p| **p == Progress::Connecting).count(), 1);
}
