//! Sender verification: what the user's mail server recorded about a
//! message's sender checks, and what that adds up to.
//!
//! A receiving server writes the results of SPF, DKIM and DMARC into an
//! `Authentication-Results` header (RFC 8601). Anyone can write such a header
//! into a message they send, so only one is ever believed: **the topmost
//! header carrying the name of the user's own server.** Headers are added at
//! the top, so nothing a sender wrote can be above what the receiving server
//! added. Lower headers with the same name are ignored even when they look
//! genuine; a server that spreads its results over several headers therefore
//! shows fewer results here, never more trust.
//!
//! The rule that matters most: **nothing here can turn into a green check
//! unless a pass is tied to the domain in the From line.** A pass for some
//! other domain says only that the message really came from that other
//! domain.

/// What the checks add up to. The numbers are stored in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// The server recorded nothing usable.
    NotChecked = 0,
    /// A pass that is tied to the From domain.
    Verified = 1,
    /// Checks were recorded, but none ties the message to its From domain,
    /// and none counts as a failure.
    Unverified = 2,
    /// A check failed.
    Failed = 3,
}

impl State {
    pub fn from_number(n: i64) -> State {
        match n {
            1 => State::Verified,
            2 => State::Unverified,
            3 => State::Failed,
            _ => State::NotChecked,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            State::NotChecked => "notChecked",
            State::Verified => "verified",
            State::Unverified => "unverified",
            State::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Spf,
    Dkim,
    Dmarc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail,
    /// SPF's "probably not allowed".
    SoftFail,
    /// The check could not be completed (a lookup failed, a record is broken).
    Error,
    /// The check ran and has nothing to say (no record, no signature,
    /// neutral).
    Nothing,
}

/// One check, as the server recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub kind: Kind,
    pub outcome: Outcome,
    /// The server's own word for the result (`pass`, `softfail`, ...).
    pub word: String,
    /// The domain the check was about: the signing domain for DKIM, the
    /// envelope sender's domain for SPF, the From domain for DMARC.
    pub domain: String,
    /// Whether that domain is the From domain or its parent or child.
    pub aligned: bool,
}

impl Check {
    /// One plain sentence about this check.
    pub fn sentence(&self) -> String {
        let domain = if self.domain.is_empty() { "the sender's domain".to_string() } else { self.domain.clone() };
        let other = if self.aligned || self.domain.is_empty() {
            String::new()
        } else {
            " That is not the domain in the From line.".to_string()
        };
        match (self.kind, self.outcome) {
            (Kind::Spf, Outcome::Pass) => format!("The server that delivered this is allowed to send for {domain}.{other}"),
            (Kind::Spf, Outcome::Fail) => format!("{domain} says the server that delivered this is not allowed to send its mail."),
            (Kind::Spf, Outcome::SoftFail) => format!("{domain} says the server that delivered this is probably not one of its own."),
            (Kind::Spf, Outcome::Error) => format!("Your mail server could not complete this check for {domain}."),
            (Kind::Spf, Outcome::Nothing) => format!("{domain} does not say which servers may send its mail."),
            (Kind::Dkim, Outcome::Pass) => format!("Signed by {domain}, and the signature is valid.{other}"),
            (Kind::Dkim, Outcome::Fail) => format!("Carries a signature from {domain} that does not match the message. It was changed on the way, or the signature is forged."),
            (Kind::Dkim, Outcome::Error) => format!("Your mail server could not check the signature from {domain}."),
            (Kind::Dkim, _) => "The message is not signed.".to_string(),
            (Kind::Dmarc, Outcome::Pass) => format!("Meets {domain}'s own rules for mail that uses its name."),
            (Kind::Dmarc, Outcome::Fail) => format!("Does not meet {domain}'s own rules for mail that uses its name."),
            (Kind::Dmarc, Outcome::Error) => format!("Your mail server could not look up {domain}'s rules."),
            (Kind::Dmarc, _) => format!("{domain} publishes no rules for mail that uses its name."),
        }
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            Kind::Spf => "SPF",
            Kind::Dkim => "DKIM",
            Kind::Dmarc => "DMARC",
        }
    }
}

