//! Go: `stego/watermark.go`, the text half: [`watermarks`] (invisible characters, variation selectors that are not emoji
//! styling, look-alike letters from other alphabets inside Latin words, trailing whitespace read as bits) with its
//! description, [`clean_text`], and [`scan_folder`] over a tree (testdata/parity/watermark.json). `CleanImage` waits for
//! Go's image encoders.

use super::clip_text;
use super::text::{bits_to_text, find_hidden, is_hidden, zero_width_name, HiddenText};
use crate::gotext::{self, decode_rune, is_print};

/// Go's `Selector`: the byte a variation selector carries, if it is one.
fn selector(r: char) -> Option<u8> {
    match r as u32 {
        c @ 0xFE00..=0xFE0F => Some((c - 0xFE00) as u8),
        c @ 0xE0100..=0xE01EF => Some((c - 0xE0100 + 16) as u8),
        _ => None,
    }
}

/// Go's `emojiStyle`: VS15/VS16 after a symbol, an emoji or an arrow.
fn emoji_style(prev: char, r: char) -> bool {
    let p = prev as u32;
    matches!(r as u32, 0xFE0E | 0xFE0F)
        && (gotext::is_symbol_other(prev) || gotext::is_symbol_math(prev) || p >= 0x1F000 || (0x2190..=0x2BFF).contains(&p))
}

/// The characters of Go's `for _, r := range s` over bytes: (byte offset, rune; U+FFFD for each byte that is not UTF-8).
fn runes(s: &[u8]) -> Vec<(usize, char)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let (c, n) = decode_rune(&s[i..]);
        out.push((i, c.unwrap_or('\u{FFFD}')));
        i += n.max(1);
    }
    out
}

/// Go's `printable`.
fn printable(s: &[u8]) -> bool {
    !s.is_empty() && runes(s).iter().all(|&(_, r)| is_print(r) || r == '\n' || r == '\t')
}

/// Go's `WatermarkReport`.
pub struct Report {
    pub hidden: HiddenText,
    pub selectors: usize,
    pub selector_text: Vec<u8>,
    /// (line, word, the other script's letters)
    pub homoglyphs: Vec<(usize, String, String)>,
    pub trailing_bits: String,
}

impl Report {
    /// Go's `Found`.
    pub fn found(&self) -> bool {
        self.hidden.count > 0
            || self.selectors > 0
            || !self.homoglyphs.is_empty()
            || !self.trailing_bits.is_empty()
            || self.hidden.trailing > 0
    }

    /// Go's `Describe`.
    pub fn describe(&self) -> Vec<u8> {
        if !self.found() {
            return b"No watermark found: no invisible characters (zero-width, tag, direction, variation selectors), no look-alike letters from other alphabets inside words, no lines ending in spaces or tabs.".to_vec();
        }
        let mut b = Vec::new();
        if self.hidden.count > 0 || self.hidden.trailing > 0 {
            b.extend(self.hidden.describe());
            b.push(b'\n');
        }
        if self.selectors > 0 {
            b.extend(format!("{} variation selectors that are not emoji styling: each can carry a byte.", self.selectors).bytes());
            if !self.selector_text.is_empty() {
                b.extend(b" Read as bytes they say: ");
                b.extend(clip_text(&self.selector_text, 500));
            }
            b.push(b'\n');
        }
        if !self.trailing_bits.is_empty() {
            b.extend(b"Trailing whitespace read as bits (space 0, tab 1) says: ");
            b.extend(clip_text(self.trailing_bits.as_bytes(), 500));
            b.push(b'\n');
        }
        for (i, (line, word, foreign)) in self.homoglyphs.iter().enumerate() {
            if i == 10 {
                b.extend(format!("... and {} more words with look-alike letters\n", self.homoglyphs.len() - 10).bytes());
                break;
            }
            b.extend(
                format!(
                    "Line {line}: {} mixes Latin letters with look-alikes from another alphabet ({foreign}): it looks like a normal word but is not the same text.\n",
                    gotext::quote(word)
                )
                .bytes(),
            );
        }
        while b.last() == Some(&b'\n') {
            b.pop();
        }
        b
    }
}

fn trim_right<'a>(mut s: &'a [u8], cut: &[u8]) -> &'a [u8] {
    while let Some((last, rest)) = s.split_last() {
        if !cut.contains(last) {
            break;
        }
        s = rest;
    }
    s
}

/// Go's `Watermarks`.
pub fn watermarks(s: &[u8]) -> Report {
    let mut w =
        Report { hidden: find_hidden(s), selectors: 0, selector_text: Vec::new(), homoglyphs: Vec::new(), trailing_bits: String::new() };
    let mut sel = Vec::new();
    let mut prev = '\0';
    for (_, r) in runes(s) {
        if let Some(v) = selector(r) {
            if !emoji_style(prev, r) {
                w.selectors += 1;
                sel.push(v);
            }
        }
        prev = r;
    }
    if sel.len() >= 2 && printable(&sel) {
        w.selector_text = sel;
    }
    let mut bits = String::new();
    for (i, line) in s.split(|&c| c == b'\n').enumerate() {
        let line = trim_right(line, b"\r");
        let trail = &line[trim_right(line, b" \t").len()..];
        bits.extend(trail.iter().map(|&c| if c == b' ' { '0' } else { '1' }));
        let rs = runes(line);
        let mut start = None;
        let mut words = Vec::new();
        for (k, &(off, r)) in rs.iter().enumerate() {
            let in_word = gotext::is_letter(r) || gotext::is_digit(r) || r == '_';
            match (in_word, start) {
                (true, None) => start = Some(off),
                (false, Some(st)) => {
                    words.push(&line[st..off]);
                    start = None;
                }
                _ => {}
            }
            if k == rs.len() - 1 {
                if let Some(st) = start {
                    words.push(&line[st..]);
                }
            }
        }
        for word in words {
            let (mut latin, mut foreign) = (false, Vec::new());
            for (_, r) in runes(word) {
                if gotext::is_latin(r) {
                    latin = true;
                } else if gotext::is_cyrillic_greek_armenian(r) || (0xFF01..=0xFF5E).contains(&(r as u32)) {
                    foreign.push(format!("U+{:04X} {r}", r as u32));
                }
            }
            if latin && !foreign.is_empty() && w.homoglyphs.len() < 50 {
                w.homoglyphs.push((i + 1, String::from_utf8_lossy(word).into_owned(), foreign.join(", ")));
            }
        }
    }
    if bits.len() >= 16 {
        if let Some(t) = bits_to_text(&bits, 8) {
            if printable(t.as_bytes()) {
                w.trailing_bits = t;
            }
        }
    }
    w
}

