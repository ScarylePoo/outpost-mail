//! The local log: what the app did and what went wrong, for troubleshooting.
//!
//! It is a plain text file in `<data>/log/`, one line per event, kept to a
//! bounded size: when the file reaches [`MAX_BYTES`] it becomes
//! `outpost.1.log` (and that one `outpost.2.log`), so at most three files
//! exist. Nothing is ever sent anywhere.
//!
//! What goes in: starts and stops, each account's connection state when it
//! changes, errors with the server's wording, panics. What never goes in:
//! passwords, and the subject, sender or text of any message.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const DIR: &str = "log";
const NAME: &str = "outpost.log";
const MAX_BYTES: u64 = 1024 * 1024;
const OLD_FILES: u32 = 2;

pub struct Log {
    /// `None` when there is no data folder: lines are then dropped.
    target: Option<Mutex<Target>>,
}

struct Target {
    path: PathBuf,
    file: Option<File>,
    written: u64,
    /// The last thing logged per subject, so a repeat is not written again.
    last: Vec<(String, String)>,
}

/// Where the log of a data folder is.
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR).join(NAME)
}

impl Log {
    pub fn open(data_dir: Option<&Path>) -> Log {
        let target = data_dir.map(|dir| {
            let path = path_in(dir);
            let _ = std::fs::create_dir_all(dir.join(DIR));
            let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Mutex::new(Target { path, file: None, written, last: Vec::new() })
        });
        Log { target }
    }

    /// Writes one line. Line breaks in `text` are flattened, so one event
    /// is always one line.
    pub fn line(&self, text: &str) {
        let Some(target) = &self.target else { return };
        let mut target = target.lock().unwrap_or_else(|e| e.into_inner());
        target.write(text);
    }

    /// As [`Log::line`], but only if it differs from the last thing said
    /// about `subject`: for states that are reported over and over.
    pub fn change(&self, subject: &str, text: &str) {
        let Some(target) = &self.target else { return };
        let mut target = target.lock().unwrap_or_else(|e| e.into_inner());
        match target.last.iter_mut().find(|(s, _)| s == subject) {
            Some((_, last)) if last == text => return,
            Some((_, last)) => *last = text.to_string(),
            None => target.last.push((subject.to_string(), text.to_string())),
        }
        target.write(&format!("{subject}: {text}"));
    }
}

impl Target {
    fn write(&mut self, text: &str) {
        if self.written >= MAX_BYTES {
            self.rotate();
        }
        if self.file.is_none() {
            self.file = OpenOptions::new().create(true).append(true).open(&self.path).ok();
        }
        let Some(file) = &mut self.file else { return };
        let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(2000).collect();
        let line = format!("{} {flat}\n", chrono::Utc::now().format("%Y-%m-%d %H:%M:%SZ"));
        // One write per line, so lines from two writers never interleave.
        if file.write_all(line.as_bytes()).is_ok() {
            self.written += line.len() as u64;
        }
    }

    fn rotate(&mut self) {
        self.file = None;
        let numbered = |n: u32| self.path.with_file_name(format!("outpost.{n}.log"));
        let _ = std::fs::remove_file(numbered(OLD_FILES));
        for n in (1..OLD_FILES).rev() {
            let _ = std::fs::rename(numbered(n), numbered(n + 1));
        }
        let _ = std::fs::rename(&self.path, numbered(1));
        self.written = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_written_flat_and_repeats_are_dropped() {
        let dir = std::env::temp_dir().join(format!("outpost-log-{}-a", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = Log::open(Some(&dir));
        log.line("started\nsecond line");
        log.change("account a", "offline");
        log.change("account a", "offline");
        log.change("account b", "offline");
        log.change("account a", "up to date");
        let text = std::fs::read_to_string(path_in(&dir)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].ends_with("started second line"));
        assert!(lines[1].ends_with("account a: offline") && lines[3].ends_with("account a: up to date"));
        // No data folder: nothing happens, nothing fails.
        Log::open(None).line("dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_stays_a_bounded_size() {
        let dir = std::env::temp_dir().join(format!("outpost-log-{}-b", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = Log::open(Some(&dir));
        let long = "x".repeat(1000);
        for _ in 0..(4 * MAX_BYTES / 1000) {
            log.line(&long);
        }
        let mut names: Vec<String> =
            std::fs::read_dir(dir.join(DIR)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["outpost.1.log", "outpost.2.log", "outpost.log"]);
        let total: u64 = std::fs::read_dir(dir.join(DIR)).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum();
        assert!(total <= 3 * (MAX_BYTES + 2000), "{total}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
