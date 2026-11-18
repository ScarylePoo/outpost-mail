//! Pins the JSON shape that `ui/src/core/api.ts` relies on, and exercises the
//! command-and-event interface the way a shell adapter does.

use outpost_core::api::{Command, ErrorCode, Event, Response, Secret, SyncState, SyncStatus, MAX_PAGE};
use outpost_core::Core;
use serde_json::{json, Value};

fn core() -> Core {
    Core::start(|_| {})
}

#[test]
fn commands_decode_from_the_documented_json() {
    let cases = [
        (json!({"type": "ping"}), Command::Ping),
        (json!({"type": "listFolders"}), Command::ListFolders),
        (
            json!({"type": "folderLayout", "folder": "demo/inbox", "conversations": false}),
            Command::FolderLayout {
                folder: "demo/inbox".into(),
                conversations: false,
            },
        ),
        (
            json!({"type": "listMessages", "folder": "demo/inbox", "offset": 5, "limit": 2, "conversations": false}),
            Command::ListMessages {
                folder: "demo/inbox".into(),
                offset: 5,
                limit: 2,
                conversations: false,
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
            json!({"type": "locateMessage", "folder": "demo/inbox", "id": 7, "conversations": false}),
            Command::LocateMessage {
                folder: "demo/inbox".into(),
                id: 7,
                conversations: false,
            },
        ),
        (json!({"type": "listAccounts"}), Command::ListAccounts),
        (
            json!({"type": "addAccount", "host": "mail.example.com", "port": 993, "encrypted": true,
                   "username": "me@example.com", "password": "hunter2", "remember": true, "sending": null}),
            Command::AddAccount {
                host: "mail.example.com".into(),
                port: 993,
                encrypted: true,
                username: "me@example.com".into(),
                password: Secret("hunter2".into()),
                remember: true,
                sending: None,
            },
        ),
        (
            json!({"type": "providePassword", "account": "acct/a", "password": "hunter2", "remember": false}),
            Command::ProvidePassword {
                account: "acct/a".into(),
                password: Secret("hunter2".into()),
                remember: false,
            },
        ),
        (json!({"type": "syncNow", "account": "acct/a"}), Command::SyncNow { account: "acct/a".into() }),
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
            "check",
            "date",
            "flagged",
            "hasAttachment",
            "id",
            "index",
            "preview",
            "recipients",
            "senderAddress",
            "senderName",
            "senders",
            "subject",
            "threadCount",
            "threadUnread",
            "unread"
        ]
    );
}

#[test]
fn events_encode_with_a_type_tag() {
    assert_eq!(
        serde_json::to_value(Event::AccountStatus {
            account: "acct/a".into(),
            status: SyncStatus { state: SyncState::UpToDate, detail: String::new() },
        })
        .unwrap(),
        json!({"type": "accountStatus", "account": "acct/a", "status": {"state": "upToDate", "detail": ""}})
    );
    assert_eq!(
        serde_json::to_value(Event::FolderChanged { folder: "acct/a/3".into() }).unwrap(),
        json!({"type": "folderChanged", "folder": "acct/a/3"})
    );
    assert_eq!(
        serde_json::to_value(Event::AccountSetupFailed { error: "no".into() }).unwrap(),
        json!({"type": "accountSetupFailed", "error": "no"})
    );
}

#[test]
fn passwords_do_not_appear_in_debug_output() {
    let command = Command::AddAccount {
        host: "h".into(),
        port: 993,
        encrypted: true,
        username: "u".into(),
        password: Secret("hunter2".into()),
        remember: false,
        sending: None,
    };
    let shown = format!("{command:?}");
    assert!(!shown.contains("hunter2") && shown.contains("***"), "{shown}");
}

