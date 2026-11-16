//! Go: `cryptotools/decode.go`: crypto_decode: which encodings a string looks like, one decoder at a time with Go's
//! own decoders' rules and error texts (encoding/hex, base64, base32, ascii85, strconv, net/url), base58 and Morse, and
//! the automatic decoding up to three layers deep with its description (Go's `hex.Dump` for bytes that are not text).
//! On Go's bytes: a decoded layer need not be UTF-8, and Go reads such a byte as U+FFFD wherever it ranges over a string
//! (testdata/parity/decode.json).

use super::classic::caesar;
use super::english::{entropy, printable, word_ratio};
use super::{fmt_f, hash::strip_sep};
use crate::gotext::{self, lossy, quote, quote_bytes};
use regex::Regex;
use std::sync::LazyLock;

/// Go's `Encodings`, in the order the automatic decoding tries them.
pub const ENCODINGS: [&str; 10] = ["hex", "base64", "base32", "base58", "ascii85", "binary", "decimal", "url", "rot13", "morse"];

fn re(p: &str) -> Regex {
    crate::goregexp::compile(p).expect("decode pattern")
}
static HEX_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:[0-9a-fA-F]{2}[\s:,-]*)+$"));
static BASE64_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9+/\s]+={0,2}\s*$"));
static BASE64_URL: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9_-]+={0,2}$"));
static BASE32_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Z2-7\s]+={0,6}\s*$"));
static BASE58_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[1-9A-HJ-NP-Za-km-z]+$"));
static BINARY_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[01\s]+$"));
static DECIMAL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:\d{1,3}[\s,]+)+\d{1,3}\s*$"));
static MORSE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[.\-/ ]+$"));
static ASCII85_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^<~[!-u\s]*~>$"));
static URL_RE: LazyLock<Regex> = LazyLock::new(|| re(r"%[0-9A-Fa-f]{2}"));

const BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn morse(code: &str) -> Option<char> {
    Some(match code {
        ".-" => 'A',
        "-..." => 'B',
        "-.-." => 'C',
        "-.." => 'D',
        "." => 'E',
        "..-." => 'F',
        "--." => 'G',
        "...." => 'H',
        ".." => 'I',
        ".---" => 'J',
        "-.-" => 'K',
        ".-.." => 'L',
        "--" => 'M',
        "-." => 'N',
        "---" => 'O',
        ".--." => 'P',
        "--.-" => 'Q',
        ".-." => 'R',
        "..." => 'S',
        "-" => 'T',
        "..-" => 'U',
        "...-" => 'V',
        ".--" => 'W',
        "-..-" => 'X',
        "-.--" => 'Y',
        "--.." => 'Z',
        "-----" => '0',
        ".----" => '1',
        "..---" => '2',
        "...--" => '3',
        "....-" => '4',
        "....." => '5',
        "-...." => '6',
        "--..." => '7',
        "---.." => '8',
        "----." => '9',
        ".-.-.-" => '.',
        "--..--" => ',',
        "..--.." => '?',
        "-....-" => '-',
        ".--.-." => '@',
        _ => return None,
    })
}

/// Go's `Looks`: whether s has the shape of an encoding.
pub fn looks(enc: &str, s: &[u8]) -> bool {
    let s = lossy(gotext::trim_space_bytes(s));
    if s.is_empty() {
        return false;
    }
    match enc {
        "hex" => HEX_RE.is_match(&s) && strip_sep(&s).len() >= 2,
        "base64" => {
            let c: String = gotext::fields(&s).concat();
            c.len() >= 4 && (BASE64_RE.is_match(&s) || BASE64_URL.is_match(&c)) && !HEX_RE.is_match(&s)
        }
        "base32" => s.len() >= 8 && BASE32_RE.is_match(&s) && s.contains(['2', '3', '4', '5', '6', '7', '=']) && !HEX_RE.is_match(&s),
        "base58" => s.len() >= 6 && BASE58_RE.is_match(&s) && !HEX_RE.is_match(&s),
        "ascii85" => ASCII85_RE.is_match(&s),
        "binary" => BINARY_RE.is_match(&s) && strip_sep(&s).len() >= 8,
        "decimal" => DECIMAL_RE.is_match(&s),
        "url" => s.contains('%') && URL_RE.is_match(&s),
        "rot13" => s.chars().filter(char::is_ascii_alphabetic).count() >= 4,
        "morse" => MORSE_RE.is_match(&s) && s.contains(['.', '-']),
        _ => false,
    }
}

