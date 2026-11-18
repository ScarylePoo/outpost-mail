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
            json!({"type": "openMessage", "folder": "demo/inbox", "id": 7}),
            Command::OpenMessage {
                folder: "demo/inbox".into(),
                id: 7,
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
        // Reading: text and (made safe) HTML, and what is not there.
        let open = |id: i64| core.handle(Command::OpenMessage { folder: inbox.clone(), id });
        let Response::Message { message } = open(rows[4].id).unwrap() else { panic!("wrong response") };
        assert!(message.downloaded);
        assert_eq!(message.subject, "message 0");
        assert_eq!(message.sender_address, "s0@example.com");
        assert_eq!(message.text.as_deref().map(str::trim), Some("inbox body 0"));
        assert_eq!(message.html, None);
        assert_eq!(open(987_654).unwrap_err().code, ErrorCode::UnknownMessage);
        let html_id = {
            let store = Store::open(data.join("accounts/home")).unwrap();
            let folder = store.index().folders().unwrap().into_iter().find(|f| f.path == "Inbox").unwrap();
            let raw = b"From: x@example.com\r\nSubject: html\r\nContent-Type: text/html\r\n\r\n\
<p onclick=\"steal()\">Hi <b>you</b></p><script>steal()</script><img src=\"https://t.example/p.gif\">\r\n";
            store.add_message(folder.id, raw, Flags::SEEN, None).unwrap()
        };
        let Response::Message { message } = open(html_id).unwrap() else { panic!("wrong response") };
        let html = message.html.unwrap();
        assert!(html.contains("<b>you</b>") && !html.contains("steal") && !html.contains("t.example"), "{html}");
        assert!(message.remote_content_blocked);

        drop(core);
        let _ = std::fs::remove_dir_all(&data);
    }
}

mod folder_tree {
    use super::*;
    use outpost_core::store::Store;
    use outpost_core::Config;

    fn tree(folders: &[&[&str]]) -> Vec<(String, u8, Option<String>)> {
        let data = std::env::temp_dir().join(format!("outpost-tree-test-{}-{}", std::process::id(), folders.len()));
        let _ = std::fs::remove_dir_all(&data);
        {
            let store = Store::open(data.join("accounts/a")).unwrap();
            for folder in folders {
                store.create_folder(folder).unwrap();
            }
        }
        let core = Core::start_with(Config { data_dir: Some(data.clone()) }, |_| {}).unwrap();
        let Response::Folders { folders } = core.handle(Command::ListFolders).unwrap() else { panic!() };
        drop(core);
        let _ = std::fs::remove_dir_all(&data);
        folders.into_iter().skip(1).map(|f| (f.name, f.depth, f.role)).collect()
    }

    fn names(tree: &[(String, u8, Option<String>)]) -> Vec<(&str, u8)> {
        tree.iter().map(|(n, d, _)| (n.as_str(), *d)).collect()
    }

    #[test]
    fn well_known_folders_come_first_then_the_rest_by_name() {
        let t = tree(&[&["zebra"], &["Trash"], &["Apple"], &["Sent Items"], &["INBOX"], &["Spam"], &["Drafts"], &["Apple", "Core"], &["Archive"]]);
        assert_eq!(
            names(&t),
            [("Inbox", 1), ("Drafts", 1), ("Sent Items", 1), ("Archive", 1), ("Spam", 1), ("Trash", 1), ("Apple", 1), ("Core", 2), ("zebra", 1)]
        );
        assert_eq!(t[0].2.as_deref(), Some("inbox"));
        assert_eq!(t[2].2.as_deref(), Some("sent"));
        assert_eq!(t[4].2.as_deref(), Some("junk"));
        assert_eq!(t[6].2, None);
    }

    #[test]
    fn folders_kept_inside_the_inbox_are_shown_beside_it() {
        let t = tree(&[&["INBOX"], &["INBOX", "Sent"], &["INBOX", "Trash"], &["INBOX", "Clients"], &["INBOX", "Clients", "Acme"], &["INBOX", "spam"]]);
        assert_eq!(
            names(&t),
            [("Inbox", 1), ("Sent", 1), ("spam", 1), ("Trash", 1), ("Clients", 1), ("Acme", 2)]
        );
    }

    #[test]
    fn inbox_children_stay_nested_when_there_are_other_top_level_folders() {
        let t = tree(&[&["INBOX"], &["INBOX", "Receipts"], &["Sent"]]);
        assert_eq!(names(&t), [("Inbox", 1), ("Receipts", 2), ("Sent", 1)]);
    }
}
