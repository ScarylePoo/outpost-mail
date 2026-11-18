//! Writing mail: turning what the composer holds into a message, and
//! turning an existing message into the starting point for a reply, a
//! forward, or further editing of a draft.

use crate::api::{DraftAttachment, DraftContent};
use crate::render::base64;
use mail_builder::headers::address::Address;
use mail_builder::MessageBuilder;
use mail_parser::{AddressList, MessageParser};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub name: String,
    pub address: String,
}

/// Who is writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub address: String,
}

/// Reads a list of recipients as people type them: separated by commas or
/// semicolons, each either a bare address or `Name <address>`.
pub fn parse_recipients(text: &str) -> Result<Vec<Recipient>, String> {
    let mut out = Vec::new();
    let mut part = String::new();
    let mut quoted = false;
    let mut parts: Vec<String> = Vec::new();
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                part.push(c);
            }
            ',' | ';' | '\n' if !quoted => parts.push(std::mem::take(&mut part)),
            _ => part.push(c),
        }
    }
    parts.push(part);
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, address) = match (part.rfind('<'), part.rfind('>')) {
            (Some(open), Some(close)) if open < close => (
                part[..open].trim().trim_matches('"').trim().to_string(),
                part[open + 1..close].trim().to_string(),
            ),
            _ => (String::new(), part.to_string()),
        };
        let valid = address.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        }) && !address.contains([' ', '<', '>', ',', ';', '"', '\r', '\n', '\t']);
        if !valid {
            return Err(format!("\"{part}\" is not an email address"));
        }
        out.push(Recipient { name, address });
    }
    Ok(out)
}

fn addresses(list: &[Recipient]) -> Address<'_> {
    Address::new_list(
        list.iter()
            .map(|r| Address::new_address((!r.name.is_empty()).then_some(r.name.as_str()), r.address.as_str()))
            .collect(),
    )
}

fn unique_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the operating system's randomness.
    let random = |salt: u64| {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.finish()
    };
    format!("{:016x}{:016x}", random(1), random(2))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut bits, mut count) = (0u32, 0);
    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = ALPHABET.iter().position(|a| *a == byte)? as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

/// A built message, ready to store and send.
pub struct Built {
    pub raw: Vec<u8>,
    pub message_id: String,
}

/// Builds the message the composer describes. `carried` are attachments
/// taken from another message (when forwarding), as (name, type, bytes).
/// Fails with words for the user if a recipient is not an address.
pub fn build(
    from: &Identity,
    draft: &DraftContent,
    carried: &[(String, String, Vec<u8>)],
) -> Result<Built, String> {
    let (to, cc, bcc) = (
        parse_recipients(&draft.to)?,
        parse_recipients(&draft.cc)?,
        parse_recipients(&draft.bcc)?,
    );
    let domain = from.address.split_once('@').map_or("localhost", |(_, d)| d);
    let message_id = format!("{}@{domain}", unique_token());
    let mut builder = MessageBuilder::new()
        .from(Address::new_address(
            (!from.name.is_empty()).then_some(from.name.as_str()),
            from.address.as_str(),
        ))
        .subject(draft.subject.as_str())
        .date(now())
        .message_id(message_id.clone())
        .text_body(draft.body.replace("\r\n", "\n").replace('\n', "\r\n"));
    if !to.is_empty() {
        builder = builder.to(addresses(&to));
    }
    if !cc.is_empty() {
        builder = builder.cc(addresses(&cc));
    }
    if !bcc.is_empty() {
        builder = builder.bcc(addresses(&bcc));
    }
    if let Some(parent) = draft.in_reply_to.as_deref().filter(|p| !p.is_empty()) {
        builder = builder.in_reply_to(parent.to_string());
    }
    if !draft.references.is_empty() {
        builder = builder.references(draft.references.clone());
    }
    for (name, mime_type, data) in carried {
        builder = builder.attachment(mime_type.clone(), name.clone(), data.clone());
    }
    for attachment in &draft.attachments {
        let data = decode_base64(&attachment.data_base64)
            .ok_or_else(|| format!("the attachment \"{}\" could not be read", attachment.name))?;
        let mime_type = if attachment.mime_type.is_empty() {
            "application/octet-stream".to_string()
        } else {
            attachment.mime_type.clone()
        };
        builder = builder.attachment(mime_type, attachment.name.clone(), data);
    }
    let raw = builder.write_to_vec().map_err(|e| e.to_string())?;
    Ok(Built { raw, message_id })
}