// ---- Go's decoders ----------------------------------------------------------------------------------------------------

const STD64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const STD32: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn decode_map(alphabet: &[u8]) -> [u8; 256] {
    let mut m = [0xffu8; 256];
    for (i, &c) in alphabet.iter().enumerate() {
        m[c as usize] = i as u8;
    }
    m
}

/// Go's `base64.Raw...Encoding.DecodeString` with an alphabet (no padding).
pub(crate) fn base64_raw(src: &[u8], alphabet: &[u8]) -> Result<Vec<u8>, String> {
    base64_decode(src, alphabet, false)
}

/// Go's `base64.Encoding.DecodeString` (not strict): padded or raw; newlines and returns skipped.
fn base64_decode(src: &[u8], alphabet: &[u8], padded: bool) -> Result<Vec<u8>, String> {
    let map = decode_map(alphabet);
    let corrupt = |at: usize| format!("illegal base64 data at input byte {at}");
    let mut out = Vec::with_capacity(src.len() / 4 * 3 + 3);
    let mut si = 0;
    while si < src.len() {
        let mut dbuf = [0u8; 4];
        let mut dlen = 4;
        let mut j = 0;
        let mut err = None;
        while j < 4 {
            if src.len() == si {
                if j == 0 {
                    return Ok(out);
                }
                if j == 1 || padded {
                    return Err(corrupt(si - j));
                }
                dlen = j;
                break;
            }
            let c = src[si];
            si += 1;
            let v = map[c as usize];
            if v != 0xff {
                dbuf[j] = v;
                j += 1;
                continue;
            }
            if c == b'\n' || c == b'\r' {
                continue;
            }
            if !padded || c != b'=' {
                return Err(corrupt(si - 1));
            }
            match j {
                0 | 1 => return Err(corrupt(si - 1)),
                2 => {
                    while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                        si += 1;
                    }
                    if si == src.len() {
                        return Err(corrupt(src.len()));
                    }
                    if src[si] != b'=' {
                        return Err(corrupt(si - 1));
                    }
                    si += 1;
                }
                _ => {}
            }
            while si < src.len() && (src[si] == b'\n' || src[si] == b'\r') {
                si += 1;
            }
            if si < src.len() {
                err = Some(corrupt(si));
            }
            dlen = j;
            break;
        }
        let val = (dbuf[0] as u32) << 18 | (dbuf[1] as u32) << 12 | (dbuf[2] as u32) << 6 | dbuf[3] as u32;
        let bytes = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
        out.extend_from_slice(&bytes[..dlen.saturating_sub(1)]);
        if let Some(e) = err {
            return Err(e);
        }
    }
    Ok(out)
}

/// Go's `base32.Encoding.DecodeString` (StdEncoding, padded or not).
fn base32_decode(src: &[u8], padded: bool) -> Result<Vec<u8>, String> {
    let src: Vec<u8> = src.iter().copied().filter(|&c| c != b'\n' && c != b'\r').collect();
    let map = decode_map(STD32);
    let corrupt = |at: usize| format!("illegal base32 data at input byte {at}");
    let olen = src.len();
    let mut rest = &src[..];
    let mut out = Vec::new();
    let mut end = false;
    while !rest.is_empty() && !end {
        let mut dbuf = [0u8; 8];
        let mut dlen = 8;
        let mut j = 0;
        while j < 8 {
            if rest.is_empty() {
                if padded {
                    return Err(corrupt(olen - rest.len() - j));
                }
                dlen = j;
                end = true;
                break;
            }
            let c = rest[0];
            rest = &rest[1..];
            // Go's byte(padChar): NoPadding (-1) is 0xff
            let pad = if padded { b'=' } else { 0xff };
            if c == pad && j >= 2 && rest.len() < 8 {
                if rest.len() + j < 7 {
                    return Err(corrupt(olen));
                }
                for k in 0..7 - j {
                    if rest.len() > k && rest[k] != pad {
                        return Err(corrupt(olen - rest.len() + k - 1));
                    }
                }
                dlen = j;
                end = true;
                if dlen == 1 || dlen == 3 || dlen == 6 {
                    return Err(corrupt(olen - rest.len() - 1));
                }
                break;
            }
            dbuf[j] = map[c as usize];
            if dbuf[j] == 0xff {
                return Err(corrupt(olen - rest.len() - 1));
            }
            j += 1;
        }
        let b = [
            dbuf[0] << 3 | dbuf[1] >> 2,
            dbuf[1] << 6 | dbuf[2] << 1 | dbuf[3] >> 4,
            dbuf[3] << 4 | dbuf[4] >> 1,
            dbuf[4] << 7 | dbuf[5] << 2 | dbuf[6] >> 3,
            dbuf[6] << 5 | dbuf[7],
        ];
        let n = match dlen {
            8 => 5,
            7 => 4,
            5 => 3,
            4 => 2,
            2 => 1,
            _ => 0,
        };
        out.extend_from_slice(&b[..n]);
    }
    Ok(out)
}

