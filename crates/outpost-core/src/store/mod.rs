//! Local storage for one account: mail files plus their index.
//!
//! ```text
//! <account dir>/
//!   mail/                 one directory per folder, nested like the folder tree
//!     Inbox/{cur,new,tmp}
//!     Projects/Alpha/{cur,new,tmp}
//!   index.sqlite          rebuildable from mail/ at any time
//! ```
//!
//! Backing up or moving an account is copying its directory.

pub mod index;
pub mod maildir;
pub mod parse;
pub mod rebuild;

use index::{Index, NewMessage};
use maildir::{decode_segment, encode_segment, Flags, Maildir};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Index(rusqlite::Error),
    /// The index was written by a newer Outpost than this one.
    IndexVersion(i32),
    NoSuchMessage(i64),
    NoSuchFolder(i64),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "file error: {e}"),
            StoreError::Index(e) => write!(f, "index error: {e}"),
            StoreError::IndexVersion(v) => {
                write!(f, "the index is version {v}, which this version of Outpost cannot read")
            }
            StoreError::NoSuchMessage(id) => write!(f, "no message {id}"),
            StoreError::NoSuchFolder(id) => write!(f, "no folder {id}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Index(e)
    }
}

type Result<T> = std::result::Result<T, StoreError>;

/// One account's mail and index.
pub struct Store {
    mail_root: PathBuf,
    index: Index,
}

