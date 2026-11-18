//! The session layer against a real Dovecot.

mod common;

use common::{message, TestServer, PASSWORD};
use outpost_core::imap::session::{SpecialUse, BODY_PIECE};
use outpost_core::imap::{ImapError, Session};
use outpost_core::store::maildir::Flags;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn sign_in_and_capabilities() {
    let Some(server) = TestServer::start() else { return };
    let session = Session::connect(&server.server(), "alice", PASSWORD).await.unwrap();
    for capability in ["IMAP4rev1", "IDLE", "CONDSTORE", "QRESYNC", "MOVE", "UIDPLUS"] {
        assert!(session.has(capability), "{capability} should be advertised after sign-in");
    }
    assert!(session.qresync());
    session.logout().await;

    let refused = Session::connect(&server.server(), "alice", "wrong").await;
    match refused {
        Err(e @ ImapError::Rejected { .. }) => assert!(!e.is_transient(), "retrying cannot fix a bad password"),
        other => panic!("expected a refusal, got {:?}", other.err()),
    }

    let mut closed = server.server();
    closed.port = 1; // nothing listens here
    let unreachable = Session::connect(&closed, "alice", PASSWORD).await;
    assert!(unreachable.err().is_some_and(|e| e.is_transient()));
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_are_listed_with_names_decoded() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("bob");
    other.cmd("CREATE \"Projects/Alpha\"");
    other.cmd("CREATE \"Entw&APw-rfe alt\"");
    other.cmd("CREATE \"Tom &- Jerry\"");

    let mut session = Session::connect(&server.server(), "bob", PASSWORD).await.unwrap();
    let folders = session.list().await.unwrap();
    let find = |name: &str| folders.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} not listed: {folders:?}"));

    assert_eq!(find("INBOX").special_use, None);
    assert_eq!(find("Sent").special_use, Some(SpecialUse::Sent));
    assert_eq!(find("Drafts").special_use, Some(SpecialUse::Drafts));
    assert_eq!(find("Trash").special_use, Some(SpecialUse::Trash));
    assert_eq!(find("Junk").special_use, Some(SpecialUse::Junk));
    assert_eq!(find("Archive").special_use, Some(SpecialUse::Archive));
    assert_eq!(find("Projects/Alpha").levels(), ["Projects", "Alpha"]);
    assert_eq!(find("Projects/Alpha").delimiter, Some('/'));
    assert_eq!(find("Entwürfe alt").raw_name, b"Entw&APw-rfe alt");
    assert_eq!(find("Tom & Jerry").raw_name, b"Tom &- Jerry");
    assert!(!find("Projects").selectable || find("Projects").selectable, "listed either way");

    // A folder can be opened by the raw name it was listed with.
    for name in ["Entwürfe alt", "Tom & Jerry", "Projects/Alpha", "INBOX"] {
        let opened = session.examine(&find(name).raw_name, None).await.unwrap();
        assert!(opened.uid_validity > 0, "{name}");
        assert_eq!(opened.exists, 0);
    }
    let status = session.status(b"INBOX").await.unwrap();
    assert_eq!((status.messages, status.uid_next), (0, 1));
    assert!(status.uid_validity > 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_are_fetched_without_changing_anything() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("carol");
    for n in 1..=5 {
        let flags = match n {
            1 => "\\Seen",
            2 => "\\Seen \\Answered",
            3 => "\\Flagged",
            4 => "$Forwarded \\Seen",
            _ => "",
        };
        other.append("INBOX", flags, &message(n, &format!("subject {n}"), &format!("body of {n}")));
    }
    // One message larger than two body pieces, with content that shows any
    // piece put in the wrong place.
    let mut large = message(6, "large", "start").to_vec();
    let mut i = 0u32;
    while large.len() < (BODY_PIECE as usize) * 2 + 12_345 {
        large.extend_from_slice(format!("line {i:08} of the large message\r\n").as_bytes());
        i += 1;
    }
    other.append("INBOX", "", &large);
    let before = other.flags_in("INBOX");

    let mut session = Session::connect(&server.server(), "carol", PASSWORD).await.unwrap();
    let opened = session.examine(b"INBOX", None).await.unwrap();
    assert_eq!((opened.exists, opened.uid_next), (6, Some(7)));
    assert!(opened.highest_modseq.is_some());

    let flags = session.fetch_flags(1, None).await.unwrap();
    let by_uid = |uid: u32| flags.iter().find(|f| f.uid == uid).unwrap().flags;
    assert_eq!(flags.len(), 6);
    assert_eq!(by_uid(1), Flags::SEEN);
    assert_eq!(by_uid(2), Flags::SEEN.with(Flags::ANSWERED));
    assert_eq!(by_uid(3), Flags::FLAGGED);
    assert_eq!(by_uid(4), Flags::SEEN.with(Flags::FORWARDED));
    assert_eq!(by_uid(5), Flags::default());

    let headers = session.fetch_headers(&[5, 1, 2, 3, 99]).await.unwrap();
    assert_eq!(headers.len(), 4, "a UID that does not exist is simply absent");
    let two = headers.iter().find(|h| h.uid == 2).unwrap();
    let text = String::from_utf8_lossy(&two.header);
    assert!(text.contains("Subject: subject 2") && text.contains("Message-ID:"), "{text}");
    assert!(!text.contains("body of"), "headers only");
    assert_eq!(two.size as usize, message(2, "subject 2", "body of 2").len());
    assert_eq!(two.flags, Flags::SEEN.with(Flags::ANSWERED));
    assert!(two.internal_date > 1_700_000_000 && two.modseq.is_some());
    assert!(session.fetch_headers(&[]).await.unwrap().is_empty());

    assert_eq!(session.fetch_message(3).await.unwrap().unwrap(), message(3, "subject 3", "body of 3"));
    assert_eq!(session.fetch_message(6).await.unwrap().unwrap(), large, "pieces reassemble exactly");
    assert_eq!(session.fetch_message(99).await.unwrap(), None);
    let piece = session.fetch_piece(6, 10, 20).await.unwrap().unwrap();
    assert_eq!(piece, &large[10..30]);
    session.logout().await;

    // The point of this milestone: reading everything changed nothing.
    assert_eq!(other.flags_in("INBOX"), before);
    assert!(!before.iter().any(|l| l.contains("UID 5") && l.contains("\\Seen")));
}

