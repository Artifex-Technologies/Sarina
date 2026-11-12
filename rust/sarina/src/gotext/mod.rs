//! Text the way the Go core makes it, so the port answers byte for byte. (From Limbo's and Heimdall's ports; the tables are
//! generated from the Go that builds Sarina.)
//!
//! Go strings may hold any bytes; Rust strings are UTF-8. Where a Go string can hold bytes that are not UTF-8 (a word of a
//! shell command after `$'\xff'`), the port keeps them as `Vec<u8>`; [`lossy`] turns bytes into a string as Go's `range`,
//! its regular expressions and its JSON encoder see them: each byte that does not start a valid UTF-8 sequence becomes one
//! U+FFFD.

mod tables;

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
