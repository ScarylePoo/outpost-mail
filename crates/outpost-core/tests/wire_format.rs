//! Pins the JSON shape that `ui/src/core/api.ts` relies on, and exercises the
//! command-and-event interface the way a shell adapter does.

use outpost_core::api::{Command, ErrorCode, Event, Response, MAX_PAGE};
use outpost_core::Core;
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

fn core() -> Core {
    Core::start(|_| {})
}

#[test]
fn commands_decode_from_the_documented_json() {
    let cases = [
        (json!({"type": "ping"}), Command::Ping),
        (json!({"type": "listFolders"}), Command::ListFolders),
        (
            json!({"type": "folderLayout", "folder": "demo/inbox"}),
            Command::FolderLayout {
                folder: "demo/inbox".into(),
            },
        ),
        (
            json!({"type": "listMessages", "folder": "demo/inbox", "offset": 5, "limit": 2}),
            Command::ListMessages {
                folder: "demo/inbox".into(),
                offset: 5,
                limit: 2,
            },
        ),
        (
            json!({"type": "search", "scope": "demo", "text": "roof", "limit": 50}),
            Command::Search {
                scope: "demo".into(),
                text: "roof".into(),
                limit: 50,
            },
        ),
        (
            json!({"type": "startDemoSync", "folder": "demo/inbox"}),
            Command::StartDemoSync {
                folder: "demo/inbox".into(),
            },
        ),
    ];
    for (wire, expected) in cases {
        assert_eq!(serde_json::from_value::<Command>(wire.clone()).unwrap(), expected);
        assert_eq!(serde_json::to_value(&expected).unwrap(), wire);
    }
}

#[test]
fn a_message_row_has_exactly_the_fields_the_interface_expects() {
    let reply: Value = serde_json::from_str(&core().handle_json(
        r#"{"type":"listMessages","folder":"demo/inbox","offset":0,"limit":1}"#,
    ))
    .unwrap();
    let ok = &reply["ok"];
    assert_eq!(ok["type"], "messages");
    assert_eq!(ok["total"], 100_000);
    assert_eq!(ok["offset"], 0);
    let row = ok["rows"][0].as_object().unwrap();
    let mut keys: Vec<&str> = row.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "date",
            "hasAttachment",
            "id",
            "index",
            "preview",
            "senderAddress",
            "senderName",
            "subject",
            "unread"
        ]
    );
}

#[test]
fn events_encode_with_a_type_tag() {
    assert_eq!(
        serde_json::to_value(Event::SyncProgress {
            folder: "f".into(),
            done: 1,
            total: 2
        })
        .unwrap(),
        json!({"type": "syncProgress", "folder": "f", "done": 1, "total": 2})
    );
}

#[test]
fn pages_are_clamped_to_the_folder_and_to_max_page() {
    let core = core();
    let page = |offset, limit| match core
        .handle(Command::ListMessages {
            folder: "demo/inbox".into(),
            offset,
            limit,
        })
        .unwrap()
    {
        Response::Messages { offset, rows, .. } => (offset, rows),
        other => panic!("unexpected {other:?}"),
    };

    let (_, rows) = page(0, u32::MAX);
    assert_eq!(rows.len(), MAX_PAGE as usize);

    let (offset, rows) = page(99_990, 50);
    assert_eq!((offset, rows.len()), (99_990, 10));
    assert_eq!(rows.last().unwrap().index, 99_999);

    let (offset, rows) = page(u32::MAX, 50);
    assert_eq!((offset, rows.len()), (100_000, 0));

    // Consecutive pages join up with no gap or overlap.
    let (_, a) = page(100, 10);
    let (_, b) = page(110, 10);
    let (_, both) = page(100, 20);
    assert_eq!([a, b].concat(), both);
}

#[test]
fn layout_agrees_with_the_rows() {
    let core = core();
    let Response::FolderLayout {
        total,
        preview_lens,
        ..
    } = core
        .handle(Command::FolderLayout {
            folder: "demo/small".into(),
        })
        .unwrap()
    else {
        panic!("wrong response")
    };
    let Response::Messages { rows, .. } = core
        .handle(Command::ListMessages {
            folder: "demo/small".into(),
            offset: 0,
            limit: 1000,
        })
        .unwrap()
    else {
        panic!("wrong response")
    };
    assert_eq!(total as usize, rows.len());
    let from_rows: Vec<u16> = rows
        .iter()
        .map(|r| r.preview.chars().count() as u16)
        .collect();
    assert_eq!(preview_lens, from_rows);
}

