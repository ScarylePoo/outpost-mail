//! Rebuilds an account's index from its mail files.
//!
//! This is the guarantee that the index is disposable: delete it, or let it
//! be damaged, and everything the mail files can tell us comes back. What
//! does not come back is state that exists only in the index and on the
//! server (sync positions), which the sync engine re-establishes.

use super::index::NewMessage;
use super::maildir::{self, Maildir};
use super::{delivery_time, parse, segment_name, Result, Store};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct RebuildReport {
    pub folders: u32,
    pub messages: u64,
    pub bytes: u64,
    /// Files that could not be read. They are skipped, not fatal.
    pub unreadable: u64,
    /// Files moved from `new` to `cur`.
    pub adopted: u64,
    /// Time spent listing directories.
    pub listing: Duration,
    /// The slowest single directory listing: (folder, files, time).
    pub slowest_listing: Option<(String, usize, Duration)>,
    /// Time spent reading, parsing and inserting.
    pub loading: Duration,
    /// Time spent building indexes and tidying up.
    pub finishing: Duration,
}

#[derive(Debug, Clone, Copy)]
pub enum Progress {
    Listing { folders_done: u32, folders: u32 },
    Loading { done: u64, total: u64 },
    Finishing,
}

struct Job {
    folder: usize,
    name: String,
}

impl Store {
    /// Empties the index and fills it again from the mail files.
    ///
    /// `optimize_search` merges the search index into its most compact form
    /// at the end, which makes searches faster and the rebuild slower.
    pub fn rebuild(
        &self,
        optimize_search: bool,
        mut progress: impl FnMut(Progress),
    ) -> Result<RebuildReport> {
        let mut report = RebuildReport::default();

        // 1. Find the folders and list their files.
        let started = Instant::now();
        let mut paths = Vec::new();
        find_folders(self.mail_root(), "", &mut paths)?;
        paths.sort();
        let mut jobs = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            progress(Progress::Listing {
                folders_done: i as u32,
                folders: paths.len() as u32,
            });
            let dir = Maildir::at(self.mail_root().join(path));
            report.adopted += dir.adopt_new()? as u64;
            let listing = Instant::now();
            let names = dir.file_names()?;
            let took = listing.elapsed();
            if report.slowest_listing.as_ref().is_none_or(|(_, _, t)| took > *t) {
                report.slowest_listing = Some((path.clone(), names.len(), took));
            }
            jobs.extend(names.into_iter().map(|name| Job { folder: i, name }));
        }
        report.folders = paths.len() as u32;
        report.listing = started.elapsed();

        // 2. Read and parse on every core; insert on this thread.
        let started = Instant::now();
        let mut bulk = self.index().begin_bulk()?;
        let mut folder_ids = Vec::with_capacity(paths.len());
        for path in &paths {
            let name = segment_name(path.rsplit('/').next().unwrap_or(path));
            folder_ids.push(bulk.add_folder(path, &name)?);
        }

        let total = jobs.len() as u64;
        let next = AtomicUsize::new(0);
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        let (tx, rx) = mpsc::sync_channel::<(usize, Option<NewMessage>)>(4096);
        let insert_result = std::thread::scope(|scope| -> Result<()> {
            for _ in 0..workers {
                let tx = tx.clone();
                let (jobs, paths, next) = (&jobs, &paths, &next);
                let root = self.mail_root();
                scope.spawn(move || loop {
                    let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let file = root.join(&paths[job.folder]).join("cur").join(&job.name);
                    let message = std::fs::read(file).ok().map(|raw| index_entry(&job.name, &raw));
                    if tx.send((job.folder, message)).is_err() {
                        break;
                    }
                });
            }
            drop(tx);
            let mut done = 0u64;
            for (folder, message) in rx {
                match message {
                    Some(message) => {
                        report.bytes += message.size;
                        report.messages += 1;
                        bulk.add(folder_ids[folder], &message)?;
                    }
                    None => report.unreadable += 1,
                }
                done += 1;
                if done % 5000 == 0 {
                    progress(Progress::Loading { done, total });
                }
            }
            Ok(())
        });
        insert_result?;
        report.loading = started.elapsed();

        // 3. Indexes, counts, statistics.
        progress(Progress::Finishing);
        let started = Instant::now();
        bulk.finish(optimize_search)?;
        report.finishing = started.elapsed();
        Ok(report)
    }
}

/// The index entry for a file found on disk.
fn index_entry(file_name: &str, raw: &[u8]) -> NewMessage {
    let (base, flags) = maildir::split_name(file_name);
    let parsed = parse::parse(raw);
    NewMessage {
        base: base.to_string(),
        file_flags: flags,
        flags,
        uid: maildir::uid_of(base),
        date: parsed.date.unwrap_or_else(|| delivery_time(base)),
        size: raw.len() as u64,
        parsed,
    }
}