/// Who a stored message is from and everyone it is to (To, Cc and Bcc), as
/// bare addresses: the envelope for sending it.
pub fn envelope(raw: &[u8]) -> Option<(String, Vec<String>)> {
    let message = MessageParser::default().parse(raw)?;
    let from = message.from()?.first()?.address()?.to_string();
    let mut recipients = Vec::new();
    for list in [message.to(), message.cc(), message.bcc()].into_iter().flatten() {
        for mailbox in list.mailboxes() {
            if let Some(address) = mailbox.address() {
                if !recipients.iter().any(|r: &String| r.eq_ignore_ascii_case(address)) {
                    recipients.push(address.to_string());
                }
            }
        }
    }
    Some((from, recipients))
}

/// The message without its Bcc header, which is how it is transmitted:
/// recipients must not see who was blind-copied. The copy kept in Sent
/// retains it.
pub fn without_bcc(raw: &[u8]) -> Vec<u8> {
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n").map_or(raw.len(), |p| p + 2);
    let mut out = Vec::with_capacity(raw.len());
    let mut skipping = false;
    for line in raw[..header_end].split_inclusive(|b| *b == b'\n') {
        let continuation = line.first().is_some_and(|b| *b == b' ' || *b == b'\t');
        if !continuation {
            skipping = line.len() >= 4 && line[..4].eq_ignore_ascii_case(b"bcc:");
        }
        if !skipping {
            out.extend_from_slice(line);
        }
    }
    out.extend_from_slice(&raw[header_end..]);
    out
}

// ---------------------------------------------------------------------------
// Starting points
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    Reply,
    ReplyAll,
    Forward,
}

fn list_text(list: Option<AddressList>, skip: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for mailbox in list.iter().flat_map(|l| l.mailboxes()) {
        let Some(address) = mailbox.address() else { continue };
        if skip(address) {
            continue;
        }
        out.push(match mailbox.name().map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => format!("{name} <{address}>"),
            None => address.to_string(),
        });
    }
    out
}

fn with_prefix(subject: &str, prefix: &str, also: &[&str]) -> String {
    let lower = subject.trim_start().to_lowercase();
    if also.iter().any(|p| lower.starts_with(p)) {
        subject.trim().to_string()
    } else {
        format!("{prefix} {}", subject.trim()).trim().to_string()
    }
}

/// A date for an attribution line: `Mon, 2 Nov 2026 at 09:15`.
fn friendly_date(unix: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {} {} {} at {:02}:{:02} UTC",
        DAYS[days.rem_euclid(7) as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        secs / 3600,
        secs % 3600 / 60
    )
}

