//! A small HTTP/1.1 client, enough for CalDAV and CardDAV.
//!
//! Written here rather than brought in for the same reason as the SMTP
//! client: TLS is set up exactly as for mail (the same certificate checks,
//! the same cryptography already in the build), and nothing else comes with
//! it. One request per connection, no compression, no cookies.

use crate::imap::conn::tls_config;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

const TIMEOUT: Duration = Duration::from_secs(60);
/// The largest response accepted. A whole address book in one answer is a
/// few megabytes; this is a guard against a runaway server.
const MAX_BODY: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Could not reach the server, or the connection dropped.
    Network(String),
    /// The server's certificate or TLS handshake was not acceptable.
    Tls(String),
    /// The server answered with something that is not HTTP.
    Protocol(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Network(e) => write!(f, "could not reach the server: {e}"),
            HttpError::Tls(e) => write!(f, "the secure connection failed: {e}"),
            HttpError::Protocol(e) => write!(f, "the server gave an answer that makes no sense: {e}"),
        }
    }
}

/// A web address taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// False only for `http://`, which is used only when the user typed it.
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Starts with `/`. Includes any query.
    pub path: String,
}

impl Url {
    /// Parses an address. Without a scheme, `https://` is assumed.
    pub fn parse(text: &str) -> Option<Url> {
        let text = text.trim();
        let (secure, rest) = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => (true, rest),
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => (false, rest),
            Some(_) => return None,
            None => (true, text),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        // User names in addresses are not supported: they end up in logs.
        if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
            return None;
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || !port.is_empty() => match port.parse() {
                Ok(port) => (host, port),
                Err(_) if host.contains('[') => (authority, if secure { 443 } else { 80 }),
                Err(_) => return None,
            },
            _ => (authority, if secure { 443 } else { 80 }),
        };
        if host.is_empty() {
            return None;
        }
        Some(Url { secure, host: host.to_ascii_lowercase(), port, path: path.to_string() })
    }

    /// The address a link found at this one points to. `None` for a link
    /// that leaves `http`/`https`.
    pub fn join(&self, link: &str) -> Option<Url> {
        let link = link.trim();
        if link.contains("://") {
            return Url::parse(link);
        }
        let path = if link.starts_with('/') {
            link.to_string()
        } else {
            let folder = self.path.rsplit_once('/').map_or("", |(folder, _)| folder);
            format!("{folder}/{link}")
        };
        Some(Url { path, ..self.clone() })
    }

    /// Whether another address is on the same server: only then are the
    /// user's name and password sent to it.
    pub fn same_server(&self, other: &Url) -> bool {
        self.host == other.host && self.port == other.port && self.secure == other.secure
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.secure { "https" } else { "http" };
        let standard = if self.secure { 443 } else { 80 };
        if self.port == standard {
            write!(f, "{scheme}://{}{}", self.host, self.path)
        } else {
            write!(f, "{scheme}://{}:{}{}", self.host, self.port, self.path)
        }
    }
}

pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a Url,
    /// Name and password, sent as Basic authentication.
    pub sign_in: Option<(&'a str, &'a str)>,
    /// Extra headers: name and value.
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Sends one request and reads the whole answer. Redirects are not
/// followed here; the caller decides whether a new address may be trusted.
pub async fn send(request: Request<'_>) -> Result<Response, HttpError> {
    tokio::time::timeout(TIMEOUT, send_inner(request))
        .await
        .map_err(|_| HttpError::Network("the server took too long to answer".into()))?
}

async fn send_inner(request: Request<'_>) -> Result<Response, HttpError> {
    let url = request.url;
    let network = |e: std::io::Error| HttpError::Network(e.to_string());
    let tcp = TcpStream::connect((url.host.trim_matches(|c| c == '[' || c == ']'), url.port)).await.map_err(network)?;
    let _ = tcp.set_nodelay(true);
    let mut stream: Box<dyn Stream> = if url.secure {
        let config = tls_config(None).map_err(HttpError::Tls)?;
        let name = ServerName::try_from(url.host.clone()).map_err(|e| HttpError::Tls(e.to_string()))?;
        Box::new(TlsConnector::from(config).connect(name, tcp).await.map_err(|e| HttpError::Tls(e.to_string()))?)
    } else {
        Box::new(tcp)
    };

    let clean = |text: &str| !text.contains(['\r', '\n']);
    if !clean(request.method) || !clean(&url.path) || request.headers.iter().any(|(n, v)| !clean(n) || !clean(v)) {
        return Err(HttpError::Protocol("a line break in a request".into()));
    }
    let standard = if url.secure { 443 } else { 80 };
    let host = if url.port == standard { url.host.clone() } else { format!("{}:{}", url.host, url.port) };
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Outpost\r\nAccept-Encoding: identity\r\nConnection: close\r\nContent-Length: {}\r\n",
        request.method,
        url.path,
        request.body.len()
    );
    if let Some((name, password)) = request.sign_in {
        let token = crate::render::base64(format!("{name}:{password}").as_bytes());
        head.push_str(&format!("Authorization: Basic {token}\r\n"));
    }
    for (name, value) in request.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.map_err(network)?;
    stream.write_all(request.body).await.map_err(network)?;
    stream.flush().await.map_err(network)?;

    // The connection closes after the answer, so read to the end.
    let mut raw = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let n = match stream.read(&mut buffer).await {
            Ok(n) => n,
            // Some servers drop a TLS connection without the closing
            // handshake; what has arrived is still the answer if complete.
            Err(_) if !raw.is_empty() => 0,
            Err(e) => return Err(network(e)),
        };
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..n]);
        if raw.len() > MAX_BODY {
            return Err(HttpError::Protocol("the answer is too large".into()));
        }
        if complete(&raw) {
            break;
        }
    }
    parse(&raw, request.method == "HEAD")
}

