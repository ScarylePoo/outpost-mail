//! A deterministic fake mailbox, used as the core-side stub in milestone 0.
//!
//! Row `i` is computed from `i` alone, so a folder of any size costs no memory
//! and no startup time. Milestone 1 replaces this with the real index and a
//! generator that writes actual message files.

use crate::api::MessageRow;

/// 17 Nov 2026 12:00:00 UTC. Fixed so that output never depends on the clock.
const NEWEST: i64 = 1_794_916_800;

const FIRST: &[&str] = &[
    "Alice", "Bram", "Chidi", "Dana", "Elif", "Farid", "Greta", "Hugo", "Ines", "Jonas", "Keiko",
    "Lars", "Mira", "Nadia", "Oskar", "Priya", "Quentin", "Rosa", "Sven", "Tamar", "Uma",
    "Viktor", "Wanjiru", "Xavier", "Yusuf", "Zofia",
];
const LAST: &[&str] = &[
    "Abbott", "Bergström", "Castellanos", "Dubois", "Eriksen", "Fontaine", "Grünewald", "Haddad",
    "Ivanova", "Jørgensen", "Kowalski", "Lindqvist", "Moreau", "Nakamura", "O'Brien", "Petrov",
    "Quigley", "Rosenthal", "Santos", "Thackeray", "Underwood", "Vasquez", "Whitfield", "Yamada",
];
const DOMAIN: &[&str] = &[
    "example.com", "example.org", "example.net", "mail.example", "corp.example", "uni.example",
];
const TOPIC: &[&str] = &[
    "Quarterly numbers", "Roof repair quote", "Dinner on Saturday", "Server migration",
    "Invoice 4471", "School trip forms", "Holiday photos", "Backup failed last night",
    "New mailing address", "Car service booking", "Draft agenda", "Password reset",
    "Garden fence", "Flight change", "Printer in the hall", "Contract renewal",
    "Lost umbrella", "Parking permit", "Reading group", "Domain renewal notice",
];
const PREFIX: &[&str] = &["", "", "", "Re: ", "Re: ", "Fwd: ", "RE: "];
const SENTENCE: &[&str] = &[
    "Thanks for getting back to me so quickly.",
    "I had a look at this over the weekend and I think we can make it work.",
    "Can you confirm the dates before Friday?",
    "The attached file has the figures you asked for, with last year's alongside for comparison.",
    "Sorry for the slow reply, it has been one of those weeks.",
    "Let me know if any of that is unclear and I will call you.",
    "We should probably loop in the others before deciding anything.",
    "Sounds good.",
    "I spoke to the supplier this morning and they can deliver on the 4th, but only before noon.",
    "No rush on this one.",
    "Could you forward me the original message? I can't find it anywhere.",
    "That works for me.",
    "I am not sure the second option is realistic given the budget, but I am happy to be persuaded.",
    "See you then!",
    "The short version is that nothing has changed since we last spoke.",
    "Please ignore my earlier message, I had the wrong account.",
];

/// SplitMix64: a small, well-mixed hash of the row number.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

struct Bits(u64);

impl Bits {
    /// Next pseudo-random value below `n`.
    fn below(&mut self, n: usize) -> usize {
        self.0 = mix(self.0);
        (self.0 % n as u64) as usize
    }
    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.below(from.len())]
    }
}

fn preview(bits: &mut Bits) -> String {
    // Roughly a quarter one-liners, a quarter medium, half long enough to
    // fill three lines, which is the case the shell test cares about.
    let sentences = match bits.below(4) {
        0 => 1,
        1 => 2,
        _ => 4 + bits.below(3),
    };
    let mut out = String::new();
    for n in 0..sentences {
        if n > 0 {
            out.push(' ');
        }
        out.push_str(bits.pick(SENTENCE));
    }
    out
}

/// Preview length in characters for row `index`, without building the row's
/// other fields. Must agree with [`row`].
pub fn preview_len(seed: u64, index: u32) -> u16 {
    let mut bits = Bits(mix(seed ^ ((index as u64) << 20)));
    // Keep the draw order identical to `row`.
    let _ = (bits.pick(FIRST), bits.pick(LAST), bits.pick(DOMAIN));
    let _ = (bits.pick(PREFIX), bits.pick(TOPIC));
    preview(&mut bits).chars().count().min(u16::MAX as usize) as u16
}

pub fn row(seed: u64, index: u32) -> MessageRow {
    let mut bits = Bits(mix(seed ^ ((index as u64) << 20)));
    let first = bits.pick(FIRST);
    let last = bits.pick(LAST);
    let domain = bits.pick(DOMAIN);
    let prefix = bits.pick(PREFIX);
    let topic = bits.pick(TOPIC);
    let preview = preview(&mut bits);
    let flags = bits.below(100);
    // Newest first, 11 minutes and a bit apart on average.
    let jitter = bits.below(600) as i64;
    let local: String = format!("{first}.{last}")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect();
    MessageRow {
        id: index as i64,
        index,
        sender_name: format!("{first} {last}"),
        sender_address: format!("{local}@{domain}"),
        date: NEWEST - (index as i64) * 700 - jitter,
        subject: format!("{prefix}{topic}"),
        preview,
        unread: flags < 12,
        flagged: false,
        thread_count: 1,
        thread_unread: u32::from(flags < 12),
        senders: String::new(),
        check: "notChecked".into(),
        has_attachment: flags >= 90,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_deterministic() {
        assert_eq!(row(7, 12_345), row(7, 12_345));
        assert_ne!(row(7, 12_345), row(7, 12_346));
        assert_ne!(row(7, 12_345), row(8, 12_345));
    }

    #[test]
    fn preview_len_matches_row() {
        for i in (0..100_000).step_by(997) {
            assert_eq!(
                preview_len(1, i) as usize,
                row(1, i).preview.chars().count(),
                "row {i}"
            );
        }
    }

    #[test]
    fn dates_descend() {
        let mut last = i64::MAX;
        for i in 0..5_000 {
            let d = row(1, i).date;
            assert!(d < last, "row {i} is not older than row {}", i.max(1) - 1);
            last = d;
        }
    }

    #[test]
    fn all_three_preview_sizes_occur() {
        let lens: Vec<u16> = (0..2_000).map(|i| preview_len(1, i)).collect();
        assert!(lens.iter().any(|&l| l < 60));
        assert!(lens.iter().any(|&l| l > 250));
    }
}