/// What the server recorded about one message, and what it adds up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub state: State,
    /// The name of the server whose header was read (lower case). Empty
    /// when there was none.
    pub server: String,
    pub checks: Vec<Check>,
    /// The message has more than one From line, or a From line with more
    /// than one address: who it is "from" is ambiguous, so it is never
    /// shown as verified.
    pub ambiguous_from: bool,
}

/// Reads the server's results from a message's header.
///
/// `from_addr` is the address the message shows as its sender. `trusted` is
/// the name of the server to believe; with `None`, the topmost
/// `Authentication-Results` header is read whoever wrote it, and the caller
/// must compare [`Assessment::server`] with the trusted name before
/// believing the result.
pub fn assess(raw: &[u8], from_addr: &str, trusted: Option<&str>) -> Assessment {
    let fields = header_fields(raw);
    let from_lines: Vec<&String> = fields.iter().filter(|(n, _)| n == "from").map(|(_, v)| v).collect();
    let ambiguous_from = from_lines.len() > 1 || from_lines.iter().any(|v| strip_quoted(v).matches('@').count() > 1);
    let from_domain = domain_of(from_addr);

    let header = fields
        .iter()
        .filter(|(name, _)| name == "authentication-results")
        .filter_map(|(_, value)| parse_results(value))
        .find(|r| trusted.is_none_or(|t| r.server.eq_ignore_ascii_case(t.trim())));
    let Some(header) = header else {
        return Assessment { state: State::NotChecked, server: String::new(), checks: Vec::new(), ambiguous_from };
    };

    let mut checks = Vec::new();
    for r in &header.results {
        let (kind, property) = match r.method.as_str() {
            "spf" => (Kind::Spf, ["smtp.mailfrom", "smtp.helo"].as_slice()),
            "dkim" => (Kind::Dkim, ["header.d", "header.i"].as_slice()),
            "dmarc" => (Kind::Dmarc, ["header.from"].as_slice()),
            _ => continue,
        };
        let domain = property
            .iter()
            .find_map(|p| r.properties.iter().find(|(k, _)| k == p))
            .map(|(_, v)| domain_of(v))
            .unwrap_or_default();
        let outcome = match r.result.as_str() {
            "pass" => Outcome::Pass,
            "fail" | "hardfail" | "policy" => Outcome::Fail,
            "softfail" => Outcome::SoftFail,
            "temperror" | "permerror" => Outcome::Error,
            _ => Outcome::Nothing,
        };
        let aligned = !from_domain.is_empty() && related(&domain, &from_domain);
        checks.push(Check { kind, outcome, word: r.result.clone(), domain, aligned });
    }

    let has = |kind: Kind, outcome: Outcome| checks.iter().any(|c| c.kind == kind && c.outcome == outcome);
    let aligned_pass = |kind: Kind| checks.iter().any(|c| c.kind == kind && c.outcome == Outcome::Pass && c.aligned);
    // A DMARC result names the From domain it was worked out for. One that
    // names some other domain was not about this message's From line.
    let dmarc_pass = checks
        .iter()
        .any(|c| c.kind == Kind::Dmarc && c.outcome == Outcome::Pass && (c.aligned || c.domain.is_empty()));

    let state = if checks.is_empty() {
        State::NotChecked
    } else if has(Kind::Dmarc, Outcome::Fail) {
        State::Failed
    } else if from_domain.is_empty() || ambiguous_from {
        State::Unverified
    } else if dmarc_pass || aligned_pass(Kind::Dkim) || aligned_pass(Kind::Spf) {
        State::Verified
    } else if has(Kind::Spf, Outcome::Fail) {
        State::Failed
    } else {
        State::Unverified
    };
    Assessment { state, server: header.server, checks, ambiguous_from }
}