/// Go's `ascii85.Decode(dst, src, true)` into a buffer of 4 bytes per input byte.
fn ascii85_decode(src: &[u8]) -> Result<Vec<u8>, String> {
    let cap = 4 * src.len();
    let mut out = Vec::new();
    let (mut v, mut nb) = (0u32, 0);
    for (i, &b) in src.iter().enumerate() {
        if cap - out.len() < 4 {
            return Ok(out);
        }
        if b <= b' ' {
            continue;
        } else if b == b'z' && nb == 0 {
            nb = 5;
            v = 0;
        } else if (b'!'..=b'u').contains(&b) {
            v = v.wrapping_mul(85).wrapping_add((b - b'!') as u32);
            nb += 1;
        } else {
            return Err(format!("illegal ascii85 data at input byte {i}"));
        }
        if nb == 5 {
            out.extend_from_slice(&v.to_be_bytes());
            nb = 0;
            v = 0;
        }
    }
    if nb > 0 {
        if nb == 1 {
            return Err(format!("illegal ascii85 data at input byte {}", src.len()));
        }
        for _ in nb..5 {
            v = v.wrapping_mul(85).wrapping_add(84);
        }
        for _ in 0..nb - 1 {
            out.push((v >> 24) as u8);
            v <<= 8;
        }
    }
    Ok(out)
}

/// Go's `%#U` of a byte read as a rune: `U+0067 'g'` (the character only when printable).
fn sharp_u(b: u8) -> String {
    let c = b as char;
    if gotext::is_print(c) {
        format!("U+{:04X} '{c}'", b)
    } else {
        format!("U+{:04X}", b)
    }
}

/// Go's `hex.DecodeString`.
pub(crate) fn hex_decode(src: &[u8]) -> Result<Vec<u8>, String> {
    let val = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(src.len() / 2);
    let mut j = 1;
    while j < src.len() {
        let (p, q) = (src[j - 1], src[j]);
        let Some(a) = val(p) else { return Err(format!("encoding/hex: invalid byte: {}", sharp_u(p))) };
        let Some(b) = val(q) else { return Err(format!("encoding/hex: invalid byte: {}", sharp_u(q))) };
        out.push(a << 4 | b);
        j += 2;
    }
    if src.len() % 2 == 1 {
        if val(src[j - 1]).is_none() {
            return Err(format!("encoding/hex: invalid byte: {}", sharp_u(src[j - 1])));
        }
        return Err("encoding/hex: odd length hex string".into());
    }
    Ok(out)
}

