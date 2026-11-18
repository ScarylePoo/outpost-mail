//! Runs Outpost's sync engine from a terminal. See docs/testing.md.
//!
//!   outpost-sync check --host HOST [--port 993] [--plaintext]
//!   outpost-sync sync  --data DIR --account NAME --host HOST --user USER
//!                      [--port 993] [--plaintext] [--once] [--no-verify] [--all-mail]
//!
//! The password is asked for at a hidden prompt (or read from the
//! OUTPOST_PASSWORD environment variable). It is used for this run only and
//! is never written anywhere.
//!
//! In this milestone sync is read-only: it cannot change anything on the
//! server. `sync --once` proves it by comparing every message's flags on the
//! server before and after.

use outpost_core::imap::{Security, Server, Session};
use outpost_core::store::maildir::Flags;
use outpost_core::store::Store;
use outpost_core::sync::{self, Account, Options, Progress};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

#[derive(Default)]
struct Args {
    command: String,
    data: Option<PathBuf>,
    account: Option<String>,
    host: Option<String>,
    user: Option<String>,
    port: u16,
    plaintext: bool,
    once: bool,
    verify: bool,
    all_mail: bool,
}

const USAGE: &str = "usage:
  outpost-sync check --host HOST [--port 993] [--plaintext]
  outpost-sync sync  --data DIR --account NAME --host HOST --user USER
                     [--port 993] [--plaintext] [--once] [--no-verify] [--all-mail]";

fn parse_args() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let mut args = Args {
        command: raw.next().ok_or("missing command")?,
        port: 993,
        verify: true,
        ..Args::default()
    };
    let mut port_given = false;
    while let Some(flag) = raw.next() {
        let mut value = |name: &str| raw.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--data" => args.data = Some(value("--data")?.into()),
            "--account" => args.account = Some(value("--account")?),
            "--host" => args.host = Some(value("--host")?),
            "--user" => args.user = Some(value("--user")?),
            "--port" => {
                args.port = value("--port")?.parse().map_err(|_| "--port needs a number")?;
                port_given = true;
            }
            "--plaintext" => args.plaintext = true,
            "--once" => args.once = true,
            "--no-verify" => args.verify = false,
            "--all-mail" => args.all_mail = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if args.plaintext && !port_given {
        args.port = 143;
    }
    Ok(args)
}

fn server(args: &Args) -> Result<Server, String> {
    Ok(Server {
        host: args.host.clone().ok_or("--host is required")?,
        port: args.port,
        security: if args.plaintext { Security::Plaintext } else { Security::Tls },
        trusted_certificate: None,
    })
}

fn status(text: &str) {
    eprint!("\r{text:<78}");
    let _ = std::io::stderr().flush();
}

async fn check(args: &Args) -> Result<(), String> {
    let server = server(args)?;
    let capabilities = Session::probe(&server).await.map_err(|e| e.to_string())?;
    println!(
        "Connected to {}:{} ({}).",
        server.host,
        server.port,
        if args.plaintext { "NOT encrypted" } else { "TLS, certificate accepted" }
    );
    println!("Before sign-in the server advertises:\n  {}", capabilities.join(" "));
    let wanted = ["IDLE", "CONDSTORE", "QRESYNC", "MOVE", "UIDPLUS", "SPECIAL-USE", "ENABLE"];
    let missing: Vec<&str> = wanted.iter().copied().filter(|c| !capabilities.iter().any(|x| x == c)).collect();
    if !missing.is_empty() {
        println!("Not advertised yet (servers often add more after sign-in): {}", missing.join(" "));
    }
    Ok(())
}

/// Every message's flags in every folder, as the server has them right now.
async fn server_flags(server: &Server, user: &str, password: &str) -> Result<BTreeMap<(String, u32), Flags>, String> {
    let e = |e: outpost_core::imap::ImapError| e.to_string();
    let mut session = Session::connect(server, user, password).await.map_err(e)?;
    let mut all = BTreeMap::new();
    for folder in session.list().await.map_err(e)? {
        if !folder.selectable {
            continue;
        }
        session.examine(&folder.raw_name, None).await.map_err(e)?;
        for state in session.fetch_flags(1, None).await.map_err(e)? {
            all.insert((folder.name.clone(), state.uid), state.flags);
        }
    }
    session.logout().await;
    Ok(all)
}

