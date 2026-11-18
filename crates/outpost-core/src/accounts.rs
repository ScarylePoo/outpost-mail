//! What is kept about an account besides its mail: how to reach its server,
//! and where its password lives.
//!
//! The server details are in `account.json` in the account's directory.
//! The password is never in that file, in the index or in a log. It is kept
//! in the operating system's credential store (Windows Credential Manager,
//! the macOS keychain, the Secret Service on Linux) when the user asks for
//! it to be remembered and such a store exists; otherwise it is held in
//! memory for as long as the app runs and asked for again next time.

use crate::imap::{Security, Server};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

const CONFIG_FILE: &str = "account.json";
const SERVICE: &str = "Outpost Mail";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountConfig {
    pub host: String,
    pub port: u16,
    /// False only if the user explicitly chose an unencrypted connection.
    pub encrypted: bool,
    pub username: String,
}

impl AccountConfig {
    pub fn server(&self) -> Server {
        Server {
            host: self.host.clone(),
            port: self.port,
            security: if self.encrypted { Security::Tls } else { Security::Plaintext },
            trusted_certificate: None,
        }
    }

    /// The name this account's password is stored under.
    pub fn secret_key(&self, account_dir_name: &str) -> String {
        format!("{account_dir_name}:{}@{}", self.username, self.host)
    }

    /// `None` if the directory holds no account details: mail that is only
    /// browsed, such as a generated test mailbox.
    pub fn load(account_dir: &Path) -> Option<AccountConfig> {
        let text = std::fs::read_to_string(account_dir.join(CONFIG_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, account_dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Written whole, then moved into place.
        let tmp = account_dir.join(format!("{CONFIG_FILE}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, account_dir.join(CONFIG_FILE))
    }
}

/// A directory name for a new account, made from the address it signs in
/// with: `Ben@Example.com` gives `ben@example.com`.
pub fn directory_name(username: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut base: String = username
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@') { c } else { '_' })
        .collect();
    base = base.trim_matches('.').to_string();
    if base.is_empty() {
        base = "account".into();
    }
    let mut name = base.clone();
    let mut n = 2;
    while taken(&name) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

/// Somewhere passwords can be kept between runs.
pub trait Secrets: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<String>;
    /// Returns an explanation if the password could not be stored.
    fn set(&self, key: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, key: &str);
}

/// The operating system's credential store.
pub struct OsSecrets;

impl Secrets for OsSecrets {
    fn get(&self, key: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, key).ok()?.get_password().ok()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, key)
            .and_then(|entry| entry.set_password(secret))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
            let _ = entry.delete_credential();
        }
    }
}

/// Keeps secrets in memory only. Used in tests, so that they never touch
/// the real credential store.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl Secrets for MemorySecrets {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_names_are_safe_and_unique() {
        let free = |_: &str| false;
        assert_eq!(directory_name("Ben@Example.com", free), "ben@example.com");
        assert_eq!(directory_name("a/b\\c:d e", free), "a_b_c_d_e");
        assert_eq!(directory_name("..", free), "account");
        assert_eq!(directory_name("", free), "account");
        let taken = |name: &str| name == "me@example.com" || name == "me@example.com-2";
        assert_eq!(directory_name("me@example.com", taken), "me@example.com-3");
    }

    #[test]
    fn the_config_file_holds_no_password_and_round_trips() {
        let dir = std::env::temp_dir().join(format!("outpost-account-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(AccountConfig::load(&dir), None);
        let config = AccountConfig {
            host: "mail.example.com".into(),
            port: 993,
            encrypted: true,
            username: "me@example.com".into(),
        };
        config.save(&dir).unwrap();
        assert_eq!(AccountConfig::load(&dir), Some(config.clone()));
        let on_disk = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(!on_disk.to_lowercase().contains("pass"), "{on_disk}");
        assert_eq!(config.secret_key("home"), "home:me@example.com@mail.example.com");
        assert_eq!(config.server().port, 993);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
