//! Maildir: one file per message, stored exactly as received.
//!
//! A folder is a directory holding `cur`, `new` and `tmp`. A message file is
//! named `<base><sep>2,<flags>`:
//!
//! - `<base>` never changes for the life of the message. Outpost's own bases
//!   look like `1794916800.M000123P4567Q8R9abc.outpost,U=42,S=3141`, where the
//!   optional `,U=` is the server UID and `,S=` is the size in bytes.
//! - `<sep>` is `:` everywhere except Windows, which forbids colons in file
//!   names, so there it is `;`. Both are accepted when reading, on every
//!   system, so a mail folder can be copied between machines.
//! - `<flags>` are the standard letters, in ASCII order.
//!
//! Outpost keeps every message in `cur`. Files another program left in `new`
//! are moved to `cur` when the folder is indexed, as the Maildir convention
//! expects of a mail client.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Message flags, as a bit set. The values are stored in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Flags(pub u8);

impl Flags {
    pub const SEEN: Flags = Flags(1);
    pub const ANSWERED: Flags = Flags(2);
    pub const FLAGGED: Flags = Flags(4);
    pub const DELETED: Flags = Flags(8);
    pub const DRAFT: Flags = Flags(16);
    pub const FORWARDED: Flags = Flags(32);

    /// Maildir letter for each flag, in the ASCII order the format requires.
    const LETTERS: [(char, Flags); 6] = [
        ('D', Flags::DRAFT),
        ('F', Flags::FLAGGED),
        ('P', Flags::FORWARDED),
        ('R', Flags::ANSWERED),
        ('S', Flags::SEEN),
        ('T', Flags::DELETED),
    ];

    pub fn contains(self, other: Flags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn with(self, other: Flags) -> Flags {
        Flags(self.0 | other.0)
    }

    pub fn without(self, other: Flags) -> Flags {
        Flags(self.0 & !other.0)
    }

    /// Parses Maildir flag letters. Unknown letters are ignored.
    pub fn from_letters(letters: &str) -> Flags {
        let mut flags = Flags::default();
        for c in letters.chars() {
            if let Some((_, f)) = Self::LETTERS.iter().find(|(l, _)| *l == c) {
                flags = flags.with(*f);
            }
        }
        flags
    }

    pub fn letters(self) -> String {
        Self::LETTERS
            .iter()
            .filter(|(_, f)| self.contains(*f))
            .map(|(l, _)| *l)
            .collect()
    }
}

/// The separator this system writes between a file's base name and its flags.
pub const INFO_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// Splits a file name into its base and its flags. A name with no flag
/// section (as found in `new`) is all base.
pub fn split_name(file_name: &str) -> (&str, Flags) {
    for sep in [":2,", ";2,"] {
        if let Some(at) = file_name.rfind(sep) {
            return (&file_name[..at], Flags::from_letters(&file_name[at + 3..]));
        }
    }
    (file_name, Flags::default())
}

/// The file name for a base and flags, with this system's separator.
pub fn join_name(base: &str, flags: Flags) -> String {
    format!("{base}{INFO_SEPARATOR}2,{}", flags.letters())
}

/// The server UID recorded in a base name, if any.
pub fn uid_of(base: &str) -> Option<u32> {
    field(base, "U=")
}

/// The message size recorded in a base name, if any.
pub fn size_of(base: &str) -> Option<u64> {
    field(base, "S=")
}

fn field<T: std::str::FromStr>(base: &str, key: &str) -> Option<T> {
    base.split(',')
        .skip(1)
        .find_map(|part| part.strip_prefix(key))
        .and_then(|v| v.parse().ok())
}

fn new_base(uid: Option<u32>, size: usize) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static NONCE: OnceLock<u32> = OnceLock::new();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    // Distinguishes two processes that happen to share a process ID, such as
    // on two machines writing into one synchronised folder.
    let nonce = NONCE.get_or_init(|| {
        let mut x = now.as_nanos() as u64 ^ ((std::process::id() as u64) << 32);
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        (x ^ (x >> 33)) as u32
    });
    let mut base = format!(
        "{}.M{:06}P{}Q{}R{:08x}.outpost",
        now.as_secs(),
        now.subsec_micros(),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed),
        nonce,
    );
    if let Some(uid) = uid {
        base.push_str(&format!(",U={uid}"));
    }
    base.push_str(&format!(",S={size}"));
    base
}

/// One mail folder on disk.
#[derive(Debug, Clone)]
pub struct Maildir {
    path: PathBuf,
}

impl Maildir {
    /// Creates the folder (and any missing parents) if it does not exist.
    pub fn create(path: impl Into<PathBuf>) -> io::Result<Maildir> {
        let path = path.into();
        for sub in ["cur", "new", "tmp"] {
            fs::create_dir_all(path.join(sub))?;
        }
        Ok(Maildir { path })
    }