/// Whether a whole response has arrived (so there is no need to wait for
/// the server to close the connection).
fn complete(raw: &[u8]) -> bool {
    matches!(parse_parts(raw, false), Ok(Some(_)))
}

fn parse(raw: &[u8], no_body: bool) -> Result<Response, HttpError> {
    match parse_parts(raw, no_body)? {
        Some(response) => Ok(response),
        None => Err(HttpError::Network("the connection closed before the whole answer arrived".into())),
    }
}

/// `Ok(None)` while more bytes are needed.
fn parse_parts(raw: &[u8], no_body: bool) -> Result<Option<Response>, HttpError> {
    let mut slots = [httparse::EMPTY_HEADER; 96];
    let mut response = httparse::Response::new(&mut slots);
    let head = match response.parse(raw) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(e) => return Err(HttpError::Protocol(e.to_string())),
    };
    let status = response.code.unwrap_or(0);
    let headers: Vec<(String, String)> = response
        .headers
        .iter()
        .map(|h| (h.name.to_ascii_lowercase(), String::from_utf8_lossy(h.value).trim().to_string()))
        .collect();
    let find = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    let rest = &raw[head..];
    let body = if no_body || status == 204 || status == 304 || (100..200).contains(&status) {
        Vec::new()
    } else if find("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
        match unchunk(rest)? {
            Some(body) => body,
            None => return Ok(None),
        }
    } else if let Some(length) = find("content-length") {
        let length: usize = length.parse().map_err(|_| HttpError::Protocol("a bad Content-Length".into()))?;
        if rest.len() < length {
            return Ok(None);
        }
        rest[..length].to_vec()
    } else {
        // No length given: the body is everything until the connection
        // closes, which only the caller knows has happened.
        rest.to_vec()
    };
    Ok(Some(Response { status, headers, body }))
}

/// Joins a chunked body. `Ok(None)` if the last chunk has not arrived.
fn unchunk(mut rest: &[u8]) -> Result<Option<Vec<u8>>, HttpError> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = rest.windows(2).position(|w| w == b"\r\n") else { return Ok(None) };
        let line = std::str::from_utf8(&rest[..line_end]).map_err(|_| HttpError::Protocol("a bad chunk".into()))?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| HttpError::Protocol("a bad chunk size".into()))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(Some(body));
        }
        if rest.len() < size + 2 {
            return Ok(None);
        }
        body.extend_from_slice(&rest[..size]);
        if body.len() > MAX_BODY {
            return Err(HttpError::Protocol("the answer is too large".into()));
        }
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_taken_apart() {
        let u = Url::parse("https://dav.example.com/remote.php/dav/").unwrap();
        assert_eq!((u.secure, u.host.as_str(), u.port, u.path.as_str()), (true, "dav.example.com", 443, "/remote.php/dav/"));
        let u = Url::parse("dav.example.com").unwrap();
        assert_eq!((u.secure, u.port, u.path.as_str()), (true, 443, "/"));
        let u = Url::parse("http://127.0.0.1:5232/ann/").unwrap();
        assert_eq!((u.secure, u.host.as_str(), u.port), (false, "127.0.0.1", 5232));
        assert_eq!(u.to_string(), "http://127.0.0.1:5232/ann/");
        assert_eq!(Url::parse("https://DAV.example.com:8443/x").unwrap().to_string(), "https://dav.example.com:8443/x");
        for bad in ["", "ftp://x/", "https://user:pw@host/", "https://", "https://host:port/"] {
            assert_eq!(Url::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn links_are_followed_from_where_they_were_found() {
        let at = Url::parse("https://dav.example.com/a/b/").unwrap();
        assert_eq!(at.join("/principals/ann/").unwrap().to_string(), "https://dav.example.com/principals/ann/");
        assert_eq!(at.join("c.ics").unwrap().to_string(), "https://dav.example.com/a/b/c.ics");
        let elsewhere = at.join("https://other.example.com/x").unwrap();
        assert!(!at.same_server(&elsewhere) && at.same_server(&at.join("/y").unwrap()));
        assert!(!at.same_server(&Url::parse("http://dav.example.com:443/").unwrap()), "not the same without encryption");
        assert_eq!(at.join("javascript://x"), None);
    }

    #[test]
    fn answers_are_read_whatever_way_their_length_is_given() {
        let plain = b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 5\r\nETag: \"abc\"\r\n\r\nhelloEXTRA";
        let r = parse(plain, false).unwrap();
        assert_eq!((r.status, r.text().as_str(), r.header("etag")), (207, "hello", Some("\"abc\"")));
        assert!(complete(plain) && !complete(&plain[..plain.len() - 8]));

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n6;x=y\r\npedia \r\n0\r\n\r\n";
        assert_eq!(parse(chunked, false).unwrap().text(), "Wikipedia ");
        assert!(!complete(&chunked[..chunked.len() - 5]));

        // No length: everything up to the close.
        assert_eq!(parse(b"HTTP/1.0 200 OK\r\n\r\nall of it", false).unwrap().text(), "all of it");
        // No body where there cannot be one.
        assert_eq!(parse(b"HTTP/1.1 204 No Content\r\n\r\n", false).unwrap().body, b"");
        assert_eq!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n", true).unwrap().body, b"");
        // Cut short, or not HTTP.
        assert!(matches!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort", false), Err(HttpError::Network(_))));
        assert!(matches!(parse(b"SSH-2.0-OpenSSH\r\n\r\n", false), Err(HttpError::Protocol(_))));
    }
}
