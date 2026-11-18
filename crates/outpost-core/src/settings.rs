//! Settings that apply to the whole app, kept in `settings.json` in the
//! data folder: the general ones and the protection ones, each its own
//! switch. (Settings of one account live with that account.)
//!
//! A password can be set. With one set, the settings cannot be changed and
//! a warning cannot be overridden without it. It is there to stop someone at
//! the keyboard from switching protections off (the owner's parents, say,
//! talked into it by a caller); it is not protection against someone who can
//! edit files on the computer, who could simply remove it. The password is
//! stored only as a salted, deliberately slow hash.

use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE: &str = "settings.json";
const ROUNDS: u32 = 210_000;

/// How the app behaves in general.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct General {
    /// Show a notification when new mail arrives in an inbox.
    pub notify_new_mail: bool,
    /// Start with the window hidden, in the tray.
    pub start_minimised: bool,
    /// Closing the window leaves the app running in the tray, so mail keeps
    /// arriving.
    pub close_to_tray: bool,
    /// How long a message waits before it is sent, during which sending can
    /// be undone. 0 sends at once.
    pub undo_send_seconds: u32,
    /// New folders open as conversations (one row each) rather than as
    /// single messages.
    pub conversations: bool,
    /// How many lines of each message's text the list shows: 0 to 3.
    pub preview_lines: u8,
}

impl Default for General {
    fn default() -> Self {
        General {
            notify_new_mail: true,
            start_minimised: false,
            close_to_tray: false,
            undo_send_seconds: 10,
            conversations: true,
            preview_lines: 3,
        }
    }
}

impl General {
    fn tidy(mut self) -> General {
        self.undo_send_seconds = self.undo_send_seconds.min(120);
        self.preview_lines = self.preview_lines.min(3);
        self
    }
}

/// The protection settings. Everything is on unless switched off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Protection {
    /// On a message that failed its sender checks: links do nothing and
    /// attachments cannot be saved.
    pub disable_failed: bool,
    /// Pictures on the internet are not fetched, except for senders in
    /// `allow_images_from`.
    pub block_remote_images: bool,
    /// Addresses whose messages may load pictures.
    pub allow_images_from: Vec<String>,
    /// Say so when a message is the first from its sender.
    pub first_time_banner: bool,
    /// Warn when the sender's domain resembles one the user hears from.
    pub lookalike_warning: bool,
    /// Warn when replies would go to a different domain than the sender's.
    pub reply_warning: bool,
    /// Clicking a link shows where it really goes before anything opens.
    pub show_link_destination: bool,
    /// Check signatures again on this computer, which looks up the
    /// signer's key in DNS.
    pub recheck_signatures: bool,
    /// Addresses whose mail is moved to the junk folder when it arrives.
    pub blocked_senders: Vec<String>,
}

impl Default for Protection {
    fn default() -> Self {
        Protection {
            disable_failed: true,
            block_remote_images: true,
            allow_images_from: Vec::new(),
            first_time_banner: true,
            lookalike_warning: true,
            reply_warning: true,
            show_link_destination: true,
            recheck_signatures: true,
            blocked_senders: Vec::new(),
        }
    }
}

impl Protection {
    pub fn images_allowed_from(&self, address: &str) -> bool {
        !self.block_remote_images || listed(&self.allow_images_from, address)
    }

    pub fn is_blocked(&self, address: &str) -> bool {
        listed(&self.blocked_senders, address)
    }

    fn tidy(mut self) -> Protection {
        for list in [&mut self.allow_images_from, &mut self.blocked_senders] {
            let mut clean: Vec<String> = list.iter().map(|a| a.trim().to_ascii_lowercase()).filter(|a| a.contains('@')).collect();
            clean.sort();
            clean.dedup();
            *list = clean;
        }
        self
    }
}

