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
fn core_command(app: tauri::AppHandle, command: Command) -> Result<Response, CoreError> {
    // The core is missing only if it could not start, in which case a
    // message saying why is on screen and the app is about to close.
    match app.try_state::<Core>() {
        Some(core) => core.handle(command),
        None => Err(CoreError::new(outpost_core::api::ErrorCode::Internal, "Outpost could not start")),
    }
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

/// Where this copy keeps its mail, settings and log.
struct DataDir(std::path::PathBuf);

/// Shows the log file in the file manager.
#[tauri::command]
fn reveal_log(data: State<'_, DataDir>) -> Result<(), String> {
    let path = outpost_core::log::path_in(&data.0);
    if !path.is_file() {
        return Err("Nothing has been logged yet.".into());
    }
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}

/// A `mailto:` address the app was started with: someone clicked an email
/// link somewhere and Outpost is the mail program. Taken once by the
/// interface, which opens a new message to it.
struct Launch(std::sync::Mutex<Option<String>>);

const MAILTO_EVENT: &str = "mailto";

fn mailto_in(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter()
        .skip(1)
        .find(|a| a.len() > 7 && a[..7].eq_ignore_ascii_case("mailto:") && a.len() < 16 * 1024)
}

#[tauri::command]
fn launch_mailto(launch: State<'_, Launch>) -> Option<String> {
    launch.0.lock().ok()?.take()
}

/// Starts the system's way of choosing the default mail program, and says
/// what happened. Outpost cannot simply make itself the default on Windows:
/// only the user can, in Settings. It can on a Linux desktop.
#[tauri::command(async)]
fn choose_default_mail_app() -> Result<String, String> {
    #[cfg(windows)]
    {
        // Opens Settings at Outpost's own page where Windows supports
        // that, and at the list of default apps otherwise.
        tauri_plugin_opener::open_url("ms-settings:defaultapps?registeredAppUser=Outpost%20Mail", None::<&str>)
            .map_err(|e| e.to_string())?;
        Ok("Windows Settings has opened. Find Outpost Mail there and choose it for MAILTO. \
            It is listed only if Outpost was installed with its installer."
            .into())
    }
    #[cfg(not(windows))]
    {
        // The name of the desktop entry the .deb and .rpm packages install.
        let done = std::process::Command::new("xdg-settings")
            .args(["set", "default-url-scheme-handler", "mailto", "Outpost Mail.desktop"])
            .status();
        match done {
            Ok(status) if status.success() => Ok("Outpost is now the program that opens email links.".into()),
            Ok(_) => Err("The desktop did not accept the change. This works when Outpost was installed from its \
                          .deb or .rpm package; otherwise choose it in your desktop's default applications."
                .into()),
            Err(_) => Err("This desktop has no xdg-settings command. Choose Outpost in its default applications settings.".into()),
        }
    }
}

/// Scales everything in the window: the "text size" setting. Zooming the
/// whole view keeps every measurement in the interface in step.
#[tauri::command]
fn set_zoom(window: tauri::WebviewWindow, factor: f64) -> Result<(), String> {
    let factor = if factor.is_finite() { factor.clamp(0.8, 2.0) } else { 1.0 };
    window.set_zoom(factor).map_err(|e| e.to_string())
}

fn main() {
    let mut builder = tauri::Builder::default();
    // One copy of the app at a time: starting it again brings the running
    // window forward (and hands over an email link, if it was started by
    // one). A copy pointed at its own data folder with OUTPOST_DATA is
    // deliberately separate; the core still refuses two on the same folder.
    if std::env::var_os("OUTPOST_DATA").is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            show_window(app);
            if let Some(link) = mailto_in(args) {
                let _ = app.emit(MAILTO_EVENT, link);
            }
        }));
    }
    builder
        .plugin(tauri_plugin_dialog::init())
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
            app.manage(DataDir(data_dir.clone()));
            app.manage(Launch(std::sync::Mutex::new(mailto_in(std::env::args()))));
            // A crash leaves a line in the log saying where.
            let log_dir = data_dir.clone();
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                outpost_core::log::Log::open(Some(&log_dir)).line(&format!("PANIC: {info}"));
                previous(info);
            }));
            let config = Config {
                data_dir: Some(data_dir),
                downloads_dir,
            };
            let core = match Core::start_with(config, move |event: Event| {
                let _ = handle.emit(CORE_EVENT, event);
            }) {
                Ok(core) => core,
                Err(e) => {
                    // Say why, then close. Without this the app would
                    // simply not appear.
                    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
                    let handle = app.handle().clone();
                    app.dialog()
                        .message(format!("Outpost could not start.\n\n{}", e.message))
                        .title("Outpost")
                        .kind(MessageDialogKind::Error)
                        .show(move |_| handle.exit(1));
                    return Ok(());
                }
            };
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
            notify,
            set_zoom,
            reveal_log,
            choose_default_mail_app,
            launch_mailto
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Outpost");
}
