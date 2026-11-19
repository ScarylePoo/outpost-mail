//! Meeting invitations: arriving by mail, shown in the message, answered,
//! filed in the calendar and replied to. Needs a mail server (Dovecot), a
//! calendar server (Radicale) and the fake outgoing server.

mod common;
#[path = "common/dav.rs"]
mod dav;

use common::smtp::SmtpSink;
use common::{TempDir, TestServer, PASSWORD};
use dav::{DavServer, DAV_PASSWORD, DAV_USER};
use outpost_core::accounts::MemorySecrets;
use outpost_core::api::{Command, ErrorCode, Event, Response, Secret, SendingSettings, SyncState};
use outpost_core::dav::client::{Dav, Kind};
use outpost_core::dav::http::Url;
use outpost_core::{Config, Core};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CAL: Kind = Kind::Calendar { events: true, tasks: false };

fn invitation(method: &str, sequence: u32) -> Vec<u8> {
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Other//EN\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:meeting-1@example.com\r\nDTSTAMP:20261118T100000Z\r\nSEQUENCE:{sequence}\r\nDTSTART:20261120T140000Z\r\nDTEND:20261120T150000Z\r\nSUMMARY:Budget review\r\nLOCATION:Room 4\r\nORGANIZER;CN=Dana Dubois:mailto:dana@example.com\r\nATTENDEE;CN=Sam;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:sam@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    format!(
        "From: Dana Dubois <dana@example.com>\r\nTo: sam@example.com\r\nSubject: {subject}Budget review\r\nDate: Wed, 18 Nov 2026 10:00:0{sequence} +0000\r\nMessage-ID: <invite-{method}-{sequence}@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nYou are invited to the budget review.\r\n--b\r\nContent-Type: text/calendar; method={method}; charset=utf-8\r\n\r\n{ics}--b--\r\n",
        subject = if method == "CANCEL" { "Cancelled: " } else { "Invitation: " }
    )
    .into_bytes()
}

