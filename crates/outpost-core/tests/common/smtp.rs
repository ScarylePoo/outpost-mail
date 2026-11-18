//! A fake outgoing mail server for tests. It accepts mail and keeps it; it
//! never delivers anything anywhere. It can be told to misbehave in the ways
//! real servers do.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    pub from: String,
    pub recipients: Vec<String>,
    /// The message as it would be delivered (transport encoding undone).
    pub message: Vec<u8>,
    pub signed_in_as: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behaviour {
    Normal,
    /// Offers only the LOGIN way of signing in.
    LoginOnly,
    /// Greets with "try again later".
    Busy,
    /// Accepts the message body, then says no.
    RefuseMessage,
    /// Accepts the message body (and keeps it), then drops the connection
    /// without saying whether it was accepted.
    VanishAfterMessage,
}

pub struct SmtpSink {
    pub port: u16,
    pub received: Arc<Mutex<Vec<Received>>>,
    behaviour: Arc<Mutex<Behaviour>>,
}

fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut bits = 0u32;
    let mut count = 0;
    for byte in text.bytes() {
        let Some(value) = ALPHABET.iter().position(|a| *a == byte) else { continue };
        bits = (bits << 6) | value as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out
}

impl SmtpSink {
    pub fn start() -> SmtpSink {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let received: Arc<Mutex<Vec<Received>>> = Arc::default();
        let behaviour = Arc::new(Mutex::new(Behaviour::Normal));
        let (kept, mode) = (received.clone(), behaviour.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let (kept, mode) = (kept.clone(), *mode.lock().unwrap());
                std::thread::spawn(move || {
                    let _ = serve(stream, mode, kept);
                });
            }
        });
        SmtpSink { port, received, behaviour }
    }

    pub fn behave(&self, behaviour: Behaviour) {
        *self.behaviour.lock().unwrap() = behaviour;
    }

    pub fn messages(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn serve(stream: TcpStream, mode: Behaviour, kept: Arc<Mutex<Vec<Received>>>) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    let mut lines = BufReader::new(stream);
    let mut say = |text: &str| out.write_all(format!("{text}\r\n").as_bytes());
    if mode == Behaviour::Busy {
        return say("421 too busy, try again later");
    }
    say("220 sink ready")?;
    let mut from = String::new();
    let mut recipients: Vec<String> = Vec::new();
    let mut user: Option<String> = None;
    let read = |lines: &mut BufReader<TcpStream>| -> std::io::Result<String> {
        let mut line = String::new();
        if lines.read_line(&mut line)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        Ok(line.trim_end().to_string())
    };
    loop {
        let line = read(&mut lines)?;
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") {
            say("250-sink")?;
            say(if mode == Behaviour::LoginOnly { "250-AUTH LOGIN" } else { "250-AUTH PLAIN LOGIN" })?;
            say("250 8BITMIME")?;
        } else if upper.starts_with("AUTH PLAIN") {
            let decoded = decode_base64(line.get(11..).unwrap_or(""));
            let parts: Vec<String> = decoded.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
            if parts.get(2).map(String::as_str) == Some(super::PASSWORD) {
                user = parts.get(1).cloned();
                say("235 welcome")?;
            } else {
                say("535 authentication failed")?;
            }
        } else if upper.starts_with("AUTH LOGIN") {
            say("334 VXNlcm5hbWU6")?;
            let name = String::from_utf8_lossy(&decode_base64(&read(&mut lines)?)).into_owned();
            say("334 UGFzc3dvcmQ6")?;
            let password = String::from_utf8_lossy(&decode_base64(&read(&mut lines)?)).into_owned();
            if password == super::PASSWORD {
                user = Some(name);
                say("235 welcome")?;
            } else {
                say("535 authentication failed")?;
            }
        } else if upper.starts_with("MAIL FROM:") {
            from = line[10..].split('>').next().unwrap_or("").trim_start_matches('<').to_string();
            recipients.clear();
            say("250 ok")?;
        } else if upper.starts_with("RCPT TO:") {
            let to = line[8..].split('>').next().unwrap_or("").trim_start_matches('<').to_string();
            if to.starts_with("nobody@") {
                say("550 no such user here")?;
            } else {
                recipients.push(to);
                say("250 ok")?;
            }
        } else if upper == "DATA" {
            say("354 go ahead")?;
            let mut message = Vec::new();
            loop {
                let mut raw = Vec::new();
                if lines.read_until(b'\n', &mut raw)? == 0 {
                    return Ok(());
                }
                if raw == b".\r\n" {
                    break;
                }
                // Undo the doubling of leading dots.
                message.extend_from_slice(if raw.starts_with(b".") { &raw[1..] } else { &raw });
            }
            if mode == Behaviour::RefuseMessage {
                say("554 message refused")?;
                continue;
            }
            kept.lock().unwrap().push(Received {
                from: from.clone(),
                recipients: recipients.clone(),
                message,
                signed_in_as: user.clone(),
            });
            if mode == Behaviour::VanishAfterMessage {
                return Ok(());
            }
            say("250 accepted")?;
        } else if upper == "QUIT" {
            return say("221 bye");
        } else {
            say("502 not implemented")?;
        }
    }
}
