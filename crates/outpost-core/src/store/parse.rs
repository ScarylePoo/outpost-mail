//! Reads a message file and extracts what the index needs.
//!
//! This is read-only: the stored message is never changed. Anything that
//! cannot be parsed still produces a record, so no file is ever invisible.

use mail_parser::{AddressList, MessageParser, TextList};

/// How many characters of preview are kept. Three list lines never need more.
pub const PREVIEW_CHARS: usize = 320;
/// Body text beyond this many bytes is not added to the search index.
const SEARCH_BODY_LIMIT: usize = 256 * 1024;

/// What the index stores about one message, apart from where it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// Unix seconds from the Date header, if it had a usable one.
    pub date: Option<i64>,
    pub from_name: String,
    pub from_addr: String,
    /// Recipients (To and Cc) as display text.
    pub recipients: String,
    pub subject: String,
    pub message_id: String,
    pub in_reply_to: String,
    /// Space-separated message IDs, oldest first.
    pub references: String,
    pub has_attachment: bool,
    /// The sender's own words: quoted history and signature removed.
    pub preview: String,
    /// Body text for the search index.
    pub body: String,
    /// What the topmost Authentication-Results header adds up to (a
    /// `verify::State` number), and which server wrote it. The verdict only
    /// means anything if that server is the user's own: see `verify.rs`.
    pub verdict: u8,
    pub auth_server: String,
}

pub fn parse(raw: &[u8]) -> Parsed {
    let Some(message) = MessageParser::default().parse(raw) else {
        return Parsed {
            subject: "(unreadable message)".into(),
            ..Parsed::default()
        };
    };

    let (from_name, from_addr) = message.from().map(|l| first_address(&l)).unwrap_or_default();
    let mut recipients = String::new();
    for list in [message.to(), message.cc()].into_iter().flatten() {
        append_addresses(&mut recipients, &list);
    }

    let checked = crate::verify::assess(raw, &from_addr, None);

    let mut body = message.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let preview = preview_of(&body);
    if body.len() > SEARCH_BODY_LIMIT {
        let mut cut = SEARCH_BODY_LIMIT;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }

    let has_attachment = message
        .attachments()
        .any(|part| part.attachment_name().is_some() || part.is_attachment());

    Parsed {
        date: message.date().map(|d| d.to_timestamp()),
        from_name,
        from_addr,
        recipients,
        subject: message.subject().unwrap_or_default().trim().to_string(),
        message_id: message.message_id().unwrap_or_default().to_string(),
        in_reply_to: ids(message.in_reply_to()),
        references: ids(message.references()),
        has_attachment,
        preview,
        body,
        verdict: checked.state as u8,
        auth_server: checked.server,
    }
}

/// A message taken apart for the reading pane. The HTML here is exactly
/// what the sender wrote and is NOT safe to display; see `render.rs`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Displayed {
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub cc: String,
    pub date: Option<i64>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<Attachment>,
    /// Pictures the HTML refers to by content ID (`cid:`).
    pub inline_images: Vec<InlineImage>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InlineImage {
    pub content_id: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub mime_type: String,
    pub size: u64,
}

pub fn display(raw: &[u8]) -> Displayed {
    let Some(message) = MessageParser::default().parse(raw) else {
        return Displayed {
            subject: "(unreadable message)".into(),
            text: Some(String::from_utf8_lossy(raw).into_owned()),
            ..Displayed::default()
        };
    };
    let (from_name, from_addr) = message.from().map(|l| first_address(&l)).unwrap_or_default();
    let mut to = String::new();
    if let Some(list) = message.to() {
        append_addresses(&mut to, &list);
    }
    let mut cc = String::new();
    if let Some(list) = message.cc() {
        append_addresses(&mut cc, &list);
    }
    // Only offer HTML if the sender wrote some; a text-only message is
    // shown as text, not as text dressed up as HTML.
    let has_html = message.html_body().any(|part| {
        part.content_type()
            .is_some_and(|c| c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("html")))
    });
    let html = has_html.then(|| message.body_html(0).map(|h| h.into_owned())).flatten();
    let text = message.body_text(0).map(|t| t.into_owned());
    let mime_of = |part: &mail_parser::MessagePart| {
        part.content_type()
            .map(|c| match c.subtype() {
                Some(sub) => format!("{}/{}", c.ctype(), sub).to_ascii_lowercase(),
                None => c.ctype().to_ascii_lowercase(),
            })
            .unwrap_or_else(|| "application/octet-stream".into())
    };
    let inline_images = message
        .attachments()
        .filter_map(|part| {
            let id = part.content_id()?.trim_matches(['<', '>']).to_string();
            let mime_type = mime_of(&part);
            (mime_type.starts_with("image/") && part.decoded_len() <= INLINE_IMAGE_LIMIT).then(|| InlineImage {
                content_id: id,
                mime_type,
                data: part.decoded().into_owned(),
            })
        })
        .collect();
    let attachments = message
        .attachments()
        .map(|part| Attachment {
            name: part.attachment_name().unwrap_or("(unnamed)").to_string(),
            mime_type: mime_of(&part),
            size: part.decoded_len() as u64,
        })
        .collect();
    Displayed {
        subject: message.subject().unwrap_or_default().trim().to_string(),
        from_name,
        from_addr,
        to,
        cc,
        date: message.date().map(|d| d.to_timestamp()),
        html,
        text,
        attachments,
        inline_images,
    }
}

