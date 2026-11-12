//! Text the way the Go core makes it, so the port answers byte for byte. (From Limbo's and Heimdall's ports; the tables are
//! generated from the Go that builds Sarina.)
//!
//! Go strings may hold any bytes; Rust strings are UTF-8. Where a Go string can hold bytes that are not UTF-8 (a word of a
//! shell command after `$'\xff'`), the port keeps them as `Vec<u8>`; [`lossy`] turns bytes into a string as Go's `range`,
//! its regular expressions and its JSON encoder see them: each byte that does not start a valid UTF-8 sequence becomes one
//! U+FFFD.

mod cf;
mod letters;
mod parsefloat;
mod tables;

pub use parsefloat::{parse_float, underscore_ok, ParseFloatError};
pub use tables::GO_UNICODE_VERSION;

fn in_table(table: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// Go's `unicode.IsPrint`.
pub fn is_print(c: char) -> bool {
    if (c as u32) < 0x80 {
        return (0x20..0x7f).contains(&(c as u32));
    }
    in_table(tables::PRINT, c)
}

/// Go's `unicode.IsSpace`.
pub fn is_space(c: char) -> bool {
    in_table(tables::SPACE, c)
}

/// Go's `strings.TrimSpace`.
pub fn trim_space(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Go's `utf8.DecodeRune`: the character at the start of `b` and its length, or None and 1 for a byte that does not start a
/// valid sequence (and None and 0 for no bytes).
pub fn decode_rune(b: &[u8]) -> (Option<char>, usize) {
    if b.is_empty() {
        return (None, 0);
    }
    let n = match b[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    match b.get(..n).and_then(|s| std::str::from_utf8(s).ok()) {
        Some(s) => (s.chars().next(), n),
        None => (None, 1),
    }
}

/// Go's `strings.TrimSpace` on bytes that may not be UTF-8: Unicode spaces off both ends; a byte that is not UTF-8 stops it.
pub fn trim_space_bytes(mut b: &[u8]) -> &[u8] {
    while let (Some(c), n) = decode_rune(b) {
        if !is_space(c) {
            break;
        }
        b = &b[n..];
    }
    // from the end: the spaces are at most 3 bytes long, so try each start position
    'outer: while !b.is_empty() {
        for n in 1..=b.len().min(4) {
            if let (Some(c), m) = decode_rune(&b[b.len() - n..]) {
                if m == n {
                    if is_space(c) {
                        b = &b[..b.len() - n];
                        continue 'outer;
                    }
                    break 'outer;
                }
            }
        }
        break;
    }
    b
}

/// Bytes to a string as Go reads them: each byte that does not start a valid UTF-8 sequence becomes one U+FFFD.
pub fn lossy(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            let mut out = String::with_capacity(b.len() + 8);
            let mut b = b;
            loop {
                match std::str::from_utf8(b) {
                    Ok(s) => {
                        out.push_str(s);
                        return out;
                    }
                    Err(e) => {
                        let good = e.valid_up_to();
                        out.push_str(std::str::from_utf8(&b[..good]).unwrap_or_default());
                        out.push('\u{FFFD}');
                        b = &b[good + 1..];
                    }
                }
            }
        }
    }
}

/// Go's `unicode.IsLetter`.
pub fn is_letter(c: char) -> bool {
    if (c as u32) < 0x80 {
        return c.is_ascii_alphabetic();
    }
    in_table(letters::LETTER, c)
}

/// Go's `unicode.IsDigit` (decimal digits of every script).
pub fn is_digit(c: char) -> bool {
    if (c as u32) < 0x80 {
        return c.is_ascii_digit();
    }
    in_table(letters::DIGIT, c)
}

/// Go's `strings.Fields`: the runs of characters between Unicode spaces.
pub fn fields(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|w| !w.is_empty()).collect()
}

/// Go's `unicode.Is(unicode.Cf, c)`: a format character (Go's table, written by go/guard's parity test).
pub fn is_format(c: char) -> bool {
    in_table(cf::CF, c)
}

/// [`lossy`] with where each part came from: the string, and for every byte offset of it that starts a character (and its
/// end) the offset in `b` (the other offsets hold their character's), so a match in the string maps back to Go's bytes.
pub fn lossy_with_offsets(b: &[u8]) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(b.len() + 8);
    let mut at = Vec::with_capacity(b.len() + 8);
    let mut i = 0;
    while i < b.len() {
        let (c, n) = decode_rune(&b[i..]);
        let c = c.unwrap_or('\u{FFFD}');
        for _ in 0..c.len_utf8() {
            at.push(i);
        }
        out.push(c);
        i += n.max(1);
    }
    at.push(b.len());
    (out, at)
}

/// Go's `strconv.Quote` (`%q`) of a valid string.
pub fn quote(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
            out.push(c);
            continue;
        }
        if is_print(c) {
            out.push(c);
            continue;
        }
        match c {
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c if (c as u32) < 0x10000 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => {
                let _ = write!(out, "\\U{:08x}", c as u32);
            }
        }
    }
    out.push('"');
    out
}

/// Go's `strings.ToLower`: each character by its simple lowercase mapping. Rust's `str::to_lowercase` differs in two places:
/// it writes the dotted capital I (U+0130) as two characters where Go writes `i`, and it writes a word-final capital sigma as
/// final sigma where Go writes σ.
pub fn to_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let mut l = c.to_lowercase();
        match (l.next(), l.next()) {
            (Some(one), None) => out.push(one),
            _ => out.push('i'), // U+0130, the only character whose lowercase is longer than one
        }
    }
    out
}

/// Go's `strings.ToLower` of a string that may hold bytes that are not UTF-8: all ASCII is lowered byte by byte; anything
/// else goes through `strings.Map`, which writes each bad byte as U+FFFD.
pub fn to_lower_bytes(b: &[u8]) -> String {
    to_lower(&lossy(b))
}

/// Go's `strconv.Atoi`: decimal, optional sign, no spaces, within int64.
pub fn atoi(s: &[u8]) -> Option<i64> {
    let t = s.strip_prefix(b"+").or_else(|| s.strip_prefix(b"-")).unwrap_or(s);
    if t.is_empty() || !t.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(s).ok()?.parse().ok()
}

/// `%v` of a float64 (`%g`, shortest): the shortest digits, in exponent form when the exponent is below -4 or at least 6 (Go uses 6 for the shortest form, whatever the digits).
pub fn float_v(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    if x.is_infinite() {
        return if x > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if x.is_nan() {
        return "NaN".into();
    }
    let e = format!("{x:e}"); // shortest round-trip digits: "-1.5e21"
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mant}e{sign}{:02}", exp.abs());
    }
    format!("{x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lossy_is_one_replacement_per_bad_byte() {
        assert_eq!(lossy(b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(lossy(b"\xe2\x84A"), "\u{FFFD}\u{FFFD}A");
        assert_eq!(lossy(b"\xed\xa0\x80"), "\u{FFFD}\u{FFFD}\u{FFFD}");
    }

    #[test]
    fn quote_matches_go() {
        assert_eq!(quote("a\u{200b}b\u{a0}c\u{1}é"), "\"a\\u200bb\\u00a0c\\x01é\"");
        assert_eq!(quote("\u{E0041}"), r#""\U000e0041""#);
    }

    #[test]
    fn atoi_matches_go() {
        assert_eq!(atoi(b"+5"), Some(5));
        assert_eq!(atoi(b"-0"), Some(0));
        assert_eq!(atoi(b"99999999999999999999"), None);
        assert_eq!(atoi(b"1 "), None);
        assert_eq!(atoi(b"-"), None);
    }
}