fn until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn an_invitation_is_shown_answered_filed_and_replied_to() {
    let Some(mail) = TestServer::start() else { return };
    let Some(calendar) = DavServer::start() else { return };
    let sink = SmtpSink::start();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let other_dav = Dav::new(Url::parse(&calendar.address()).unwrap(), DAV_USER, DAV_PASSWORD);
    runtime.block_on(async { other_dav.create_collection("/ann/diary/", "Diary", CAL).await.unwrap() });
    let mut other = mail.other_client("sam");
    other.append("INBOX", "", &invitation("REQUEST", 0));
    other.append("INBOX", "", b"From: x@example.com\r\nSubject: ordinary\r\n\r\nNo invitation here.\r\n");

    let data = TempDir::new();
    let (tx, events) = mpsc::channel();
    let config = Config { data_dir: Some(data.path().to_path_buf()), downloads_dir: None };
    let core = Core::start_with_secrets(config, Arc::new(MemorySecrets::default()), move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    let accounts_up = |core: &Core| match core.handle(Command::ListAccounts).unwrap() {
        Response::Accounts { accounts } => accounts.first().is_some_and(|a| a.status.state == SyncState::UpToDate),
        _ => false,
    };
    core.handle(Command::AddAccount {
        host: "127.0.0.1".into(),
        port: mail.port,
        encrypted: false,
        username: "sam".into(),
        password: Secret(PASSWORD.into()),
        remember: false,
        sending: Some(SendingSettings {
            name: "Sam Sender".into(),
            address: "sam@example.com".into(),
            host: "127.0.0.1".into(),
            port: sink.port,
            security: "plaintext".into(),
        }),
    })
    .unwrap();
    core.handle(Command::AddDavAccount { url: calendar.address(), username: DAV_USER.into(), password: Secret(DAV_PASSWORD.into()), remember: false })
        .unwrap();
    let dav_id = loop {
        match events.recv_timeout(Duration::from_secs(30)).expect("the calendar account") {
            Event::DavAccountAdded { account } => break account,
            Event::DavAccountSetupFailed { error } => panic!("{error}"),
            _ => {}
        }
    };
    until("mail to sync", || accounts_up(&core));
    let Response::Folders { folders } = core.handle(Command::ListFolders).unwrap() else { panic!() };
    let inbox = folders.iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
    let message = |core: &Core, subject: &str| {
        let Response::Messages { rows, .. } =
            core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 50, conversations: false }).unwrap()
        else {
            panic!()
        };
        let id = rows.iter().find(|r| r.subject == subject).unwrap_or_else(|| panic!("no {subject}")).id;
        match core.handle(Command::OpenMessage { folder: inbox.clone(), id }).unwrap() {
            Response::Message { message } => message,
            other => panic!("{other:?}"),
        }
    };
    let diary = || match core.handle(Command::ListDavAccounts).unwrap() {
        Response::DavAccounts { accounts } => accounts[0].collections.first().map(|c| c.id),
        _ => None,
    };
    until("the calendar to sync", || diary().is_some());
    let diary = diary().unwrap();
    let in_calendar = |core: &Core| match core.handle(Command::ListEvents { from: 0, to: i64::MAX / 4 }).unwrap() {
        Response::Events { events } => events.into_iter().map(|e| e.summary).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };

    let received = || sink.received.lock().unwrap().clone();
    // The message shows the invitation; an ordinary message shows none.
    let opened = message(&core, "Invitation: Budget review");
    let shown = opened.invitation.clone().expect("the invitation is shown");
    assert_eq!((shown.kind.as_str(), shown.event.summary.as_str(), shown.event.location.as_str()), ("request", "Budget review", "Room 4"));
    assert_eq!((shown.organizer.as_str(), shown.organizer_name.as_str(), shown.in_calendar), ("dana@example.com", "Dana Dubois", false));
    assert_eq!(shown.event.end - shown.event.start, 3600);
    assert_eq!(message(&core, "ordinary").invitation, None);

    // Nonsense is refused before anything is sent.
    let answer = |core: &Core, id: i64, answer: &str, collection: i64| {
        core.handle(Command::AnswerInvitation { folder: inbox.clone(), id, answer: answer.into(), account: dav_id.clone(), collection })
    };
    assert_eq!(answer(&core, opened.id, "perhaps", diary).unwrap_err().code, ErrorCode::BadCommand);
    assert!(answer(&core, opened.id, "accepted", 9_999).is_err());
    let ordinary = message(&core, "ordinary").id;
    assert_eq!(answer(&core, ordinary, "accepted", diary).unwrap_err().code, ErrorCode::NotPossible);
    assert!(received().is_empty() && in_calendar(&core).is_empty());

    // Accepted: in the calendar here and on the server, and the organiser
    // is told, once, in a form a calendar program reads.
    answer(&core, opened.id, "accepted", diary).unwrap();
    assert_eq!(in_calendar(&core), ["Budget review"]);
    until("the reply to be sent", || received().len() == 1);
    let sent = received().remove(0);
    assert_eq!((sent.from.as_str(), &sent.recipients), ("sam@example.com", &vec!["dana@example.com".to_string()]));
    let text = String::from_utf8_lossy(&sent.message).replace("\r\n ", "");
    assert!(text.contains("Subject: Accepted: Budget review"), "{text}");
    assert!(text.to_lowercase().contains("content-type: text/calendar") && text.contains("method=\"REPLY\"") || text.contains("method=REPLY"), "{text}");
    for wanted in ["METHOD:REPLY", "UID:meeting-1@example.com", "PARTSTAT=ACCEPTED", "mailto:sam@example.com", "Sam Sender has accepted"] {
        assert!(text.contains(wanted), "no {wanted} in\n{text}");
    }
    until("the event to reach the calendar server", || {
        runtime.block_on(async {
            let hrefs: Vec<String> = other_dav.list("/ann/diary/").await.unwrap().into_iter().map(|i| i.href).collect();
            let items = other_dav.fetch("/ann/diary/", CAL, &hrefs).await.unwrap();
            items.len() == 1 && items[0].data.contains("PARTSTAT=ACCEPTED") && !items[0].data.contains("METHOD")
        })
    });
    assert!(message(&core, "Invitation: Budget review").invitation.unwrap().in_calendar);

    // Answering again (changing one's mind) replaces the event rather than
    // adding a second one, and tells the organiser again.
    answer(&core, opened.id, "tentative", diary).unwrap();
    assert_eq!(in_calendar(&core), ["Budget review"]);
    until("the second reply", || received().len() == 2);
    assert!(String::from_utf8_lossy(&received()[1].message).contains("PARTSTAT=TENTATIVE"));

    // Declined: out of the calendar, organiser told.
    answer(&core, opened.id, "declined", diary).unwrap();
    assert!(in_calendar(&core).is_empty());
    until("the third reply", || received().len() == 3);

    // Accepted again, then the organiser cancels: the cancellation is
    // shown as one, and removing takes the event out and mails nothing.
    answer(&core, opened.id, "accepted", diary).unwrap();
    until("the fourth reply", || received().len() == 4);
    other.append("INBOX", "", &invitation("CANCEL", 1));
    until("the cancellation to arrive", || {
        let Response::Messages { rows, .. } =
            core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 50, conversations: false }).unwrap()
        else {
            panic!()
        };
        rows.iter().any(|r| r.subject.starts_with("Cancelled"))
    });
    // The row is listed as soon as its headers are known; what the message
    // says about the meeting is only known once its body is here too.
    until("the cancellation to be downloaded", || message(&core, "Cancelled: Budget review").downloaded);
    let cancelled = message(&core, "Cancelled: Budget review");
    let shown = cancelled.invitation.clone().unwrap();
    assert_eq!((shown.kind.as_str(), shown.in_calendar), ("cancel", true));
    answer(&core, cancelled.id, "remove", diary).unwrap();
    assert!(in_calendar(&core).is_empty());
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(received().len(), 4, "a removal is not announced to anyone");
}
