//! Things that go wrong around the core rather than in it: a second copy
//! of the app, a damaged index, a damaged settings file. None of these may
//! lose mail or stop the app from starting.

use outpost_core::api::{Command, ErrorCode, Response};
use outpost_core::store::maildir::Flags;
use outpost_core::store::Store;
use outpost_core::{Config, Core};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("outpost-hardening-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start(data: &PathBuf) -> Result<Core, outpost_core::api::CoreError> {
    Core::start_with(Config { data_dir: Some(data.clone()), downloads_dir: None }, |_| {})
}

fn inbox_count(core: &Core) -> Option<u32> {
    let Response::Folders { folders } = core.handle(Command::ListFolders).unwrap() else { panic!("wrong response") };
    folders.iter().find(|f| f.id.starts_with("acct/home/") && f.name == "Inbox").map(|f| f.total)
}

#[test]
fn a_second_copy_cannot_open_the_same_mail() {
    let data = temp("second");
    let first = start(&data).unwrap();
    let refused = start(&data).err().expect("a second core on the same folder must be refused");
    assert_eq!(refused.code, ErrorCode::AlreadyRunning);
    // The first is unharmed, and a clone of it keeps the folder claimed.
    let clone = first.clone();
    drop(first);
    assert!(matches!(clone.handle(Command::Ping), Ok(Response::Pong { .. })));
    assert_eq!(start(&data).err().map(|e| e.code), Some(ErrorCode::AlreadyRunning));
    // Once the last handle is gone, the folder can be opened again.
    drop(clone);
    let again = start(&data).unwrap();
    drop(again);
    // The log recorded the starts and stops, and nothing else of note.
    let log = std::fs::read_to_string(outpost_core::log::path_in(&data)).unwrap();
    assert_eq!(log.matches("started").count(), 2, "{log}");
    assert_eq!(log.matches("Outpost stopped").count(), 2, "{log}");
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_damaged_index_is_set_aside_and_rebuilt_from_the_mail() {
    let data = temp("index");
    let account = data.join("accounts/home");
    {
        let store = Store::open(&account).unwrap();
        let inbox = store.create_folder(&["Inbox"]).unwrap();
        for n in 0..25 {
            let raw = format!("From: s{n}@example.com\r\nSubject: message {n}\r\nMessage-ID: <m{n}@example.com>\r\n\r\nbody {n}\r\n");
            store.add_message(inbox, raw.as_bytes(), Flags::SEEN, None).unwrap();
        }
    }
    let core = start(&data).unwrap();
    assert_eq!(inbox_count(&core), Some(25));
    drop(core);

    // The index is overwritten with rubbish, as a failing disk might.
    let index = account.join("index.sqlite");
    let _ = std::fs::remove_file(account.join("index.sqlite-wal"));
    let _ = std::fs::remove_file(account.join("index.sqlite-shm"));
    std::fs::write(&index, vec![0x5a; 64 * 1024]).unwrap();

    // The app starts all the same, and reads the mail back in.
    let core = start(&data).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while inbox_count(&core) != Some(25) {
        assert!(Instant::now() < until, "the mail was not read back in: {:?}", inbox_count(&core));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(account.join("index.sqlite.damaged").exists(), "the damaged file is kept for looking at");
    drop(core);

    // A file cut short part way through is handled the same way.
    let whole = std::fs::read(&index).unwrap();
    let _ = std::fs::remove_file(account.join("index.sqlite-wal"));
    let _ = std::fs::remove_file(account.join("index.sqlite-shm"));
    std::fs::write(&index, &whole[..whole.len().min(3000)]).unwrap();
    let core = start(&data).unwrap();
    let until = Instant::now() + Duration::from_secs(30);
    while inbox_count(&core) != Some(25) {
        assert!(Instant::now() < until, "the mail was not read back in after truncation");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(core);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn damaged_settings_fall_back_to_the_protective_defaults() {
    let data = temp("settings");
    std::fs::write(data.join("settings.json"), b"{\"general\": {\"undoSendSeconds\": 5}, \"protec").unwrap();
    std::fs::write(data.join("update-check.json"), b"\x00\x00").unwrap();
    let core = start(&data).unwrap();
    let Response::Protection { protection, password_set } = core.handle(Command::GetProtection).unwrap() else { panic!() };
    assert!(protection.disable_failed && protection.block_remote_images && !password_set);
    let Response::Appearance { appearance } = core.handle(Command::GetAppearance).unwrap() else { panic!() };
    assert_eq!(appearance.text_size, 100);
    // With checking switched off, nothing is asked of GitHub unprompted.
    let Response::General { mut general } = core.handle(Command::GetGeneral).unwrap() else { panic!() };
    general.check_for_updates = false;
    core.handle(Command::SetGeneral { general, password: None }).unwrap();
    let Response::Update { newer, checked, checking } = core.handle(Command::CheckForUpdate { force: false }).unwrap() else {
        panic!()
    };
    assert!(newer.is_none() && !checked && !checking);
    drop(core);
    let _ = std::fs::remove_dir_all(&data);
}