fn listed(list: &[String], address: &str) -> bool {
    let address = address.trim();
    !address.is_empty() && list.iter().any(|a| a.eq_ignore_ascii_case(address))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Stored {
    general: General,
    protection: Protection,
    password: Option<PasswordHash>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PasswordHash {
    rounds: u32,
    salt: String,
    hash: String,
}

/// Why a change was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denied {
    /// A password is set and the one given is missing or wrong.
    WrongPassword,
    /// The settings could not be written.
    NotSaved(String),
}

pub struct Settings {
    /// `None` when there is no data folder: settings then last only as
    /// long as the app runs.
    path: Option<PathBuf>,
    stored: Mutex<Stored>,
}

impl Settings {
    /// Reads the settings. A file that is missing or cannot be read gives
    /// the defaults, which are the protective ones.
    pub fn load(data_dir: Option<&Path>) -> Settings {
        let path = data_dir.map(|d| d.join(FILE));
        let stored = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .unwrap_or_default();
        Settings { path, stored: Mutex::new(stored) }
    }

    pub fn protection(&self) -> Protection {
        self.stored.lock().unwrap().protection.clone()
    }

    pub fn general(&self) -> General {
        self.stored.lock().unwrap().general.clone()
    }

    pub fn set_general(&self, general: General, password: Option<&str>) -> Result<(), Denied> {
        self.change(password, |stored| stored.general = general.tidy())
    }

    pub fn password_set(&self) -> bool {
        self.stored.lock().unwrap().password.is_some()
    }

    /// Whether `password` allows a change: always when none is set.
    pub fn allows(&self, password: Option<&str>) -> bool {
        match &self.stored.lock().unwrap().password {
            None => true,
            Some(hash) => password.is_some_and(|p| hash.matches(p)),
        }
    }

    pub fn set_protection(&self, protection: Protection, password: Option<&str>) -> Result<(), Denied> {
        self.change(password, |stored| stored.protection = protection.tidy())
    }

    /// Sets, changes or (with `new` empty or absent) removes the password.
    pub fn set_password(&self, current: Option<&str>, new: Option<&str>) -> Result<(), Denied> {
        let hash = match new {
            Some(new) if !new.is_empty() => Some(PasswordHash::of(new)),
            _ => None,
        };
        self.change(current, |stored| stored.password = hash)
    }

    fn change(&self, password: Option<&str>, apply: impl FnOnce(&mut Stored)) -> Result<(), Denied> {
        if !self.allows(password) {
            return Err(Denied::WrongPassword);
        }
        let mut stored = self.stored.lock().unwrap();
        let mut next = stored.clone();
        apply(&mut next);
        if let Some(path) = &self.path {
            let text = serde_json::to_vec_pretty(&next).map_err(|e| Denied::NotSaved(e.to_string()))?;
            // Written beside the file and moved over it, so a crash cannot
            // leave half a file (which would read as "all defaults").
            let temporary = path.with_extension("json.new");
            std::fs::write(&temporary, text)
                .and_then(|_| std::fs::rename(&temporary, path))
                .map_err(|e| Denied::NotSaved(e.to_string()))?;
        }
        *stored = next;
        Ok(())
    }
}

impl PasswordHash {
    fn of(password: &str) -> PasswordHash {
        let mut salt = [0u8; 16];
        ring::rand::SystemRandom::new().fill(&mut salt).expect("the system has a source of random numbers");
        PasswordHash { rounds: ROUNDS, salt: hex(&salt), hash: hex(&derive(password, &salt, ROUNDS)) }
    }

    fn matches(&self, password: &str) -> bool {
        let (Some(salt), Some(hash)) = (unhex(&self.salt), unhex(&self.hash)) else { return false };
        let Some(rounds) = NonZeroU32::new(self.rounds) else { return false };
        ring::pbkdf2::verify(ring::pbkdf2::PBKDF2_HMAC_SHA256, rounds, &salt, password.as_bytes(), &hash).is_ok()
    }
}

fn derive(password: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(rounds).expect("rounds is not zero"),
        salt,
        password.as_bytes(),
        &mut out,
    );
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("outpost-settings-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_defaults_protect() {
        let p = Settings::load(None).protection();
        assert!(p.disable_failed && p.block_remote_images && p.first_time_banner && p.lookalike_warning);
        assert!(p.reply_warning && p.show_link_destination);
        assert!(!p.images_allowed_from("anyone@example.com"));
        // A damaged or half-understood file falls back to them, setting by
        // setting.
        let dir = temp();
        std::fs::write(dir.join(FILE), b"{\"protection\": {\"firstTimeBanner\": false}}").unwrap();
        let p = Settings::load(Some(&dir)).protection();
        assert!(!p.first_time_banner && p.disable_failed && p.block_remote_images);
        std::fs::write(dir.join(FILE), b"not json").unwrap();
        assert_eq!(Settings::load(Some(&dir)).protection(), Protection::default());
    }

    #[test]
    fn settings_are_kept_and_lists_tidied() {
        let dir = temp();
        let settings = Settings::load(Some(&dir));
        let mut p = settings.protection();
        p.lookalike_warning = false;
        p.allow_images_from = vec![" News@Shop.Example ".into(), "news@shop.example".into(), "not an address".into()];
        p.blocked_senders = vec!["Pest@Spam.Example".into()];
        settings.set_protection(p, None).unwrap();
        let again = Settings::load(Some(&dir)).protection();
        assert!(!again.lookalike_warning);
        assert_eq!(again.allow_images_from, ["news@shop.example"]);
        assert!(again.images_allowed_from("NEWS@shop.example") && !again.images_allowed_from("other@shop.example"));
        assert!(again.is_blocked("pest@spam.example") && !again.is_blocked(""));
    }

    #[test]
    fn general_settings_are_kept_and_kept_in_range() {
        let dir = temp();
        let settings = Settings::load(Some(&dir));
        assert_eq!(settings.general(), General::default());
        assert_eq!(settings.general().undo_send_seconds, 10);
        let wanted = General { undo_send_seconds: 100_000, preview_lines: 9, close_to_tray: true, ..General::default() };
        settings.set_general(wanted, None).unwrap();
        let again = Settings::load(Some(&dir));
        assert_eq!((again.general().undo_send_seconds, again.general().preview_lines), (120, 3));
        assert!(again.general().close_to_tray);
        // Changing one group leaves the other alone.
        assert_eq!(again.protection(), Protection::default());
        // The password guards these too.
        again.set_password(None, Some("sesame")).unwrap();
        assert_eq!(again.set_general(General::default(), None), Err(Denied::WrongPassword));
        again.set_general(General::default(), Some("sesame")).unwrap();
    }

    #[test]
    fn a_password_guards_every_change() {
        let dir = temp();
        let settings = Settings::load(Some(&dir));
        assert!(!settings.password_set() && settings.allows(None));
        settings.set_password(None, Some("correct horse")).unwrap();
        assert!(settings.password_set());

        let mut weaker = settings.protection();
        weaker.disable_failed = false;
        assert_eq!(settings.set_protection(weaker.clone(), None), Err(Denied::WrongPassword));
        assert_eq!(settings.set_protection(weaker.clone(), Some("wrong")), Err(Denied::WrongPassword));
        assert!(settings.protection().disable_failed, "nothing changed");
        assert_eq!(settings.set_password(Some("wrong"), None), Err(Denied::WrongPassword));
        settings.set_protection(weaker, Some("correct horse")).unwrap();
        assert!(!settings.protection().disable_failed);

        // It survives a restart, and is not in the file.
        let saved = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert!(!saved.contains("correct horse"), "{saved}");
        let again = Settings::load(Some(&dir));
        assert!(again.password_set() && !again.allows(Some("Correct Horse")) && again.allows(Some("correct horse")));

        // Changed, then removed, each needing the current one.
        again.set_password(Some("correct horse"), Some("battery staple")).unwrap();
        assert!(!again.allows(Some("correct horse")));
        again.set_password(Some("battery staple"), None).unwrap();
        assert!(!again.password_set() && again.allows(None));
    }
}