/// Go's `CleanText`: the text without the marks, and what was removed ("N kind", sorted).
pub fn clean_text(s: &[u8]) -> (Vec<u8>, Vec<String>) {
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut b = String::new();
    let mut prev = '\0';
    for (i, r) in runes(s) {
        if r == '\u{FEFF}' && i == 0 {
            b.push(r);
        } else if is_hidden(r) {
            *counts.entry(zero_width_name(r).unwrap_or("tag character")).or_default() += 1;
        } else if selector(r).is_some() && !emoji_style(prev, r) {
            *counts.entry("variation selector").or_default() += 1;
        } else {
            b.push(r);
        }
        prev = r;
    }
    let mut lines = Vec::new();
    for l in b.as_bytes().split(|&c| c == b'\n') {
        let cr = l.ends_with(b"\r");
        let body = l.strip_suffix(b"\r").unwrap_or(l);
        let mut t = trim_right(body, b" \t").to_vec();
        if t.len() != body.len() {
            *counts.entry("line with trailing spaces or tabs").or_default() += 1;
        }
        if cr {
            t.push(b'\r');
        }
        lines.push(t);
    }
    let mut removed: Vec<String> = counts.iter().map(|(k, n)| format!("{n} {k}")).collect();
    removed.sort();
    (lines.join(&b'\n'), removed)
}

/// Go's `ScanFolder`: the text files under dir (at most `limit`) checked for watermarks, and the pictures listed.
pub fn scan_folder(dir: &str, limit: usize) -> Vec<u8> {
    struct Scan {
        limit: usize,
        files: usize,
        marked: usize,
        pictures: Vec<String>,
        out: Vec<u8>,
    }
    /// Go's walk function for one entry: false to stop the whole walk (SkipAll).
    fn visit(sc: &mut Scan, path: &std::path::Path, rel: &str, meta: &std::fs::Metadata) -> bool {
        if sc.files >= sc.limit {
            return false;
        }
        if !meta.file_type().is_file() || meta.len() > 4 << 20 {
            return true;
        }
        let Ok(data) = std::fs::read(path) else { return true };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = name.rfind('.').map(|i| name[i..].to_lowercase()).unwrap_or_default();
        if matches!(ext.as_str(), ".png" | ".jpg" | ".jpeg" | ".gif" | ".webp" | ".bmp") {
            sc.pictures.push(rel.to_owned());
            return true;
        }
        if data.contains(&0) {
            return true;
        }
        sc.files += 1;
        let w = watermarks(&data);
        if w.found() {
            sc.marked += 1;
            sc.out.extend(format!("== {rel}\n").bytes());
            sc.out.extend(w.describe());
            sc.out.push(b'\n');
        }
        true
    }
    fn walk(sc: &mut Scan, path: &std::path::Path, rel: &str) -> bool {
        let Ok(mut entries) = std::fs::read_dir(path).map(|r| r.filter_map(Result::ok).collect::<Vec<_>>()) else { return true };
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel1 = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if matches!(name.as_str(), ".git" | "node_modules" | "target" | ".port_sessions" | "__pycache__") {
                    continue;
                }
                if !walk(sc, &e.path(), &rel1) {
                    return false;
                }
                continue;
            }
            let Ok(meta) = e.metadata() else {
                if sc.files >= sc.limit {
                    return false;
                }
                continue;
            };
            if !visit(sc, &e.path(), &rel1, &meta) {
                return false;
            }
        }
        true
    }
    let mut sc = Scan { limit, files: 0, marked: 0, pictures: Vec::new(), out: Vec::new() };
    let root = std::path::Path::new(dir);
    if let Ok(meta) = std::fs::symlink_metadata(root) {
        let base = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if meta.is_dir() {
            if !matches!(base.as_str(), ".git" | "node_modules" | "target" | ".port_sessions" | "__pycache__") {
                walk(&mut sc, root, "");
            }
        } else {
            visit(&mut sc, root, ".", &meta);
        }
    }
    let mut head = format!("Looked at {} text files under {dir}: {} with marks.", sc.files, sc.marked);
    if sc.marked == 0 {
        head += " None has invisible characters, look-alike letters or trailing whitespace.";
    }
    if !sc.pictures.is_empty() {
        head +=
            &format!("\nPictures there (look at each with action inspect: metadata, data after the end, LSB): {}", sc.pictures.join(", "));
    }
    let mut out = head.into_bytes();
    out.push(b'\n');
    out.extend(sc.out);
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}
