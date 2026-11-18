//! Performance test tool for Outpost's store and index. See docs/testing.md.
//!
//!   outpost-bench generate --data DIR [--messages N] [--seed N] [--no-attachments]
//!   outpost-bench rebuild  --data DIR [--no-optimize]
//!   outpost-bench measure  --data DIR [--seed N]
//!   outpost-bench all      --data DIR [--messages N] ...
//!
//! DIR is an account directory: mail goes in DIR/mail and the index in
//! DIR/index.sqlite.

mod generate;
mod measure;

use outpost_core::store::rebuild::Progress;
use outpost_core::store::Store;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

struct Args {
    command: String,
    data: PathBuf,
    messages: u64,
    seed: u64,
    attachments: bool,
    optimize: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let command = raw.next().ok_or("missing command")?;
    let mut args = Args {
        command,
        data: PathBuf::new(),
        messages: 100_000,
        seed: 1,
        attachments: true,
        optimize: true,
    };
    while let Some(flag) = raw.next() {
        let mut value = |name: &str| raw.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--data" => args.data = PathBuf::from(value("--data")?),
            "--messages" => {
                args.messages = value("--messages")?
                    .replace(['_', ','], "")
                    .parse()
                    .map_err(|_| "--messages needs a number")?
            }
            "--seed" => args.seed = value("--seed")?.parse().map_err(|_| "--seed needs a number")?,
            "--no-attachments" => args.attachments = false,
            "--no-optimize" => args.optimize = false,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if args.data.as_os_str().is_empty() {
        return Err("--data DIR is required".into());
    }
    Ok(args)
}

fn status(text: &str) {
    eprint!("\r{text:<70}");
    let _ = std::io::stderr().flush();
}

fn generate(args: &Args) -> Result<(), String> {
    let mail = args.data.join("mail");
    if mail.exists() && std::fs::read_dir(&mail).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!(
            "{} already has mail in it. Use an empty directory.",
            mail.display()
        ));
    }
    let options = generate::Options {
        messages: args.messages,
        seed: args.seed,
        attachments: args.attachments,
    };
    let summary = generate::generate(&args.data, &options, |done, total| {
        status(&format!("Writing messages: {done} of {total}"));
    })
    .map_err(|e| format!("could not write the mailbox: {e}"))?;
    status("");
    let secs = summary.elapsed.as_secs_f64();
    println!("\rGenerated mailbox (seed {})", args.seed);
    println!(
        "  {} messages, {:.2} GB, in {:.0} s ({:.0} messages/s)",
        summary.messages,
        summary.bytes as f64 / 1e9,
        secs,
        summary.messages as f64 / secs
    );
    for (name, count) in &summary.per_folder {
        println!("  {count:>9}  {name}");
    }
    Ok(())
}

fn rebuild(args: &Args) -> Result<(), String> {
    let store = Store::open(&args.data).map_err(|e| e.to_string())?;
    let report = store
        .rebuild(args.optimize, |p| match p {
            Progress::Listing { folders_done, folders } => {
                status(&format!("Listing folders: {folders_done} of {folders}"))
            }
            Progress::Loading { done, total } => status(&format!("Indexing: {done} of {total}")),
            Progress::Finishing => status("Building indexes…"),
        })
        .map_err(|e| e.to_string())?;
    status("");
    let total = report.listing + report.loading + report.finishing;
    println!("\rRebuilt index");
    println!(
        "  {} messages in {} folders, {:.2} GB of mail, {} unreadable",
        report.messages,
        report.folders,
        report.bytes as f64 / 1e9,
        report.unreadable
    );
    println!(
        "  total {:.0} s: listing {:.1} s, reading and indexing {:.0} s ({:.0} messages/s), finishing {:.0} s",
        total.as_secs_f64(),
        report.listing.as_secs_f64(),
        report.loading.as_secs_f64(),
        report.messages as f64 / report.loading.as_secs_f64().max(0.001),
        report.finishing.as_secs_f64()
    );
    if let Some((folder, files, time)) = &report.slowest_listing {
        println!(
            "  slowest directory listing: {folder}, {files} files in {:.2} s",
            time.as_secs_f64()
        );
    }
    let size = std::fs::metadata(store.index().path()).map(|m| m.len()).unwrap_or(0);
    println!(
        "  index file: {:.2} GB ({:.1}% of the mail)",
        size as f64 / 1e9,
        100.0 * size as f64 / report.bytes.max(1) as f64
    );
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n\nusage: outpost-bench <generate|rebuild|measure|all> --data DIR [--messages N] [--seed N] [--no-attachments] [--no-optimize]");
            return ExitCode::from(2);
        }
    };
    let result = match args.command.as_str() {
        "generate" => generate(&args),
        "rebuild" => rebuild(&args),
        "measure" => measure::measure(&args.data, args.seed).map(|_| ()),
        "all" => generate(&args)
            .and_then(|_| rebuild(&args))
            .and_then(|_| measure::measure(&args.data, args.seed).map(|_| ())),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\n{e}");
            ExitCode::FAILURE
        }
    }
}