/// One `Authentication-Results` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Results {
    /// The name of the server that wrote it, lower case.
    pub server: String,
    pub results: Vec<MethodResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodResult {
    /// `spf`, `dkim`, `dmarc`, ... lower case.
    pub method: String,
    /// `pass`, `fail`, ... lower case.
    pub result: String,
    /// `header.d`, `smtp.mailfrom`, ... (lower case) and their values.
    pub properties: Vec<(String, String)>,
}

/// Parses the value of an `Authentication-Results` header. `None` when it
/// does not even name a server.
pub fn parse_results(value: &str) -> Option<Results> {
    let clean = without_comments(value);
    let mut parts = split_outside_quotes(&clean, ';').into_iter();
    let server = parts.next()?.split_whitespace().next()?.trim_matches('"').to_ascii_lowercase();
    if server.is_empty() || server.contains('=') {
        return None;
    }
    let mut results = Vec::new();
    for part in parts {
        let mut pairs = pairs_of(&part).into_iter();
        let Some((method, result)) = pairs.next() else { continue };
        // "dkim/1" carries a version; the method is what is before it.
        let method = method.split('/').next().unwrap_or_default().to_string();
        if method.is_empty() || method.contains('.') || method == "reason" {
            continue;
        }
        let properties = pairs.filter(|(k, _)| k.contains('.')).collect();
        results.push(MethodResult { method, result: result.to_ascii_lowercase(), properties });
    }
    Some(Results { server, results })
}

/// `key=value` pairs in a piece of a header, allowing spaces around `=` and
/// quoted values. Keys are lower-cased.
fn pairs_of(text: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let skip_space = |i: &mut usize| {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    };
    loop {
        skip_space(&mut i);
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' {
            i += 1;
        }
        if i == start {
            if i >= chars.len() {
                break;
            }
            i += 1;
            continue;
        }
        let key: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
        skip_space(&mut i);
        if i >= chars.len() || chars[i] != '=' {
            // A bare word ("none"): not a pair.
            continue;
        }
        i += 1;
        skip_space(&mut i);
        let mut value = String::new();
        if i < chars.len() && chars[i] == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                value.push(chars[i]);
                i += 1;
            }
            i += 1;
        } else {
            while i < chars.len() && !chars[i].is_whitespace() {
                value.push(chars[i]);
                i += 1;
            }
        }
        out.push((key, value));
    }
    out
}

