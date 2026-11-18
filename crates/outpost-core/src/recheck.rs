//! Re-checking a message's DKIM signature on this computer.
//!
//! The signature travels with the message, so it can be checked again
//! without taking anyone's word for it: fetch the signer's public key from
//! DNS and verify. This is the one sender check a mail client can repeat.
//! (SPF cannot be: it is about the connection the message arrived on.)
//!
//! A re-check can come out three ways, and the difference matters:
//!
//! - **Valid**: the signature matches the message as stored here.
//! - **Does not match**: the key is there and the message is not what was
//!   signed. It was changed after signing, by someone on the way or by a
//!   server that adds to messages (mailing lists and some filters do).
//! - **Could not re-check**: the key is no longer published, the signature
//!   has expired, or the lookup failed. Senders replace their keys, so this
//!   is normal for old mail and is never reported as a failure.
//!
//! Looking up a key asks the computer's DNS server about the signing domain,
//! much as following a link in the message would.

use mail_auth::dkim::DkimError;
use mail_auth::dns::{DnsCache, NoCache, ResolverCache, TxtRecord};
use mail_auth::{AuthenticatedMessage, DkimResult, DnsError, Error, MessageAuthenticator, Parameters};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a re-check may take before it is given up on.
const PATIENCE: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Valid,
    /// The key was found and the message is not what was signed.
    Mismatch,
    /// No verdict either way.
    Unknown,
}

/// The re-check of one signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureCheck {
    /// The domain that signed.
    pub domain: String,
    pub outcome: Outcome,
    /// One plain sentence.
    pub sentence: String,
}

/// Re-checks signatures, remembering the keys it has fetched.
#[derive(Default)]
pub struct Rechecker {
    keys: Keys,
}

impl Rechecker {
    pub fn new() -> Rechecker {
        Rechecker::default()
    }

    /// Re-checks every signature on a message. Empty if it has none.
    pub async fn check(&self, raw: &[u8]) -> Vec<SignatureCheck> {
        let Some(message) = AuthenticatedMessage::parse(raw) else {
            return Vec::new();
        };
        let Ok(resolver) = MessageAuthenticator::new_system_conf() else {
            return unknown(raw, "this computer's DNS settings could not be read");
        };
        self.check_with(&resolver, &message, raw).await
    }

    async fn check_with(
        &self,
        resolver: &MessageAuthenticator,
        message: &AuthenticatedMessage<'_>,
        raw: &[u8],
    ) -> Vec<SignatureCheck> {
        let params = Parameters::new(message).with_cache(&self.keys);
        let Ok(outputs) = tokio::time::timeout(PATIENCE, resolver.verify_dkim(params)).await else {
            return unknown(raw, "looking up the sender's key took too long");
        };
        outputs
            .iter()
            .map(|output| {
                let domain = output.signature().map(|s| s.d.to_ascii_lowercase()).unwrap_or_default();
                let (outcome, sentence) = explain(output.result(), &domain);
                SignatureCheck { domain, outcome, sentence }
            })
            .collect()
    }

}

/// "Could not re-check" for every signature on the message.
fn unknown(raw: &[u8], why: &str) -> Vec<SignatureCheck> {
    crate::verify::header_fields(raw)
        .iter()
        .filter(|(name, _)| name == "dkim-signature")
        .map(|(_, value)| {
            let domain = value
                .split(';')
                .filter_map(|tag| tag.split_once('='))
                .find(|(k, _)| k.trim() == "d")
                .map(|(_, v)| v.trim().to_ascii_lowercase())
                .unwrap_or_default();
            SignatureCheck {
                sentence: format!("Could not re-check the signature from {domain}: {why}."),
                domain,
                outcome: Outcome::Unknown,
            }
        })
        .collect()
}

fn explain(result: &DkimResult, domain: &str) -> (Outcome, String) {
    let could_not = |why: &str| (Outcome::Unknown, format!("Could not re-check the signature from {domain}: {why}. That is normal for older mail and does not mean the message is forged."));
    match result {
        DkimResult::Pass => (Outcome::Valid, format!("Outpost checked the signature from {domain} again itself, and it is valid.")),
        DkimResult::Fail(_) | DkimResult::Neutral(Error::Dkim(DkimError::BodyHashMismatch)) => (
            Outcome::Mismatch,
            format!("Outpost checked the signature from {domain} again itself, and it does not match the message as it is stored here. The message was changed after it was signed. Mailing lists and some mail filters do that; so do forgers."),
        ),
        DkimResult::Neutral(Error::Dkim(DkimError::SignatureExpired)) => could_not("the signature has passed its own expiry date"),
        DkimResult::PermError(Error::Dkim(DkimError::PublicKeyRevoked)) => could_not("the sender has withdrawn the key it was made with"),
        DkimResult::PermError(Error::Dns(DnsError::RecordNotFound(_) | DnsError::InvalidRecordType)) => {
            could_not("the sender no longer publishes the key it was made with")
        }
        DkimResult::TempError(_) => (
            Outcome::Unknown,
            format!("Could not re-check the signature from {domain}: the sender's key could not be looked up just now."),
        ),
        _ => could_not("it is of a kind Outpost cannot check"),
    }
}

/// Fetched keys, by DNS name, until the time their DNS answer allows.
#[derive(Default)]
struct Keys(Mutex<HashMap<Box<str>, (TxtRecord, Instant)>>);

