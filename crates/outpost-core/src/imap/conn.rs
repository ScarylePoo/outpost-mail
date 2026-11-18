//! One IMAP connection: the socket, TLS, framing, and the command/response
//! exchange. Parsing and serialising are `imap-codec`'s job; this module moves
//! bytes and keeps the conversation in step.
//!
//! Nothing here knows about folders or mail. See `session.rs` for that.

use imap_codec::fragmentizer::Fragmentizer;
use imap_codec::imap_types::command::{Command, CommandBody};
use imap_codec::imap_types::core::{LiteralMode, Tag};
use imap_codec::imap_types::response::{
    Capability, Code, Data, Greeting, GreetingKind, Response, Status, StatusBody, StatusKind,
};
use imap_codec::imap_types::ToStatic;
use imap_codec::encode::{Encoder, Fragment};
use imap_codec::{CommandCodec, GreetingCodec, ResponseCodec};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;

/// How long to wait for the server before giving up on a connection.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// The largest single response accepted. Message bodies are fetched in
/// pieces far smaller than this.
const MAX_RESPONSE: u32 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the first byte (port 993). The default.
    Tls,
    /// No encryption. Only ever used when the user has explicitly chosen it
    /// for one account, and for the local test server.
    Plaintext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub security: Security,
    /// A certificate (DER) to trust for this server instead of the
    /// operating system's trust store. For a server with a self-signed
    /// certificate that the user has chosen to accept.
    pub trusted_certificate: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum ImapError {
    /// Could not reach the server, or the connection dropped.
    Io(std::io::Error),
    /// The server's certificate or TLS handshake was not acceptable.
    Tls(String),
    /// The server did not answer in time.
    Timeout,
    /// The server closed the conversation (BYE).
    Bye(String),
    /// The server answered NO or BAD.
    Rejected { command: &'static str, bad: bool, text: String },
    /// The server said something this client cannot make sense of.
    Protocol(String),
}

impl std::fmt::Display for ImapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImapError::Io(e) => write!(f, "connection error: {e}"),
            ImapError::Tls(e) => write!(f, "could not establish a secure connection: {e}"),
            ImapError::Timeout => write!(f, "the server did not respond in time"),
            ImapError::Bye(text) => write!(f, "the server closed the connection: {text}"),
            ImapError::Rejected { command, text, .. } => {
                write!(f, "the server refused {command}: {text}")
            }
            ImapError::Protocol(what) => write!(f, "unexpected reply from the server: {what}"),
        }
    }
}

impl std::error::Error for ImapError {}

impl From<std::io::Error> for ImapError {
    fn from(e: std::io::Error) -> Self {
        ImapError::Io(e)
    }
}

impl ImapError {
    /// Whether trying again later could help. A refused login will not get
    /// better by retrying; a dropped connection will.
    pub fn is_transient(&self) -> bool {
        matches!(self, ImapError::Io(_) | ImapError::Timeout | ImapError::Bye(_))
    }
}