/// Go's `url.QueryUnescape` on bytes.
fn query_unescape(s: &[u8]) -> Result<Vec<u8>, String> {
    let ishex = |c: u8| c.is_ascii_hexdigit();
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            if i + 2 >= s.len() || !ishex(s[i + 1]) || !ishex(s[i + 2]) {
                let e = &s[i..(i + 3).min(s.len())];
                return Err(format!("invalid URL escape {}", quote_bytes(e)));
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    let unhex = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                out.push(unhex(s[i + 1]) << 4 | unhex(s[i + 2]));
                i += 2;
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    Ok(out)
}

/// Go's `strconv.QuoteRune` (`%q` of a rune).
fn quote_rune(c: char) -> String {
    match c {
        '\'' => "'\\''".into(),
        '"' => "'\"'".into(),
        _ => {
            let q = quote(&c.to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    }
}

pub(crate) fn base58_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut n: Vec<u8> = Vec::new(); // big-endian, no leading zeros
    for r in s.chars() {
        let Some(i) = BASE58.find(r) else { return Err(format!("{} is not a base58 character", quote_rune(r))) };
        let mut carry = i as u32;
        for byte in n.iter_mut().rev() {
            let x = *byte as u32 * 58 + carry;
            *byte = x as u8;
            carry = x >> 8;
        }
        while carry > 0 {
            n.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    let lead = s.chars().take_while(|&r| r == '1').count();
    let mut out = vec![0u8; lead];
    out.extend(n.into_iter().skip_while(|&b| b == 0));
    Ok(out)
}

/// Go's `DecodeOne`: s decoded as one named encoding.
pub fn decode_one(enc: &str, s: &[u8]) -> Result<Vec<u8>, String> {
    let s = gotext::trim_space_bytes(s);
    let text = lossy(s);
    match enc {
        "hex" => hex_decode(strip_sep(&text).as_bytes()),
        "base64" => {
            let c: Vec<u8> = gotext::fields_bytes(s).concat();
            let raw: &[u8] = {
                let mut end = c.len();
                while end > 0 && c[end - 1] == b'=' {
                    end -= 1;
                }
                &c[..end]
            };
            base64_decode(&c, STD64, true)
                .or_else(|_| base64_decode(raw, STD64, false))
                .or_else(|_| base64_decode(&c, URL64, true))
                .or_else(|_| base64_decode(raw, URL64, false))
        }
        "base32" => {
            let c: Vec<u8> = gotext::fields_bytes(s).concat();
            let mut end = c.len();
            while end > 0 && c[end - 1] == b'=' {
                end -= 1;
            }
            base32_decode(&c, true).or_else(|_| base32_decode(&c[..end], false))
        }
        "base58" => base58_decode(&text),
        "ascii85" => {
            let body = s.strip_prefix(b"<~").unwrap_or(s);
            let body = body.strip_suffix(b"~>").unwrap_or(body);
            ascii85_decode(body)
        }
        "binary" => {
            let bits = strip_sep(&text);
            let b = bits.as_bytes();
            if !b.len().is_multiple_of(8) {
                return Err(format!("{} bits is not a whole number of bytes", b.len()));
            }
            b.chunks(8)
                .map(|c| {
                    if c.iter().all(|&x| x == b'0' || x == b'1') {
                        Ok(c.iter().fold(0u8, |a, &x| a << 1 | (x - b'0')))
                    } else {
                        Err(format!("strconv.ParseUint: parsing {}: invalid syntax", quote_bytes(c)))
                    }
                })
                .collect()
        }
        "decimal" => s
            .split(|&c| matches!(c, b' ' | b',' | b'\n' | b'\t'))
            .filter(|f| !f.is_empty())
            .map(|f| match gotext::atoi(f) {
                Some(v) if (0..=255).contains(&v) => Ok(v as u8),
                _ => Err(format!("{} is not a byte value", quote_bytes(f))),
            })
            .collect(),
        "url" => query_unescape(s),
        "rot13" => Ok(caesar(&text, 13).into_bytes()),
        "morse" => {
            let mut b = String::new();
            for word in text.split('/') {
                for letter in gotext::fields(word) {
                    match morse(letter) {
                        Some(r) => b.push(r),
                        None => return Err(format!("{} is not Morse", quote(letter))),
                    }
                }
                b.push(' ');
            }
            Ok(gotext::trim_space(&b).as_bytes().to_vec())
        }
        _ => Err(format!("unknown encoding {} (one of {})", quote(enc), ENCODINGS.join(", "))),
    }
}

/// Go's `LooksLikeText` of bytes (its length is the bytes', the rest read as Go reads the string).
pub fn looks_like_text(b: &[u8]) -> bool {
    b.len() >= 2 && super::english::looks_like_text_after_len(&lossy(b))
}

/// Go's `TruncateText` of bytes: the bytes as they are when short enough, else the first n characters (a byte that is
/// not UTF-8 written as U+FFFD) and how many more there were.
pub fn truncate_bytes(b: &[u8], n: usize) -> Vec<u8> {
    let s = lossy(b);
    if s.chars().count() <= n {
        return b.to_vec();
    }
    super::truncate_text(&s, n).into_bytes()
}

/// One decoding step (Go's `Step`).
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub encoding: &'static str,
    pub out: Vec<u8>,
}

/// A chain of decodings (Go's `Chain`); `text`: the final bytes read as plain text.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub steps: Vec<Step>,
    pub text: bool,
}

/// Go's `DecodeAuto`: the shortest chain of up to `depth` layers that ends in text, else the longest one found.
pub fn decode_auto(s: &[u8], depth: usize) -> Option<Chain> {
    let depth = if depth == 0 { 3 } else { depth };
    let mut queue: std::collections::VecDeque<(Vec<u8>, Vec<Step>)> = std::collections::VecDeque::from([(s.to_vec(), Vec::new())]);
    let mut longest: Option<Chain> = None;
    while let Some((text, steps)) = queue.pop_front() {
        if steps.len() >= depth {
            continue;
        }
        for enc in ENCODINGS {
            if enc == "rot13" && !steps.is_empty() {
                continue;
            }
            if !looks(enc, &text) {
                continue;
            }
            let Ok(out) = decode_one(enc, &text) else { continue };
            if out.is_empty() {
                continue;
            }
            if enc == "rot13" && word_ratio(&lossy(&out)) <= word_ratio(&lossy(&text)) {
                continue;
            }
            let mut next = steps.clone();
            next.push(Step { encoding: enc, out: out.clone() });
            if looks_like_text(&out) && (enc != "rot13" || word_ratio(&lossy(&out)) >= 0.3) {
                return Some(Chain { steps: next, text: true });
            }
            if longest.as_ref().is_none_or(|l| next.len() > l.steps.len()) {
                longest = Some(Chain { steps: next.clone(), text: false });
            }
            if printable(&lossy(&out)) > 0.9 {
                queue.push_back((out, next));
            }
        }
    }
    longest
}

/// Go's `hex.Dump` (the `hexdump -C` layout).
pub fn hex_dump(data: &[u8]) -> String {
    let mut out = String::new();
    for (k, line) in data.chunks(16).enumerate() {
        out += &format!("{:08x}  ", k * 16);
        for (i, b) in line.iter().enumerate() {
            out += &format!("{b:02x} ");
            if i == 7 {
                out.push(' ');
            } else if i == 15 {
                out += " |";
            }
        }
        if line.len() < 16 {
            for i in line.len()..16 {
                out += match i {
                    7 => "    ",
                    15 => "    |",
                    _ => "   ",
                };
            }
        }
        out.extend(line.iter().map(|&b| if !(32..=126).contains(&b) { '.' } else { b as char }));
        out += "|\n";
    }
    out
}

/// Go's `Chain.Describe` (None: Go's nil chain).
pub fn describe(c: Option<&Chain>) -> Vec<u8> {
    let Some(c) = c else {
        return b"Nothing decoded: none of the encodings I know fit, or the result was not readable.".to_vec();
    };
    let names: Vec<&str> = c.steps.iter().map(|s| s.encoding).collect();
    let mut b = format!(
        "Decoded {} ({} layer(s): {}).\n",
        if c.text { "to text" } else { "to bytes that are not plain text" },
        c.steps.len(),
        names.join(" -> ")
    )
    .into_bytes();
    let last = &c.steps[c.steps.len() - 1].out;
    if c.text {
        b.extend_from_slice(format!("Result ({} characters):\n", lossy(last).chars().count()).as_bytes());
        b.extend_from_slice(&truncate_bytes(last, 4000));
        b.push(b'\n');
    } else {
        b.extend_from_slice(
            format!(
                "Result: {} bytes, entropy {} bits/byte. Hex dump of the start:\n{}\n",
                last.len(),
                fmt_f(entropy(last), 2),
                hex_dump(&last[..last.len().min(256)])
            )
            .as_bytes(),
        );
    }
    b
}
