//! A private Dovecot for tests, and a second, independent IMAP client.
//!
//! Each test starts its own server in a temporary directory on a free local
//! port, running as the current user, so tests cannot affect each other or
//! any real mail. Any user name is accepted with the password `testpass`.
//!
//! Dovecot only exists on Linux here. Where it is not installed the tests
//! report that they were skipped; set OUTPOST_REQUIRE_DOVECOT=1 (as CI does)
//! to make a missing server a failure instead.

#![allow(dead_code)]

pub mod smtp;

use outpost_core::imap::{Security, Server};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const PASSWORD: &str = "testpass";

fn dovecot_binary() -> Option<PathBuf> {
    ["/usr/sbin/dovecot", "/usr/local/sbin/dovecot", "/usr/bin/dovecot"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

fn output_of(program: &str, args: &[&str]) -> String {
    let out = Command::new(program).args(args).output().expect(program);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub struct TestServer {
    dir: PathBuf,
    binary: PathBuf,
    pub port: u16,
    /// The TLS port, if `openssl` was available to make a certificate.
    pub tls_port: Option<u16>,
}

impl TestServer {
    /// Starts a server, or returns `None` (after saying so) if Dovecot is not
    /// installed on this machine.
    pub fn start() -> Option<TestServer> {
        let Some(binary) = dovecot_binary() else {
            if std::env::var_os("OUTPOST_REQUIRE_DOVECOT").is_some() {
                panic!("Dovecot is required for this test run but is not installed");
            }
            eprintln!("SKIPPED: Dovecot is not installed, so this sync test did not run");
            return None;
        };
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "outpost-dovecot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["run", "state", "home"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (user, group) = (output_of("id", &["-un"]), output_of("id", &["-gn"]));
        // A self-signed certificate for "localhost", for the TLS tests.
        let made_certificate = Command::new("openssl")
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=localhost"])
            .args(["-addext", "subjectAltName=DNS:localhost", "-addext", "basicConstraints=critical,CA:FALSE"])
            .arg("-keyout").arg(dir.join("key.pem"))
            .arg("-out").arg(dir.join("cert.pem"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        let tls_port = made_certificate
            .then(|| TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port());
        let ssl = match tls_port {
            Some(_) => format!("ssl = yes\nssl_server_cert_file = {0}/cert.pem\nssl_server_key_file = {0}/key.pem", dir.display()),
            None => "ssl = no".to_string(),
        };
        let tls_listener = tls_port.unwrap_or(0);
        let version = output_of(binary.to_str().unwrap(), &["--version"]);
        let version = version.split_whitespace().next().unwrap_or("2.4.0").to_string();
        let d = dir.display();
        let config = format!(
            r#"dovecot_config_version = {version}
dovecot_storage_version = {version}
base_dir = {d}/run
state_dir = {d}/state
instance_name = outpost-test-{port}
log_path = {d}/dovecot.log
protocols = imap
listen = 127.0.0.1
default_internal_user = {user}
default_internal_group = {group}
default_login_user = {user}
first_valid_uid = 1
{ssl}
auth_allow_cleartext = yes
auth_mechanisms = plain login
auth_failure_delay = 0
mail_driver = maildir
mail_home = {d}/home/%{{user}}
mail_path = ~/Maildir
mail_uid = {user}
mail_gid = {group}

passdb static {{
  password = {PASSWORD}
}}
userdb static {{
}}

namespace inbox {{
  inbox = yes
  separator = /
  mailbox Drafts {{
    special_use = \Drafts
    auto = subscribe
  }}
  mailbox Sent {{
    special_use = \Sent
    auto = subscribe
  }}
  mailbox Trash {{
    special_use = \Trash
    auto = subscribe
  }}
  mailbox Junk {{
    special_use = \Junk
    auto = subscribe
  }}
  mailbox Archive {{
    special_use = \Archive
    auto = subscribe
  }}
}}

service imap-login {{
  chroot =
  inet_listener imap {{
    port = {port}
  }}
  inet_listener imaps {{
    port = {tls_listener}
    ssl = yes
  }}
}}
service anvil {{
  chroot =
}}
service imap {{
  chroot =
}}
service stats {{
  chroot =
}}
"#
        );
        std::fs::write(dir.join("dovecot.conf"), config).unwrap();
        let server = TestServer { dir, binary, port, tls_port };
        let status = server.dovecot(&[]);
        assert!(status, "Dovecot did not start; see {}", server.dir.join("dovecot.log").display());

        // Ready when it greets.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() && line.contains("ready") {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "Dovecot did not become ready");
            std::thread::sleep(Duration::from_millis(50));
        }
        Some(server)
    }

    fn dovecot(&self, args: &[&str]) -> bool {
        Command::new(&self.binary)
            .arg("-c")
            .arg(self.dir.join("dovecot.conf"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// How the code under test should reach this server.
    pub fn server(&self) -> Server {
        Server {
            host: "127.0.0.1".into(),
            port: self.port,
            security: Security::Plaintext,
            trusted_certificate: None,
        }
    }

    /// The same server over TLS, trusting its self-signed certificate.
    /// `None` if no certificate could be made on this machine.
    pub fn tls_server(&self) -> Option<Server> {
        Some(Server {
            host: "localhost".into(),
            port: self.tls_port?,
            security: Security::Tls,
            trusted_certificate: Some(self.certificate_der()),
        })
    }

    pub fn certificate_der(&self) -> Vec<u8> {
        let out = Command::new("openssl")
            .args(["x509", "-outform", "DER", "-in"])
            .arg(self.dir.join("cert.pem"))
            .output()
            .unwrap();
        out.stdout
    }

    /// Signs in as `user` with the independent client.
    pub fn other_client(&self, user: &str) -> OtherClient {
        OtherClient::connect(self.port, user)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.dovecot(&["stop"]);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A second mail client, written separately from the code under test: plain
/// text over a socket, with no shared parsing. Tests use it to change the
/// mailbox "from somewhere else" and to check what the server holds.
pub struct OtherClient {
    reader: BufReader<TcpStream>,
    tag: u32,
}

impl OtherClient {
    fn connect(port: u16, user: &str) -> OtherClient {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut client = OtherClient {
            reader: BufReader::new(stream),
            tag: 0,
        };
        let mut greeting = String::new();
        client.reader.read_line(&mut greeting).unwrap();
        client.cmd(&format!("LOGIN {user} {PASSWORD}"));
        client
    }

    /// Runs a command, expecting OK. Returns every line the server sent.
    pub fn cmd(&mut self, command: &str) -> Vec<String> {
        self.tag += 1;
        let tag = format!("T{}", self.tag);
        self.reader.get_mut().write_all(format!("{tag} {command}\r\n").as_bytes()).unwrap();
        self.read_until(&tag, command)
    }

    fn read_until(&mut self, tag: &str, command: &str) -> Vec<String> {
        let mut lines = Vec::new();
        loop {
            let mut raw = Vec::new();
            self.reader.read_until(b'\n', &mut raw).unwrap();
            assert!(!raw.is_empty(), "connection closed during {command}");
            let line = String::from_utf8_lossy(&raw).trim_end().to_string();
            // A literal follows: read it and attach it to the line.
            if let Some(size) = line.strip_suffix('}').and_then(|l| l.rsplit_once('{')).map(|(_, n)| n) {
                if let Ok(size) = size.parse::<usize>() {
                    let mut literal = vec![0; size];
                    self.reader.read_exact(&mut literal).unwrap();
                    lines.push(format!("{line}{}", String::from_utf8_lossy(&literal)));
                    continue;
                }
            }
            if let Some(rest) = line.strip_prefix(&format!("{tag} ")) {
                assert!(rest.starts_with("OK"), "{command} failed: {line}");
                lines.push(line);
                return lines;
            }
            lines.push(line);
        }
    }

    /// Adds a message to a folder. `flags` is IMAP syntax, such as `\Seen`.
    pub fn append(&mut self, folder: &str, flags: &str, message: &[u8]) {
        self.tag += 1;
        let tag = format!("T{}", self.tag);
        let stream = self.reader.get_mut();
        stream
            .write_all(format!("{tag} APPEND \"{folder}\" ({flags}) {{{}}}\r\n", message.len()).as_bytes())
            .unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).unwrap();
        assert!(line.starts_with('+'), "APPEND was not invited: {line}");
        let stream = self.reader.get_mut();
        stream.write_all(message).unwrap();
        stream.write_all(b"\r\n").unwrap();
        self.read_until(&tag, "APPEND");
    }

    /// `UID FLAGS` of every message in a folder, as the server has them now.
    pub fn flags_in(&mut self, folder: &str) -> Vec<String> {
        self.cmd(&format!("EXAMINE \"{folder}\""));
        let mut lines: Vec<String> = self
            .cmd("UID FETCH 1:* (FLAGS)")
            .into_iter()
            .filter(|l| l.contains("FETCH ("))
            .map(|l| l.split_once("FETCH ").unwrap().1.to_string())
            .collect();
        lines.sort();
        lines
    }
}

/// A plain test message.
pub fn message(n: u32, subject: &str, body: &str) -> Vec<u8> {
    format!(
        "From: Sender {n} <sender{n}@example.com>\r\nTo: Test User <user@example.com>\r\n\
         Subject: {subject}\r\nDate: Mon, 2 Nov 2026 09:{:02}:{:02} +0000\r\n\
         Message-ID: <{n}.{subject_id}@example.com>\r\n\r\n{body}\r\n",
        n / 60 % 60,
        n % 60,
        subject_id = subject.len(),
    )
    .into_bytes()
}

/// A directory that is deleted when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "outpost-sync-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
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

/// Sits between the code under test and the server, and can cut the
/// connection after a set number of bytes have come back from the server:
/// a dropped connection in the middle of whatever was happening.
pub struct FlakyProxy {
    pub port: u16,
    /// Bytes the server may still send before the connection is cut.
    /// Negative means no limit.
    budget: std::sync::Arc<std::sync::atomic::AtomicI64>,
}

impl FlakyProxy {
    pub fn start(server_port: u16) -> FlakyProxy {
        use std::sync::atomic::AtomicI64;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let budget = std::sync::Arc::new(AtomicI64::new(-1));
        let shared = budget.clone();
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { break };
                let Ok(server) = TcpStream::connect(("127.0.0.1", server_port)) else { continue };
                let (mut client_in, mut server_out) = (client.try_clone().unwrap(), server.try_clone().unwrap());
                // Client to server: passed through untouched.
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut client_in, &mut server_out);
                    let _ = server_out.shutdown(std::net::Shutdown::Both);
                });
                // Server to client: counted against the budget.
                let budget = shared.clone();
                let (mut server_in, mut client_out) = (server, client);
                std::thread::spawn(move || {
                    let mut buffer = [0u8; 4096];
                    loop {
                        let Ok(n) = server_in.read(&mut buffer) else { break };
                        if n == 0 {
                            break;
                        }
                        let left = budget.load(Ordering::SeqCst);
                        let allowed = if left < 0 { n } else { n.min(left as usize) };
                        if left >= 0 {
                            budget.store(left - allowed as i64, Ordering::SeqCst);
                        }
                        if client_out.write_all(&buffer[..allowed]).is_err() || allowed < n {
                            break;
                        }
                    }
                    let _ = client_out.shutdown(std::net::Shutdown::Both);
                    let _ = server_in.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        FlakyProxy { port, budget }
    }

    /// Cuts the connection once the server has sent `bytes` more bytes.
    pub fn cut_after(&self, bytes: i64) {
        self.budget.store(bytes, Ordering::SeqCst);
    }

    /// Lets everything through again (for connections made from now on).
    pub fn heal(&self) {
        self.budget.store(-1, Ordering::SeqCst);
    }

    pub fn server(&self) -> Server {
        Server {
            host: "127.0.0.1".into(),
            port: self.port,
            security: Security::Plaintext,
            trusted_certificate: None,
        }
    }
}

impl OtherClient {
    /// (subject, flags) of every message in a folder, as the server has
    /// them now, sorted by subject.
    pub fn subjects_in(&mut self, folder: &str) -> Vec<(String, String)> {
        self.cmd(&format!("EXAMINE \"{folder}\""));
        let mut found: Vec<(String, String)> = self
            .cmd("UID FETCH 1:* (FLAGS BODY.PEEK[HEADER.FIELDS (SUBJECT)])")
            .into_iter()
            .filter(|l| l.contains("FETCH ("))
            .map(|l| {
                let subject = l
                    .split_once("Subject: ")
                    .map(|(_, rest)| rest.lines().next().unwrap_or("").trim().to_string())
                    .unwrap_or_default();
                // Flags sorted, without the ones the server adds by itself.
                let mut flags: Vec<&str> = l
                    .split_once("FLAGS (")
                    .and_then(|(_, rest)| rest.split_once(')'))
                    .map(|(f, _)| f.split_whitespace().collect())
                    .unwrap_or_default();
                flags.retain(|f| *f != "\\Recent" && !f.starts_with("$Has"));
                flags.sort();
                let flags = flags.join(" ");
                (subject, flags)
            })
            .collect();
        found.sort();
        found
    }
}