pub type Result<T> = std::result::Result<T, ImapError>;

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// Something the server sent that was not the final answer to a command.
#[derive(Debug)]
pub enum Untagged {
    Data(Data<'static>),
    /// `* OK`, `* NO` or `* BAD`, usually carrying a response code.
    Status(StatusBody<'static>),
}

pub struct Connection {
    stream: Box<dyn Stream>,
    fragments: Fragmentizer,
    next_tag: u32,
    capabilities: BTreeSet<String>,
    /// Extra TLS roots, for tests against a server with a private certificate.
    pub(crate) greeting_was_preauth: bool,
}

fn tls_config(trusted: Option<&[u8]>) -> std::result::Result<Arc<ClientConfig>, String> {
    use rustls_platform_verifier::BuilderVerifierExt;
    use tokio_rustls::rustls::crypto::ring::default_provider;
    use tokio_rustls::rustls::pki_types::CertificateDer;
    use tokio_rustls::rustls::RootCertStore;
    let builder = ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let config = match trusted {
        // Normally certificates are checked against the operating system's
        // trust store, so a private or company certificate authority
        // installed there works.
        None => builder
            .with_platform_verifier()
            .map_err(|e| e.to_string())?
            .with_no_client_auth(),
        // With a certificate the user accepted, exactly that one is trusted.
        // The name on it must still match the server's.
        Some(der) => {
            let mut roots = RootCertStore::empty();
            roots
                .add(CertificateDer::from(der.to_vec()))
                .map_err(|e| format!("the saved certificate is not usable: {e}"))?;
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    Ok(Arc::new(config))
}

impl Connection {
    /// Connects and reads the greeting.
    pub async fn open(server: &Server) -> Result<Connection> {
        let tcp = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect((server.host.as_str(), server.port)),
        )
        .await
        .map_err(|_| ImapError::Timeout)??;
        tcp.set_nodelay(true)?;

        let stream: Box<dyn Stream> = match server.security {
            Security::Plaintext => Box::new(tcp),
            Security::Tls => {
                let name = ServerName::try_from(server.host.clone())
                    .map_err(|e| ImapError::Tls(e.to_string()))?;
                let config = tls_config(server.trusted_certificate.as_deref());
                let connector = TlsConnector::from(config.map_err(ImapError::Tls)?);
                let tls = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, tcp))
                    .await
                    .map_err(|_| ImapError::Timeout)?
                    .map_err(|e| ImapError::Tls(e.to_string()))?;
                Box::new(tls)
            }
        };

        let mut conn = Connection {
            stream,
            fragments: Fragmentizer::new(MAX_RESPONSE),
            next_tag: 0,
            capabilities: BTreeSet::new(),
            greeting_was_preauth: false,
        };
        let greeting = conn.read_greeting().await?;
        match greeting.kind {
            GreetingKind::Ok => {}
            GreetingKind::PreAuth => conn.greeting_was_preauth = true,
            GreetingKind::Bye => return Err(ImapError::Bye(greeting.text.to_string())),
        }
        if let Some(Code::Capability(caps)) = &greeting.code {
            conn.set_capabilities(caps.as_ref());
        }
        Ok(conn)
    }

    /// Whether the server has advertised `name` (case-insensitive), in the
    /// most recent capability list it sent.
    pub fn has(&self, name: &str) -> bool {
        self.capabilities.contains(&name.to_ascii_uppercase())
    }

    /// Everything the server last advertised, upper-cased and sorted.
    pub fn capabilities(&self) -> Vec<String> {
        self.capabilities.iter().cloned().collect()
    }

    pub fn capabilities_known(&self) -> bool {
        !self.capabilities.is_empty()
    }

    fn set_capabilities(&mut self, caps: &[Capability<'_>]) {
        self.capabilities = caps.iter().map(|c| c.to_string().to_ascii_uppercase()).collect();
    }

    // ---- reading ----------------------------------------------------------

    /// Reads from the socket until the fragmentizer holds a whole message.
    async fn fill(&mut self, timeout: Duration) -> Result<()> {
        loop {
            while self.fragments.progress().is_some() {
                if self.fragments.is_message_complete() {
                    return Ok(());
                }
            }
            let mut buffer = [0u8; 16 * 1024];
            let n = tokio::time::timeout(timeout, self.stream.read(&mut buffer))
                .await
                .map_err(|_| ImapError::Timeout)??;
            if n == 0 {
                return Err(ImapError::Io(std::io::ErrorKind::UnexpectedEof.into()));
            }
            self.fragments.enqueue_bytes(&buffer[..n]);
        }
    }

    async fn read_greeting(&mut self) -> Result<Greeting<'static>> {
        self.fill(RESPONSE_TIMEOUT).await?;
        self.fragments
            .decode_message(&GreetingCodec::default())
            .map(|g| g.to_static())
            .map_err(|_| ImapError::Protocol("the greeting was not an IMAP greeting".into()))
    }

    /// The next response. `Ok(None)` means the server sent a line this
    /// client could not parse and it was skipped; the conversation is still
    /// in step, because lines are framed independently of their meaning.
    async fn read_response(&mut self, timeout: Duration) -> Result<Option<Response<'static>>> {
        self.fill(timeout).await?;
        if self.fragments.is_max_message_size_exceeded() {
            return Err(ImapError::Protocol("a response was too large".into()));
        }
        match self.fragments.decode_message(&ResponseCodec::default()) {
            Ok(response) => Ok(Some(response.to_static())),
            Err(_) => {
                let line = self.fragments.message_bytes();
                let shown = String::from_utf8_lossy(&line[..line.len().min(120)]).trim().to_string();
                // An unparseable *final* answer cannot be skipped: the
                // command it answers would wait for ever.
                if !line.starts_with(b"* ") && !line.starts_with(b"+") {
                    return Err(ImapError::Protocol(shown));
                }
                Ok(None)
            }
        }
    }

    // ---- writing ----------------------------------------------------------

    /// Sends a command and returns its tag. Untagged responses that arrive
    /// while waiting to send a literal are passed to `on_untagged`.
    async fn send(
        &mut self,
        body: CommandBody<'_>,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<String> {
        self.next_tag += 1;
        let tag = format!("A{}", self.next_tag);
        let name = body.name();
        let command = Command::new(tag.as_str(), body)
            .map_err(|_| ImapError::Protocol("could not build a command tag".into()))?;
        for fragment in CommandCodec::default().encode(&command) {
            match fragment {
                Fragment::Line { data } => self.stream.write_all(&data).await?,
                Fragment::Literal { data, mode } => {
                    if mode == LiteralMode::Sync {
                        // The server must invite the literal before it is sent.
                        self.stream.flush().await?;
                        self.await_continuation(&tag, name, on_untagged).await?;
                    }
                    self.stream.write_all(&data).await?;
                }
            }
        }
        self.stream.flush().await?;
        Ok(tag)
    }