/// The starting point for a reply to, or a forward of, a stored message.
/// `mine` says whether an address is one of the user's own, so that a
/// reply to all does not include them.
pub fn respond(raw: &[u8], id: i64, mode: ReplyMode, mine: &dyn Fn(&str) -> bool) -> Option<DraftContent> {
    let message = MessageParser::default().parse(raw)?;
    let subject = message.subject().unwrap_or_default();
    let text = message.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let sender = list_text(message.from(), &|_| false).join(", ");
    let date = message.date().map(|d| friendly_date(d.to_timestamp())).unwrap_or_default();
    let nobody = |_: &str| false;

    if mode == ReplyMode::Forward {
        let mut body = String::from("\n\n---------- Forwarded message ----------\n");
        body.push_str(&format!("From: {sender}\n"));
        if !date.is_empty() {
            body.push_str(&format!("Date: {date}\n"));
        }
        body.push_str(&format!("Subject: {subject}\n"));
        let to = list_text(message.to(), &nobody).join(", ");
        if !to.is_empty() {
            body.push_str(&format!("To: {to}\n"));
        }
        body.push('\n');
        body.push_str(text.trim_end());
        body.push('\n');
        return Some(DraftContent {
            subject: with_prefix(subject, "Fwd:", &["fwd:", "fw:"]),
            body,
            forward_of: Some(id),
            ..DraftContent::default()
        });
    }

    // A reply goes to Reply-To if the sender set one, otherwise to them.
    let mut to = list_text(message.reply_to(), &nobody);
    if to.is_empty() {
        to = list_text(message.from(), &nobody);
    }
    let mut cc = Vec::new();
    if mode == ReplyMode::ReplyAll {
        // If the user wrote the original, "reply all" goes to its recipients.
        let from_me = message.from().and_then(|f| f.first()).and_then(|m| m.address()).is_some_and(mine);
        if from_me {
            to = list_text(message.to(), &|a| mine(a));
        }
        let in_to = to.clone();
        let already = |address: &str| mine(address) || in_to.iter().any(|t| t.contains(address));
        if !from_me {
            cc.extend(list_text(message.to(), &already));
        }
        let so_far = cc.clone();
        cc.extend(list_text(message.cc(), &|a| already(a) || so_far.iter().any(|c| c.contains(a))));
    }

    let mut body = String::from("\n\n");
    body.push_str(&match (date.is_empty(), sender.is_empty()) {
        (false, false) => format!("On {date}, {sender} wrote:\n"),
        (true, false) => format!("{sender} wrote:\n"),
        _ => String::new(),
    });
    for line in text.trim_end().lines() {
        body.push_str(if line.starts_with('>') { ">" } else { "> " });
        body.push_str(line);
        body.push('\n');
    }

    let message_id = message.message_id().unwrap_or_default().to_string();
    let mut references: Vec<String> =
        message.references().map(|r| r.iter().map(String::from).collect()).unwrap_or_default();
    if references.is_empty() {
        // Older clients only set In-Reply-To.
        references.extend(message.in_reply_to().iter().flat_map(|r| r.iter().map(String::from)));
    }
    if !message_id.is_empty() {
        references.push(message_id.clone());
    }
    Some(DraftContent {
        to: to.join(", "),
        cc: cc.join(", "),
        subject: with_prefix(subject, "Re:", &["re:"]),
        body,
        in_reply_to: (!message_id.is_empty()).then_some(message_id),
        references,
        reply_to_message: Some(id),
        ..DraftContent::default()
    })
}

