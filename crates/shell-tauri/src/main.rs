//! The Tauri adapter: connects the core's command-and-event interface to the
//! webview and does nothing else. If the shell changes to Electron, this crate
//! is the only Rust that is replaced.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use outpost_core::api::{Command, CoreError, Event, Response};
use outpost_core::{Config, Core};
use tauri::{Emitter, Manager, State};

/// Name of the single webview event that carries every core [`Event`].
const CORE_EVENT: &str = "core-event";

// `async` here means "run off the main thread", so a command can never stall
// the window.
#[tauri::command(async)]
fn core_command(core: State<'_, Core>, command: Command) -> Result<Response, CoreError> {
    core.handle(command)
}

/// Shell test support (docs/testing.md). When OUTPOST_SHELL_TEST names a file,
/// the interface runs the scroll test on startup, the report is written to
/// that file and the app exits. Otherwise this does nothing.
const SHELL_TEST_VAR: &str = "OUTPOST_SHELL_TEST";

#[tauri::command]
fn shell_test_requested() -> bool {
    std::env::var_os(SHELL_TEST_VAR).is_some()
}

#[tauri::command]
fn shell_test_report(app: tauri::AppHandle, report: String) {
    if let Some(path) = std::env::var_os(SHELL_TEST_VAR) {
        // "READY" is a progress marker from the app self-test: the test
        // driver waits for it, and the app carries on.
        let finished = report != "READY";
        if let Err(e) = std::fs::write(&path, report) {
            eprintln!("could not write shell test report: {e}");
            app.exit(1);
            return;
        }
        if finished {
            app.exit(0);
        }
    }
}

/// The folder attachments are saved to, if the system has one.
struct Downloads(Option<std::path::PathBuf>);

/// Opens a link from a message in the user's browser or mail program. Only
/// web and mail links: a message must not be able to open a file or start a
/// program by way of an unusual address.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    let lower = url.trim_start().to_ascii_lowercase();
    if !["http://", "https://", "mailto:"].iter().any(|scheme| lower.starts_with(scheme)) {
        return Err("Only web and email links can be opened.".into());
    }
    tauri_plugin_opener::open_url(url.trim(), None::<&str>).map_err(|e| e.to_string())
}

/// Shows a saved attachment in the file manager. It is shown, not opened,
/// and only if it is inside the downloads folder.
#[tauri::command]
fn reveal_download(downloads: State<'_, Downloads>, path: String) -> Result<(), String> {
    let path = std::path::PathBuf::from(path);
    let inside = downloads.0.as_ref().is_some_and(|dir| path.parent() == Some(dir.as_path()));
    if !inside || !path.is_file() {
        return Err("That file is not in the downloads folder.".into());
    }
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

/// App self-test support (docs/testing.md). When OUTPOST_PROBE_ACCOUNT holds
/// account details as JSON, the interface adds that account, watches what
/// happens for a few seconds and reports what the window shows through
/// `shell_test_report`. Only ever pointed at a local test server.
#[tauri::command]
fn probe_account() -> Option<String> {
    std::env::var("OUTPOST_PROBE_ACCOUNT").ok()
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            // Mail lives in the per-user application data folder unless
            // OUTPOST_DATA points somewhere else.
            let data_dir = match std::env::var_os("OUTPOST_DATA") {
                Some(dir) => dir.into(),
                None => app.path().app_data_dir()?,
            };
            let downloads_dir = app.path().download_dir().ok();
            app.manage(Downloads(downloads_dir.clone()));
            let config = Config {
                data_dir: Some(data_dir),
                downloads_dir,
            };
            let core = Core::start_with(config, move |event: Event| {
                let _ = handle.emit(CORE_EVENT, event);
            })?;
            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_command,
            shell_test_requested,
            shell_test_report,
            probe_account,
            open_link,
            reveal_download
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Outpost");
}