    /// Opens an existing folder. `None` if `path` is not a Maildir.
    pub fn open(path: impl Into<PathBuf>) -> Option<Maildir> {
        let path = path.into();
        path.join("cur").is_dir().then_some(Maildir { path })
    }

    /// A handle to a folder the caller knows exists. Does not touch the disk.
    pub fn at(path: impl Into<PathBuf>) -> Maildir {
        Maildir { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stores a message and returns its base name. The bytes are written to
    /// `tmp` and then renamed into `cur`, so a reader never sees a partial
    /// file. With `durable`, the file is flushed to the disk before the
    /// rename; use that for mail that exists nowhere else.
    pub fn deliver(
        &self,
        message: &[u8],
        flags: Flags,
        uid: Option<u32>,
        durable: bool,
    ) -> io::Result<String> {
        let base = new_base(uid, message.len());
        let tmp = self.path.join("tmp").join(&base);
        let mut file = File::create(&tmp)?;
        file.write_all(message)?;
        if durable {
            file.sync_all()?;
        }
        drop(file);
        let dest = self.file_path(&base, flags);
        if let Err(e) = fs::rename(&tmp, &dest) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(base)
    }

    /// Where the message with this base and these on-disk flags lives.
    pub fn file_path(&self, base: &str, flags: Flags) -> PathBuf {
        self.path.join("cur").join(join_name(base, flags))
    }

    /// Reads a message. `flags` must be the flags in the file's name.
    pub fn read(&self, base: &str, flags: Flags) -> io::Result<Vec<u8>> {
        fs::read(self.file_path(base, flags))
    }

    /// Renames a message's file to carry new flags. The contents are untouched.
    pub fn rename_flags(&self, base: &str, from: Flags, to: Flags) -> io::Result<()> {
        if from == to {
            return Ok(());
        }
        fs::rename(self.file_path(base, from), self.file_path(base, to))
    }

    pub fn remove(&self, base: &str, flags: Flags) -> io::Result<()> {
        fs::remove_file(self.file_path(base, flags))
    }

    /// Moves everything in `new` to `cur`. Returns how many files moved.
    pub fn adopt_new(&self) -> io::Result<u32> {
        let mut moved = 0;
        for entry in fs::read_dir(self.path.join("new"))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with('.') || !entry.file_type()?.is_file() {
                continue;
            }
            let (base, flags) = split_name(&name);
            fs::rename(entry.path(), self.file_path(base, flags))?;
            moved += 1;
        }
        Ok(moved)
    }

    /// The file names in `cur`, in no particular order.
    pub fn file_names(&self) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(self.path.join("cur"))? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if !name.starts_with('.') {
                    names.push(name.to_owned());
                }
            }
        }
        Ok(names)
    }
}

// ---------------------------------------------------------------------------
// Folder names on disk
// ---------------------------------------------------------------------------

/// Windows device names, which cannot be used as a file or directory name
/// there, with or without an extension.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Turns one level of a folder's name into a directory name that is valid on
/// Windows, Linux and macOS and cannot collide with `cur`, `new` or `tmp`.
/// Ordinary names come through unchanged; anything else is percent-encoded.
/// [`decode_segment`] reverses it exactly.
pub fn encode_segment(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '%') || c < ' ' {
            out.push_str(&format!("%{:02X}", c as u32));
        } else {
            out.push(c);
        }
    }
    let lower = out.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or("");
    let clashes = matches!(lower.as_str(), "cur" | "new" | "tmp")
        || RESERVED.contains(&stem)
        || lower.starts_with('.');
    if clashes {
        // All of these start with an ASCII character, so this is one byte.
        let first = out.remove(0);
        out.insert_str(0, &format!("%{:02X}", first as u32));
    }
    if let Some(last @ ('.' | ' ')) = out.chars().last() {
        out.pop();
        out.push_str(&format!("%{:02X}", last as u32));
    }
    if out.is_empty() {
        out.push_str("%00");
    }
    out
}

