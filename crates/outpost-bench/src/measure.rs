//! Times the operations the handoff doc sets targets for, against an account
//! directory that has been generated and indexed.
//!
//! Each operation is run once ("first": includes whatever was not yet in
//! memory in this process) and then 15 more times ("typical": the median).
//! Nothing here can empty the operating system's file cache, so "first" is
//! only truly cold straight after a reboot.

use crate::generate::{Rng, World};
use outpost_core::store::Store;
use std::path::Path;
use std::time::{Duration, Instant};

struct Line {
    name: String,
    detail: String,
    first: Duration,
    typical: Duration,
    target: Option<Duration>,
}

fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> (Duration, Duration, T) {
    let started = Instant::now();
    let mut result = f();
    let first = started.elapsed();
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        result = f();
        times.push(started.elapsed());
    }
    times.sort();
    (first, times[times.len() / 2], result)
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// Returns whether every target was met.
pub fn measure(account_dir: &Path, seed: u64) -> Result<bool, String> {
    let err = |e: outpost_core::store::StoreError| e.to_string();
    let mut lines: Vec<Line> = Vec::new();
    let ms_target = |n: u64| Some(Duration::from_millis(n));

    // Opening the account: must not depend on mailbox size.
    let started = Instant::now();
    let store = Store::open(account_dir).map_err(err)?;
    let folders = store.index().folders().map_err(err)?;
    let open_time = started.elapsed();
    let index = store.index();
    let total = index.message_count().map_err(err)?;
    if total == 0 {
        return Err("the index is empty: run `rebuild` first".into());
    }
    lines.push(Line {
        name: "Open the account and list folders".into(),
        detail: format!("{} folders", folders.len()),
        first: open_time,
        typical: time(15, || index.folders().unwrap()).1,
        target: None,
    });

    // Opening a folder: everything the list needs before it can draw.
    let largest = folders.iter().max_by_key(|f| f.total).unwrap();
    let inbox = folders.iter().find(|f| f.path == "Inbox").unwrap_or(largest);
    for folder in [inbox, largest] {
        let (first, typical, _) = time(15, || {
            let lens = index.preview_lens(folder.id).unwrap();
            let page = index.page(folder.id, 0, 200).unwrap();
            (lens.len(), page.len())
        });
        lines.push(Line {
            name: format!("Open folder {}", folder.path),
            detail: format!("{} messages: layout + first 200 rows", folder.total),
            first,
            typical,
            target: ms_target(100),
        });
        if std::ptr::eq(inbox, largest) {
            break;
        }
    }

    // The same folders as conversations: one row per conversation.
    for folder in [inbox, largest] {
        let (first, typical, (rows, _)) = time(15, || {
            let lens = index.conversation_lens(folder.id).unwrap();
            let page = index.conversation_page(folder.id, 0, 200).unwrap();
            (lens.len(), page.len())
        });
        lines.push(Line {
            name: format!("Open {} as conversations", folder.path),
            detail: format!("{rows} conversations: layout + first 200 rows"),
            first,
            typical,
            target: ms_target(100),
        });
        if std::ptr::eq(inbox, largest) {
            break;
        }
    }

    // Scrolling: pages from the middle and the far end.
    for (label, offset) in [("middle", largest.total / 2), ("end", largest.total.saturating_sub(200))] {
        let (first, typical, _) = time(15, || index.page(largest.id, offset, 200).unwrap().len());
        lines.push(Line {
            name: format!("Load a page at the {label} of {}", largest.path),
            detail: format!("200 rows at offset {offset}"),
            first,
            typical,
            target: ms_target(16),
        });
    }

    // Opening messages: locate and read the file.
    let page = index.page(largest.id, largest.total / 3, 200).map_err(err)?;
    let mut rng = Rng::new(seed, 99);
    let mut pick = || page[rng.below(page.len() as u64) as usize].id;
    let (first, typical, _) = time(50, || store.read_message(pick()).unwrap().len());
    lines.push(Line {
        name: "Read one message file".into(),
        detail: "locate in index + read from disk".into(),
        first,
        typical,
        target: ms_target(100),
    });

    // Search. The generator's vocabulary is ranked by how often each word is
    // used, so words can be picked by how many messages they are in.
    let world = World::new(seed);
    let very_common = world.word_at(0).to_string();
    let common = world.word_at(60).to_string();
    let medium = world.word_at(1_500).to_string();
    let rare = world.word_at(25_000).to_string();
    let prefix: String = medium.chars().take(3).collect();
    let searches: Vec<(&str, String, Option<i64>)> = vec![
        ("in nearly every message", format!("{very_common} "), None),
        ("common word", format!("{common} "), None),
        ("ordinary word", format!("{medium} "), None),
        ("rare word", format!("{rare} "), None),
        ("two common words", format!("{common} {} ", world.word_at(80)), None),
        ("common + rare word", format!("{common} {rare} "), None),
        ("3-letter prefix, as typed", prefix, None),
        ("a person's name", "Bergström ".to_string(), None),
        ("common word, one folder", format!("{common} "), Some(inbox.id)),
        ("no matches", "zzzxqj ".to_string(), None),
    ];
    for (label, query, folder) in &searches {
        let (first, typical, found) = time(15, || index.search(query, *folder, 50).unwrap().len());
        lines.push(Line {
            name: format!("Search: {label}"),
            detail: format!("\"{}\" -> first {found}", query.trim()),
            first,
            typical,
            target: ms_target(300),
        });
    }

    // Report.
    let index_size = std::fs::metadata(index.path()).map(|m| m.len()).unwrap_or(0);
    println!("\nOutpost store and index measurements");
    println!(
        "  {} messages, index file {:.2} GB, {} {}, {} CPUs",
        total,
        index_size as f64 / 1e9,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
    println!();
    println!("  {:<44} {:>11} {:>11} {:>9}  ", "operation", "first", "typical", "target");
    let mut all_met = true;
    for line in &lines {
        let (target, verdict) = match line.target {
            Some(t) => {
                let met = line.first <= t && line.typical <= t;
                all_met &= met;
                (format!("{} ms", t.as_millis()), if met { "ok" } else { "MISSED" })
            }
            None => ("-".into(), ""),
        };
        println!(
            "  {:<44} {:>11} {:>11} {:>9}  {:<6} {}",
            line.name,
            ms(line.first),
            ms(line.typical),
            target,
            verdict,
            line.detail
        );
    }
    println!();
    println!(
        "  {}",
        if all_met { "All targets met." } else { "Some targets were MISSED." }
    );
    Ok(all_met)
}