#[test]
fn errors_are_reported_not_panicked() {
    let core = core();
    let err = core
        .handle(Command::ListMessages {
            folder: "nope".into(),
            offset: 0,
            limit: 1,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::UnknownFolder);

    let reply: Value = serde_json::from_str(&core.handle_json("{not json")).unwrap();
    assert_eq!(reply["error"]["code"], "badCommand");
    let reply: Value = serde_json::from_str(&core.handle_json(r#"{"type":"formatDisk"}"#)).unwrap();
    assert_eq!(reply["error"]["code"], "badCommand");
}

#[test]
fn events_reach_the_sink_in_order_and_finish() {
    let (tx, rx) = mpsc::channel();
    let core = Core::start(move |e| {
        let _ = tx.send(e);
    });
    let reply = core
        .handle(Command::StartDemoSync {
            folder: "demo/small".into(),
        })
        .unwrap();
    assert_eq!(reply, Response::Accepted);

    let mut last_done = 0;
    loop {
        match rx.recv_timeout(Duration::from_secs(10)).expect("event") {
            Event::SyncProgress { done, total, .. } => {
                assert!(done >= last_done && done <= total);
                last_done = done;
            }
            Event::SyncFinished { folder } => {
                assert_eq!(folder, "demo/small");
                break;
            }
        }
    }
    assert_eq!(last_done, 40);
}

mod stored_mail {
    use super::*;
    use outpost_core::api::FolderInfo;
    use outpost_core::store::maildir::Flags;
    use outpost_core::store::Store;
    use outpost_core::Config;

    fn mail(n: u32, body: &str) -> Vec<u8> {
        format!(
            "From: Sender {n} <s{n}@example.com>\r\nSubject: message {n}\r\n\
             Date: Mon, 2 Nov 2026 09:{n:02}:00 +0000\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    /// The commands the interface uses, answered from an account on disk.
    #[test]
    fn commands_are_served_from_the_index() {
        let data = std::env::temp_dir().join(format!("outpost-core-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        {
            let store = Store::open(data.join("accounts/home")).unwrap();
            let inbox = store.create_folder(&["Inbox"]).unwrap();
            let nested = store.create_folder(&["Projects", "Alpha"]).unwrap();
            for n in 0..30 {
                let flags = if n < 25 { Flags::SEEN } else { Flags::default() };
                store.add_message(inbox, &mail(n, &format!("inbox body {n}")), flags, None).unwrap();
            }
            store.add_message(nested, &mail(40, "the alpha plan"), Flags::SEEN, None).unwrap();
        }

        let core = Core::start_with(Config { data_dir: Some(data.clone()) }, |_| {}).unwrap();
        let Response::Folders { folders } = core.handle(Command::ListFolders).unwrap() else {
            panic!("wrong response")
        };
        let shape: Vec<(&str, u8, u32, u32)> = folders
            .iter()
            .map(|f: &FolderInfo| (f.name.as_str(), f.depth, f.total, f.unread))
            .collect();
        assert_eq!(
            shape,
            [("home", 0, 0, 0), ("Inbox", 1, 30, 5), ("Projects", 1, 0, 0), ("Alpha", 2, 1, 0)]
        );
        let account = folders[0].id.clone();
        let inbox = folders[1].id.clone();

        let Response::FolderLayout { total, preview_lens, .. } =
            core.handle(Command::FolderLayout { folder: inbox.clone() }).unwrap()
        else {
            panic!("wrong response")
        };
        assert_eq!((total, preview_lens.len()), (30, 30));

        let Response::Messages { offset, total, rows, .. } = core
            .handle(Command::ListMessages { folder: inbox.clone(), offset: 25, limit: 50 })
            .unwrap()
        else {
            panic!("wrong response")
        };
        assert_eq!((offset, total, rows.len()), (25, 30, 5));
        assert_eq!(rows[0].index, 25);
        assert_eq!(rows[4].subject, "message 0", "oldest is last");
        assert_eq!(rows[4].sender_name, "Sender 0");
        assert!(!rows[4].unread);
        assert_eq!(preview_lens[29] as usize, rows[4].preview.chars().count());

        let search = |scope: &str, text: &str| -> Vec<String> {
            let Response::SearchResults { rows } = core
                .handle(Command::Search { scope: scope.into(), text: text.into(), limit: 100 })
                .unwrap()
            else {
                panic!("wrong response")
            };
            rows.into_iter().map(|r| r.subject).collect()
        };
        assert_eq!(search(&account, "alpha "), ["message 40"]);
        assert_eq!(search(&inbox, "alpha "), Vec::<String>::new());
        assert_eq!(search(&account, "body").len(), 30);

        for bad in ["acct/home/999", "acct/nobody", "acct/home/x", "demo/inbox"] {
            let err = core.handle(Command::FolderLayout { folder: bad.into() }).unwrap_err();
            assert_eq!(err.code, ErrorCode::UnknownFolder, "{bad}");
        }
        drop(core);
        let _ = std::fs::remove_dir_all(&data);
    }
}
