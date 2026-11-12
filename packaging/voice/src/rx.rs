//! Regular expressions with Python's meaning. The patterns are written with these spellings of Python's classes, because the
//! regex crate's \s and \w are not quite Python's: Python's \s also matches the separators \x1c-\x1f (one of which marks a
//! withdrawn draft), and Python's \w is a letter, a number or "_" (no combining marks).
//!
//! Python's look-behinds and back-references (the regex crate has neither) are done by hand below.

use regex::Regex;
use std::sync::LazyLock;

/// Python's \s
#[macro_export]
macro_rules! S {
    () => {
        r"[\s\x1c-\x1f]"
    };
}

/// A compiled pattern, made on first use.
#[macro_export]
macro_rules! rx {
    ($name:ident, $pat:expr) => {
        static $name: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new($pat).unwrap());
    };
}

static ALNUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}]$").unwrap());
static RUST_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\w$").unwrap());

/// The text as the regex crate must see it for a case-insensitive pattern with \b to match where Python's re matched:
/// - "ı" and "İ" become "i" (Python's re.IGNORECASE matches both to i; the regex crate's case folding does not);
/// - a character Python counts as a word character and the regex crate does not (a number like "²") becomes "中";
/// - one the regex crate counts and Python does not (a combining mark, U+FE0F, the joiner U+200D, "‿") becomes "·".
///
/// One character for one, so a position found in the result is the same character's position in the text. The patterns
/// this is used for name only ASCII letters, spaces and punctuation, so the stand-ins never match anything themselves.
pub fn pyb(s: &str) -> std::borrow::Cow<'_, str> {
    if s.is_ascii() {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
            continue;
        }
        if c == 'ı' || c == 'İ' {
            out.push('i');
            continue;
        }
        let mut b = [0u8; 4];
        let py = c == '_' || ALNUM.is_match(c.encode_utf8(&mut b));
        let rs = RUST_WORD.is_match(c.encode_utf8(&mut b));
        out.push(match (py, rs) {
            (true, false) => '中',
            (false, true) => '·',
            _ => c,
        });
    }
    std::borrow::Cow::Owned(out)
}
static UNPRINTABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{C}\p{Z}]$").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!(S!(), "+")).unwrap());

pub fn is_py_alnum(c: char) -> bool {
    let mut b = [0u8; 4];
    ALNUM.is_match(c.encode_utf8(&mut b))
}

/// Python's \w for one character.
pub fn is_py_word(c: char) -> bool {
    c == '_' || crate::py::is_alnum(c)
}

/// str.isprintable() for one character.
pub fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    let mut b = [0u8; 4];
    !UNPRINTABLE.is_match(c.encode_utf8(&mut b))
}

/// re.split(r"(?<=[.!?])\s+", text): split at each run of whitespace that follows ".", "!" or "?".
pub fn split_sentences(text: &str) -> Vec<&str> {
    split_at_runs(text, |before| before.ends_with(['.', '!', '?']))
}

/// The streamer's split, r"(?<=[.!?])(?<!\s\d\.)(?<!\s\d\d\.)\s+": as above, but " 1." and " 12." (a list number) do not end a sentence.
pub fn split_sentences_not_numbers(text: &str) -> Vec<&str> {
    split_at_runs(text, |before| {
        if !before.ends_with(['.', '!', '?']) {
            return false;
        }
        let tail: Vec<char> = before.chars().rev().take(4).collect(); // last chars, newest first
        let ws = |c: char| crate::py::is_space(c);
        let digit = |c: char| c.is_ascii_digit() || (!c.is_ascii() && is_unicode_decimal(c));
        if tail.len() >= 3 && tail[0] == '.' && digit(tail[1]) && ws(tail[2]) {
            return false;
        }
        if tail.len() >= 4 && tail[0] == '.' && digit(tail[1]) && digit(tail[2]) && ws(tail[3]) {
            return false;
        }
        true
    })
}

fn split_at_runs(text: &str, splits: impl Fn(&str) -> bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for m in SPACES.find_iter(text) {
        if m.start() > 0 && splits(&text[..m.start()]) {
            out.push(&text[start..m.start()]);
            start = m.end();
        }
    }
    out.push(&text[start..]);
    out
}

static DECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").unwrap());

/// \d for a non-ASCII character (Unicode decimal digits).
pub fn is_unicode_decimal(c: char) -> bool {
    let mut b = [0u8; 4];
    DECIMAL.is_match(c.encode_utf8(&mut b))
}

/// The value of a non-ASCII decimal digit: digits come in runs of ten from a zero, so walk back to the start of the run.
pub fn unicode_digit_value(c: char) -> u32 {
    let mut start = c as u32;
    while start > 0 {
        match char::from_u32(start - 1) {
            Some(p) if is_unicode_decimal(p) => start -= 1,
            _ => break,
        }
    }
    (c as u32 - start) % 10
}

/// re.sub(r"<(\w+)>.*?</\1>", "", text, flags=re.S): a <tag>...</tag> block (a think block) is removed, the first matching
/// close ending it.
pub fn strip_tag_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some(end) = tag_block_at(text, i) {
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn tag_block_at(text: &str, i: usize) -> Option<usize> {
    let rest = &text[i..];
    if !rest.starts_with('<') {
        return None;
    }
    let name_len: usize = rest[1..].chars().take_while(|&c| is_py_word(c)).map(|c| c.len_utf8()).sum();
    if name_len == 0 || !rest[1 + name_len..].starts_with('>') {
        return None;
    }
    let name = &rest[1..1 + name_len];
    let close = format!("</{}>", name);
    let body = 2 + name_len;
    rest[body..].find(&close).map(|f| i + body + f + close.len())
}

/// re.search(r"<(\w+)>", text).start()
pub fn open_tag(text: &str) -> Option<usize> {
    let mut i = 0;
    while let Some(f) = text[i..].find('<') {
        let at = i + f;
        let rest = &text[at + 1..];
        let n: usize = rest.chars().take_while(|&c| is_py_word(c)).map(|c| c.len_utf8()).sum();
        if n > 0 && rest[n..].starts_with('>') {
            return Some(at);
        }
        i = at + 1;
    }
    None
}
