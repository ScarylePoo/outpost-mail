//! Checking for a new version: asks GitHub what the latest release of
//! Outpost is and says so if it is newer than this one. Nothing is
//! downloaded or installed; the user fetches the installer themselves.
//!
//! The request carries nothing about the user: no account, no identifier,
//! not even this version's number. GitHub sees the computer's internet
//! address, as any website does.

use crate::dav::http::{self, Url};
use serde::{Deserialize, Serialize};
use std::path::Path;

const LATEST: &str = "https://api.github.com/repos/ScarylePoo/outpost-mail/releases/latest";
const RELEASES: &str = "https://github.com/ScarylePoo/outpost-mail/releases";
const FILE: &str = "update-check.json";
/// How long an answer is good for, in seconds: a day.
const FRESH: i64 = 24 * 60 * 60;

/// A version newer than this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Newer {
    pub version: String,
    /// The release's page on GitHub.
    pub url: String,
}

/// What the last check found, kept so that the question is asked of GitHub
/// at most once a day however often the app starts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Checked {
    /// When, in seconds since 1970. 0 is never.
    pub at: i64,
    /// The latest release's version at that time, if there was a release.
    pub latest: Option<String>,
    pub url: Option<String>,
}

impl Checked {
    pub fn load(data_dir: Option<&Path>) -> Checked {
        data_dir
            .and_then(|d| std::fs::read(d.join(FILE)).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: Option<&Path>) {
        if let (Some(dir), Ok(text)) = (data_dir, serde_json::to_vec(self)) {
            let _ = std::fs::write(dir.join(FILE), text);
        }
    }

    pub fn fresh(&self, now: i64) -> bool {
        self.at > 0 && now >= self.at && now - self.at < FRESH
    }

    /// The newer version, if what was found is newer than `current`.
    pub fn newer_than(&self, current: &str) -> Option<Newer> {
        let latest = self.latest.as_deref()?;
        is_newer(latest, current).then(|| Newer {
            version: latest.to_string(),
            url: self.url.clone().unwrap_or_else(|| RELEASES.to_string()),
        })
    }
}

/// Asks GitHub. `Ok` with no version means there is no release yet.
pub async fn ask(now: i64) -> Result<Checked, String> {
    let url = Url::parse(LATEST).expect("the address is well formed");
    let answer = http::send(http::Request {
        method: "GET",
        url: &url,
        sign_in: None,
        headers: &[("Accept", "application/vnd.github+json")],
        body: &[],
    })
    .await
    .map_err(|e| e.to_string())?;
    match answer.status {
        200 => read_release(&answer.body).map(|(latest, url)| Checked { at: now, latest: Some(latest), url: Some(url) }),
        // No release has been published.
        404 => Ok(Checked { at: now, latest: None, url: None }),
        403 | 429 => Err("GitHub is limiting requests from this network just now. Try again later.".into()),
        other => Err(format!("GitHub answered with status {other}")),
    }
}

/// The version and page of a release, from GitHub's description of it.
fn read_release(body: &[u8]) -> Result<(String, String), String> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        #[serde(default)]
        html_url: String,
    }
    let release: Release = serde_json::from_slice(body).map_err(|_| "GitHub's answer could not be read".to_string())?;
    let version = release.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
    if numbers(&version).is_none() {
        return Err("the latest release has no version number".into());
    }
    // The page is opened in the browser, so it must be the project's own
    // and nothing else, whatever the answer says.
    let url = if release.html_url.starts_with(&format!("{RELEASES}/")) && !release.html_url.contains(char::is_whitespace) {
        release.html_url
    } else {
        RELEASES.to_string()
    };
    Ok((version, url))
}

/// "1.2.3" as numbers. Anything after the numbers (a "-beta" suffix) is
/// ignored. `None` if it does not start with a number.
fn numbers(version: &str) -> Option<Vec<u64>> {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let parts: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 4)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (numbers(candidate), numbers(current)) else { return false };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.1.0", "0.0.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(is_newer("1.0.1", "1.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("nonsense", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        // A pre-release of the same number is not announced as newer.
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
    }

    #[test]
    fn a_release_is_read_and_its_link_is_never_trusted_blindly() {
        let body = br#"{"tag_name": "v0.2.0", "html_url": "https://github.com/ScarylePoo/outpost-mail/releases/tag/v0.2.0", "body": "notes"}"#;
        let (version, url) = read_release(body).unwrap();
        assert_eq!(version, "0.2.0");
        assert_eq!(url, "https://github.com/ScarylePoo/outpost-mail/releases/tag/v0.2.0");
        // A link to anywhere else is replaced by the project's releases page.
        let body = br#"{"tag_name": "0.3.0", "html_url": "https://evil.example/outpost-setup.exe"}"#;
        assert_eq!(read_release(body).unwrap(), ("0.3.0".to_string(), RELEASES.to_string()));
        let body = br#"{"tag_name": "0.3.0", "html_url": "https://github.com/ScarylePoo/outpost-mail/releases.evil.example/x"}"#;
        assert_eq!(read_release(body).unwrap().1, RELEASES);
        assert!(read_release(br#"{"tag_name": "latest"}"#).is_err());
        assert!(read_release(b"<html>").is_err());
    }

    #[test]
    fn an_answer_is_kept_for_a_day() {
        let dir = std::env::temp_dir().join(format!("outpost-updates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!Checked::load(Some(&dir)).fresh(1_000_000));
        let checked = Checked { at: 1_000_000, latest: Some("0.2.0".into()), url: None };
        checked.save(Some(&dir));
        let again = Checked::load(Some(&dir));
        assert_eq!(again, checked);
        assert!(again.fresh(1_000_000 + FRESH - 1) && !again.fresh(1_000_000 + FRESH));
        // A clock set back does not make an old answer last for ever.
        assert!(!again.fresh(999_999));
        assert_eq!(again.newer_than("0.1.0").unwrap().url, RELEASES);
        assert_eq!(again.newer_than("0.2.0"), None);
        assert_eq!(Checked::default().newer_than("0.1.0"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