async fn run_sync(args: &Args) -> Result<(), String> {
    let server = server(args)?;
    let user = args.user.clone().ok_or("--user is required")?;
    let data = args.data.clone().ok_or("--data is required")?;
    let name = args.account.clone().ok_or("--account is required")?;
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return Err("--account must be a plain name".into());
    }
    let password = match std::env::var("OUTPOST_PASSWORD") {
        Ok(p) => p,
        Err(_) => rpassword::prompt_password(format!("Password for {user} on {}: ", server.host))
            .map_err(|e| format!("could not read the password: {e}"))?,
    };
    let store = Store::open(data.join("accounts").join(&name)).map_err(|e| e.to_string())?;
    let account = Account { server: server.clone(), username: user.clone() };
    let options = Options { include_all_mail: args.all_mail, ..Options::default() };
    let show = |p: Progress| match p {
        Progress::Connecting => status("Connecting…"),
        Progress::Folders { count } => status(&format!("{count} folders")),
        Progress::Headers { folder, done, total } => status(&format!("{folder}: message list {done} of {total}")),
        Progress::Bodies { folder, done, total } => status(&format!("{folder}: downloading {done} of {total}")),
        Progress::Idle => status("Up to date. Waiting for new mail (Ctrl-C to stop)."),
        Progress::Offline { error, retry_in } => {
            status("");
            eprintln!("\rOffline: {error}. Trying again in {} s.", retry_in.as_secs());
        }
        Progress::SignInFailed { error } => {
            status("");
            eprintln!("\rSign-in failed: {error}");
        }
        Progress::FolderChanged { .. } => {}
    };

    if !args.once {
        let (handle, stop) = sync::stop_signal();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            handle.stop();
        });
        let now = tokio::sync::Notify::new();
        sync::run(&store, &account, &password, &options, stop, &now, show).await;
        status("");
        eprintln!("\rStopped.");
        return Ok(());
    }

    let before = if args.verify {
        status("Recording the server's flags before syncing…");
        Some(server_flags(&server, &user, &password).await?)
    } else {
        None
    };
    let started = Instant::now();
    let (_handle, stop) = sync::stop_signal();
    let summary = sync::sync_once(&store, &account, &password, &options, &stop, show)
        .await
        .map_err(|e| e.to_string())?;
    let elapsed = started.elapsed();
    status("");
    println!("\rSync finished in {:.0} s.", elapsed.as_secs_f64());
    println!(
        "  {} folders; {} message list entries fetched, {} bodies downloaded, {} removed, {} flag changes, {} folders renumbered",
        summary.folders, summary.headers_fetched, summary.bodies_fetched, summary.removed,
        summary.flags_changed, summary.folders_reset
    );
    for folder in store.index().folders().map_err(|e| e.to_string())? {
        println!("  {:>8} messages, {:>6} unread  {}", folder.total, folder.unread, folder.path);
    }
    println!("  still to download: {}", store.index().missing_body_count().map_err(|e| e.to_string())?);

    if let Some(before) = before {
        status("Checking that nothing on the server changed…");
        let after = server_flags(&server, &user, &password).await?;
        status("");
        let changed: Vec<_> = before.iter().filter(|(k, v)| after.get(*k).is_some_and(|a| a != *v)).collect();
        let vanished = before.keys().filter(|k| !after.contains_key(*k)).count();
        let arrived = after.keys().filter(|k| !before.contains_key(*k)).count();
        if changed.is_empty() && vanished == 0 {
            println!("\rRead-only check passed: all {} messages on the server have the same flags as before the sync.", before.len());
        } else {
            println!("\rRead-only check: {} messages changed flags and {} disappeared during the sync.", changed.len(), vanished);
            println!("  Outpost does not send commands that can do this; another mail client or the server's own rules are the likely cause. The first few:");
            for ((folder, uid), was) in changed.iter().take(10) {
                println!("    {folder} UID {uid}: {} -> {}", was.letters(), after[&(folder.clone(), *uid)].letters());
            }
        }
        if arrived > 0 {
            println!("  ({arrived} new messages arrived while syncing.)");
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match args.command.as_str() {
        "check" => check(&args).await,
        "sync" => run_sync(&args).await,
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\n{e}");
            ExitCode::FAILURE
        }
    }
}