/// A stored draft, back in the form the composer edits.
pub fn reopen(raw: &[u8], id: i64) -> Option<DraftContent> {
    let message = MessageParser::default().parse(raw)?;
    let nobody = |_: &str| false;
    Some(DraftContent {
        to: list_text(message.to(), &nobody).join(", "),
        cc: list_text(message.cc(), &nobody).join(", "),
        bcc: list_text(message.bcc(), &nobody).join(", "),
        subject: message.subject().unwrap_or_default().to_string(),
        body: message.body_text(0).map(|t| t.replace("\r\n", "\n")).unwrap_or_default(),
        in_reply_to: message.in_reply_to().and_then(|r| r.first().map(String::from)),
        references: message.references().map(|r| r.iter().map(String::from).collect()).unwrap_or_default(),
        attachments: message
            .attachments()
            .map(|part| DraftAttachment {
                name: part.attachment_name().unwrap_or("attachment").to_string(),
                mime_type: part
                    .content_type()
                    .map(|c| match c.subtype() {
                        Some(sub) => format!("{}/{sub}", c.ctype()),
                        None => c.ctype().to_string(),
                    })
                    .unwrap_or_default(),
                data_base64: base64(&part.decoded()),
            })
            .collect(),
        replaces: Some(id),
        ..DraftContent::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::parse;

    fn me() -> Identity {
        Identity {
            name: "Ben Owner".into(),
            address: "ben@example.com".into(),
        }
    }

    #[test]
    fn recipients_are_read_as_people_type_them() {
        let got = parse_recipients("alice@example.com; \"Dubois, Dana\" <dana@example.org>,Bob Bergström <bob@example.net>\n carol@example.com,").unwrap();
        let shown: Vec<(&str, &str)> = got.iter().map(|r| (r.name.as_str(), r.address.as_str())).collect();
        assert_eq!(
            shown,
            [("", "alice@example.com"), ("Dubois, Dana", "dana@example.org"), ("Bob Bergström", "bob@example.net"), ("", "carol@example.com")]
        );
        assert_eq!(parse_recipients("").unwrap(), []);
        assert_eq!(parse_recipients(" ,; ").unwrap(), []);
        for bad in ["alice", "alice@", "@example.com", "alice@localhost", "a b@example.com", "x <not an address>", "a@b.c\r\nBcc: evil@example.com"] {
            assert!(parse_recipients(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_built_message_says_what_the_composer_held() {
        let draft = DraftContent {
            to: "Alice Abbott <alice@example.com>".into(),
            cc: "bob@example.net".into(),
            bcc: "secret@example.org".into(),
            subject: "Déjeuner — vendredi?".into(),
            body: "Line one.\nLine two with ünïcödé.\n".into(),
            in_reply_to: Some("parent@example.com".into()),
            references: vec!["root@example.com".into(), "parent@example.com".into()],
            attachments: vec![DraftAttachment { name: "notes.txt".into(), mime_type: "text/plain".into(), data_base64: base64(b"attached text") }],
            ..DraftContent::default()
        };
        let carried = vec![("photo.png".to_string(), "image/png".to_string(), vec![0x89, b'P', b'N', b'G'])];
        let built = build(&me(), &draft, &carried).unwrap();

        let p = parse::parse(&built.raw);
        assert_eq!((p.from_name.as_str(), p.from_addr.as_str()), ("Ben Owner", "ben@example.com"));
        assert_eq!(p.subject, "Déjeuner — vendredi?");
        assert_eq!(p.message_id, built.message_id);
        assert!(built.message_id.ends_with("@example.com"));
        assert_eq!(p.in_reply_to, "parent@example.com");
        assert_eq!(p.references, "root@example.com parent@example.com");
        assert!(p.date.is_some_and(|d| (d - now()).abs() < 60));
        assert!(p.has_attachment);
        let shown = parse::display(&built.raw);
        assert_eq!(shown.text.as_deref().map(str::trim), Some("Line one.\r\nLine two with ünïcödé."));
        let names: Vec<&str> = shown.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["photo.png", "notes.txt"]);
        assert_eq!(parse::attachment(&built.raw, 1).unwrap().1, b"attached text");
        assert_eq!(parse::attachment(&built.raw, 0).unwrap().1, [0x89, b'P', b'N', b'G']);

        // Two messages never share an id.
        assert_ne!(build(&me(), &draft, &[]).unwrap().message_id, built.message_id);
        // A bad recipient stops the build, with words for the user.
        let bad = DraftContent { to: "not-an-address".into(), ..draft.clone() };
        assert!(build(&me(), &bad, &[]).err().unwrap().contains("not-an-address"));

        // The envelope includes blind copies; the transmitted form hides them.
        let (from, recipients) = envelope(&built.raw).unwrap();
        assert_eq!(from, "ben@example.com");
        assert_eq!(recipients, ["alice@example.com", "bob@example.net", "secret@example.org"]);
        let sent = without_bcc(&built.raw);
        assert!(!String::from_utf8_lossy(&sent).contains("secret@example.org"));
        assert!(String::from_utf8_lossy(&built.raw).contains("secret@example.org"));
        assert_eq!(parse::display(&sent).text, shown.text, "the body is untouched");
        assert_eq!(envelope(&sent).unwrap().1, ["alice@example.com", "bob@example.net"]);
    }

    #[test]
    fn bcc_is_removed_even_when_folded_and_only_from_the_header() {
        let raw = b"From: a@example.com\r\nBCC: one@example.com,\r\n\ttwo@example.com\r\nSubject: s\r\n\r\nBcc: this line is body text\r\n";
        assert_eq!(without_bcc(raw), b"From: a@example.com\r\nSubject: s\r\n\r\nBcc: this line is body text\r\n");
    }

    const ORIGINAL: &[u8] = b"From: Alice Abbott <alice@example.com>\r\n\
To: Ben Owner <ben@example.com>, Carol <carol@example.com>\r\n\
Cc: dana@example.org\r\n\
Subject: Lunch on Friday?\r\n\
Date: Mon, 2 Nov 2026 09:15:00 +0000\r\n\
Message-ID: <m2@example.com>\r\n\
References: <m1@example.com>\r\n\
\r\n\
How about Friday?\r\n> Earlier line\r\n";

    fn is_me(address: &str) -> bool {
        address.eq_ignore_ascii_case("ben@example.com")
    }

    #[test]
    fn a_reply_goes_to_the_sender_and_quotes_them() {
        let draft = respond(ORIGINAL, 7, ReplyMode::Reply, &is_me).unwrap();
        assert_eq!(draft.to, "Alice Abbott <alice@example.com>");
        assert_eq!(draft.cc, "");
        assert_eq!(draft.subject, "Re: Lunch on Friday?");
        assert_eq!(draft.body, "\n\nOn Mon, 2 Nov 2026 at 09:15 UTC, Alice Abbott <alice@example.com> wrote:\n> How about Friday?\n>> Earlier line\n");
        assert_eq!(draft.in_reply_to.as_deref(), Some("m2@example.com"));
        assert_eq!(draft.references, ["m1@example.com", "m2@example.com"]);
        assert_eq!(draft.reply_to_message, Some(7));

        // "Re:" is not stacked, and Reply-To is honoured.
        let again = b"From: a@example.com\r\nReply-To: List <list@example.com>\r\nSubject: RE: x\r\n\r\nb\r\n";
        let draft = respond(again, 1, ReplyMode::Reply, &is_me).unwrap();
        assert_eq!((draft.to.as_str(), draft.subject.as_str()), ("List <list@example.com>", "RE: x"));
    }

    #[test]
    fn reply_all_includes_everyone_but_the_user() {
        let draft = respond(ORIGINAL, 7, ReplyMode::ReplyAll, &is_me).unwrap();
        assert_eq!(draft.to, "Alice Abbott <alice@example.com>");
        assert_eq!(draft.cc, "Carol <carol@example.com>, dana@example.org");

        // Replying to all on a message the user sent goes to its recipients.
        let mine = b"From: Ben Owner <ben@example.com>\r\nTo: alice@example.com\r\nCc: carol@example.com\r\nSubject: x\r\n\r\nb\r\n";
        let draft = respond(mine, 1, ReplyMode::ReplyAll, &is_me).unwrap();
        assert_eq!((draft.to.as_str(), draft.cc.as_str()), ("alice@example.com", "carol@example.com"));
    }

    #[test]
    fn a_forward_carries_the_original_and_names_its_source() {
        let draft = respond(ORIGINAL, 7, ReplyMode::Forward, &is_me).unwrap();
        assert_eq!(draft.to, "");
        assert_eq!(draft.subject, "Fwd: Lunch on Friday?");
        assert!(draft.body.contains("---------- Forwarded message ----------\nFrom: Alice Abbott <alice@example.com>\nDate: Mon, 2 Nov 2026 at 09:15 UTC\nSubject: Lunch on Friday?\nTo: Ben Owner <ben@example.com>, Carol <carol@example.com>\n\nHow about Friday?"));
        assert_eq!(draft.forward_of, Some(7));
        assert_eq!(draft.in_reply_to, None);
    }

    #[test]
    fn a_draft_can_be_reopened_as_it_was() {
        let draft = DraftContent {
            to: "Alice Abbott <alice@example.com>, bob@example.net".into(),
            cc: "carol@example.com".into(),
            bcc: "secret@example.org".into(),
            subject: "half written".into(),
            body: "first\nsecond".into(),
            in_reply_to: Some("p@example.com".into()),
            references: vec!["p@example.com".into()],
            attachments: vec![DraftAttachment { name: "a.bin".into(), mime_type: "application/octet-stream".into(), data_base64: base64(&[1, 2, 3, 250]) }],
            ..DraftContent::default()
        };
        let built = build(&me(), &draft, &[]).unwrap();
        let again = reopen(&built.raw, 42).unwrap();
        assert_eq!(again.to, draft.to);
        assert_eq!((again.cc, again.bcc, again.subject), (draft.cc.clone(), draft.bcc.clone(), draft.subject.clone()));
        assert_eq!(again.body.trim_end(), "first\nsecond");
        assert_eq!(again.in_reply_to, draft.in_reply_to);
        assert_eq!(again.references, draft.references);
        assert_eq!(again.attachments, draft.attachments);
        assert_eq!(again.replaces, Some(42));
    }
}