/// Embedded pictures larger than this are left as attachments.
const INLINE_IMAGE_LIMIT: usize = 3 * 1024 * 1024;

/// The name and contents of a message's attachment, by its position in
/// [`Displayed::attachments`].
pub fn attachment(raw: &[u8], index: usize) -> Option<(String, Vec<u8>)> {
    let message = MessageParser::default().parse(raw)?;
    let part = message.attachments().nth(index)?;
    Some((
        part.attachment_name().unwrap_or("attachment").to_string(),
        part.decoded().into_owned(),
    ))
}

fn first_address(address: &AddressList) -> (String, String) {
    address
        .first()
        .map(|a| {
            (
                a.name().unwrap_or_default().trim().to_string(),
                a.address().unwrap_or_default().trim().to_string(),
            )
        })
        .unwrap_or_default()
}

fn append_addresses(out: &mut String, address: &AddressList) {
    for a in address.mailboxes() {
        if !out.is_empty() {
            out.push_str(", ");
        }
        match (a.name(), a.address()) {
            (Some(name), Some(addr)) => {
                out.push_str(name.trim());
                out.push_str(" <");
                out.push_str(addr);
                out.push('>');
            }
            (Some(one), None) | (None, Some(one)) => out.push_str(one.trim()),
            (None, None) => {}
        }
    }
}