impl ResolverCache<Box<str>, TxtRecord> for Keys {
    fn get<Q>(&self, name: &Q) -> Option<TxtRecord>
    where
        Box<str>: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let keys = self.0.lock().unwrap();
        keys.get(name).filter(|(_, until)| *until > Instant::now()).map(|(record, _)| record.clone())
    }

    fn remove<Q>(&self, name: &Q) -> Option<TxtRecord>
    where
        Box<str>: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.0.lock().unwrap().remove(name).map(|(record, _)| record)
    }

    fn insert(&self, key: Box<str>, value: TxtRecord, valid_until: Instant) {
        let mut keys = self.0.lock().unwrap();
        if keys.len() > 2_000 {
            let now = Instant::now();
            keys.retain(|_, (_, until)| *until > now);
        }
        // An hour at most, however long the sender's DNS says.
        keys.insert(key, (value, valid_until.min(Instant::now() + Duration::from_secs(3600))));
    }
}

impl DnsCache for Keys {
    type Txt = Keys;
    type Mx = NoCache;
    type Ipv4 = NoCache;
    type Ipv6 = NoCache;
    type Ptr = NoCache;

    fn txt(&self) -> Option<&Keys> {
        Some(self)
    }
    fn mx(&self) -> Option<&NoCache> {
        None
    }
    fn ipv4(&self) -> Option<&NoCache> {
        None
    }
    fn ipv6(&self) -> Option<&NoCache> {
        None
    }
    fn ptr(&self) -> Option<&NoCache> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_auth::crypto::Ed25519Key;
    use mail_auth::dkim::{DkimSigner, DomainKey};
    use mail_auth::dns::TxtRecordParser;
    use mail_auth::headers::HeaderWriter;
    use mail_auth::hickory_resolver::config::{ResolverConfig, ResolverOpts};

    fn base64(data: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let n = chunk.iter().fold(0u32, |n, b| (n << 8) | *b as u32) << (8 * (3 - chunk.len()));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    const MESSAGE: &str = "From: Alice <alice@bank.example>\r\nTo: ann@home.example\r\nSubject: Your statement\r\nDate: Mon, 16 Nov 2026 10:00:00 +0000\r\n\r\nYour statement is ready.\r\n";

    /// A signed message, and the DNS name and record of the key it was
    /// signed with.
    fn signed() -> (String, String, String) {
        let key = Ed25519Key::from_pkcs8_der(&Ed25519Key::generate_pkcs8().unwrap()).unwrap();
        let record = format!("v=DKIM1; k=ed25519; p={}", base64(&key.public_key()));
        let signature = DkimSigner::from_key(key)
            .domain("bank.example")
            .selector("s1")
            .headers(["From", "To", "Subject", "Date"])
            .sign(MESSAGE.as_bytes())
            .unwrap();
        (format!("{}{MESSAGE}", signature.to_header()), "s1._domainkey.bank.example.".into(), record)
    }

    /// Re-checks with the given DNS answers and no network: a name that is
    /// not given an answer here is given "no such record".
    fn recheck(raw: &str, answers: &[(&str, Result<&str, Error>)]) -> Vec<SignatureCheck> {
        let checker = Rechecker::new();
        let later = Instant::now() + Duration::from_secs(60);
        for (name, answer) in answers {
            let record = match answer {
                Ok(text) => TxtRecord::from(DomainKey::parse(text.as_bytes()).unwrap()),
                Err(e) => TxtRecord::Error(e.clone()),
            };
            checker.keys.insert((*name).into(), record, later);
        }
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let resolver = MessageAuthenticator::new(ResolverConfig::default(), ResolverOpts::default()).unwrap();
            let message = AuthenticatedMessage::parse(raw.as_bytes()).unwrap();
            checker.check_with(&resolver, &message, raw.as_bytes()).await
        })
    }

    #[test]
    fn a_good_signature_is_valid() {
        let (raw, name, record) = signed();
        let got = recheck(&raw, &[(&name, Ok(&record))]);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].domain.as_str(), got[0].outcome), ("bank.example", Outcome::Valid), "{got:?}");
    }

    #[test]
    fn a_changed_message_does_not_match() {
        let (raw, name, record) = signed();
        // The body, then a signed header.
        for changed in [
            raw.replace("statement is ready", "statement is ready at evil.example"),
            raw.replace("Subject: Your statement", "Subject: Urgent"),
            raw.replace("alice@bank.example", "alice@bank.example.evil.example"),
        ] {
            assert_ne!(changed, raw);
            let got = recheck(&changed, &[(&name, Ok(&record))]);
            assert_eq!(got[0].outcome, Outcome::Mismatch, "{got:?}");
        }
        // A different key under the same name: someone else's signature.
        let (_, _, other_key) = signed();
        assert_eq!(recheck(&raw, &[(&name, Ok(&other_key))])[0].outcome, Outcome::Mismatch);
    }

    #[test]
    fn a_missing_key_is_could_not_recheck_and_never_a_failure() {
        let (raw, name, _) = signed();
        for (answer, expected) in [
            (Error::Dns(DnsError::InvalidRecordType), "no longer publishes"),
            (Error::Dkim(DkimError::PublicKeyRevoked), "withdrawn"),
            (Error::Dns(DnsError::Resolver("timed out".into())), "could not be looked up just now"),
        ] {
            let got = recheck(&raw, &[(&name, Err(answer))]);
            assert_eq!(got[0].outcome, Outcome::Unknown, "{got:?}");
            assert!(got[0].sentence.contains(expected), "{}", got[0].sentence);
            assert!(got[0].sentence.starts_with("Could not re-check"), "{}", got[0].sentence);
        }
    }

    #[test]
    fn a_message_without_a_signature_has_nothing_to_recheck() {
        assert_eq!(recheck(MESSAGE, &[]), []);
        assert_eq!(unknown(MESSAGE.as_bytes(), "x"), []);
        let (raw, _, _) = signed();
        let got = unknown(raw.as_bytes(), "offline");
        assert_eq!((got[0].domain.as_str(), got[0].outcome), ("bank.example", Outcome::Unknown));
    }
}