/// Collects every Maildir under `dir` as a '/'-separated path relative to
/// the mail root.
fn find_folders(dir: &Path, relative: &str, found: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if matches!(name.as_str(), "cur" | "new" | "tmp") || name.starts_with('.') {
            continue;
        }
        let path = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if Maildir::open(entry.path()).is_some() {
            found.push(path.clone());
        }
        find_folders(&entry.path(), &path, found)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::maildir::Flags;
    use super::super::testing::TempDir;
    use super::*;

    fn mail(n: u32, body: &str) -> Vec<u8> {
        format!(
            "From: Sender {n} <s{n}@example.com>\r\nSubject: message {n}\r\n\
             Date: Mon, 2 Nov 2026 09:{:02}:00 +0000\r\nMessage-ID: <{n}@test>\r\n\r\n{body}\r\n",
            n % 60
        )
        .into_bytes()
    }

    #[test]
    fn a_rebuilt_index_matches_the_original() {
        let tmp = TempDir::new();
        let store = Store::open(tmp.path()).unwrap();
        let inbox = store.create_folder(&["Inbox"]).unwrap();
        let nested = store.create_folder(&["Projects", "Alpha"]).unwrap();
        let mut ids = Vec::new();
        for n in 0..40 {
            let folder = if n % 4 == 0 { nested } else { inbox };
            let flags = if n % 3 == 0 { Flags::SEEN } else { Flags::default() };
            let uid = (n % 2 == 0).then_some(1000 + n);
            ids.push(store.add_message(folder, &mail(n, &format!("body {n} zebra")), flags, uid).unwrap());
        }
        // A flag change that has reached the file, and one that has not.
        store.set_flags(ids[1], Flags::SEEN.with(Flags::FLAGGED)).unwrap();
        store.reconcile_flags(10).unwrap();
        store.set_flags(ids[2], Flags::SEEN).unwrap();

        let snapshot = |store: &Store| {
            let folders = store.index().folders().unwrap();
            let rows: Vec<_> = folders
                .iter()
                .flat_map(|f| store.index().page(f.id, 0, 1000).unwrap())
                .map(|r| (r.subject, r.from_addr, r.date, r.preview, r.flags, r.has_attachment))
                .collect();
            let folders: Vec<_> =
                folders.into_iter().map(|f| (f.path, f.name, f.total, f.unread)).collect();
            (folders, rows)
        };
        let mut before = snapshot(&store);
        // The pending flag change lived only in the index, so the rebuilt
        // index shows the file's flags for that one message.
        let pending = before.1.iter_mut().find(|r| r.0 == "message 2").unwrap();
        assert_eq!(pending.4, Flags::SEEN);
        pending.4 = Flags::default();
        before.0[0].3 += 1;

        let report = store.rebuild(true, |_| {}).unwrap();
        assert_eq!((report.folders, report.messages, report.unreadable), (3, 40, 0));
        assert_eq!(snapshot(&store), before);

        // Search and file lookup work on the rebuilt index.
        assert_eq!(store.index().search("zebra", None, 100).unwrap().len(), 40);
        let found = store.index().search("\"message 7\"", None, 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(store.read_message(found[0].id).unwrap(), mail(7, "body 7 zebra"));

        // Server UIDs come back from the file names.
        let with_uid: u32 = store
            .index()
            .page(inbox, 0, 1000)
            .unwrap()
            .iter()
            .filter(|r| maildir::uid_of(&store.index().locate(r.id).unwrap().base).is_some())
            .count() as u32;
        assert_eq!(with_uid, 10);

        // Rebuilding twice gives the same answer.
        store.rebuild(false, |_| {}).unwrap();
        assert_eq!(snapshot(&store), before);
    }

    #[test]
    fn mail_from_another_program_is_picked_up() {
        let tmp = TempDir::new();
        let store = Store::open(tmp.path()).unwrap();
        // A Maildir Outpost has never seen, with standard and Windows names
        // and an unread file waiting in new/.
        let dir = tmp.path().join("mail/Imported/Old stuff");
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        std::fs::write(dir.join("cur/1700000001.a.host;2,S"), mail(1, "one")).unwrap();
        std::fs::write(dir.join("cur/1700000002.b.host;2,FS"), mail(2, "two")).unwrap();
        std::fs::write(dir.join("new/1700000003.c.host"), b"\r\nno headers here").unwrap();
        std::fs::write(dir.join("tmp/1700000004.d.host"), b"half written").unwrap();

        let report = store.rebuild(false, |_| {}).unwrap();
        assert_eq!((report.folders, report.messages, report.adopted), (1, 3, 1));

        let folder = store.index().folders().unwrap().remove(0);
        assert_eq!((folder.path.as_str(), folder.name.as_str()), ("Imported/Old stuff", "Old stuff"));
        assert_eq!((folder.total, folder.unread), (3, 1));
        let rows = store.index().page(folder.id, 0, 10).unwrap();
        // The headerless file has no Date, so it sorts by its delivery time,
        // which is far older than the others' 2026 dates.
        assert_eq!(rows[2].date, 1_700_000_003);
        assert_eq!(rows[2].preview, "no headers here");
        assert!(rows.iter().any(|r| r.flags == Flags::SEEN.with(Flags::FLAGGED)));
    }
}