/// Reverses [`encode_segment`]. `None` if `segment` is not validly encoded.
pub fn decode_segment(segment: &str) -> Option<String> {
    if segment == "%00" {
        return Some(String::new());
    }
    let mut out = String::with_capacity(segment.len());
    let mut rest = segment;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let hex = rest.get(at + 1..at + 3)?;
        out.push(char::from(u8::from_str_radix(hex, 16).ok()?));
        rest = &rest[at + 3..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::TempDir;

    #[test]
    fn flag_letters_round_trip_in_ascii_order() {
        let all = Flags(0b11_1111);
        assert_eq!(all.letters(), "DFPRST");
        assert_eq!(Flags::from_letters("DFPRST"), all);
        assert_eq!(Flags::from_letters("SR"), Flags::SEEN.with(Flags::ANSWERED));
        assert_eq!(Flags::SEEN.with(Flags::ANSWERED).letters(), "RS");
        assert_eq!(Flags::from_letters("Sxyz"), Flags::SEEN, "unknown letters are ignored");
        assert_eq!(Flags::default().letters(), "");
    }

    #[test]
    fn both_separators_are_read_on_every_system() {
        assert_eq!(split_name("abc,S=10:2,FS"), ("abc,S=10", Flags::SEEN.with(Flags::FLAGGED)));
        assert_eq!(split_name("abc,S=10;2,FS"), ("abc,S=10", Flags::SEEN.with(Flags::FLAGGED)));
        assert_eq!(split_name("abc:2,"), ("abc", Flags::default()));
        assert_eq!(split_name("abc"), ("abc", Flags::default()), "no flag section, as in new/");
        let name = join_name("abc", Flags::SEEN);
        assert_eq!(name, if cfg!(windows) { "abc;2,S" } else { "abc:2,S" });
        assert_eq!(split_name(&name), ("abc", Flags::SEEN));
    }

    #[test]
    fn uid_and_size_are_read_from_the_base() {
        assert_eq!(uid_of("1.M1P1Q1.host,U=42,S=3141"), Some(42));
        assert_eq!(size_of("1.M1P1Q1.host,U=42,S=3141"), Some(3141));
        assert_eq!(uid_of("1.M1P1Q1.host,S=3141"), None);
        assert_eq!(size_of("1.M1P1Q1.host"), None);
    }

    #[test]
    fn deliver_read_reflag_remove() {
        let tmp = TempDir::new();
        let dir = Maildir::create(tmp.path().join("Inbox")).unwrap();
        let body = b"Subject: hi\r\n\r\nhello\r\n";
        let base = dir.deliver(body, Flags::default(), Some(7), true).unwrap();
        assert_eq!(uid_of(&base), Some(7));
        assert_eq!(size_of(&base), Some(body.len() as u64));
        assert_eq!(dir.read(&base, Flags::default()).unwrap(), body);
        assert!(fs::read_dir(tmp.path().join("Inbox/tmp")).unwrap().next().is_none());

        dir.rename_flags(&base, Flags::default(), Flags::SEEN).unwrap();
        assert_eq!(dir.read(&base, Flags::SEEN).unwrap(), body, "contents never change");
        assert!(dir.read(&base, Flags::default()).is_err());
        assert_eq!(dir.file_names().unwrap(), [join_name(&base, Flags::SEEN)]);

        dir.remove(&base, Flags::SEEN).unwrap();
        assert!(dir.file_names().unwrap().is_empty());
    }

    #[test]
    fn bases_are_unique() {
        let tmp = TempDir::new();
        let dir = Maildir::create(tmp.path().join("f")).unwrap();
        let mut bases: Vec<String> = (0..500)
            .map(|_| dir.deliver(b"x", Flags::default(), None, false).unwrap())
            .collect();
        bases.sort();
        bases.dedup();
        assert_eq!(bases.len(), 500);
    }

    #[test]
    fn files_in_new_are_adopted_into_cur() {
        let tmp = TempDir::new();
        let dir = Maildir::create(tmp.path().join("f")).unwrap();
        fs::write(dir.path().join("new/1700000000.1.otherhost"), b"a").unwrap();
        assert_eq!(dir.adopt_new().unwrap(), 1);
        assert_eq!(dir.read("1700000000.1.otherhost", Flags::default()).unwrap(), b"a");
        assert_eq!(dir.adopt_new().unwrap(), 0);
    }

    #[test]
    fn open_only_accepts_a_maildir() {
        let tmp = TempDir::new();
        assert!(Maildir::open(tmp.path()).is_none());
        Maildir::create(tmp.path().join("f")).unwrap();
        assert!(Maildir::open(tmp.path().join("f")).is_some());
    }

    #[test]
    fn folder_names_are_made_safe_and_restored() {
        let plain = ["Inbox", "Sent Items", "Ünïcødé 日本語", "a.b", "2026"];
        for name in plain {
            assert_eq!(encode_segment(name), name, "ordinary names are unchanged");
        }
        let awkward = [
            "cur", "New", "TMP", "CON", "nul.txt", "com1", ".hidden", "trailing.", "trailing ",
            "a/b", "a\\b", "what?", "50%", "a:b", "<x>", "tab\there", "", ".", "..",
        ];
        for name in plain.iter().chain(&awkward) {
            let encoded = encode_segment(name);
            assert_eq!(decode_segment(&encoded).as_deref(), Some(*name), "{name:?} -> {encoded:?}");
            let lower = encoded.to_ascii_lowercase();
            assert!(!matches!(lower.as_str(), "cur" | "new" | "tmp" | "." | ".."), "{encoded:?}");
            assert!(!encoded.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']), "{encoded:?}");
            assert!(!encoded.ends_with(['.', ' ']) && !encoded.starts_with('.'), "{encoded:?}");
            assert!(!RESERVED.contains(&lower.split('.').next().unwrap()), "{encoded:?}");
        }
        assert_eq!(decode_segment("bad%zz"), None);
        assert_eq!(decode_segment("short%4"), None);
    }
}
