//! IMAP's "modified UTF-7" (RFC 3501 section 5.1.3), which servers use for
//! folder names that are not plain ASCII: `&` starts a run of base64-encoded
//! UTF-16, `-` ends it, and `&-` is a literal ampersand.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

/// Decodes a folder name. A name that is not valid modified UTF-7 is
/// returned as it is (lossily, if it is not UTF-8 either): some servers send
/// raw UTF-8, and a folder with an odd name must still be shown.
pub fn decode(raw: &[u8]) -> String {
    try_decode(raw).unwrap_or_else(|| String::from_utf8_lossy(raw).into_owned())
}

fn try_decode(raw: &[u8]) -> Option<String> {
    let mut out = String::new();
    let mut i = 0;
    while i < raw.len() {
        let byte = raw[i];
        if byte != b'&' {
            if !(0x20..=0x7e).contains(&byte) {
                return None;
            }
            out.push(byte as char);
            i += 1;
            continue;
        }
        let end = i + 1 + raw[i + 1..].iter().position(|b| *b == b'-')?;
        let run = &raw[i + 1..end];
        if run.is_empty() {
            out.push('&');
        } else {
            let mut bits: u32 = 0;
            let mut count = 0;
            let mut units: Vec<u16> = Vec::new();
            for symbol in run {
                let value = ALPHABET.iter().position(|a| a == symbol)? as u32;
                bits = (bits << 6) | value;
                count += 6;
                if count >= 16 {
                    count -= 16;
                    units.push((bits >> count) as u16);
                    bits &= (1 << count) - 1;
                }
            }
            // Left-over bits are padding and must be zero.
            if bits != 0 || count >= 6 {
                return None;
            }
            out.push_str(&String::from_utf16(&units).ok()?);
        }
        i = end + 1;
    }
    Some(out)
}

/// Encodes a folder name for the server.
pub fn encode(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pending: Vec<u16> = Vec::new();
    fn flush(pending: &mut Vec<u16>, out: &mut Vec<u8>) {
        if pending.is_empty() {
            return;
        }
        out.push(b'&');
        let mut bits: u32 = 0;
        let mut count = 0;
        for unit in pending.drain(..) {
            bits = (bits << 16) | unit as u32;
            count += 16;
            while count >= 6 {
                count -= 6;
                out.push(ALPHABET[((bits >> count) & 63) as usize]);
            }
            bits &= (1 << count) - 1;
        }
        if count > 0 {
            out.push(ALPHABET[((bits << (6 - count)) & 63) as usize]);
        }
        out.push(b'-');
    }
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut pending, &mut out);
            out.push(c as u8);
            if c == '&' {
                out.push(b'-');
            }
        } else {
            let mut buffer = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut buffer));
        }
    }
    flush(&mut pending, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_examples_and_round_trips() {
        let cases: &[(&str, &str)] = &[
            ("INBOX", "INBOX"),
            ("Sent Items", "Sent Items"),
            ("Tom & Jerry", "Tom &- Jerry"),
            ("Entwürfe", "Entw&APw-rfe"),
            ("~peter/mail/台北/日本語", "~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            ("Отправленные", "&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-"),
            ("😀 fun", "&2D3eAA- fun"),
            ("&&", "&-&-"),
        ];
        for (name, wire) in cases {
            assert_eq!(decode(wire.as_bytes()), *name, "decoding {wire}");
            assert_eq!(String::from_utf8(encode(name)).unwrap(), *wire, "encoding {name}");
        }
    }

    #[test]
    fn invalid_input_is_shown_as_it_is() {
        assert_eq!(decode(b"Bad & name"), "Bad & name");
        assert_eq!(decode(b"&!!!-"), "&!!!-");
        assert_eq!(decode("Entwürfe".as_bytes()), "Entwürfe", "raw UTF-8 from a lax server");
        assert_eq!(decode(b"&AA"), "&AA", "unterminated");
        assert_eq!(decode(b"&2D0-"), "&2D0-", "half a surrogate pair");
    }
}
