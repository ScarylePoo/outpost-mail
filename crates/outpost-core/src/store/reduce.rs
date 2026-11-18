//! Reduces a message to what its sender wrote: the quoted history and the
//! signature are found and set aside.
//!
//! Nothing is ever deleted. The pieces are returned separately so that the
//! reading pane can show the new text and offer the rest behind a link, and
//! "show original" always gives the untouched message.
//!
//! This works on the plain-text form of the body. Signatures that only
//! repetition can reveal (a disclaimer with no separator) are found across
//! the messages of a conversation by [`shared_endings`].

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reduced {
    /// What the sender wrote.
    pub text: String,
    /// The quoted history, as it appeared.
    pub quoted: String,
    pub quoted_lines: usize,
    /// The signature, without its separator line.
    pub signature: String,
}

fn is_quoted(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// "On Mon, 2 Nov 2026, Alice wrote:" in the forms common clients produce,
/// in a few languages. Only counts when quoted text follows it.
fn is_attribution(line: &str) -> bool {
    let line = line.trim();
    let lower = line.to_lowercase();
    let ends = ["wrote:", "writes:", "schrieb:", "a écrit :", "a écrit:", "escribió:", "ha scritto:", "skrev:", "schreef:"];
    line.len() < 300 && ends.iter().any(|e| lower.ends_with(e))
}

/// A line that begins a block of forwarded or replied-to history, after
/// which everything is someone else's text.
fn starts_history(lines: &[&str], i: usize) -> bool {
    let line = lines[i].trim();
    let lower = line.to_lowercase();
    if lower.starts_with("-----original message-----")
        || lower.starts_with("----- original message -----")
        || lower.starts_with("-------- original message --------")
        || lower.starts_with("---------- forwarded message")
        || lower.starts_with("-----ursprüngliche nachricht-----")
        || (line.len() >= 20 && line.chars().all(|c| c == '_'))
    {
        return true;
    }
    // The header block Outlook puts above the message being answered:
    // "From:" followed closely by "Sent:" (or "Date:") and "Subject:".
    if lower.starts_with("from:") || lower.starts_with("von:") || lower.starts_with("de:") {
        let next: Vec<String> = lines[i + 1..].iter().take(6).map(|l| l.trim().to_lowercase()).collect();
        let has = |names: &[&str]| next.iter().any(|l| names.iter().any(|n| l.starts_with(n)));
        return has(&["sent:", "date:", "gesendet:", "envoyé:", "enviado:"]) && has(&["subject:", "betreff:", "objet:", "asunto:"]);
    }
    false
}

/// Closing lines that mail apps add by themselves.
fn is_app_signature(line: &str) -> bool {
    let lower = line.trim().to_lowercase();
    lower.starts_with("sent from my ")
        || lower.starts_with("sent from outlook")
        || lower.starts_with("get outlook for ")
        || lower.starts_with("sent from mail for ")
        || lower.starts_with("von meinem ")
}

pub fn reduce(body: &str) -> Reduced {
    let body = body.replace("\r\n", "\n");
    let lines: Vec<&str> = body.lines().collect();

    // Where the history starts, if a marker says so.
    let mut end = lines.len();
    for i in 0..lines.len() {
        if starts_history(&lines, i) {
            end = i;
            break;
        }
        if is_attribution(lines[i]) {
            // An attribution counts when the next non-blank line is quoted,
            // or when nothing but quoted and blank lines follow.
            let mut rest = lines[i + 1..].iter().filter(|l| !l.trim().is_empty());
            match rest.next() {
                Some(next) if is_quoted(next) => {
                    if lines[i + 1..].iter().all(|l| l.trim().is_empty() || is_quoted(l)) {
                        end = i;
                        break;
                    }
                }
                None => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
    }

    // Above that point, quoted lines may be interleaved with answers.
    let mut own: Vec<&str> = Vec::new();
    let mut quoted: Vec<&str> = Vec::new();
    for i in 0..end {
        let line = lines[i];
        if is_quoted(line) {
            quoted.push(line);
        } else if is_attribution(line)
            && lines[i + 1..end].iter().find(|l| !l.trim().is_empty()).is_some_and(|l| is_quoted(l))
        {
            quoted.push(line);
        } else {
            own.push(line);
        }
    }
    quoted.extend_from_slice(&lines[end..]);

    // The signature: from the standard separator, or an app's closing line.
    let mut signature: Vec<&str> = Vec::new();
    let separator = own.iter().rposition(|l| *l == "-- " || l.trim_end() == "--");
    if let Some(at) = separator {
        signature = own[at + 1..].to_vec();
        own.truncate(at);
    } else {
        while own.last().is_some_and(|l| l.trim().is_empty()) {
            own.pop();
        }
        if own.last().is_some_and(|l| is_app_signature(l)) {
            signature = vec![own.pop().expect("checked")];
        }
    }

    let tidy = |lines: &[&str]| -> String {
        let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(lines.len());
        let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(start, |e| e + 1);
        // Runs of blank lines left where quotes were removed become one.
        let mut out: Vec<&str> = Vec::new();
        for line in &lines[start..end] {
            if line.trim().is_empty() && out.last().is_some_and(|l: &&str| l.trim().is_empty()) {
                continue;
            }
            out.push(line);
        }
        out.join("\n")
    };
    Reduced {
        text: tidy(&own),
        quoted_lines: quoted.iter().filter(|l| !l.trim().is_empty()).count(),
        quoted: tidy(&quoted),
        signature: tidy(&signature),
    }
}

/// Finds signatures by repetition. Given the reduced texts of several
/// messages from one sender, returns how many closing lines they all share
/// (at least two messages, at least two shared lines): a block that ends
/// every message is that sender's signature or disclaimer, separator or not.
pub fn shared_ending(texts: &[&str]) -> usize {
    if texts.len() < 2 {
        return 0;
    }
    let lines: Vec<Vec<&str>> = texts
        .iter()
        .map(|t| t.lines().map(str::trim_end).rev().collect())
        .collect();
    let mut shared = 0;
    'grow: loop {
        let Some(line) = lines[0].get(shared) else { break };
        for other in &lines[1..] {
            if other.get(shared) != Some(line) {
                break 'grow;
            }
        }
        shared += 1;
    }
    // Trailing blank lines do not count, and the whole of a message is
    // never its own signature.
    let meaningful = lines[0][..shared].iter().filter(|l| !l.trim().is_empty()).count();
    if meaningful < 2 || lines.iter().any(|l| l.len() <= shared) {
        return 0;
    }
    shared
}

/// Splits the last `lines` lines off a text.
pub fn split_ending(text: &str, lines: usize) -> (String, String) {
    let all: Vec<&str> = text.lines().collect();
    let at = all.len().saturating_sub(lines);
    (all[..at].join("\n").trim_end().to_string(), all[at..].join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_reply_is_split_into_its_parts() {
        let r = reduce("Friday works.\r\nShall we say one o'clock?\r\n\r\nOn Mon, 16 Nov 2026, Bob wrote:\r\n> How about Friday?\r\n> > Lunch this week?\r\n\r\n-- \r\nAlice Abbott\r\nExample Corp\r\n");
        assert_eq!(r.text, "Friday works.\nShall we say one o'clock?");
        assert_eq!(r.quoted, "On Mon, 16 Nov 2026, Bob wrote:\n> How about Friday?\n> > Lunch this week?");
        assert_eq!(r.quoted_lines, 3);
        // A signature below the quote belongs to the quoted message here;
        // one above it is the sender's.
        let r = reduce("Yes.\n-- \nAlice\nExample Corp\n\nOn Monday Bob wrote:\n> ok?\n");
        assert_eq!((r.text.as_str(), r.signature.as_str()), ("Yes.", "Alice\nExample Corp"));
        assert_eq!(r.quoted_lines, 2);
    }

    #[test]
    fn answers_between_quotes_are_kept() {
        let r = reduce("> Can you do Friday?\nYes.\n\n> And bring the report?\nWill do.\n");
        assert_eq!(r.text, "Yes.\n\nWill do.");
        assert_eq!(r.quoted_lines, 2);
    }

    #[test]
    fn outlook_and_forward_blocks_end_the_new_text() {
        let outlook = "Sounds good.\n\nFrom: Bob Bergström <bob@example.com>\nSent: Monday, November 16, 2026 9:15 AM\nTo: Alice\nSubject: Lunch\n\nHow about Friday?\n";
        let r = reduce(outlook);
        assert_eq!(r.text, "Sounds good.");
        assert!(r.quoted.starts_with("From: Bob Bergström") && r.quoted.ends_with("How about Friday?"));
        assert_eq!(reduce("See below.\n\n-----Original Message-----\nFrom: x\n\nold").text, "See below.");
        assert_eq!(reduce("FYI\n\n---------- Forwarded message ---------\nFrom: x\n\nold").text, "FYI");
        assert_eq!(reduce("Top.\n________________________________\nFrom: x\nold").text, "Top.");
        // A line that merely starts with "From:" is not a header block.
        assert_eq!(reduce("From: here on, things improve.\nReally.").text, "From: here on, things improve.\nReally.");
    }

    #[test]
    fn nothing_is_hidden_from_an_ordinary_message() {
        let plain = "Hello,\n\nThe report is attached.\n\nRegards,\nAlice";
        let r = reduce(plain);
        assert_eq!(r.text, plain);
        assert_eq!((r.quoted_lines, r.signature.as_str()), (0, ""));
        // "wrote:" in a sentence is not an attribution.
        let prose = "As Dickens once wrote:\nIt was the best of times.";
        assert_eq!(reduce(prose).text, prose);
        assert_eq!(reduce("").text, "");
        assert_eq!(reduce("> only a quote\n").text, "");
    }

    #[test]
    fn app_signatures_are_recognised() {
        let r = reduce("On my way.\n\nSent from my iPhone\n");
        assert_eq!((r.text.as_str(), r.signature.as_str()), ("On my way.", "Sent from my iPhone"));
    }

    #[test]
    fn a_block_that_ends_every_message_is_a_signature() {
        let disclaimer = "Kind regards,\nDana Dubois\nThis email is confidential.";
        let a = format!("First point.\n\n{disclaimer}");
        let b = format!("A different reply entirely.\nWith two lines.\n\n{disclaimer}");
        let c = format!("Third.\n\n{disclaimer}");
        let shared = shared_ending(&[&a, &b, &c]);
        assert_eq!(shared, 4, "three lines and the blank before them");
        assert_eq!(split_ending(&a, shared), ("First point.".to_string(), disclaimer.to_string()));
        // Not enough evidence: one message, one shared line, or identical messages.
        assert_eq!(shared_ending(&[&a]), 0);
        assert_eq!(shared_ending(&["Thanks,\nBob", "No.\nBob"]), 0);
        assert_eq!(shared_ending(&["Same text.\nTwice.", "Same text.\nTwice."]), 0);
        assert_eq!(shared_ending(&["a\nb", "c\nd"]), 0);
    }
}