    async fn await_continuation(
        &mut self,
        tag: &str,
        command: &'static str,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<()> {
        loop {
            match self.read_response(RESPONSE_TIMEOUT).await? {
                None => {}
                Some(Response::CommandContinuationRequest(_)) => return Ok(()),
                Some(Response::Data(data)) => on_untagged(Untagged::Data(data)),
                Some(Response::Status(status)) => {
                    if let Some(done) = self.status(status, tag, command, on_untagged)? {
                        // Answered before the literal was sent: OK makes no
                        // sense here, and NO/BAD already became an error.
                        return Err(ImapError::Protocol(format!(
                            "{command} finished early: {}",
                            done.text
                        )));
                    }
                }
            }
        }
    }

    /// Handles one status response. Returns the body if it completes the
    /// command tagged `tag` successfully.
    fn status(
        &mut self,
        status: Status<'static>,
        tag: &str,
        command: &'static str,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<Option<StatusBody<'static>>> {
        match status {
            Status::Bye(bye) => Err(ImapError::Bye(bye.text.to_string())),
            Status::Untagged(body) => {
                if let Some(Code::Capability(caps)) = &body.code {
                    self.set_capabilities(caps.as_ref());
                }
                on_untagged(Untagged::Status(body));
                Ok(None)
            }
            Status::Tagged(tagged) => {
                if tagged.tag != Tag::try_from(tag).expect("our own tag is valid") {
                    return Err(ImapError::Protocol(format!(
                        "answer to {:?} while waiting for {tag}",
                        tagged.tag
                    )));
                }
                if let Some(Code::Capability(caps)) = &tagged.body.code {
                    self.set_capabilities(caps.as_ref());
                }
                match tagged.body.kind {
                    StatusKind::Ok => Ok(Some(tagged.body)),
                    kind => Err(ImapError::Rejected {
                        command,
                        bad: kind == StatusKind::Bad,
                        text: tagged.body.text.to_string(),
                    }),
                }
            }
        }
    }

    // ---- commands ---------------------------------------------------------

    /// Runs one command to completion. Everything the server sends before
    /// the final answer goes to `on_untagged`. Returns the final OK.
    pub async fn run(
        &mut self,
        body: CommandBody<'_>,
        mut on_untagged: impl FnMut(Untagged) + Send,
    ) -> Result<StatusBody<'static>> {
        let name = body.name();
        let tag = self.send(body, &mut on_untagged).await?;
        self.finish(&tag, name, &mut on_untagged).await
    }

    async fn finish(
        &mut self,
        tag: &str,
        command: &'static str,
        on_untagged: &mut (dyn FnMut(Untagged) + Send),
    ) -> Result<StatusBody<'static>> {
        loop {
            match self.read_response(RESPONSE_TIMEOUT).await? {
                None => {}
                Some(Response::CommandContinuationRequest(_)) => {
                    return Err(ImapError::Protocol(format!(
                        "the server asked for more data during {command}"
                    )))
                }
                Some(Response::Data(Data::Capability(caps))) => {
                    self.set_capabilities(caps.as_ref())
                }
                Some(Response::Data(data)) => on_untagged(Untagged::Data(data)),
                Some(Response::Status(status)) => {
                    if let Some(done) = self.status(status, tag, command, on_untagged)? {
                        return Ok(done);
                    }
                }
            }
        }
    }

    /// Waits in IDLE until `wake` resolves, `limit` passes, or the server
    /// says something; then ends the IDLE. Returns what the server sent.
    ///
    /// The caller is expected to act on the result and idle again. Servers
    /// drop idle connections after about 30 minutes, so keep `limit` under
    /// that.
    pub async fn idle(
        &mut self,
        limit: Duration,
        wake: impl std::future::Future<Output = ()> + Send,
    ) -> Result<Vec<Untagged>> {
        let mut seen = Vec::new();
        let tag = self.send(CommandBody::Idle, &mut |u| seen.push(u)).await?;
        // The server confirms with a continuation request.
        loop {
            match self.read_response(RESPONSE_TIMEOUT).await? {
                None => {}
                Some(Response::CommandContinuationRequest(_)) => break,
                Some(Response::Data(data)) => seen.push(Untagged::Data(data)),
                Some(Response::Status(status)) => {
                    if self.status(status, &tag, "IDLE", &mut |u| seen.push(u))?.is_some() {
                        return Err(ImapError::Protocol("IDLE ended before it began".into()));
                    }
                }
            }
        }

        // Wait for the first thing to happen. Reading must not be cancelled
        // half-way through a response, so the race is only on whether any
        // bytes are ready, never on parsing them.
        tokio::pin!(wake);
        let deadline = tokio::time::sleep(limit);
        tokio::pin!(deadline);
        let mut buffer = [0u8; 16 * 1024];
        let mut woken = !seen.is_empty();
        while !woken {
            tokio::select! {
                _ = &mut wake => woken = true,
                _ = &mut deadline => woken = true,
                read = self.stream.read(&mut buffer) => {
                    let n = read?;
                    if n == 0 {
                        return Err(ImapError::Io(std::io::ErrorKind::UnexpectedEof.into()));
                    }
                    self.fragments.enqueue_bytes(&buffer[..n]);
                    woken = true;
                }
            }
        }

        self.stream.write_all(b"DONE\r\n").await?;
        self.stream.flush().await?;
        self.finish(&tag, "IDLE", &mut |u| seen.push(u)).await?;
        Ok(seen)
    }
}
