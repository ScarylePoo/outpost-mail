//! Synthetic mailbox generator.
//!
//! Writes real message files into a Maildir tree: conversations with reply
//! headers and quoted history, signatures, HTML alternatives and attachments,
//! spread over a dozen years and several folders. The same seed always gives
//! the same mailbox (apart from file names, which carry the time of writing).
//!
//! Word frequencies follow a long-tailed distribution over a 30,000-word
//! vocabulary, so the search index sees a realistic mix of words that are in
//! most messages and words that are in a handful.

use outpost_core::store::maildir::{encode_segment, Flags, Maildir};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// 17 Nov 2026 12:00:00 UTC: the newest any generated message can be.
const NEWEST: i64 = 1_794_916_800;
const SPAN: i64 = 12 * 365 * 86_400;
const VOCABULARY: usize = 30_000;
const PEOPLE: usize = 3_000;

/// (folder path, share of conversations in percent). Mail the owner sends
/// goes to Sent whatever the conversation's folder.
const FOLDERS: &[(&[&str], u64)] = &[
    (&["Inbox"], 22),
    (&["Archive"], 48),
    (&["Lists", "Announcements"], 9),
    (&["Projects", "Alpha"], 6),
    (&["Projects", "Beta"], 4),
    (&["Receipts"], 5),
    (&["Family"], 3),
    (&["Junk"], 3),
];
const SENT: usize = FOLDERS.len();

pub struct Options {
    pub messages: u64,
    pub seed: u64,
    pub attachments: bool,
}

pub struct Summary {
    pub messages: u64,
    pub bytes: u64,
    pub elapsed: Duration,
    pub per_folder: Vec<(String, u32)>,
}

