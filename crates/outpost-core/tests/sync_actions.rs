//! What the user does locally reaches the server: once, in order, and
//! without sync undoing it on the way. Against a real Dovecot.

mod common;

use common::{message, TempDir, TestServer, PASSWORD};
use outpost_core::imap::Server;
use outpost_core::store::maildir::Flags;
use outpost_core::store::Store;
use outpost_core::sync::{self, Account, Options, Progress, Summary};
use std::sync::{Arc, Mutex};

async fn sync_with(store: &Store, server: Server, user: &str, seen: Arc<Mutex<Vec<Progress>>>) -> Summary {
    let (_handle, stop) = sync::stop_signal();
    let account = Account { server, username: user.into() };
    sync::sync_once(store, &account, PASSWORD, &Options::default(), &stop, move |p| seen.lock().unwrap().push(p))
        .await
        .unwrap()
}

async fn sync(store: &Store, server: Server, user: &str) -> Summary {
    sync_with(store, server, user, Arc::default()).await
}

fn folder_id(store: &Store, path: &str) -> i64 {
    store.index().folders().unwrap().into_iter().find(|f| f.path == path).unwrap_or_else(|| panic!("no folder {path}")).id
}

/// (id, subject, flags, has a body) of the messages in a folder.
fn rows(store: &Store, path: &str) -> Vec<(i64, String, Flags, bool)> {
    let mut out: Vec<_> = store
        .index()
        .page(folder_id(store, path), 0, 1000)
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.subject.clone(), r.flags, store.read_message(r.id).is_ok()))
        .collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn id_of(store: &Store, path: &str, subject: &str) -> i64 {
    rows(store, path).into_iter().find(|r| r.1 == subject).unwrap_or_else(|| panic!("no {subject} in {path}")).0
}

fn subjects(store: &Store, path: &str) -> Vec<String> {
    rows(store, path).into_iter().map(|r| r.1).collect()
}