/// The text with `(comments)` removed. Comments nest; quoted strings are
/// left alone.
fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0u32;
    let mut quoted = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let next = chars.next();
                if depth == 0 {
                    out.push(c);
                    out.extend(next);
                }
            }
            '"' if depth == 0 => {
                quoted = !quoted;
                out.push(c);
            }
            '(' if !quoted => depth += 1,
            ')' if !quoted && depth > 0 => {
                depth -= 1;
                out.push(' ');
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn split_outside_quotes(text: &str, at: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut quoted = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && quoted {
            parts.last_mut().unwrap().push(c);
            parts.last_mut().unwrap().extend(chars.next());
        } else if c == '"' {
            quoted = !quoted;
            parts.last_mut().unwrap().push(c);
        } else if c == at && !quoted {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

fn strip_quoted(text: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut chars = without_comments(text).chars().collect::<Vec<_>>().into_iter();
    while let Some(c) = chars.next() {
        if c == '\\' && quoted {
            chars.next();
        } else if c == '"' {
            quoted = !quoted;
        } else if !quoted {
            out.push(c);
        }
    }
    out
}

/// The header fields of a message in order, names lower-cased, folded lines
/// joined.
pub fn header_fields(raw: &[u8]) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in raw.split(|b| *b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            break;
        }
        let text = String::from_utf8_lossy(line);
        if line[0] == b' ' || line[0] == b'\t' {
            if let Some(last) = fields.last_mut() {
                last.1.push(' ');
                last.1.push_str(text.trim());
            }
        } else if let Some((name, value)) = text.split_once(':') {
            fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    fields
}

/// The domain of an address, or the text itself if it is already a domain.
/// Lower case, without a trailing dot.
pub fn domain_of(address: &str) -> String {
    let address = address.trim().trim_matches(|c| c == '<' || c == '>');
    let domain = address.rsplit_once('@').map_or(address, |(_, d)| d);
    domain.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Whether two domains are the same, or one is inside the other
/// (`mail.example.com` and `example.com`). Two different names under one
/// parent are not related: without a list of public suffixes there is no
/// telling `a.example.com` and `b.example.com` from `a.co.uk` and `b.co.uk`.
pub fn related(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let inside = |child: &str, parent: &str| {
        parent.contains('.') && child.len() > parent.len() && child.ends_with(parent) && child[..child.len() - parent.len()].ends_with('.')
    };
    a == b || inside(a, b) || inside(b, a)
}

/// A domain reduced to what the eye sees: characters that pass for one
/// another are made the same, and separators are dropped.
fn skeleton(domain: &str) -> String {
    let mut out = String::with_capacity(domain.len());
    for c in domain.chars() {
        match c.to_ascii_lowercase() {
            '0' => out.push('o'),
            '1' | 'i' | '|' => out.push('l'),
            '5' => out.push('s'),
            '-' | '_' => {}
            c => out.push(c),
        }
    }
    out.replace("rn", "m").replace("vv", "w").replace("cl", "d")
}

/// The number of single-character edits (insert, delete, replace, or swap
/// of neighbours) between two short strings, counted up to `most + 1`.
fn edits(a: &str, b: &str, most: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > most {
        return most + 1;
    }
    let mut before: Vec<usize> = Vec::new();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let same = a[i - 1] == b[j - 1];
            row[j] = (previous[j] + 1).min(row[j - 1] + 1).min(previous[j - 1] + usize::from(!same));
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(before[j - 2] + 1);
            }
        }
        before = std::mem::replace(&mut previous, row);
    }
    previous[b.len()]
}

/// A domain the user hears from that `domain` could be mistaken for, if
/// there is one. `known` is every sender domain with how many messages have
/// come from it.
///
/// Only a domain with next to no history here is ever called a lookalike,
/// and only of a domain with real history: three messages at least, and
/// more than three times as many as the newcomer.
pub fn lookalike(domain: &str, known: &[(String, u32)]) -> Option<String> {
    let domain = domain.to_ascii_lowercase();
    let own = known.iter().find(|k| k.0 == domain).map_or(0, |k| k.1);
    if domain.is_empty() || own >= 3 {
        return None;
    }
    let look = skeleton(&domain);
    let mut best: Option<(&str, u32)> = None;
    for (other, messages) in known {
        if *messages < 3 || *messages <= own.saturating_mul(3) || *other == domain || related(other, &domain) {
            continue;
        }
        let other_look = skeleton(other);
        // Short names are too close to each other to compare by edits:
        // "abc.com" and "abd.com" are simply different companies.
        let allowed = match other.len().min(domain.len()) {
            0..=7 => 0,
            8..=14 => 1,
            _ => 2,
        };
        // The well-known name with something added: "bank.example.evil.net",
        // "bank.example-login.net".
        let wraps = other.len() >= 8 && other.contains('.') && domain.starts_with(&format!("{other}."))
            || domain.starts_with(&format!("{other}-"));
        if (look == other_look || wraps || edits(&look, &other_look, allowed) <= allowed)
            && best.is_none_or(|b| *messages > b.1)
        {
            best = Some((other, *messages));
        }
    }
    best.map(|b| b.0.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(headers: &str) -> Vec<u8> {
        format!("{}\r\nSubject: x\r\n\r\nbody\r\n", headers.trim().replace('\n', "\r\n")).into_bytes()
    }

    fn state(headers: &str, from: &str, trusted: Option<&str>) -> State {
        assess(&message(headers), from, trusted).state
    }

    const MX: Option<&str> = Some("mx.home.example");

    #[test]
    fn a_header_is_taken_apart() {
        let r = parse_results(
            "mx.home.example (the best server); dkim=pass (2048-bit key; unprotected) header.d=bank.example header.i=@bank.example header.b=\"abc;def\";\r\n\tspf=pass (mx: domain of a@bank.example designates 192.0.2.1 as permitted sender) smtp.mailfrom=a@bank.example;\r\n dmarc = pass (p=REJECT sp=REJECT dis=NONE) header.from=bank.example",
        )
        .unwrap();
        assert_eq!(r.server, "mx.home.example");
        let got: Vec<(&str, &str)> = r.results.iter().map(|m| (m.method.as_str(), m.result.as_str())).collect();
        assert_eq!(got, [("dkim", "pass"), ("spf", "pass"), ("dmarc", "pass")]);
        assert!(r.results[0].properties.contains(&("header.d".into(), "bank.example".into())));
        assert!(r.results[0].properties.contains(&("header.b".into(), "abc;def".into())), "a quoted semicolon is not a separator");
        assert_eq!(r.results[1].properties, [("smtp.mailfrom".into(), "a@bank.example".into())]);

        // No results, a version, a reason, and rubbish.
        assert_eq!(parse_results("mx.home.example 1; none").unwrap().results, []);
        let r = parse_results("MX.Home.Example; dkim/1=FAIL reason=\"bad signature\" header.d=x.example").unwrap();
        assert_eq!((r.server.as_str(), r.results[0].method.as_str(), r.results[0].result.as_str()), ("mx.home.example", "dkim", "fail"));
        assert_eq!(r.results[0].properties, [("header.d".into(), "x.example".into())]);
        assert_eq!(parse_results(""), None);
        assert_eq!(parse_results("(only a comment)"), None);
        assert_eq!(parse_results("dkim=pass header.d=x.example"), None, "no server named");
        // Comments that try to hide or fake results.
        let r = parse_results("mx.home.example; spf=fail (dkim=pass header.d=bank.example) smtp.mailfrom=x.example").unwrap();
        assert_eq!(r.results.len(), 1);
        assert_eq!(r.results[0].result, "fail");
    }

    #[test]
    fn passes_tied_to_the_from_domain_verify() {
        let from = "alice@bank.example";
        // DMARC pass.
        assert_eq!(state("Authentication-Results: mx.home.example; dmarc=pass header.from=bank.example", from, MX), State::Verified);
        // DKIM pass by the From domain, or its parent, or a child.
        for d in ["bank.example", "mail.bank.example"] {
            let h = format!("Authentication-Results: mx.home.example; dkim=pass header.d={d}");
            assert_eq!(state(&h, from, MX), State::Verified, "{d}");
        }
        assert_eq!(
            state("Authentication-Results: mx.home.example; dkim=pass header.d=bank.example", "alice@news.bank.example", MX),
            State::Verified
        );
        // SPF pass for the From domain.
        assert_eq!(
            state("Authentication-Results: mx.home.example; spf=pass smtp.mailfrom=bounce@bank.example", from, MX),
            State::Verified
        );
        // The server's name is compared without regard to case.
        assert_eq!(state("Authentication-Results: MX.HOME.EXAMPLE; dmarc=pass header.from=bank.example", from, MX), State::Verified);
    }

    #[test]
    fn a_pass_for_another_domain_is_never_a_green_check() {
        let from = "alice@bank.example";
        for h in [
            // Signed and sent by the attacker's own domain: both pass.
            "Authentication-Results: mx.home.example; dkim=pass header.d=evil.example; spf=pass smtp.mailfrom=x@evil.example",
            // A lookalike that merely ends the same way.
            "Authentication-Results: mx.home.example; dkim=pass header.d=notbank.example",
            "Authentication-Results: mx.home.example; dkim=pass header.d=bank.example.evil.example",
            // Two names under one parent are not each other.
            "Authentication-Results: mx.home.example; dkim=pass header.d=other.example",
            // A parent that is only a suffix.
            "Authentication-Results: mx.home.example; dkim=pass header.d=example",
            // A DMARC pass about a different From domain.
            "Authentication-Results: mx.home.example; dmarc=pass header.from=evil.example",
            // A pass with no domain at all.
            "Authentication-Results: mx.home.example; dkim=pass; spf=pass",
            // Other methods passing mean nothing here.
            "Authentication-Results: mx.home.example; auth=pass smtp.auth=alice; iprev=pass; arc=pass",
        ] {
            let got = state(h, from, MX);
            assert!(got == State::Unverified || got == State::NotChecked, "{h}: {got:?}");
        }
        assert_eq!(state("Authentication-Results: mx.home.example; dkim=pass header.d=a.co.example", "x@b.co.example", MX), State::Unverified);
    }

    #[test]
    fn only_the_users_own_server_is_believed() {
        let from = "alice@bank.example";
        // Written by the sender, under any other name.
        let forged = "Authentication-Results: mx.evil.example; dmarc=pass header.from=bank.example";
        assert_eq!(state(forged, from, MX), State::NotChecked);
        // A name that merely contains the trusted one.
        for name in ["mx.home.example.evil.example", "evil-mx.home.example", "mx.home.example."] {
            let h = format!("Authentication-Results: {name}; dmarc=pass header.from=bank.example");
            assert_eq!(state(&h, from, MX), State::NotChecked, "{name}");
        }
        // Written by the sender under the trusted name, below the real one:
        // the real, topmost one is what counts, and nothing is merged in.
        let h = "Received: from evil by mx.home.example\n\
                 Authentication-Results: mx.home.example; spf=softfail smtp.mailfrom=x@bank.example; dkim=none\n\
                 Authentication-Results: mx.home.example; dmarc=pass header.from=bank.example; dkim=pass header.d=bank.example";
        let a = assess(&message(h), from, MX);
        assert_eq!(a.state, State::Unverified);
        assert_eq!(a.checks.len(), 2);
        // A forged header in the body is not a header.
        let raw = b"From: alice@bank.example\r\n\r\nAuthentication-Results: mx.home.example; dmarc=pass header.from=bank.example\r\n";
        assert_eq!(assess(raw, from, MX).state, State::NotChecked);
        // With no trusted name given, the topmost header is read and its
        // writer reported, for the caller to compare.
        let a = assess(&message(forged), from, None);
        assert_eq!((a.state, a.server.as_str()), (State::Verified, "mx.evil.example"));
    }

    #[test]
    fn failures() {
        let from = "alice@bank.example";
        // A DMARC failure is a failure whatever else passed.
        let h = "Authentication-Results: mx.home.example; dkim=pass header.d=evil.example; spf=pass smtp.mailfrom=evil.example; dmarc=fail header.from=bank.example";
        assert_eq!(state(h, from, MX), State::Failed);
        let h = "Authentication-Results: mx.home.example; dkim=pass header.d=bank.example; dmarc=fail header.from=bank.example";
        assert_eq!(state(h, from, MX), State::Failed);
        // SPF says no, and nothing vouches for the message.
        let h = "Authentication-Results: mx.home.example; spf=fail smtp.mailfrom=bank.example; dkim=none";
        assert_eq!(state(h, from, MX), State::Failed);
        // SPF fails but the signature holds (a forwarded message): fine.
        let h = "Authentication-Results: mx.home.example; spf=fail smtp.mailfrom=bank.example; dkim=pass header.d=bank.example; dmarc=pass header.from=bank.example";
        assert_eq!(state(h, from, MX), State::Verified);
        // A broken signature or a soft SPF failure alone is not proof of
        // forgery (mailing lists do both): neutral, never green.
        for h in [
            "Authentication-Results: mx.home.example; dkim=fail header.d=bank.example",
            "Authentication-Results: mx.home.example; spf=softfail smtp.mailfrom=bank.example",
            "Authentication-Results: mx.home.example; spf=temperror; dkim=permerror header.d=bank.example; dmarc=none",
        ] {
            assert_eq!(state(h, from, MX), State::Unverified, "{h}");
        }
        // Nothing recorded.
        assert_eq!(state("Received: from a by b", from, MX), State::NotChecked);
        assert_eq!(state("Authentication-Results: mx.home.example; none", from, MX), State::NotChecked);
    }

    #[test]
    fn an_ambiguous_from_line_is_never_verified() {
        let pass = "Authentication-Results: mx.home.example; dmarc=pass header.from=bank.example; dkim=pass header.d=bank.example";
        // Two From lines: the server may have checked one and the reader may
        // be shown the other.
        let h = format!("{pass}\nFrom: alice@bank.example\nFrom: boss@victim.example");
        let a = assess(&message(&h), "alice@bank.example", MX);
        assert_eq!((a.state, a.ambiguous_from), (State::Unverified, true));
        // Two addresses in one From line.
        let h = format!("{pass}\nFrom: Alice <alice@bank.example>, boss@victim.example");
        assert_eq!(state(&h, "alice@bank.example", MX), State::Unverified);
        // An address inside a display name is one address.
        let h = format!("{pass}\nFrom: \"alice@bank.example\" <alice@bank.example>");
        assert_eq!(state(&h, "alice@bank.example", MX), State::Verified);
        // No usable From address.
        assert_eq!(state(pass, "", MX), State::Unverified);
    }

    #[test]
    fn lookalike_domains_are_spotted() {
        let known: Vec<(String, u32)> = [
            ("firstnational.example", 40),
            ("paypal.com", 25),
            ("shop.example", 12),
            ("abc.com", 30),
            ("rare.example", 1),
            ("mail.firstnational.example", 9),
        ]
        .map(|(d, n)| (d.to_string(), n))
        .into();
        let like = |d: &str| lookalike(d, &known);
        // One character out, swapped, added or dropped.
        assert_eq!(like("firstnationa1.example").as_deref(), Some("firstnational.example"));
        assert_eq!(like("firstnatoinal.example").as_deref(), Some("firstnational.example"));
        assert_eq!(like("firstnationall.example").as_deref(), Some("firstnational.example"));
        assert_eq!(like("first-national.example").as_deref(), Some("firstnational.example"));
        // Characters that pass for each other.
        assert_eq!(like("paypa1.com").as_deref(), Some("paypal.com"));
        assert_eq!(like("PAYPAI.com").as_deref(), Some("paypal.com"));
        assert_eq!(like("firstnationai.example").as_deref(), Some("firstnational.example"));
        // The real name with something attached.
        assert_eq!(like("paypal.com.account-check.example").as_deref(), Some("paypal.com"));
        assert_eq!(like("paypal.com-login.example").as_deref(), Some("paypal.com"));

        // Not lookalikes: the domain itself, its own subdomains, unrelated
        // names, short names that differ by a letter, and anything that
        // only resembles a domain the user barely hears from.
        for d in ["paypal.com", "mail.paypal.com", "news.shop.example", "example.org", "abd.com", "rars.example", ""] {
            assert_eq!(like(d), None, "{d}");
        }
        // A domain with history of its own is never called a lookalike.
        let mut both = known.clone();
        both.push(("firstnationa1.example".into(), 5));
        assert_eq!(lookalike("firstnationa1.example", &both), None);
    }

    #[test]
    fn each_check_has_a_plain_sentence() {
        let h = "Authentication-Results: mx.home.example; spf=pass smtp.mailfrom=bounce@mailer.example; dkim=fail header.d=bank.example; dmarc=fail header.from=bank.example";
        let a = assess(&message(h), "alice@bank.example", MX);
        let lines: Vec<String> = a.checks.iter().map(|c| format!("{} {}: {}", c.name(), c.word, c.sentence())).collect();
        assert_eq!(
            lines,
            [
                "SPF pass: The server that delivered this is allowed to send for mailer.example. That is not the domain in the From line.",
                "DKIM fail: Carries a signature from bank.example that does not match the message. It was changed on the way, or the signature is forged.",
                "DMARC fail: Does not meet bank.example's own rules for mail that uses its name.",
            ]
        );
    }
}