// ---------------------------------------------------------------------------
// Randomness
// ---------------------------------------------------------------------------

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64, stream: u64) -> Rng {
        let mut rng = Rng(seed ^ stream.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        rng.next();
        rng
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low + 1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

// ---------------------------------------------------------------------------
// Vocabulary and people
// ---------------------------------------------------------------------------

const COMMON: &[&str] = &[
    "the", "to", "and", "a", "of", "I", "in", "is", "that", "it", "for", "you", "on", "with",
    "this", "be", "have", "are", "we", "as", "not", "at", "but", "will", "from", "can", "or",
    "your", "if", "was", "by", "all", "an", "me", "so", "do", "about", "would", "there", "my",
    "has", "what", "one", "know", "just", "get", "out", "time", "up", "when", "they", "some",
    "could", "please", "our", "more", "been", "also", "which", "thanks", "let", "need", "any",
    "think", "should", "like", "them", "week", "see", "work", "had", "back", "now", "new",
    "meeting", "then", "send", "next", "here", "how", "other", "over", "after", "call", "still",
    "today", "sure", "before", "good", "make", "want", "tomorrow", "email", "last", "look",
    "project", "day", "date", "report", "team", "attached", "update", "invoice", "order",
    "account", "payment", "schedule", "office", "question", "review", "plan", "budget", "draft",
    "contract", "deadline", "delivery", "weekend", "dinner", "holiday", "family", "school",
];

const ONSETS: &[&str] = &[
    "b", "br", "c", "ch", "cl", "d", "dr", "f", "fl", "g", "gr", "h", "j", "k", "l", "m", "n",
    "p", "pl", "pr", "qu", "r", "s", "sh", "sl", "st", "t", "th", "tr", "v", "w", "z",
];
const VOWELS: &[&str] = &["a", "e", "i", "o", "u", "ai", "ea", "ou", "io", "oo", "au", "ie"];
const CODAS: &[&str] = &["", "", "n", "r", "s", "t", "l", "m", "nd", "st", "ck", "x", "rd", "ng"];

const FIRST: &[&str] = &[
    "Alice", "Bram", "Chidi", "Dana", "Elif", "Farid", "Greta", "Hugo", "Ines", "Jonas", "Keiko",
    "Lars", "Mira", "Nadia", "Oskar", "Priya", "Quentin", "Rosa", "Sven", "Tamar", "Uma",
    "Viktor", "Wanjiru", "Xavier", "Yusuf", "Zofia", "Anders", "Beatriz", "Cormac", "Dolores",
    "Emeka", "Fiona", "Goran", "Hana", "Ivo", "Jamila", "Kasper", "Leila", "Matteo", "Noor",
];
const LAST: &[&str] = &[
    "Abbott", "Bergström", "Castellanos", "Dubois", "Eriksen", "Fontaine", "Grünewald", "Haddad",
    "Ivanova", "Jørgensen", "Kowalski", "Lindqvist", "Moreau", "Nakamura", "O'Brien", "Petrov",
    "Quigley", "Rosenthal", "Santos", "Thackeray", "Underwood", "Vasquez", "Whitfield", "Yamada",
    "Zielinski", "Achebe", "Baumann", "Costa", "Dimitrov", "Engel", "Farouk", "Gallagher",
    "Horvat", "Ishikawa", "Janssen", "Khan", "Lombardi", "Mbeki", "Novak", "Okafor",
];
const DOMAINS: &[&str] = &[
    "example.com", "example.org", "example.net", "mail.example", "corp.example", "uni.example",
    "shop.example", "studio.example", "clinic.example", "lists.example",
];
const ROLES: &[&str] = &[
    "Operations", "Accounts", "Head of Sales", "Engineering", "Customer Support", "Director",
    "Project Manager", "Office Manager",
];

pub struct World {
    words: Vec<String>,
    /// Cumulative weights for sampling `words`.
    cumulative: Vec<f64>,
    people: Vec<Person>,
}

struct Person {
    name: String,
    address: String,
    /// Empty if this person does not sign their mail.
    signature: String,
}

impl World {
    pub fn new(seed: u64) -> World {
        let mut rng = Rng::new(seed, 0xC0FFEE);
        let mut seen: HashSet<String> = COMMON.iter().map(|w| w.to_lowercase()).collect();
        let mut words: Vec<String> = COMMON.iter().map(|w| w.to_string()).collect();
        while words.len() < VOCABULARY {
            let mut word = String::new();
            for _ in 0..rng.range(2, 4) {
                word.push_str(ONSETS[rng.below(ONSETS.len() as u64) as usize]);
                word.push_str(VOWELS[rng.below(VOWELS.len() as u64) as usize]);
                word.push_str(CODAS[rng.below(CODAS.len() as u64) as usize]);
            }
            if seen.insert(word.clone()) {
                words.push(word);
            }
        }
        // Zipf-like: the word at rank r is used in proportion to 1 / (r + 3).
        let mut total = 0.0;
        let cumulative = (0..words.len())
            .map(|rank| {
                total += 1.0 / (rank as f64 + 3.0);
                total
            })
            .collect();

        let mut people = vec![Person {
            name: "Owner Example".into(),
            address: "owner@example.com".into(),
            signature: String::new(),
        }];
        for i in 1..PEOPLE {
            let first = FIRST[i % FIRST.len()];
            let last = LAST[(i / FIRST.len() + i) % LAST.len()];
            let domain = DOMAINS[rng.below(DOMAINS.len() as u64) as usize];
            let local: String = format!("{first}.{last}{}", i / (FIRST.len() * LAST.len()) + 1)
                .to_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
                .collect();
            let name = format!("{first} {last}");
            let signature = if rng.chance(40) {
                format!(
                    "{name}\r\n{}, {} Ltd\r\n+44 20 7946 {:04}\r\n",
                    ROLES[rng.below(ROLES.len() as u64) as usize],
                    last,
                    rng.below(10_000)
                )
            } else {
                String::new()
            };
            people.push(Person {
                name,
                address: format!("{local}@{domain}"),
                signature,
            });
        }
        World {
            words,
            cumulative,
            people,
        }
    }

    fn word(&self, rng: &mut Rng) -> &str {
        let target = rng.unit() * self.cumulative[self.cumulative.len() - 1];
        let rank = self.cumulative.partition_point(|&c| c < target);
        &self.words[rank.min(self.words.len() - 1)]
    }

    /// The word at a given frequency rank (0 = most used), for search tests.
    pub fn word_at(&self, rank: usize) -> &str {
        &self.words[rank]
    }

    fn sentence(&self, rng: &mut Rng, out: &mut String) {
        let count = rng.range(4, 18);
        for i in 0..count {
            let word = self.word(rng);
            if i == 0 {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                }
            } else {
                out.push(' ');
                out.push_str(word);
            }
        }
        out.push_str(if rng.chance(12) { "?" } else { "." });
    }

    /// A message's own text: paragraphs separated by blank lines, unwrapped.
    fn text(&self, rng: &mut Rng) -> Vec<String> {
        // A third of mail is a line or two.
        let paragraphs = if rng.chance(35) { 1 } else { rng.range(1, 6) };
        (0..paragraphs)
            .map(|_| {
                let mut paragraph = String::new();
                for s in 0..rng.range(1, 5) {
                    if s > 0 {
                        paragraph.push(' ');
                    }
                    self.sentence(rng, &mut paragraph);
                }
                paragraph
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// RFC 5322 date for a unix time, in UTC.
fn rfc_date(unix: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
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
        "{}, {} {} {} {:02}:{:02}:{:02} +0000",
        DAYS[days.rem_euclid(7) as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Appends `text` wrapped at 72 columns, each line starting with `prefix`.
fn wrap(out: &mut String, text: &str, prefix: &str) {
    let mut column = 0;
    for word in text.split(' ') {
        if column > 0 && column + 1 + word.len() > 72 {
            out.push_str("\r\n");
            column = 0;
        }
        if column == 0 {
            out.push_str(prefix);
        } else {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += word.len();
    }
    out.push_str("\r\n");
}

fn base64(out: &mut String, bytes: &[u8]) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut column = 0;
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i > chunk.len() {
                out.push('=');
            } else {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        column += 4;
        if column >= 76 {
            out.push_str("\r\n");
            column = 0;
        }
    }
    if column > 0 {
        out.push_str("\r\n");
    }
}

fn mailbox(person: &Person) -> String {
    // Names with an apostrophe or non-ASCII letters are sent as raw UTF-8,
    // as modern servers do.
    format!("\"{}\" <{}>", person.name, person.address)
}

// ---------------------------------------------------------------------------
// Conversations
// ---------------------------------------------------------------------------

struct Message {
    folder: usize,
    flags: Flags,
    raw: Vec<u8>,
}

/// How many messages conversation `thread` has.
fn thread_size(seed: u64, thread: u64) -> u64 {
    let mut rng = Rng::new(seed, thread);
    if rng.chance(55) {
        return 1;
    }
    let mut size = 2;
    while size < 40 && rng.chance(62) {
        size += 1;
    }
    size
}

fn generate_thread(world: &World, options: &Options, thread: u64, size: u64) -> Vec<Message> {
    let mut rng = Rng::new(options.seed, thread);
    // Skip the draws thread_size made, so content is independent of them.
    let mut rng = Rng::new(rng.next(), thread);

    let mut pick = rng.below(100);
    let mut folder = 0;
    for (i, (_, share)) in FOLDERS.iter().enumerate() {
        if pick < *share {
            folder = i;
            break;
        }
        pick -= share;
    }
    let is_list = FOLDERS[folder].0[0] == "Lists" || FOLDERS[folder].0[0] == "Junk";

    // The people in the conversation. Index 0 is the owner.
    let mut participants: Vec<usize> = Vec::new();
    if !is_list {
        participants.push(0);
    }
    for _ in 0..rng.range(1, 4) {
        // A few correspondents account for most mail.
        let person = if rng.chance(60) {
            1 + rng.below(40) as usize
        } else {
            1 + rng.below(PEOPLE as u64 - 1) as usize
        };
        if !participants.contains(&person) {
            participants.push(person);
        }
    }

    let mut subject = String::new();
    for i in 0..rng.range(2, 6) {
        if i > 0 {
            subject.push(' ');
        }
        subject.push_str(world.word(&mut rng));
    }
    let mut chars = subject.chars();
    let subject = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => subject,
    };

    let mut time = NEWEST - SPAN + rng.below(SPAN as u64 - 40 * 3 * 86_400) as i64;
    let mut senders: Vec<usize> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    // Each message's own paragraphs, kept for quoting by replies.
    let mut texts: Vec<Vec<String>> = Vec::new();
    let mut quoted: Vec<String> = Vec::new();
    let mut messages = Vec::new();

    for n in 0..size as usize {
        let sender = if n == 0 {
            // The owner starts about one conversation in six.
            if !is_list && rng.chance(16) { 0 } else { *participants.last().unwrap() }
        } else {
            let others: Vec<usize> =
                participants.iter().copied().filter(|p| *p != senders[n - 1]).collect();
            if others.is_empty() { participants[0] } else { others[rng.below(others.len() as u64) as usize] }
        };
        let longest_gap = if rng.chance(70) { 6 * 3600 } else { 3 * 86_400 };
        time += rng.range(120, longest_gap) as i64;
        let person = &world.people[sender];
        let id = format!("{thread:x}.{n}.{:08x}@{}", rng.next() as u32, person.address.split('@').nth(1).unwrap());
        let own = world.text(&mut rng);

        // ---- plain text body ----
        let mut body = String::new();
        for paragraph in &own {
            wrap(&mut body, paragraph, "");
            body.push_str("\r\n");
        }
        let mut this_quote = String::new();
        if n > 0 && rng.chance(75) {
            let parent = n - 1;
            let _ = write!(
                this_quote,
                "On {}, {} wrote:\r\n",
                &rfc_date(time - 60)[..16],
                world.people[senders[parent]].name
            );
            for paragraph in &texts[parent] {
                wrap(&mut this_quote, paragraph, "> ");
                this_quote.push_str(">\r\n");
            }
            // Carry some of the older history too, as real replies do.
            for line in quoted[parent].lines().take(60) {
                this_quote.push_str("> ");
                this_quote.push_str(line);
                this_quote.push_str("\r\n");
            }
            body.push_str(&this_quote);
        }
        if !person.signature.is_empty() {
            body.push_str("\r\n-- \r\n");
            body.push_str(&person.signature);
        }

        // ---- headers ----
        let mut raw = String::with_capacity(body.len() + 1024);
        let _ = write!(raw, "From: {}\r\n", mailbox(person));
        let to: Vec<String> = participants
            .iter()
            .filter(|p| **p != sender)
            .map(|p| mailbox(&world.people[*p]))
            .collect();
        if to.is_empty() {
            raw.push_str("To: undisclosed-recipients:;\r\n");
        } else {
            let _ = write!(raw, "To: {}\r\n", to.join(",\r\n\t"));
        }
        let _ = write!(
            raw,
            "Subject: {}{subject}\r\nDate: {}\r\nMessage-ID: <{id}>\r\n",
            if n > 0 { "Re: " } else { "" },
            rfc_date(time)
        );
        // One reply in twenty-five loses its reply headers, as happens with
        // some web forms and old clients; threading must cope.
        if n > 0 && !rng.chance(4) {
            let _ = write!(raw, "In-Reply-To: <{}>\r\n", ids[n - 1]);
            let chain: Vec<String> = ids.iter().rev().take(12).rev().map(|i| format!("<{i}>")).collect();
            let _ = write!(raw, "References: {}\r\n", chain.join("\r\n\t"));
        }
        raw.push_str("MIME-Version: 1.0\r\n");

        // ---- structure ----
        let attachment = options.attachments && rng.chance(4);
        let html = rng.chance(30);
        let boundary = format!("=_outpost_{:016x}", rng.next());
        let text_part = |raw: &mut String| {
            raw.push_str("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n");
            raw.push_str(&body);
        };
        let content = |raw: &mut String| {
            if html {
                let inner = format!("{boundary}_alt");
                let _ = write!(raw, "Content-Type: multipart/alternative; boundary=\"{inner}\"\r\n\r\n--{inner}\r\n");
                text_part(raw);
                let _ = write!(raw, "\r\n--{inner}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n<html><body>\r\n");
                for paragraph in &own {
                    let _ = write!(raw, "<p>{paragraph}</p>\r\n");
                }
                if !this_quote.is_empty() {
                    raw.push_str("<blockquote type=\"cite\"><pre>\r\n");
                    raw.push_str(&this_quote.replace('<', "&lt;").replace('>', "&gt;"));
                    raw.push_str("</pre></blockquote>\r\n");
                }
                let _ = write!(raw, "</body></html>\r\n--{inner}--\r\n");
            } else {
                text_part(raw);
            }
        };
        if attachment {
            let _ = write!(raw, "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\n");
            content(&mut raw);
            let stem = world.word(&mut rng).to_string();
            let name = format!("{stem}-{}.pdf", rng.below(1000));
            let _ = write!(
                raw,
                "\r\n--{boundary}\r\nContent-Type: application/pdf; name=\"{name}\"\r\n\
                 Content-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n"
            );
            // 4 KB to 256 KB, small ones far more common.
            let size = (4096u64 << rng.below(7)) + rng.below(4096);
            let mut bytes = Vec::with_capacity(size as usize + 8);
            while (bytes.len() as u64) < size {
                bytes.extend_from_slice(&rng.next().to_le_bytes());
            }
            base64(&mut raw, &bytes);
            let _ = write!(raw, "--{boundary}--\r\n");
        } else {
            content(&mut raw);
        }

        // ---- where it goes and how it is marked ----
        let mine = sender == 0;
        let mut flags = Flags::default();
        let recent = time > NEWEST - 30 * 86_400;
        if mine || !(rng.chance(3) || (recent && rng.chance(30))) {
            flags = flags.with(Flags::SEEN);
        }
        if rng.chance(2) {
            flags = flags.with(Flags::FLAGGED);
        }
        messages.push(Message {
            folder: if mine { SENT } else { folder },
            flags,
            raw: raw.into_bytes(),
        });
        senders.push(sender);
        ids.push(id);
        texts.push(own);
        quoted.push(this_quote);
    }

    // A message the owner replied to is marked answered.
    for n in 1..messages.len() {
        if senders[n] == 0 && senders[n - 1] != 0 {
            messages[n - 1].flags = messages[n - 1].flags.with(Flags::ANSWERED).with(Flags::SEEN);
        }
    }
    messages
}

// ---------------------------------------------------------------------------
// Writing the mailbox
// ---------------------------------------------------------------------------

/// Writes a mailbox of `options.messages` messages under `account_dir/mail`.
pub fn generate(
    account_dir: &Path,
    options: &Options,
    mut progress: impl FnMut(u64, u64),
) -> std::io::Result<Summary> {
    let started = Instant::now();
    let world = World::new(options.seed);
    let mail = account_dir.join("mail");

    let mut names: Vec<String> = Vec::new();
    let mut folders: Vec<Maildir> = Vec::new();
    let sent: &[&str] = &["Sent"];
    for path in FOLDERS.iter().map(|(p, _)| *p).chain([sent]) {
        let relative: Vec<String> = path.iter().map(|s| encode_segment(s)).collect();
        // Parents are folders too.
        for depth in 1..relative.len() {
            Maildir::create(mail.join(relative[..depth].join("/")))?;
        }
        folders.push(Maildir::create(mail.join(relative.join("/")))?);
        names.push(path.join("/"));
    }
    let counts: Vec<AtomicU32> = folders.iter().map(|_| AtomicU32::new(0)).collect();

    // Plan: which conversations, and how many messages each, to reach the
    // requested total exactly.
    let mut plan: Vec<(u64, u64)> = Vec::new();
    let mut planned = 0;
    let mut thread = 0;
    while planned < options.messages {
        let size = thread_size(options.seed, thread).min(options.messages - planned);
        plan.push((thread, size));
        planned += size;
        thread += 1;
    }

    let next = AtomicUsize::new(0);
    let written = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    let failure: std::sync::Mutex<Option<std::io::Error>> = std::sync::Mutex::new(None);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some(&(thread, size)) = plan.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                if failure.lock().unwrap().is_some() {
                    break;
                }
                for message in generate_thread(&world, options, thread, size) {
                    let uid = counts[message.folder].fetch_add(1, Ordering::Relaxed) + 1;
                    match folders[message.folder].deliver(&message.raw, message.flags, Some(uid), false) {
                        Ok(_) => {
                            written.fetch_add(1, Ordering::Relaxed);
                            bytes.fetch_add(message.raw.len() as u64, Ordering::Relaxed);
                        }
                        Err(e) => {
                            *failure.lock().unwrap() = Some(e);
                            return;
                        }
                    }
                }
            });
        }
        // Report progress from this thread until the workers are done.
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let done = written.load(Ordering::Relaxed);
            progress(done, options.messages);
            if done >= options.messages || failure.lock().unwrap().is_some() {
                break;
            }
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }

    Ok(Summary {
        messages: written.into_inner(),
        bytes: bytes.into_inner(),
        elapsed: started.elapsed(),
        per_folder: names
            .into_iter()
            .zip(counts.iter().map(|c| c.load(Ordering::Relaxed)))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_formatted_correctly() {
        assert_eq!(rfc_date(0), "Thu, 1 Jan 1970 00:00:00 +0000");
        assert_eq!(rfc_date(NEWEST), "Tue, 17 Nov 2026 12:00:00 +0000");
        assert_eq!(rfc_date(951_782_399), "Mon, 28 Feb 2000 23:59:59 +0000");
        assert_eq!(rfc_date(951_782_400), "Tue, 29 Feb 2000 00:00:00 +0000");
    }

    #[test]
    fn base64_matches_the_standard() {
        let encode = |b: &[u8]| {
            let mut s = String::new();
            base64(&mut s, b);
            s
        };
        assert_eq!(encode(b"f"), "Zg==\r\n");
        assert_eq!(encode(b"fo"), "Zm8=\r\n");
        assert_eq!(encode(b"foo"), "Zm9v\r\n");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy\r\n");
        assert!(encode(&[0u8; 200]).lines().all(|l| l.len() <= 76));
    }

    #[test]
    fn generated_mail_parses_and_threads_are_linked() {
        let options = Options { messages: 0, seed: 7, attachments: true };
        let world = World::new(options.seed);
        let mut replies = 0;
        let mut linked = 0;
        let mut attachments = 0;
        let mut total = 0;
        for thread in 0..400 {
            let size = thread_size(options.seed, thread);
            let messages = generate_thread(&world, &options, thread, size);
            assert_eq!(messages.len() as u64, size);
            for (n, m) in messages.iter().enumerate() {
                let p = outpost_core::store::parse::parse(&m.raw);
                assert!(!p.from_addr.is_empty() && !p.subject.is_empty() && !p.message_id.is_empty());
                assert!(p.date.is_some_and(|d| d <= NEWEST && d > NEWEST - SPAN));
                assert!(!p.preview.is_empty(), "thread {thread} message {n} has no preview");
                assert!(!p.preview.contains("wrote:"), "quote leaked into preview: {}", p.preview);
                assert_eq!(m.folder == SENT, p.from_addr == "owner@example.com");
                if n > 0 {
                    replies += 1;
                    linked += u32::from(!p.in_reply_to.is_empty());
                }
                attachments += u32::from(p.has_attachment);
                total += 1;
            }
        }
        assert!(total > 600, "expected a mix of thread sizes, got {total} messages");
        assert!(linked < replies && linked * 100 / replies > 90, "{linked} of {replies} replies linked");
        assert!(attachments > 0 && attachments * 100 / total < 10);
    }

    #[test]
    fn the_same_seed_gives_the_same_mail() {
        let options = Options { messages: 0, seed: 3, attachments: true };
        let world = World::new(3);
        let a = generate_thread(&world, &options, 11, 5);
        let b = generate_thread(&World::new(3), &options, 11, 5);
        assert!(a.iter().zip(&b).all(|(x, y)| x.raw == y.raw && x.folder == y.folder));
        assert_ne!(World::new(4).word_at(5000), world.word_at(5000));
    }
}