#[test]
fn pages_are_clamped_to_the_folder_and_to_max_page() {
    let core = core();
    let page = |offset, limit| match core
        .handle(Command::ListMessages {
            folder: "demo/inbox".into(),
            offset,
            limit,
            conversations: false,
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
            conversations: false,
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
            conversations: false,
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
            conversations: false,
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::UnknownFolder);

    let reply: Value = serde_json::from_str(&core.handle_json("{not json")).unwrap();
    assert_eq!(reply["error"]["code"], "badCommand");
    let reply: Value = serde_json::from_str(&core.handle_json(r#"{"type":"formatDisk"}"#)).unwrap();
    assert_eq!(reply["error"]["code"], "badCommand");
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

        let core = Core::start_with(Config { data_dir: Some(data.clone()), downloads_dir: None }, |_| {}).unwrap();
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
            core.handle(Command::FolderLayout { folder: inbox.clone(), conversations: false }).unwrap()
        else {
            panic!("wrong response")
        };
        assert_eq!((total, preview_lens.len()), (30, 30));

        let Response::Messages { offset, total, rows, .. } = core
            .handle(Command::ListMessages { folder: inbox.clone(), offset: 25, limit: 50, conversations: false })
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

        // The synthetic account is not listed once there is a real one.
        assert!(!folders.iter().any(|f| f.id.starts_with("demo")));
        assert_eq!(
            core.handle(Command::SyncNow { account: account.clone() }).unwrap_err().code,
            ErrorCode::NotPossible,
            "mail that is only browsed has no server to sync with"
        );
        let Response::Located { index } =
            core.handle(Command::LocateMessage { folder: inbox.clone(), id: rows[4].id, conversations: false }).unwrap()
        else {
            panic!("wrong response")
        };
        assert_eq!(index, Some(29));
        let Response::Accounts { accounts } = core.handle(Command::ListAccounts).unwrap() else { panic!() };
        assert_eq!(accounts.len(), 1);
        assert_eq!((accounts[0].name.as_str(), accounts[0].status.state), ("home", SyncState::NotSyncing));

        for bad in ["acct/home/999", "acct/nobody", "acct/home/x"] {
            let err = core.handle(Command::FolderLayout { folder: bad.into(), conversations: false }).unwrap_err();
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

        // Saving an attachment: needs a downloads folder, never replaces
        // a file, and cannot be steered outside the folder by its name.
        assert_eq!(
            core.handle(Command::SaveAttachment { folder: inbox.clone(), id: html_id, index: 0 }).unwrap_err().code,
            ErrorCode::NotPossible
        );
        drop(core);
        let downloads = data.join("downloads");
        let core = Core::start_with(
            Config { data_dir: Some(data.clone()), downloads_dir: Some(downloads.clone()) },
            |_| {},
        )
        .unwrap();
        let with_file = {
            let store = Store::open(data.join("accounts/home")).unwrap();
            let folder = store.index().folders().unwrap().into_iter().find(|f| f.path == "Inbox").unwrap();
            let raw = b"From: x@example.com\r\nSubject: file\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nsee file\r\n\
--b\r\nContent-Type: text/plain; name=\"../../evil.txt\"\r\n\
Content-Disposition: attachment; filename=\"../../evil.txt\"\r\n\r\npayload\r\n--b--\r\n";
            store.add_message(folder.id, raw, Flags::SEEN, None).unwrap()
        };
        let save = || match core.handle(Command::SaveAttachment { folder: inbox.clone(), id: with_file, index: 0 }).unwrap() {
            Response::AttachmentSaved { path, name } => (std::path::PathBuf::from(path), name),
            other => panic!("{other:?}"),
        };
        let (first, name) = save();
        assert_eq!(name, "_.._evil.txt");
        assert_eq!(first.parent().unwrap(), downloads);
        assert_eq!(std::fs::read_to_string(&first).unwrap().trim(), "payload");
        let (second, name) = save();
        assert_eq!(name, "_.._evil (2).txt");
        assert!(first.exists() && second.exists());
        assert!(!data.join("evil.txt").exists() && !data.parent().unwrap().join("evil.txt").exists());
        assert_eq!(
            core.handle(Command::SaveAttachment { folder: inbox.clone(), id: with_file, index: 5 }).unwrap_err().code,
            ErrorCode::UnknownMessage
        );

        drop(core);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn file_names_from_mail_are_made_safe() {
        use outpost_core::safe_file_name as safe;
        assert_eq!(safe("report.pdf"), "report.pdf");
        assert_eq!(safe("Übersicht 2026.xlsx"), "Übersicht 2026.xlsx");
        assert_eq!(safe("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(safe("C:\\Windows\\x.dll"), "C__Windows_x.dll");
        assert_eq!(safe(".bashrc"), "bashrc");
        assert_eq!(safe("con.txt"), "attachment con.txt");
        assert_eq!(safe("COM1"), "attachment COM1");
        assert_eq!(safe(""), "attachment");
        assert_eq!(safe("   "), "attachment");
        assert_eq!(safe("trailing. . "), "trailing");
        assert_eq!(safe("a\u{0}b\nc"), "a_b_c");
        let long = format!("{}.pdf", "x".repeat(400));
        assert_eq!(safe(&long).chars().count(), 120);
        assert!(safe(&long).ends_with(".pdf"));
    }
}

mod conversations {
    use super::*;
    use outpost_core::store::maildir::Flags;
    use outpost_core::store::Store;
    use outpost_core::Config;

    #[test]
    fn a_conversation_is_shown_reduced_with_who_joined_and_left() {
        let data = std::env::temp_dir().join(format!("outpost-thread-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let sign = "\r\n\r\nKind regards,\r\nDana Dubois\r\nThis email is confidential.\r\n";
        {
            let store = Store::open(data.join("accounts/home")).unwrap();
            let inbox = store.create_folder(&["INBOX"]).unwrap();
            let sent = store.create_folder(&["Sent"]).unwrap();
            let first = format!("From: Dana Dubois <dana@example.com>\r\nTo: Ben <ben@example.com>, Sam <sam@example.com>\r\nSubject: Cutover window\r\nDate: Mon, 9 Nov 2026 09:14:00 +0000\r\nMessage-ID: <a@x>\r\n\r\nProposing Saturday night.{sign}");
            let second = "From: Ben <ben@example.com>\r\nTo: Dana Dubois <dana@example.com>, Sam <sam@example.com>, Priya <priya@example.com>\r\nSubject: Re: Cutover window\r\nDate: Mon, 9 Nov 2026 09:40:00 +0000\r\nMessage-ID: <b@x>\r\nIn-Reply-To: <a@x>\r\nReferences: <a@x>\r\n\r\nSaturday works. Adding Priya.\r\n\r\nOn Mon, 9 Nov 2026, Dana Dubois wrote:\r\n> Proposing Saturday night.\r\n> Kind regards,\r\n";
            let third = format!("From: Dana Dubois <dana@example.com>\r\nTo: Ben <ben@example.com>, Priya <priya@example.com>\r\nSubject: Re: Cutover window\r\nDate: Fri, 13 Nov 2026 16:47:00 +0000\r\nMessage-ID: <c@x>\r\nIn-Reply-To: <b@x>\r\nReferences: <a@x> <b@x>\r\n\r\nConfirmed.{sign}");
            store.add_message(inbox, first.as_bytes(), Flags::SEEN, None).unwrap();
            store.add_message(sent, second.as_bytes(), Flags::SEEN, None).unwrap();
            store.add_message(inbox, third.as_bytes(), Flags::default(), None).unwrap();
        }
        let core = Core::start_with(Config { data_dir: Some(data.clone()), downloads_dir: None }, |_| {}).unwrap();
        let Response::Folders { folders } = core.handle(Command::ListFolders).unwrap() else { panic!() };
        let inbox = folders.iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
        let Response::Messages { rows, .. } =
            core.handle(Command::ListMessages { folder: inbox.clone(), offset: 0, limit: 10, conversations: false }).unwrap()
        else {
            panic!()
        };
        let Response::Thread { thread } = core.handle(Command::OpenThread { folder: inbox, id: rows[0].id }).unwrap() else {
            panic!()
        };
        // The inbox as conversations: one row for the three messages, shown
        // by its newest message there, with the user's own reply counted.
        let inbox_id = folders.iter().find(|f| f.role.as_deref() == Some("inbox")).unwrap().id.clone();
        let Response::Messages { rows: conversations, total, .. } = core
            .handle(Command::ListMessages { folder: inbox_id.clone(), offset: 0, limit: 10, conversations: true })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!((total, conversations.len()), (1, 1));
        assert_eq!(conversations[0].subject, "Re: Cutover window");
        assert_eq!((conversations[0].thread_count, conversations[0].thread_unread), (3, 1));
        assert_eq!(conversations[0].senders, "Dana, Ben");
        let Response::FolderLayout { total, .. } =
            core.handle(Command::FolderLayout { folder: inbox_id.clone(), conversations: true }).unwrap()
        else {
            panic!()
        };
        assert_eq!(total, 1);
        let Response::Located { index } = core
            .handle(Command::LocateMessage { folder: inbox_id, id: rows[1].id, conversations: true })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(index, Some(0));
        assert_eq!(thread.subject, "Cutover window");
        let m = &thread.messages;
        assert_eq!(m.len(), 3);
        // Each reduced to what its sender wrote.
        assert_eq!(m[0].text, "Proposing Saturday night.");
        assert_eq!(m[1].text, "Saturday works. Adding Priya.");
        assert_eq!(m[2].text, "Confirmed.");
        assert_eq!(m[1].quoted_lines, 3);
        assert!(m[1].quoted.contains("> Proposing Saturday night."));
        // Dana's closing lines have no separator; repetition gives them away.
        assert_eq!(m[0].signature, "Kind regards,\nDana Dubois\nThis email is confidential.");
        assert_eq!(m[2].signature, m[0].signature);
        assert_eq!(m[1].signature, "");
        // The tree, across folders.
        assert_eq!((m[0].parent, m[1].parent, m[2].parent), (None, Some(m[0].id), Some(m[1].id)));
        assert_eq!((m[0].folder_name.as_str(), m[1].folder_name.as_str()), ("Inbox", "Sent"));
        assert!(m[2].unread && !m[0].unread);
        // Who joined and who was left out.
        assert_eq!((m[1].added.clone(), m[1].dropped.clone()), (vec!["Priya".to_string()], Vec::<String>::new()));
        assert_eq!((m[2].added.clone(), m[2].dropped.clone()), (Vec::<String>::new(), vec!["Sam".to_string()]));
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
        let core = Core::start_with(Config { data_dir: Some(data.clone()), downloads_dir: None }, |_| {}).unwrap();
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