fn pairs(found: Vec<(String, String)>) -> Vec<(String, String)> {
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn flags_moves_and_deletes_reach_the_server() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("amy");
    other.cmd("CREATE Filed");
    for n in 1..=5 {
        other.append("INBOX", "", &message(n, &format!("m{n}"), &format!("body {n}")));
    }
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "amy").await;
    let (inbox, filed) = (folder_id(&store, "INBOX"), folder_id(&store, "Filed"));

    // The user reads one, flags one, files one and deletes one.
    store.change_flags(&[id_of(&store, "INBOX", "m1")], Flags::SEEN, Flags::default()).unwrap();
    store.change_flags(&[id_of(&store, "INBOX", "m2")], Flags::FLAGGED.with(Flags::SEEN), Flags::default()).unwrap();
    let moved = id_of(&store, "INBOX", "m3");
    assert_eq!(store.move_messages(&[moved], filed).unwrap().into_iter().collect::<Vec<_>>(), [inbox.min(filed), inbox.max(filed)]);
    store.expunge_messages(&[id_of(&store, "INBOX", "m4")]).unwrap();

    // All of it shows locally at once, before the server knows anything.
    assert_eq!(subjects(&store, "INBOX"), ["m1", "m2", "m5"]);
    assert_eq!(subjects(&store, "Filed"), ["m3"]);
    assert!(rows(&store, "Filed")[0].3, "the moved message is still readable");
    assert_eq!(store.read_message(moved).unwrap(), message(3, "m3", "body 3"));
    assert_eq!(store.index().queued_action_count().unwrap(), 4);
    assert_eq!(other.subjects_in("INBOX").len(), 5, "nothing has been sent yet");

    let summary = sync(&store, server.server(), "amy").await;
    assert_eq!(summary.actions_replayed, 4);
    assert_eq!(summary.bodies_fetched, 0, "the moved message was not downloaded again");
    assert_eq!(summary.headers_fetched, 0, "nor listed as new: it was recognised");
    assert_eq!(store.index().queued_action_count().unwrap(), 0);

    assert_eq!(
        pairs(other.subjects_in("INBOX")),
        [("m1".into(), "\\Seen".into()), ("m2".into(), "\\Flagged \\Seen".into()), ("m5".into(), "".into())]
    );
    assert_eq!(pairs(other.subjects_in("Filed")), [("m3".into(), "".into())]);

    // Local and server agree, and stay that way.
    let again = sync(&store, server.server(), "amy").await;
    assert_eq!((again.actions_replayed, again.headers_fetched, again.bodies_fetched, again.removed, again.flags_changed), (0, 0, 0, 0, 0));
    assert_eq!(subjects(&store, "INBOX"), ["m1", "m2", "m5"]);
    assert_eq!(subjects(&store, "Filed"), ["m3"]);
    assert_eq!(rows(&store, "INBOX")[1].2, Flags::SEEN.with(Flags::FLAGGED));
    assert_eq!(store.read_message(moved).unwrap(), message(3, "m3", "body 3"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chain_of_changes_made_offline_is_replayed_in_order() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("bea");
    other.cmd("CREATE First");
    other.cmd("CREATE Second");
    other.append("INBOX", "", &message(1, "wanderer", "body"));
    other.append("INBOX", "", &message(2, "stays", "body"));
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "bea").await;
    let id = id_of(&store, "INBOX", "wanderer");

    // Offline: moved, flagged, moved again, marked read, un-flagged, and
    // finally moved back to where it started and flagged once more.
    store.move_messages(&[id], folder_id(&store, "First")).unwrap();
    store.change_flags(&[id], Flags::FLAGGED, Flags::default()).unwrap();
    store.move_messages(&[id], folder_id(&store, "Second")).unwrap();
    store.change_flags(&[id], Flags::SEEN, Flags::FLAGGED).unwrap();
    store.move_messages(&[id], folder_id(&store, "INBOX")).unwrap();
    store.change_flags(&[id], Flags::ANSWERED, Flags::default()).unwrap();
    assert_eq!(subjects(&store, "INBOX"), ["stays", "wanderer"]);

    let summary = sync(&store, server.server(), "bea").await;
    assert_eq!(summary.actions_replayed, 6);
    assert_eq!(
        pairs(other.subjects_in("INBOX")),
        [("stays".into(), "".into()), ("wanderer".into(), "\\Answered \\Seen".into())]
    );
    assert!(other.subjects_in("First").is_empty() && other.subjects_in("Second").is_empty());
    assert_eq!(summary.bodies_fetched, 0);

    let again = sync(&store, server.server(), "bea").await;
    assert_eq!((again.headers_fetched, again.removed, again.flags_changed), (0, 0, 0), "{again:?}");
    assert_eq!(subjects(&store, "INBOX"), ["stays", "wanderer"], "one copy, not two");
    assert_eq!(rows(&store, "INBOX")[1].2, Flags::SEEN.with(Flags::ANSWERED));
    assert_eq!(store.read_message(id).unwrap(), message(1, "wanderer", "body"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_the_server_refuses_is_undone_and_reported() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("cal");
    other.cmd("CREATE Doomed");
    other.append("INBOX", "\\Seen", &message(1, "stranded", "body"));
    other.append("INBOX", "", &message(2, "fine", "body"));
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "cal").await;
    let id = id_of(&store, "INBOX", "stranded");

    // The user files a message into a folder that, unknown to this
    // computer, has just been deleted elsewhere. A later action is queued
    // behind it.
    store.move_messages(&[id], folder_id(&store, "Doomed")).unwrap();
    store.change_flags(&[id_of(&store, "INBOX", "fine")], Flags::FLAGGED, Flags::default()).unwrap();
    other.cmd("DELETE Doomed");

    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    sync_with(&store, server.server(), "cal", seen.clone()).await;
    let failures: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .filter_map(|p| match p {
            Progress::ActionFailed { what, .. } => Some(what.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(failures, ["Moving a message"]);

    // The message is back where it was, intact, here and there; and the
    // action queued behind the failed one was still carried out.
    assert_eq!(subjects(&store, "INBOX"), ["fine", "stranded"]);
    assert_eq!(store.read_message(id).unwrap(), message(1, "stranded", "body"));
    assert_eq!(
        pairs(other.subjects_in("INBOX")),
        [("fine".into(), "\\Flagged".into()), ("stranded".into(), "\\Seen".into())]
    );
    assert_eq!(store.index().queued_action_count().unwrap(), 0);
    let again = sync(&store, server.server(), "cal").await;
    assert_eq!((again.headers_fetched, again.bodies_fetched, again.removed), (0, 0, 0), "{again:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_saved_here_is_uploaded_exactly_once() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("dee");
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "dee").await;
    let drafts = folder_id(&store, "Drafts");

    let draft = message(1, "a draft", "unfinished thought");
    let id = store.add_local_message(drafts, &draft, Flags::DRAFT.with(Flags::SEEN)).unwrap();
    assert_eq!(subjects(&store, "Drafts"), ["a draft"], "visible at once");
    sync(&store, server.server(), "dee").await;
    assert_eq!(pairs(other.subjects_in("Drafts")), [("a draft".into(), "\\Draft \\Seen".into())]);
    let again = sync(&store, server.server(), "dee").await;
    assert_eq!((again.headers_fetched, again.bodies_fetched), (0, 0), "not downloaded back as a new message");
    assert_eq!(subjects(&store, "Drafts"), ["a draft"]);
    assert_eq!(store.read_message(id).unwrap(), draft);

    // The upload got through last time but we never heard (simulated by
    // putting the message on the server ourselves). The retry must notice
    // and not upload a second copy.
    let second = message(2, "sent twice?", "no");
    store.add_local_message(drafts, &second, Flags::SEEN).unwrap();
    other.append("Drafts", "\\Seen", &second);
    sync(&store, server.server(), "dee").await;
    assert_eq!(other.subjects_in("Drafts").len(), 2, "one of each on the server");
    assert_eq!(subjects(&store, "Drafts"), ["a draft", "sent twice?"]);
    let again = sync(&store, server.server(), "dee").await;
    assert_eq!((again.headers_fetched, again.bodies_fetched), (0, 0));
    assert_eq!(subjects(&store, "Drafts"), ["a draft", "sent twice?"], "and one of each here");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_marked_for_deletion_elsewhere_disappears_here() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("eli");
    for n in 1..=3 {
        other.append("INBOX", "", &message(n, &format!("m{n}"), "body"));
    }
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "eli").await;

    other.cmd("SELECT INBOX");
    other.cmd("UID STORE 2 +FLAGS (\\Deleted)");
    let summary = sync(&store, server.server(), "eli").await;
    assert_eq!(summary.removed, 1);
    assert_eq!(subjects(&store, "INBOX"), ["m1", "m3"]);
    // It is still on the server: we did not expunge someone else's mark.
    assert_eq!(other.subjects_in("INBOX").len(), 3);
    // And it comes back if they change their mind.
    other.cmd("SELECT INBOX");
    other.cmd("UID STORE 2 -FLAGS (\\Deleted)");
    sync(&store, server.server(), "eli").await;
    assert_eq!(subjects(&store, "INBOX"), ["m1", "m2", "m3"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_does_not_undo_what_the_user_just_did() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("fay");
    other.cmd("CREATE Filed");
    for n in 1..=3 {
        other.append("INBOX", "", &message(n, &format!("m{n}"), "body"));
    }
    let tmp = TempDir::new();
    let store = Store::open(tmp.path()).unwrap();
    sync(&store, server.server(), "fay").await;
    let inbox = folder_id(&store, "INBOX");

    store.change_flags(&[id_of(&store, "INBOX", "m1")], Flags::SEEN, Flags::default()).unwrap();
    store.move_messages(&[id_of(&store, "INBOX", "m2")], folder_id(&store, "Filed")).unwrap();
    store.expunge_messages(&[id_of(&store, "INBOX", "m3")]).unwrap();
    // The server still has all three, unread, in the inbox. Sync is told
    // which of them have changes on the way, so that it leaves them alone.
    let mut pending: Vec<u32> = store.index().pending_uids(inbox).unwrap().into_iter().collect();
    pending.sort();
    assert_eq!(pending, [1, 2, 3]);
    assert!(store.index().pending_uids(folder_id(&store, "Filed")).unwrap().is_empty());
}
