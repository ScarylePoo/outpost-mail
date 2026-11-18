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

/// Whether the tray icon could be created. Without one, the window is never
/// hidden: there would be no way to bring it back.
struct Tray(bool);

const TRAY_ID: &str = "main";

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn general(app: &tauri::AppHandle) -> Option<outpost_core::settings::General> {
    match app.try_state::<Core>()?.handle(Command::GetGeneral) {
        Ok(Response::General { general }) => Some(general),
        _ => None,
    }
}

/// The tray icon: a click opens the window, and its menu can quit the app.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let open = MenuItem::with_id(app, "open", "Open Outpost", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Outpost")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Shows how much unread mail there is on the tray icon.
#[tauri::command]
fn set_unread(app: tauri::AppHandle, count: u32) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let text = match count {
        0 => "Outpost".to_string(),
        1 => "Outpost: 1 unread message".to_string(),
        n => format!("Outpost: {n} unread messages"),
    };
    let _ = tray.set_tooltip(Some(&text));
    // Where the tray can show text beside the icon (Linux, macOS), the
    // count goes there too.
    #[cfg(not(windows))]
    let _ = tray.set_title(if count == 0 { None } else { Some(count.to_string()) });
}

/// A system notification. The interface decides when; the wording comes
/// from mail, so it is cut to a sensible length here.
#[tauri::command]
fn notify(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    let short = |text: String, most: usize| text.chars().filter(|c| !c.is_control()).take(most).collect::<String>();
    app.notification()
        .builder()
        .title(short(title, 80))
        .body(short(body, 200))
        .show()
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .on_window_event(|window, event| {
            // With "keep running in the tray" on, closing the window hides
            // it and mail keeps arriving.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let has_tray = app.try_state::<Tray>().is_some_and(|t| t.0);
                if has_tray && general(app).is_some_and(|g| g.close_to_tray) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
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
            // A desktop without a tray is not an error; the app then simply
            // never hides.
            let tray = match build_tray(app) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("no tray icon: {e}");
                    false
                }
            };
            app.manage(Tray(tray));
            let start_hidden = tray
                && std::env::var_os(SHELL_TEST_VAR).is_none()
                && general(app.handle()).is_some_and(|g| g.start_minimised);
            if !start_hidden {
                show_window(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_command,
            shell_test_requested,
            shell_test_report,
            probe_account,
            open_link,
            reveal_download,
            set_unread,
            notify
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Outpost");
}