fn ids(value: Option<TextList>) -> String {
    value.map(|list| list.iter().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

/// The start of what the sender wrote, on one line.
///
/// This is the simple version: it drops `>`-quoted lines, the "On … wrote:"
/// line that introduces them, and everything from a signature separator or an
/// "Original Message" divider onwards. Milestone 5 replaces it with detection
/// that also handles HTML quoting and repeated signatures.
pub fn preview_of(body: &str) -> String {
    let mut out = String::new();
    let mut chars = 0;
    let lines: Vec<&str> = body.lines().collect();
    'lines: for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('>') {
            continue;
        }
        // "-- " is the standard signature separator; anything after is not
        // the sender's message.
        if *line == "-- " || trimmed == "--" {
            break;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("-----original message-----")
            || lower.starts_with("----- original message -----")
            || lower.starts_with("________________________________")
        {
            break;
        }
        if lower.starts_with("on ") && lower.ends_with("wrote:") {
            // Only an attribution if quoted text follows it.
            let next = lines[i + 1..].iter().map(|l| l.trim()).find(|l| !l.is_empty());
            if next.is_none_or(|l| l.starts_with('>')) {
                continue;
            }
        }
        for word in trimmed.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
                chars += 1;
            }
            for c in word.chars() {
                if chars >= PREVIEW_CHARS {
                    break 'lines;
                }
                out.push(c);
                chars += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = b"From: \"Alice Abbott\" <alice@example.com>\r\n\
To: Bob <bob@example.org>, carol@example.net\r\n\
Cc: Dana Dubois <dana@example.com>\r\n\
Subject: Re: Lunch on Friday?\r\n\
Date: Tue, 17 Nov 2026 12:00:00 +0000\r\n\
Message-ID: <m3@example.com>\r\n\
In-Reply-To: <m2@example.org>\r\n\
References: <m1@example.com> <m2@example.org>\r\n\
\r\n\
Friday works.\r\n\
Shall we say   one o'clock?\r\n\
\r\n\
On Mon, 16 Nov 2026, Bob wrote:\r\n\
> How about Friday?\r\n\
> > Lunch this week?\r\n\
\r\n\
-- \r\n\
Alice Abbott\r\n\
Example Corp\r\n";

    #[test]
    fn headers_are_extracted() {
        let p = parse(SIMPLE);
        assert_eq!(p.from_name, "Alice Abbott");
        assert_eq!(p.from_addr, "alice@example.com");
        assert_eq!(
            p.recipients,
            "Bob <bob@example.org>, carol@example.net, Dana Dubois <dana@example.com>"
        );
        assert_eq!(p.subject, "Re: Lunch on Friday?");
        assert_eq!(p.date, Some(1_794_916_800));
        assert_eq!(p.message_id, "m3@example.com");
        assert_eq!(p.in_reply_to, "m2@example.org");
        assert_eq!(p.references, "m1@example.com m2@example.org");
        assert!(!p.has_attachment);
    }

    #[test]
    fn preview_is_only_what_the_sender_wrote() {
        let p = parse(SIMPLE);
        assert_eq!(p.preview, "Friday works. Shall we say one o'clock?");
        assert!(p.body.contains("How about Friday?"), "search still covers quoted text");
    }

    #[test]
    fn preview_rules() {
        assert_eq!(preview_of("a\n> q\nb"), "a b");
        assert_eq!(preview_of("hello\n-- \nsig"), "hello");
        assert_eq!(preview_of("hello\n-----Original Message-----\nFrom: x"), "hello");
        assert_eq!(
            preview_of("On reflection she wrote:\nnot a quote"),
            "On reflection she wrote: not a quote",
            "an 'on … wrote:' line is kept unless quoted text follows"
        );
        assert_eq!(preview_of("On Monday, Bob wrote:\n\n> quoted"), "");
        assert_eq!(preview_of(""), "");
        let long = "word ".repeat(500);
        assert_eq!(preview_of(&long).chars().count(), PREVIEW_CHARS);
        let wide = "é".repeat(1000);
        assert_eq!(preview_of(&wide).chars().count(), PREVIEW_CHARS, "counts characters, not bytes");
    }

    #[test]
    fn attachments_and_html_are_recognised() {
        let raw = b"From: a@example.com\r\n\
Subject: report\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>See the <b>attached</b> report.</p>\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--b--\r\n";
        let p = parse(raw);
        assert!(p.has_attachment);
        assert_eq!(p.preview, "See the attached report.");
    }

    #[test]
    fn a_message_is_taken_apart_for_reading() {
        let shown = display(SIMPLE);
        assert_eq!(shown.subject, "Re: Lunch on Friday?");
        assert_eq!((shown.from_name.as_str(), shown.from_addr.as_str()), ("Alice Abbott", "alice@example.com"));
        assert_eq!(shown.to, "Bob <bob@example.org>, carol@example.net");
        assert_eq!(shown.cc, "Dana Dubois <dana@example.com>");
        assert_eq!(shown.html, None, "a text-only message offers no HTML");
        let text = shown.text.unwrap();
        assert!(text.contains("Friday works.") && text.contains("> How about Friday?"), "the whole text, quotes included");
        assert!(shown.attachments.is_empty());

        let raw = b"From: a@example.com\r\nSubject: report\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
--alt\r\nContent-Type: text/plain\r\n\r\nplain version\r\n\
--alt\r\nContent-Type: text/html\r\n\r\n<p>html <b>version</b></p>\r\n--alt--\r\n\
--b\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n\
JVBERi0xLjQK\r\n--b--\r\n";
        let shown = display(raw);
        assert!(shown.html.unwrap().contains("<b>version</b>"));
        assert!(shown.text.unwrap().contains("plain version"));
        assert_eq!(shown.attachments.len(), 1);
        assert_eq!(shown.attachments[0].name, "report.pdf");
        assert_eq!(shown.attachments[0].mime_type, "application/pdf");
        assert_eq!(shown.attachments[0].size, 9);
        let (name, bytes) = attachment(raw, 0).unwrap();
        assert_eq!((name.as_str(), bytes.as_slice()), ("report.pdf", &b"%PDF-1.4\n"[..]));
        assert_eq!(attachment(raw, 1), None);
    }

    #[test]
    fn pictures_embedded_by_content_id_are_found() {
        let raw = b"From: a@example.com\r\nSubject: logo\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"r\"\r\n\r\n\
--r\r\nContent-Type: text/html\r\n\r\n<p>Hi</p><img src=\"cid:logo@x\">\r\n\
--r\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--r--\r\n";
        let shown = display(raw);
        assert_eq!(shown.inline_images.len(), 1);
        assert_eq!(shown.inline_images[0].content_id, "logo@x");
        assert_eq!(shown.inline_images[0].mime_type, "image/png");
        assert_eq!(shown.inline_images[0].data, b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn garbage_still_produces_a_record() {
        for raw in [&b""[..], b"\xff\xfe\x00\x01", b"no headers at all"] {
            let p = parse(raw);
            assert_eq!(p.date, None);
            assert!(p.from_addr.is_empty());
        }
    }
}
