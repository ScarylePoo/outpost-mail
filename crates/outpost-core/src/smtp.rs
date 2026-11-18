//! Sending mail: a small SMTP submission client.
//!
//! It does one thing, in order, with no cleverness: connect, secure the
//! connection, sign in, name the sender and recipients, hand over the
//! message, and read the verdict. The cases are kept apart carefully,
//! because they need different handling by the outbox:
//!
//! - failed before the message was handed over: nothing was sent, try later;
//! - the server said no: nothing was sent, and trying again will not help;
//! - the connection was lost after the message was handed over and before
//!   the verdict arrived: it may or may not have been sent. This is the one
//!   case that must never be retried automatically, or the recipient could
//!   get the message twice.

use crate::imap::conn::tls_config;
use crate::render::base64;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

const STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// Servers may take minutes to accept a large message.
const VERDICT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    /// TLS from the first byte (port 465).
    Tls,
    /// Starts unencrypted and upgrades before anything private is sent
    /// (port 587). If the server will not upgrade, nothing is sent.
    StartTls,
    /// No encryption. Only for a server on the same machine.
    Plaintext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpServer {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    /// As for IMAP: a certificate the user chose to accept for this server.
    pub trusted_certificate: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// Nothing was sent, and trying again later may work.
    Unavailable(String),
    /// Nothing was sent, and the server's answer will not change: a wrong
    /// password, a refused recipient, a message it will not accept.
    Refused(String),
    /// The message was handed over and the connection was lost before the
    /// server said whether it accepted it.
    Uncertain(String),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Unavailable(e) => write!(f, "could not reach the outgoing mail server: {e}"),
            SendError::Refused(e) => write!(f, "the outgoing mail server refused: {e}"),
            SendError::Uncertain(e) => write!(
                f,
                "the connection was lost while sending, so it is not known whether the message went: {e}"
            ),
        }
    }
}

impl std::error::Error for SendError {}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    fn text(&self) -> String {
        format!("{} {}", self.code, self.lines.join(" "))
    }
}

struct Conversation {
    stream: Box<dyn Stream>,
    buffer: Vec<u8>,
}

fn unavailable(e: impl std::fmt::Display) -> SendError {
    SendError::Unavailable(e.to_string())
}

impl Conversation {
    async fn read_line(&mut self, timeout: Duration) -> Result<String, String> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                return Ok(String::from_utf8_lossy(&line).trim_end().to_string());
            }
            if self.buffer.len() > 64 * 1024 {
                return Err("the server sent an overlong line".into());
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(timeout, self.stream.read(&mut chunk))
                .await
                .map_err(|_| "the server did not respond in time".to_string())?
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("the server closed the connection".into());
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    /// Reads one reply, which may span several lines (`250-…`, `250 …`).
    async fn reply(&mut self, timeout: Duration) -> Result<Reply, String> {
        let mut lines = Vec::new();
        loop {
            let line = self.read_line(timeout).await?;
            let code: u16 = line.get(..3).and_then(|c| c.parse().ok()).ok_or("not an SMTP reply")?;
            lines.push(line.get(4..).unwrap_or("").to_string());
            if line.as_bytes().get(3) != Some(&b'-') {
                return Ok(Reply { code, lines });
            }
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stream.write_all(bytes).await.map_err(|e| e.to_string())?;
        self.stream.flush().await.map_err(|e| e.to_string())
    }

    /// Sends a command and requires a reply in the `expected` hundred.
    /// A 4xx reply means "not now"; a 5xx reply means "no".
    async fn command(&mut self, line: &str, expected: u16) -> Result<Reply, SendError> {
        self.write(format!("{line}\r\n").as_bytes()).await.map_err(unavailable)?;
        let reply = self.reply(STEP_TIMEOUT).await.map_err(unavailable)?;
        check(reply, expected)
    }
}

fn check(reply: Reply, expected: u16) -> Result<Reply, SendError> {
    match reply.code / 100 {
        n if n == expected / 100 => Ok(reply),
        4 => Err(SendError::Unavailable(reply.text())),
        _ => Err(SendError::Refused(reply.text())),
    }
}

async fn secure(tcp: Box<dyn Stream>, server: &SmtpServer) -> Result<Box<dyn Stream>, SendError> {
    let name = ServerName::try_from(server.host.clone()).map_err(unavailable)?;
    let config = tls_config(server.trusted_certificate.as_deref()).map_err(unavailable)?;
    let tls = tokio::time::timeout(STEP_TIMEOUT, TlsConnector::from(config).connect(name, tcp))
        .await
        .map_err(|_| unavailable("the secure connection timed out"))?
        // A certificate problem will not fix itself by retrying.
        .map_err(|e| SendError::Refused(format!("could not establish a secure connection: {e}")))?;
    Ok(Box::new(tls))
}

fn capability<'a>(ehlo: &'a Reply, name: &str) -> Option<&'a str> {
    ehlo.lines.iter().skip(1).find_map(|line| {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        word.eq_ignore_ascii_case(name).then_some(rest)
    })
}