#[tokio::test(flavor = "multi_thread")]
async fn fast_resync_reports_what_changed_while_away() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("dave");
    for n in 1..=6 {
        other.append("INBOX", "", &message(n, "m", "b"));
    }
    let mut session = Session::connect(&server.server(), "dave", PASSWORD).await.unwrap();
    let first = session.examine(b"INBOX", None).await.unwrap();
    let known = (first.uid_validity, first.highest_modseq.unwrap());
    session.logout().await;

    // While we are away: two messages are deleted, one is flagged, one arrives.
    other.cmd("SELECT INBOX");
    other.cmd("UID STORE 2,5 +FLAGS (\\Deleted)");
    other.cmd("EXPUNGE");
    other.cmd("UID STORE 3 +FLAGS (\\Flagged)");
    other.append("INBOX", "", &message(7, "new", "b"));

    let mut session = Session::connect(&server.server(), "dave", PASSWORD).await.unwrap();
    let again = session.examine(b"INBOX", Some(known)).await.unwrap();
    assert_eq!(again.uid_validity, first.uid_validity);
    assert_eq!(again.exists, 5);
    assert_eq!(again.uid_next, Some(8));
    let mut gone: Vec<u32> = again.vanished.iter().flat_map(|(a, b)| *a..=*b).collect();
    gone.sort();
    assert_eq!(gone, [2, 5]);
    assert!(again.changed.iter().any(|c| c.uid == 3 && c.flags == Flags::FLAGGED), "{:?}", again.changed);
    assert!(again.highest_modseq.unwrap() > known.1);

    // Without QRESYNC data the same facts are available by asking.
    let changed = session.fetch_flags(1, Some(known.1)).await.unwrap();
    assert!(changed.iter().any(|c| c.uid == 3 && c.flags == Flags::FLAGGED));
    assert!(!changed.iter().any(|c| c.uid == 1), "unchanged messages are not reported");
    let recent: Vec<u32> = session.fetch_flags(6, None).await.unwrap().iter().map(|f| f.uid).collect();
    assert_eq!(recent, [6, 7], "a starting UID narrows the list");
    assert!(session.fetch_flags(50, None).await.unwrap().is_empty(), "nothing at or above 50");
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_wakes_for_new_mail_for_a_signal_and_for_the_limit() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("erin");
    let mut session = Session::connect(&server.server(), "erin", PASSWORD).await.unwrap();
    session.examine(b"INBOX", None).await.unwrap();

    // New mail from elsewhere ends the wait promptly.
    let started = Instant::now();
    let deliver = tokio::task::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(300));
        other.append("INBOX", "", &message(1, "hello", "b"));
        other
    });
    let changed = session.idle(Duration::from_secs(20), std::future::pending()).await.unwrap();
    assert!(changed, "the server announced the new message");
    assert!(started.elapsed() < Duration::from_secs(10));
    let _other = deliver.await.unwrap();

    // A signal from our own side ends it too, reporting no change.
    let started = Instant::now();
    let changed = session.idle(Duration::from_secs(20), tokio::time::sleep(Duration::from_millis(200))).await.unwrap();
    assert!(!changed);
    assert!(started.elapsed() < Duration::from_secs(5));

    // So does the time limit. The session is still usable afterwards.
    let changed = session.idle(Duration::from_millis(200), std::future::pending()).await.unwrap();
    assert!(!changed);
    assert_eq!(session.fetch_flags(1, None).await.unwrap().len(), 1);
    session.noop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_connections_work_and_certificates_are_checked() {
    let Some(server) = TestServer::start() else { return };
    let Some(tls) = server.tls_server() else {
        eprintln!("SKIPPED: no openssl to make a test certificate");
        return;
    };
    let mut other = server.other_client("tina");
    other.append("INBOX", "", &message(1, "over tls", "secret body"));

    // With the server's certificate accepted, everything works as usual.
    let mut session = Session::connect(&tls, "tina", PASSWORD).await.unwrap();
    session.examine(b"INBOX", None).await.unwrap();
    assert_eq!(session.fetch_message(1).await.unwrap().unwrap(), message(1, "over tls", "secret body"));
    session.logout().await;

    // Without it, the self-signed certificate is refused: checked against
    // the system's trust store, where it is not.
    let untrusted = outpost_core::imap::Server { trusted_certificate: None, ..tls.clone() };
    match Session::connect(&untrusted, "tina", PASSWORD).await {
        Err(ImapError::Tls(_)) => {}
        other => panic!("expected the certificate to be refused, got {:?}", other.err()),
    }

    // The right certificate under the wrong name is refused too.
    let wrong_name = outpost_core::imap::Server { host: "127.0.0.1".into(), ..tls.clone() };
    match Session::connect(&wrong_name, "tina", PASSWORD).await {
        Err(ImapError::Tls(_)) => {}
        other => panic!("expected a name mismatch, got {:?}", other.err()),
    }

    // A different certificate does not vouch for this server.
    let Some(second) = TestServer::start() else { return };
    let someone_else = outpost_core::imap::Server { trusted_certificate: Some(second.certificate_der()), ..tls.clone() };
    assert!(matches!(Session::connect(&someone_else, "tina", PASSWORD).await, Err(ImapError::Tls(_))));

    // Speaking TLS to a port that does not is an error, not a hang.
    let not_tls = outpost_core::imap::Server { port: server.port, ..tls };
    assert!(Session::connect(&not_tls, "tina", PASSWORD).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_are_made_exactly_and_only_where_asked() {
    let Some(server) = TestServer::start() else { return };
    let mut other = server.other_client("wes");
    other.cmd("CREATE Target");
    for n in 1..=6 {
        other.append("INBOX", "", &message(n, &format!("m{n}"), "b"));
    }
    // Another client has marked a message for deletion without expunging.
    other.cmd("SELECT INBOX");
    other.cmd("UID STORE 6 +FLAGS (\\Deleted)");

    let mut session = Session::connect(&server.server(), "wes", PASSWORD).await.unwrap();
    session.select(b"INBOX").await.unwrap();

    // Flags.
    session.store_flags(&[1, 2], Flags::SEEN.with(Flags::FLAGGED), true).await.unwrap();
    session.store_flags(&[2], Flags::FLAGGED, false).await.unwrap();
    session.store_flags(&[3], Flags::FORWARDED.with(Flags::ANSWERED), true).await.unwrap();
    let flags = session.fetch_flags(1, None).await.unwrap();
    let of = |uid: u32| flags.iter().find(|f| f.uid == uid).unwrap().flags;
    assert_eq!(of(1), Flags::SEEN.with(Flags::FLAGGED));
    assert_eq!(of(2), Flags::SEEN);
    assert_eq!(of(3), Flags::FORWARDED.with(Flags::ANSWERED));
    assert_eq!(of(4), Flags::default());

    // Move: gone from here, present there, and we are told its new UID.
    let pairs = session.move_messages(&[4, 5], b"Target").await.unwrap();
    assert_eq!(pairs.iter().map(|p| p.0).collect::<Vec<_>>(), [4, 5]);
    let remaining: Vec<u32> = session.fetch_flags(1, None).await.unwrap().iter().map(|f| f.uid).collect();
    assert_eq!(remaining, [1, 2, 3, 6]);
    session.examine(b"Target", None).await.unwrap();
    let moved: Vec<u32> = session.fetch_flags(1, None).await.unwrap().iter().map(|f| f.uid).collect();
    assert_eq!(moved, pairs.iter().map(|p| p.1).collect::<Vec<_>>());
    assert_eq!(session.fetch_message(pairs[0].1).await.unwrap().unwrap(), message(4, "m4", "b"));

    // Expunge by UID removes only what was named: the message the other
    // client marked for deletion is still there afterwards.
    session.select(b"INBOX").await.unwrap();
    assert!(session.expunge(&[2]).await.unwrap());
    let remaining: Vec<u32> = session.fetch_flags(1, None).await.unwrap().iter().map(|f| f.uid).collect();
    assert_eq!(remaining, [1, 3, 6], "UID 6 was marked deleted by someone else and must survive");

    // Append, and finding a message by its Message-ID.
    let raw = message(50, "appended", "body");
    let uid = session.append(b"Target", Flags::SEEN.with(Flags::DRAFT), &raw).await.unwrap().unwrap();
    session.examine(b"Target", None).await.unwrap();
    assert_eq!(session.fetch_message(uid).await.unwrap().unwrap(), raw);
    let state = session.fetch_flags(uid, None).await.unwrap();
    assert_eq!(state[0].flags, Flags::SEEN.with(Flags::DRAFT));
    assert_eq!(session.find_by_message_id("50.8@example.com").await.unwrap(), [uid]);
    assert_eq!(session.find_by_message_id("<50.8@example.com>").await.unwrap(), [uid]);
    assert!(session.find_by_message_id("nobody@nowhere").await.unwrap().is_empty());

    // Folders.
    session.create_folder(b"Made/Here").await.unwrap();
    session.rename_folder(b"Made/Here", b"Made/There").await.unwrap();
    let names: Vec<String> = session.list().await.unwrap().into_iter().map(|f| f.name).collect();
    assert!(names.contains(&"Made/There".to_string()) && !names.contains(&"Made/Here".to_string()), "{names:?}");
    session.delete_folder(b"Made/There").await.unwrap();
    let names: Vec<String> = session.list().await.unwrap().into_iter().map(|f| f.name).collect();
    assert!(!names.contains(&"Made/There".to_string()));
}