impl Store {
    /// Opens the account stored in `account_dir`, creating it if it is new.
    /// This never scans the mail directories.
    pub fn open(account_dir: impl AsRef<Path>) -> Result<Store> {
        let account_dir = account_dir.as_ref();
        let mail_root = account_dir.join("mail");
        std::fs::create_dir_all(&mail_root)?;
        Ok(Store {
            mail_root,
            index: Index::open(account_dir.join("index.sqlite"))?,
        })
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn mail_root(&self) -> &Path {
        &self.mail_root
    }

    fn maildir(&self, folder_path: &str) -> Maildir {
        // The path is stored with '/' and holds only encoded segments, so it
        // can be joined as it is on every system.
        Maildir::at(self.mail_root.join(folder_path))
    }

    /// Creates a folder (and any missing parents) on disk and in the index.
    /// `names` is the folder's place in the tree, outermost first.
    pub fn create_folder(&self, names: &[&str]) -> Result<i64> {
        assert!(!names.is_empty(), "a folder needs a name");
        let mut path = String::new();
        let mut id = 0;
        for name in names {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(&encode_segment(name));
            Maildir::create(self.mail_root.join(&path))?;
            id = self.index.ensure_folder(&path, name)?;
        }
        Ok(id)
    }

    /// Stores a message in a folder and indexes it. Returns its id.
    pub fn add_message(
        &self,
        folder_id: i64,
        raw: &[u8],
        flags: Flags,
        uid: Option<u32>,
    ) -> Result<i64> {
        let path = self
            .index
            .folder_path(folder_id)?
            .ok_or(StoreError::NoSuchFolder(folder_id))?;
        let base = self.maildir(&path).deliver(raw, flags, uid, true)?;
        let parsed = parse::parse(raw);
        let message = NewMessage {
            date: parsed.date.unwrap_or_else(|| delivery_time(&base)),
            base,
            file_flags: flags,
            flags,
            uid,
            size: raw.len() as u64,
            parsed,
        };
        Ok(self.index.insert(folder_id, &[message])?[0])
    }

    /// The message exactly as it was stored.
    pub fn read_message(&self, id: i64) -> Result<Vec<u8>> {
        let at = self.index.locate(id)?;
        Ok(self.maildir(&at.folder_path).read(&at.base, at.file_flags)?)
    }

    /// Changes a message's flags. Takes effect at once in the index; the
    /// file is renamed by the next [`Store::reconcile_flags`].
    pub fn set_flags(&self, id: i64, flags: Flags) -> Result<()> {
        self.index.set_flags(id, flags)
    }

    /// Renames up to `limit` files whose names are behind their flags.
    /// Returns how many were renamed. Meant to be called in the background
    /// until it returns 0.
    pub fn reconcile_flags(&self, limit: u32) -> Result<u32> {
        let pending = self.index.pending_renames(limit)?;
        let mut done = 0;
        for p in pending {
            self.maildir(&p.folder_path).rename_flags(&p.base, p.from, p.to)?;
            self.index.renamed(p.id, p.to)?;
            done += 1;
        }
        Ok(done)
    }
}

/// The time in a base name's leading number (when the file was delivered),
/// used as a message's date when it has no usable Date header.
pub(crate) fn delivery_time(base: &str) -> i64 {
    base.split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// The display name of a folder directory.
pub(crate) fn segment_name(segment: &str) -> String {
    decode_segment(segment).unwrap_or_else(|| segment.to_string())
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A directory that is deleted when dropped.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new() -> TempDir {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "outpost-test-{}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .subsec_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TempDir;
    use super::*;

    fn mail(from: &str, subject: &str, date: &str, body: &str) -> Vec<u8> {
        format!(
            "From: {from}\r\nTo: Me <me@example.com>\r\nSubject: {subject}\r\nDate: {date}\r\n\
             Message-ID: <{subject}@test>\r\n\r\n{body}\r\n"
        )
        .into_bytes()
    }

    fn sample() -> (TempDir, Store, i64) {
        let tmp = TempDir::new();
        let store = Store::open(tmp.path()).unwrap();
        let inbox = store.create_folder(&["Inbox"]).unwrap();
        (tmp, store, inbox)
    }

    #[test]
    fn folders_nest_on_disk_like_the_tree() {
        let (tmp, store, inbox) = sample();
        let alpha = store.create_folder(&["Projects", "Alpha"]).unwrap();
        let odd = store.create_folder(&["Projects", "cur"]).unwrap();
        assert_eq!(store.create_folder(&["Inbox"]).unwrap(), inbox, "creating twice is harmless");
        assert!(tmp.path().join("mail/Projects/Alpha/cur").is_dir());
        assert!(tmp.path().join("mail/Projects/%63ur/cur").is_dir());

        let folders = store.index().folders().unwrap();
        let listed: Vec<(&str, &str)> =
            folders.iter().map(|f| (f.path.as_str(), f.name.as_str())).collect();
        assert_eq!(
            listed,
            [
                ("Inbox", "Inbox"),
                ("Projects", "Projects"),
                ("Projects/%63ur", "cur"),
                ("Projects/Alpha", "Alpha"),
            ]
        );
        assert_ne!(alpha, odd);
    }

    #[test]
    fn a_message_is_stored_untouched_and_listed_newest_first() {
        let (_tmp, store, inbox) = sample();
        let old = mail("Alice <a@example.com>", "first", "Mon, 2 Nov 2026 09:00:00 +0000", "one");
        let new = mail("Bob <b@example.com>", "second", "Tue, 3 Nov 2026 09:00:00 +0000", "two");
        let old_id = store.add_message(inbox, &old, Flags::SEEN, Some(1)).unwrap();
        let new_id = store.add_message(inbox, &new, Flags::default(), Some(2)).unwrap();

        assert_eq!(store.read_message(old_id).unwrap(), old);
        assert_eq!(store.read_message(new_id).unwrap(), new);

        let page = store.index().page(inbox, 0, 10).unwrap();
        assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), [new_id, old_id]);
        assert_eq!(page[0].from_name, "Bob");
        assert_eq!(page[0].subject, "second");
        assert_eq!(page[0].preview, "two");
        assert!(!page[0].flags.contains(Flags::SEEN));
        assert_eq!(store.index().page(inbox, 1, 10).unwrap().len(), 1);
        assert_eq!(store.index().preview_lens(inbox).unwrap(), [3, 3]);

        let folder = &store.index().folders().unwrap()[0];
        assert_eq!((folder.total, folder.unread), (2, 1));
    }

    #[test]
    fn flags_change_at_once_and_files_follow() {
        let (_tmp, store, inbox) = sample();
        let raw = mail("a@example.com", "s", "Mon, 2 Nov 2026 09:00:00 +0000", "b");
        let id = store.add_message(inbox, &raw, Flags::default(), None).unwrap();

        let read_and_starred = Flags::SEEN.with(Flags::FLAGGED);
        store.set_flags(id, read_and_starred).unwrap();
        assert_eq!(store.index().page(inbox, 0, 1).unwrap()[0].flags, read_and_starred);
        assert_eq!(store.index().folders().unwrap()[0].unread, 0);
        // Not renamed yet, and still readable.
        assert_eq!(store.index().locate(id).unwrap().file_flags, Flags::default());
        assert_eq!(store.read_message(id).unwrap(), raw);

        assert_eq!(store.reconcile_flags(100).unwrap(), 1);
        assert_eq!(store.reconcile_flags(100).unwrap(), 0);
        let at = store.index().locate(id).unwrap();
        assert_eq!(at.file_flags, read_and_starred);
        let name = maildir::join_name(&at.base, read_and_starred);
        assert!(store.mail_root().join("Inbox/cur").join(name).is_file());
        assert_eq!(store.read_message(id).unwrap(), raw);

        store.set_flags(id, Flags::FLAGGED).unwrap();
        assert_eq!(store.index().folders().unwrap()[0].unread, 1);
        assert!(matches!(store.set_flags(9999, Flags::SEEN), Err(StoreError::NoSuchMessage(9999))));
    }

    #[test]
    fn search_finds_words_in_subject_sender_and_body() {
        let (_tmp, store, inbox) = sample();
        let other = store.create_folder(&["Archive"]).unwrap();
        let date = "Mon, 2 Nov 2026 09:00:00 +0000";
        let a = store
            .add_message(inbox, &mail("Alice Abbott <a@example.com>", "Roof repair quote", date, "The slates need replacing."), Flags::SEEN, None)
            .unwrap();
        let b = store
            .add_message(other, &mail("Bob <b@example.com>", "Dinner", "Tue, 3 Nov 2026 09:00:00 +0000", "Shall we repair to the café afterwards?"), Flags::SEEN, None)
            .unwrap();

        let ids = |text: &str, folder: Option<i64>| -> Vec<i64> {
            store.index().search(text, folder, 50).unwrap().iter().map(|r| r.id).collect()
        };
        assert_eq!(ids("repair", None), [b, a], "newest first, across folders");
        assert_eq!(ids("repair", Some(inbox)), [a]);
        assert_eq!(ids("roof repair", None), [a], "every word must match");
        assert_eq!(ids("slat", None), [a], "the last word matches as a prefix");
        assert_eq!(ids("slat ", None), Vec::<i64>::new(), "but not once it is finished");
        assert_eq!(ids("abbott", None), [a], "sender names are searchable");
        assert_eq!(ids("b@example.com", None), [b], "and addresses");
        assert_eq!(ids("cafe", None), [b], "accents are ignored");
        assert_eq!(ids("ROOF", None), [a], "case is ignored");
        assert_eq!(ids("\"); DROP TABLE message; --", None), Vec::<i64>::new());
        assert_eq!(ids("", None), Vec::<i64>::new());
        assert_eq!(store.index().message_count().unwrap(), 2);
    }

    #[test]
    fn messages_with_the_same_or_impossible_dates_all_list_and_search() {
        let (_tmp, store, inbox) = sample();
        let same = "Mon, 2 Nov 2026 09:00:00 +0000";
        let mut ids = Vec::new();
        for n in 0..5 {
            ids.push(store.add_message(inbox, &mail("a@example.com", &format!("same{n}"), same, "twin"), Flags::SEEN, None).unwrap());
        }
        ids.push(store.add_message(inbox, &mail("a@example.com", "ancient", "Mon, 1 Jan 1900 00:00:00 +0000", "twin"), Flags::SEEN, None).unwrap());
        ids.push(store.add_message(inbox, &mail("a@example.com", "future", "Mon, 1 Jan 9999 00:00:00 +0000", "twin"), Flags::SEEN, None).unwrap());
        ids.push(store.add_message(inbox, b"Subject: undated\r\n\r\ntwin\r\n", Flags::SEEN, None).unwrap());

        let page = store.index().page(inbox, 0, 100).unwrap();
        assert_eq!(page.len(), 8);
        assert_eq!(page[0].subject, "future");
        assert_eq!(page[1].subject, "undated", "no Date header: dated by when it was stored");
        assert_eq!(page[7].subject, "ancient");
        // Equal dates keep the order they were added in, newest first.
        let twins: Vec<&str> = page[2..7].iter().map(|r| r.subject.as_str()).collect();
        assert_eq!(twins, ["same4", "same3", "same2", "same1", "same0"]);

        let found = store.index().search("twin ", None, 100).unwrap();
        assert_eq!(found.iter().map(|r| r.id).collect::<Vec<_>>(), page.iter().map(|r| r.id).collect::<Vec<_>>(), "search order is list order");
        assert_eq!(store.index().search("twin ", None, 3).unwrap().len(), 3);
        for id in ids {
            assert!(store.read_message(id).is_ok());
        }
    }

    #[test]
    fn an_index_from_an_older_version_is_replaced_and_can_be_rebuilt() {
        let tmp = TempDir::new();
        {
            let store = Store::open(tmp.path()).unwrap();
            let inbox = store.create_folder(&["Inbox"]).unwrap();
            store.add_message(inbox, &mail("a@example.com", "kept", "Mon, 2 Nov 2026 09:00:00 +0000", "x"), Flags::default(), None).unwrap();
        }
        {
            let conn = rusqlite::Connection::open(tmp.path().join("index.sqlite")).unwrap();
            conn.execute_batch("ALTER TABLE message DROP COLUMN verdict; PRAGMA user_version = 1").unwrap();
        }
        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(store.index().message_count().unwrap(), 0, "the old index is discarded");
        store.rebuild(false, |_| {}).unwrap();
        let folder = store.index().folders().unwrap().remove(0);
        assert_eq!(store.index().page(folder.id, 0, 10).unwrap()[0].subject, "kept");

        // An index from the future is left alone and refused.
        drop(store);
        let conn = rusqlite::Connection::open(tmp.path().join("index.sqlite")).unwrap();
        conn.execute_batch("PRAGMA user_version = 999").unwrap();
        drop(conn);
        assert!(matches!(Store::open(tmp.path()), Err(StoreError::IndexVersion(999))));
    }

    #[test]
    fn reopening_keeps_everything_and_scans_nothing() {
        let tmp = TempDir::new();
        let id = {
            let store = Store::open(tmp.path()).unwrap();
            let inbox = store.create_folder(&["Inbox"]).unwrap();
            store
                .add_message(inbox, &mail("a@example.com", "kept", "Mon, 2 Nov 2026 09:00:00 +0000", "x"), Flags::default(), None)
                .unwrap()
        };
        // Remove the mail files: opening and listing must not need them.
        std::fs::remove_dir_all(tmp.path().join("mail/Inbox")).unwrap();
        let store = Store::open(tmp.path()).unwrap();
        let folder = store.index().folders().unwrap().remove(0);
        assert_eq!(store.index().page(folder.id, 0, 10).unwrap()[0].id, id);
        assert!(matches!(store.read_message(id), Err(StoreError::Io(_))));
    }
}