/// The message as it goes on the wire: CRLF line endings, and a leading dot
/// doubled so that no line can be mistaken for the end of the message.
fn wire_form(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + message.len() / 40 + 8);
    let mut at_line_start = true;
    let mut previous = 0u8;
    for &byte in message {
        if at_line_start && byte == b'.' {
            out.push(b'.');
        }
        if byte == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(byte);
        at_line_start = byte == b'\n';
        previous = byte;
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// Sends one message. `from` and `recipients` are bare addresses: the
/// envelope, which is separate from the headers inside `message`.
pub async fn send(
    server: &SmtpServer,
    username: &str,
    password: &str,
    from: &str,
    recipients: &[String],
    message: &[u8],
) -> Result<(), SendError> {
    if recipients.is_empty() {
        return Err(SendError::Refused("the message has no recipients".into()));
    }
    for address in recipients.iter().map(String::as_str).chain([from]) {
        // An address is put between angle brackets on a command line; one
        // that could break out of them is never sent.
        if address.is_empty() || address.contains(['<', '>', '\r', '\n', ' ']) {
            return Err(SendError::Refused(format!("{address:?} is not a usable address")));
        }
    }

    let tcp = tokio::time::timeout(STEP_TIMEOUT, TcpStream::connect((server.host.as_str(), server.port)))
        .await
        .map_err(|_| unavailable("connecting timed out"))?
        .map_err(unavailable)?;
    let _ = tcp.set_nodelay(true);
    let mut stream: Box<dyn Stream> = Box::new(tcp);
    if server.security == SmtpSecurity::Tls {
        stream = secure(stream, server).await?;
    }
    let mut smtp = Conversation { stream, buffer: Vec::new() };
    check(smtp.reply(STEP_TIMEOUT).await.map_err(unavailable)?, 220)?;

    const HELLO: &str = "EHLO localhost";
    let mut ehlo = smtp.command(HELLO, 250).await?;
    if server.security == SmtpSecurity::StartTls {
        if capability(&ehlo, "STARTTLS").is_none() {
            return Err(SendError::Refused(
                "the server does not offer encryption on this port, so nothing was sent".into(),
            ));
        }
        smtp.command("STARTTLS", 220).await?;
        // Anything read before the upgrade is discarded: it was not secure.
        let Conversation { stream, .. } = smtp;
        smtp = Conversation { stream: secure(stream, server).await?, buffer: Vec::new() };
        ehlo = smtp.command(HELLO, 250).await?;
    }

    // Sign in. Without a way to sign in, only a local unencrypted server
    // (a test server) is used as it is.
    let mechanisms = capability(&ehlo, "AUTH").unwrap_or("").to_ascii_uppercase();
    if mechanisms.split_whitespace().any(|m| m == "PLAIN") {
        let token = base64(format!("\0{username}\0{password}").as_bytes());
        smtp.command(&format!("AUTH PLAIN {token}"), 235).await.map_err(sign_in_failure)?;
    } else if mechanisms.split_whitespace().any(|m| m == "LOGIN") {
        smtp.command("AUTH LOGIN", 334).await.map_err(sign_in_failure)?;
        smtp.command(&base64(username.as_bytes()), 334).await.map_err(sign_in_failure)?;
        smtp.command(&base64(password.as_bytes()), 235).await.map_err(sign_in_failure)?;
    } else if server.security != SmtpSecurity::Plaintext {
        return Err(SendError::Refused(
            "the server offers no way to sign in that this version of Outpost supports".into(),
        ));
    }

    let needs_utf8 = !from.is_ascii() || recipients.iter().any(|r| !r.is_ascii());
    let utf8 = if needs_utf8 && capability(&ehlo, "SMTPUTF8").is_some() { " SMTPUTF8" } else { "" };
    smtp.command(&format!("MAIL FROM:<{from}>{utf8}"), 250).await?;
    for recipient in recipients {
        smtp.command(&format!("RCPT TO:<{recipient}>"), 250).await.map_err(|e| match e {
            SendError::Refused(text) => SendError::Refused(format!("{recipient}: {text}")),
            other => other,
        })?;
    }
    smtp.command("DATA", 354).await?;

    // From here until the verdict, a failure leaves us not knowing.
    let uncertain = |e: String| SendError::Uncertain(e);
    smtp.write(&wire_form(message)).await.map_err(uncertain)?;
    let verdict = smtp.reply(VERDICT_TIMEOUT).await.map_err(uncertain)?;
    check(verdict, 250)?;

    let _ = smtp.write(b"QUIT\r\n").await;
    Ok(())
}

fn sign_in_failure(e: SendError) -> SendError {
    match e {
        SendError::Refused(text) => SendError::Refused(format!("sign-in failed: {text}")),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_cannot_end_early_or_lose_line_endings() {
        assert_eq!(wire_form(b"a\r\nb\r\n"), b"a\r\nb\r\n.\r\n");
        assert_eq!(wire_form(b"a\nb"), b"a\r\nb\r\n.\r\n", "bare newlines become CRLF, and a final one is added");
        assert_eq!(wire_form(b".\r\n..x\r\ny.\r\n"), b"..\r\n...x\r\ny.\r\n.\r\n", "leading dots are doubled");
        assert_eq!(wire_form(b"body\r\n.\r\nMAIL FROM:<evil>\r\n"), b"body\r\n..\r\nMAIL FROM:<evil>\r\n.\r\n");
        assert_eq!(wire_form(b""), b"\r\n.\r\n");
    }

    #[test]
    fn replies_are_sorted_into_the_three_outcomes() {
        let reply = |code| Reply { code, lines: vec!["text".into()] };
        assert!(check(reply(250), 250).is_ok());
        assert!(check(reply(354), 354).is_ok());
        assert_eq!(check(reply(451), 250).err(), Some(SendError::Unavailable("451 text".into())));
        assert_eq!(check(reply(550), 250).err(), Some(SendError::Refused("550 text".into())));
        assert_eq!(check(reply(250), 354).err(), Some(SendError::Refused("250 text".into())), "an unexpected yes is not a yes");
    }

    #[test]
    fn capabilities_are_read_from_the_greeting_reply() {
        let ehlo = Reply { code: 250, lines: vec!["mail.example.com".into(), "SIZE 1000".into(), "auth PLAIN LOGIN".into(), "STARTTLS".into()] };
        assert_eq!(capability(&ehlo, "AUTH"), Some("PLAIN LOGIN"));
        assert_eq!(capability(&ehlo, "STARTTLS"), Some(""));
        assert_eq!(capability(&ehlo, "SMTPUTF8"), None);
        assert_eq!(capability(&ehlo, "mail.example.com"), None, "the first line is the server's name, not a capability");
    }
}
