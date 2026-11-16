//! Go: `stego/text.go`: invisible characters in text (zero-width, tag characters, direction marks), what they may spell
//! (tag characters as ASCII, two kinds as bits), and lines ending in spaces or tabs.

use super::clip_text;
use std::collections::BTreeMap;

pub(crate) fn zero_width_name(r: char) -> Option<&'static str> {
    Some(match r as u32 {
        0x200B => "zero-width space",
        0x200C => "zero-width non-joiner",
        0x200D => "zero-width joiner",
        0x2060 => "word joiner",
        0x2061 => "function application",
        0x2062 => "invisible times",
        0x2063 => "invisible separator",
        0x2064 => "invisible plus",
        0xFEFF => "zero-width no-break space (BOM)",
        0x00AD => "soft hyphen",
        0x180E => "Mongolian vowel separator",
        0x200E => "left-to-right mark",
        0x200F => "right-to-left mark",
        0x202A => "LRE",
        0x202B => "RLE",
        0x202C => "PDF",
        0x202D => "LRO",
        0x202E => "right-to-left override",
        0x2066 => "LRI",
        0x2067 => "RLI",
        0x2068 => "FSI",
        0x2069 => "PDI",
        0x034F => "combining grapheme joiner",
        0x115F => "Hangul filler",
        0x1160 => "Hangul jungseong filler",
        0x3164 => "Hangul filler (wide)",
        0xFFA0 => "halfwidth Hangul filler",
        _ => return None,
    })
}

pub(crate) fn is_hidden(r: char) -> bool {
    zero_width_name(r).is_some() || (0xE0000..=0xE007F).contains(&(r as u32))
}

/// What a text hides (Go's `HiddenText`); the decoded lines are Go's bytes (a cut can fall inside a character).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HiddenText {
    pub count: usize,
    pub kinds: BTreeMap<char, usize>,
    pub decoded: Vec<Vec<u8>>,
    pub trailing: usize,
}

/// Go's `FindHidden`.
pub fn find_hidden(s: &[u8]) -> HiddenText {
    let mut h = HiddenText::default();
    let hidden: Vec<char> = crate::gotext::lossy(s).chars().filter(|&r| is_hidden(r)).collect();
    for &r in &hidden {
        h.count += 1;
        *h.kinds.entry(r).or_default() += 1;
    }
    for line in s.split(|&c| c == b'\n') {
        let mut t = line;
        while let Some(rest) = t.strip_suffix(b"\r") {
            t = rest;
        }
        let mut u = t;
        while let Some((&last, rest)) = u.split_last() {
            if last == b' ' || last == b'\t' {
                u = rest;
            } else {
                break;
            }
        }
        if t != u {
            h.trailing += 1;
        }
    }
    if h.count == 0 {
        return h;
    }
    let tag: String = hidden
        .iter()
        .filter(|&&r| (0xE0020..=0xE007E).contains(&(r as u32)))
        .map(|&r| char::from_u32(r as u32 - 0xE0000).unwrap_or('?'))
        .collect();
    if tag.len() >= 2 {
        let mut line = b"tag characters (U+E00xx) read as ASCII: ".to_vec();
        line.extend_from_slice(&clip_text(tag.as_bytes(), 500));
        h.decoded.push(line);
    }
    let mut kinds: Vec<(char, usize)> = h.kinds.iter().filter(|(r, _)| (**r as u32) < 0xE0000).map(|(r, n)| (*r, *n)).collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if kinds.len() >= 2 {
        let (a, b) = (kinds[0].0, kinds[1].0);
        let sep = kinds.get(2).map(|k| k.0);
        for assign in [(a, b), (b, a)] {
            let bits: String = hidden
                .iter()
                .filter_map(|&r| {
                    if r == assign.0 {
                        Some('0')
                    } else if r == assign.1 {
                        Some('1')
                    } else if Some(r) == sep {
                        Some(' ')
                    } else {
                        None
                    }
                })
                .collect();
            for width in [8, 7, 16] {
                if let Some(t) = bits_to_text(&bits, width) {
                    let mut line =
                        format!("U+{:04X} as 0 and U+{:04X} as 1, {width} bits per character: ", assign.0 as u32, assign.1 as u32)
                            .into_bytes();
                    line.extend_from_slice(&clip_text(t.as_bytes(), 500));
                    h.decoded.push(line);
                    break;
                }
            }
            if h.decoded.last().is_some_and(|d| d.windows(8).any(|w| w == b"as 0 and")) {
                break;
            }
        }
    }
    h
}

/// Go's `bitsToText`: the bits (groups split by spaces, else `width` at a time) as characters, all printable or a
/// newline or tab and in the BMP; None otherwise or for fewer than two.
pub(crate) fn bits_to_text(bits: &str, width: usize) -> Option<String> {
    let groups: Vec<&str> = if bits.contains(' ') {
        bits.split_whitespace().collect()
    } else {
        (0..bits.len() / width).map(|i| &bits[i * width..i * width + width]).collect()
    };
    if groups.len() < 2 {
        return None;
    }
    let mut out = String::new();
    for g in groups {
        let v = g.bytes().fold(0i64, |v, c| v.wrapping_shl(1) | (c - b'0') as i64);
        let r = v as i32;
        let c = char::from_u32(r as u32).filter(|&c| r >= 0 && (crate::gotext::is_print(c) || c == '\n' || c == '\t') && r <= 0xFFFF)?;
        out.push(c);
    }
    Some(out)
}

impl HiddenText {
    /// Go's `HiddenText.Describe`.
    pub fn describe(&self) -> Vec<u8> {
        if self.count == 0 && self.trailing == 0 {
            return b"No invisible characters (zero-width, tag or direction marks) and no lines ending in spaces or tabs.".to_vec();
        }
        let mut b: Vec<u8> = Vec::new();
        if self.count > 0 {
            b.extend_from_slice(format!("{} invisible characters:", self.count).as_bytes());
            for (r, n) in &self.kinds {
                let name = zero_width_name(*r).unwrap_or("tag character");
                b.extend_from_slice(format!(" U+{:04X} {name} x{n};", *r as u32).as_bytes());
            }
            b.push(b'\n');
            if self.decoded.is_empty() {
                b.extend_from_slice(b"They do not decode as ASCII by the usual schemes (tag characters, or two kinds as bits). A single kind repeated is sometimes just copy-paste residue; many kinds in a pattern is a hidden message in a scheme I do not know.\n");
            }
            for d in &self.decoded {
                b.extend_from_slice(b"Hidden text, ");
                b.extend_from_slice(d);
                b.push(b'\n');
            }
        }
        if self.trailing > 0 {
            b.extend_from_slice(format!("{} line(s) end in spaces or tabs: trailing whitespace can carry data (the snow method); editors also leave it by accident.\n", self.trailing).as_bytes());
        }
        while b.last() == Some(&b'\n') {
            b.pop();
        }
        b
    }
}
