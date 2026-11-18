//! Calendars, tasks and contacts through the core's commands, against a
//! real server (Radicale).

#[path = "common/dav.rs"]
mod dav;

use dav::{DavServer, DAV_PASSWORD, DAV_USER};
use outpost_core::accounts::{MemorySecrets, Secrets};
use outpost_core::api::{Command, ContactContent, ErrorCode, Event, EventContent, Response, Secret, SyncState, TaskContent};
use outpost_core::dav::client::{Dav, Kind};
use outpost_core::dav::http::Url;
use outpost_core::{Config, Core};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CAL: Kind = Kind::Calendar { events: true, tasks: true };
const DAY: i64 = 86_400;
const NOV_20: i64 = 20_777 * DAY;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("outpost-davcore-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn start(data: &std::path::Path, secrets: Arc<MemorySecrets>) -> (Core, Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    let config = Config { data_dir: Some(data.to_path_buf()), downloads_dir: None };
    let core = Core::start_with_secrets(config, secrets, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (core, rx)
}

fn wait_for<T>(events: &Receiver<Event>, what: &str, mut wanted: impl FnMut(&Event) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(event) => {
                if let Some(found) = wanted(&event) {
                    return found;
                }
            }
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

/// Waits until `check` holds, looking again whenever something changes.
fn until(events: &Receiver<Event>, what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        let _ = events.recv_timeout(Duration::from_millis(200));
    }
}

#[test]
fn calendars_tasks_and_contacts_work_end_to_end() {
    let Some(server) = DavServer::start() else { return };
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let other = Dav::new(Url::parse(&server.address()).unwrap(), DAV_USER, DAV_PASSWORD);
    runtime.block_on(async {
        other.create_collection("/ann/work/", "Work", CAL).await.unwrap();
        other.create_collection("/ann/people/", "People", Kind::AddressBook).await.unwrap();
        let theirs = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:x\r\nBEGIN:VEVENT\r\nUID:theirs\r\nDTSTAMP:20261118T100000Z\r\nDTSTART:20261120T090000Z\r\nDTEND:20261120T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Standup\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        other.put("/ann/work/theirs.ics", CAL, theirs, None).await.unwrap();
    });

    let data = temp("end-to-end");
    let secrets = Arc::new(MemorySecrets::default());
    let (core, events) = start(&data, secrets.clone());
    let accounts = |core: &Core| match core.handle(Command::ListDavAccounts).unwrap() {
        Response::DavAccounts { accounts } => accounts,
        other => panic!("{other:?}"),
    };
    assert!(accounts(&core).is_empty());

    // A wrong password adds nothing.
    let add = |password: &str| {
        core.handle(Command::AddDavAccount {
            url: server.address(),
            username: DAV_USER.into(),
            password: Secret(password.into()),
            remember: true,
        })
        .unwrap()
    };
    add("wrong");
    let error = wait_for(&events, "the refusal", |e| match e {
        Event::DavAccountSetupFailed { error } => Some(error.clone()),
        _ => None,
    });
    assert!(error.contains("password"), "{error}");
    assert!(accounts(&core).is_empty() && !data.join("dav").exists());

    // The right one adds the account and syncs it.
    add(DAV_PASSWORD);
    let id = wait_for(&events, "the account", |e| match e {
        Event::DavAccountAdded { account } => Some(account.clone()),
        _ => None,
    });
    assert_eq!(id, "dav/ann-127.0.0.1");
    until(&events, "the first sync", || accounts(&core)[0].status.state == SyncState::UpToDate);
    let account = accounts(&core).remove(0);
    let names: Vec<(&str, bool, bool, bool)> = account.collections.iter().map(|c| (c.name.as_str(), c.events, c.tasks, c.contacts)).collect();
    assert_eq!(names, [("Work", true, true, false), ("People", false, false, true)]);
    let (work, people) = (account.collections[0].id, account.collections[1].id);
    // The password is in the credential store and not on disk.
    assert_eq!(secrets.get(&format!("dav:ann-127.0.0.1:ann@{}", server.address())).as_deref(), Some(DAV_PASSWORD));
    let saved = std::fs::read_to_string(data.join("dav/ann-127.0.0.1/account.json")).unwrap();
    assert!(saved.contains("127.0.0.1") && !saved.contains(DAV_PASSWORD), "{saved}");

    let events_on = |core: &Core, from: i64, to: i64| match core.handle(Command::ListEvents { from, to }).unwrap() {
        Response::Events { events } => events,
        other => panic!("{other:?}"),
    };
    // The repeating event from the server: three days, one row each.
    let week = events_on(&core, NOV_20, NOV_20 + 7 * DAY);
    assert_eq!(week.iter().map(|e| (e.summary.as_str(), e.start - NOV_20)).collect::<Vec<_>>(), [("Standup", 9 * 3600), ("Standup", DAY + 9 * 3600), ("Standup", 2 * DAY + 9 * 3600)]);
    assert!(week[0].recurring);

    // An event made here shows at once and reaches the server.
    let content = EventContent {
        summary: "Lunch, with Dana".into(),
        location: "Cafe".into(),
        description: "Bring the plan".into(),
        start: NOV_20 + 12 * 3600,
        end: NOV_20 + 13 * 3600,
        all_day: false,
    };
    let Response::DavSaved { id: lunch } =
        core.handle(Command::SaveEvent { account: id.clone(), collection: work, id: None, event: content.clone() }).unwrap()
    else {
        panic!()
    };
    assert_eq!(events_on(&core, NOV_20, NOV_20 + DAY).iter().map(|e| e.summary.as_str()).collect::<Vec<_>>(), ["Standup", "Lunch, with Dana"]);
    let Response::DavItem { item } = core.handle(Command::OpenDavItem { account: id.clone(), id: lunch }).unwrap() else { panic!() };
    assert_eq!((item.event, item.recurring, item.collection), (Some(content.clone()), false, work));
    let on_server = |name: &str| {
        runtime.block_on(async {
            let hrefs: Vec<String> = other.list(name).await.unwrap().into_iter().map(|i| i.href).collect();
            let kind = if name.contains("people") { Kind::AddressBook } else { CAL };
            other.fetch(name, kind, &hrefs).await.unwrap().into_iter().map(|i| i.data).collect::<Vec<String>>()
        })
    };
    until(&events, "the event to reach the server", || on_server("/ann/work/").iter().any(|d| d.contains("SUMMARY:Lunch\\, with Dana")));

    // Changed, then deleted.
    let moved = EventContent { summary: "Lunch moved".into(), start: content.start + 3600, end: content.end + 3600, ..content.clone() };
    core.handle(Command::SaveEvent { account: id.clone(), collection: work, id: Some(lunch), event: moved }).unwrap();
    until(&events, "the change to reach the server", || on_server("/ann/work/").iter().any(|d| d.contains("SUMMARY:Lunch moved")));
    let backwards = EventContent { start: 10, end: 5, ..content.clone() };
    let refused = core.handle(Command::SaveEvent { account: id.clone(), collection: work, id: None, event: backwards }).unwrap_err();
    assert_eq!(refused.code, ErrorCode::NotPossible);
    core.handle(Command::DeleteDavItem { account: id.clone(), id: lunch }).unwrap();
    assert_eq!(events_on(&core, NOV_20, NOV_20 + DAY).len(), 1, "gone here at once");
    until(&events, "the deletion to reach the server", || on_server("/ann/work/").len() == 1);

    // A task: made, ticked off.
    let task = TaskContent { summary: "Buy milk".into(), description: String::new(), due: Some(NOV_20), completed: false };
    let Response::DavSaved { id: milk } = core.handle(Command::SaveTask { account: id.clone(), collection: work, id: None, task: task.clone() }).unwrap() else {
        panic!()
    };
    let tasks = |core: &Core| match core.handle(Command::ListTasks).unwrap() {
        Response::Tasks { tasks } => tasks,
        other => panic!("{other:?}"),
    };
    assert_eq!(tasks(&core).iter().map(|t| (t.summary.as_str(), t.due, t.completed)).collect::<Vec<_>>(), [("Buy milk", Some(NOV_20), false)]);
    core.handle(Command::SaveTask { account: id.clone(), collection: work, id: Some(milk), task: TaskContent { completed: true, ..task } }).unwrap();
    assert!(tasks(&core)[0].completed);
    until(&events, "the finished task to reach the server", || on_server("/ann/work/").iter().any(|d| d.contains("STATUS:COMPLETED")));

    // A contact: made, found, and something another program adds arrives.
    let dana = ContactContent { name: "Dana Dubois".into(), emails: vec!["dana@example.com".into()], phones: vec!["+1 555 0100".into()], ..ContactContent::default() };
    core.handle(Command::SaveContact { account: id.clone(), collection: people, id: None, contact: dana.clone() }).unwrap();
    let nameless = core.handle(Command::SaveContact { account: id.clone(), collection: people, id: None, contact: ContactContent::default() }).unwrap_err();
    assert_eq!(nameless.code, ErrorCode::NotPossible);
    until(&events, "the contact to reach the server", || on_server("/ann/people/").iter().any(|d| d.contains("FN:Dana Dubois")));
    runtime.block_on(async {
        let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:sam\r\nFN:Sam Okafor\r\nN:Okafor;Sam;;;\r\nEMAIL:sam@example.com\r\nEND:VCARD\r\n";
        other.put("/ann/people/sam.vcf", Kind::AddressBook, card, None).await.unwrap();
    });
    core.handle(Command::SyncDav { account: id.clone() }).unwrap();
    let contacts = |core: &Core, query: &str| match core.handle(Command::ListContacts { query: query.into(), limit: 50 }).unwrap() {
        Response::Contacts { contacts } => contacts.into_iter().map(|c| c.name).collect::<Vec<_>>(),
        other => panic!("{other:?}"),
    };
    until(&events, "the other program's contact", || contacts(&core, "") == ["Dana Dubois", "Sam Okafor"]);
    assert_eq!(contacts(&core, "okafor"), ["Sam Okafor"]);

    // A calendar switched off disappears from the lists.
    core.handle(Command::SetDavCollection { account: id.clone(), collection: work, enabled: false, color: Some("#00aa55".into()) }).unwrap();
    assert!(events_on(&core, NOV_20, NOV_20 + 7 * DAY).is_empty() && tasks(&core).is_empty());
    core.handle(Command::SetDavCollection { account: id.clone(), collection: work, enabled: true, color: Some("#00aa55".into()) }).unwrap();
    until(&events, "the calendar to come back", || events_on(&core, NOV_20, NOV_20 + DAY).len() == 1);
    assert_eq!(events_on(&core, NOV_20, NOV_20 + DAY)[0].color.as_deref(), Some("#00aa55"));

    // After a restart everything is there before any sync, and the account
    // signs in by itself with the stored password.
    drop(core);
    let (core, events) = start(&data, secrets.clone());
    assert_eq!(events_on(&core, NOV_20, NOV_20 + DAY).len(), 1);
    assert_eq!(contacts(&core, "").len(), 2);
    until(&events, "the sync after the restart", || accounts(&core)[0].status.state == SyncState::UpToDate);

    // Removing the account takes its files and password, and nothing on
    // the server.
    core.handle(Command::RemoveDavAccount { account: id.clone(), password: None }).unwrap();
    assert!(accounts(&core).is_empty() && events_on(&core, 0, i64::MAX / 4).is_empty());
    assert_eq!(secrets.get(&format!("dav:ann-127.0.0.1:ann@{}", server.address())), None);
    assert_eq!(on_server("/ann/people/").len(), 2);
    drop(core);
    let (core, _events) = start(&data, secrets);
    assert!(accounts(&core).is_empty());
    assert!(!data.join("dav/ann-127.0.0.1").exists());
}
