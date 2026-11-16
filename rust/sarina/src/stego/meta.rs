//! Go: `stego/meta.go`: what a container's own structure says (PNG chunks with their text, zlib-compressed through
//! [`inflate`], JPEG segments with Exif and XMP, GIF comments, RIFF chunks with LIST/INFO), where the format ends and
//! what follows, and the printable strings worth reading (testdata/parity/stegometa.json).

use super::clip_text;
use super::flate::zlib_reader;
use super::goio::{BytesReader, GoErr, GoRead};
use crate::gotext;
use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use std::sync::LazyLock;

/// Go's `Container`. Texts are bytes: Go formats chunk names and keywords as they are in the file.
#[derive(Debug, Default)]
pub struct Container {
    pub parts: Vec<Vec<u8>>,
    pub text: Vec<Vec<u8>>,
    pub unusual: Vec<Vec<u8>>,
    /// where the format says the file ends (-1: unknown)
    pub end: i64,
    pub problems: Vec<Vec<u8>>,
}

impl Container {
    fn new() -> Container {
        Container { end: -1, ..Container::default() }
    }

    /// Go's `Trailing`: what lies after End.
    pub fn trailing<'a>(&self, b: &'a [u8]) -> &'a [u8] {
        if self.end < 0 || self.end as usize >= b.len() {
            return &[];
        }
        &b[self.end as usize..]
    }
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

const PNG_STANDARD: [&[u8]; 29] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tEXt", b"zTXt", b"iTXt", b"gAMA", b"cHRM", b"sRGB", b"iCCP", b"bKGD", b"pHYs", b"tIME", b"tRNS",
    b"sBIT", b"hIST", b"sPLT", b"eXIf", b"acTL", b"fcTL", b"fdAT", b"oFFs", b"pCAL", b"sCAL", b"sTER", b"cICP", b"mDCv", b"cLLi",
];

fn cut(b: &[u8]) -> Option<(&[u8], &[u8])> {
    b.iter().position(|&c| c == 0).map(|i| (&b[..i], &b[i + 1..]))
}

/// Go's `ParsePNG`.
pub fn parse_png(b: &[u8]) -> Container {
    let mut c = Container::new();
    let mut pos = 8usize;
    let mut idat = 0;
    while pos + 8 <= b.len() {
        let n = be32(&b[pos..]) as usize;
        let typ = &b[pos + 4..pos + 8];
        if pos + 12 + n > b.len() {
            c.problems.push(cat(&[b"chunk ", typ, format!(" at {pos} claims {n} bytes, past the end of the file").as_bytes()]));
            break;
        }
        let data = &b[pos + 8..pos + 8 + n];
        if typ == b"IDAT" {
            idat += 1;
        } else {
            c.parts.push(cat(&[typ, format!(" ({n} bytes)").as_bytes()]));
        }
        if !PNG_STANDARD.contains(&typ) {
            let private = if typ[0].is_ascii_lowercase() { " (private chunk: not in the standard)" } else { "" };
            c.unusual.push(cat(&[typ, format!(" at offset {pos}, {n} bytes{private}").as_bytes()]));
        }
        match typ {
            b"tEXt" => {
                if let Some((k, v)) = cut(data) {
                    c.text.push(cat(&[k, b": ", &clip_text(v, 400)]));
                }
            }
            b"zTXt" => {
                if let Some((k, rest)) = cut(data) {
                    if rest.len() > 1 {
                        if let Some(v) = inflate(&rest[1..]) {
                            c.text.push(cat(&[k, b" (compressed): ", &clip_text(&v, 400)]));
                        }
                    }
                }
            }
            b"iTXt" => {
                if let Some((k, rest)) = cut(data) {
                    if rest.len() > 2 {
                        let compressed = rest[0] == 1;
                        let mut rest = &rest[2..];
                        for _ in 0..2 {
                            // the language, then the translated keyword
                            rest = cut(rest).map_or(&[][..], |(_, r)| r);
                        }
                        let mut v = rest.to_vec();
                        if compressed {
                            if let Some(d) = inflate(rest) {
                                v = d;
                            }
                        }
                        c.text.push(cat(&[k, b": ", &clip_text(&v, 400)]));
                    }
                }
            }
            b"IHDR" if n >= 13 => {
                let last = c.parts.len() - 1;
                c.parts[last] =
                    format!("IHDR {}x{}, bit depth {}, colour type {}", be32(data), be32(&data[4..]), data[8], data[9]).into_bytes();
            }
            b"eXIf" => c.text.extend(exif_text(data)),
            _ => {}
        }
        pos += 12 + n;
        if typ == b"IEND" {
            c.end = pos as i64;
            break;
        }
    }
    if idat > 0 {
        c.parts.push(format!("IDAT x{idat} (the picture)").into_bytes());
    }
    if c.end < 0 && c.problems.is_empty() {
        c.problems.push(b"no IEND chunk: the file is cut short".to_vec());
    }
    c
}

/// What inflate keeps (Go's `io.LimitReader(r, 1<<20)`).
const INFLATE_LIMIT: usize = 1 << 20;

/// Go's `inflate`: `io.ReadAll(io.LimitReader(zlib.NewReader(bytes.NewReader(b)), 1<<20))`, None on an error. The first
/// MiB once the stream gives that much: 1 MiB is a whole number of flate's 32 KiB windows, so Go's reader gets it before
/// flate decodes on, and never sees an error or a checksum that follows.
pub fn inflate(b: &[u8]) -> Option<Vec<u8>> {
    let mut z = zlib_reader(BytesReader::new(b)).ok()?;
    let mut out = vec![0u8; INFLATE_LIMIT];
    let mut have = 0;
    while have < INFLATE_LIMIT {
        let (n, err) = z.read(&mut out[have..]);
        have += n;
        match err {
            None => {}
            Some(GoErr::Eof) => break,
            Some(_) => return None,
        }
    }
    out.truncate(have);
    Some(out)
}

fn jpeg_app(m: u8) -> Option<&'static str> {
    Some(match m {
        0xe0 => "APP0 (JFIF)",
        0xe1 => "APP1 (Exif or XMP)",
        0xe2 => "APP2 (ICC profile)",
        0xed => "APP13 (Photoshop IPTC)",
        0xee => "APP14 (Adobe)",
        0xdb => "DQT (quantisation)",
        0xc4 => "DHT (Huffman)",
        0xc0 => "SOF0 (baseline)",
        0xc2 => "SOF2 (progressive)",
        0xdd => "DRI",
        0xfe => "COM (comment)",
        0xda => "SOS (image data)",
        _ => return None,
    })
}

/// Go's `ParseJPEG`.
pub fn parse_jpeg(b: &[u8]) -> Container {
    let mut c = Container::new();
    let mut pos = 2usize;
    while pos + 4 <= b.len() {
        if b[pos] != 0xff {
            c.problems.push(format!("expected a marker at {pos}").into_bytes());
            break;
        }
        let m = b[pos + 1];
        if m == 0xd8 || (0xd0..=0xd7).contains(&m) || m == 0x01 || m == 0xff {
            pos += 2;
            continue;
        }
        let n = usize::from(be16(&b[pos + 2..]));
        if n < 2 {
            c.problems.push(format!("segment {m:02x} at {pos} claims {n} bytes, under its own 2 bytes").into_bytes());
            break;
        }
        if pos + 2 + n > b.len() {
            c.problems.push(format!("segment {m:02x} at {pos} runs past the end").into_bytes());
            break;
        }
        let data = &b[pos + 4..pos + 2 + n];
        let mut name = jpeg_app(m).map_or_else(|| format!("marker FF{m:02X}"), str::to_owned);
        match m {
            0xfe => c.text.push(cat(&[b"comment: ", &clip_text(data, 400)])),
            0xe1 if data.starts_with(b"Exif\x00\x00") => {
                name = "APP1 (Exif)".into();
                c.text.extend(exif_text(&data[6..]));
            }
            0xe1 if data.starts_with(b"http://ns.adobe.com/xap/1.0/") => {
                name = "APP1 (XMP)".into();
                c.text.extend(xmp_text(data));
            }
            0xc0..=0xc2 if data.len() >= 6 => name += &format!(" {}x{}", be16(&data[3..]), be16(&data[1..])),
            _ => {}
        }
        if (0xe3..=0xef).contains(&m) && m != 0xed && m != 0xee {
            c.unusual.push(format!("APP{} segment at {pos}, {n} bytes: not one of the usual ones", m - 0xe0).into_bytes());
        }
        c.parts.push(format!("{name} ({n} bytes)").into_bytes());
        pos += 2 + n;
        if m == 0xda {
            match b[pos..].windows(2).position(|w| w == [0xff, 0xd9]) {
                Some(i) => c.end = (pos + i + 2) as i64,
                None => c.problems.push(b"no end-of-image marker: the file is cut short".to_vec()),
            }
            break;
        }
    }
    c
}

fn exif_name(tag: u16) -> Option<&'static str> {
    Some(match tag {
        0x010e => "description",
        0x010f => "make",
        0x0110 => "model",
        0x0131 => "software",
        0x0132 => "modified",
        0x013b => "artist",
        0x8298 => "copyright",
        0x9003 => "taken",
        0x9286 => "user comment",
        0xa420 => "image id",
        0x9c9b => "XP title",
        0x9c9c => "XP comment",
        0x9c9e => "XP keywords",
        _ => return None,
    })
}

/// Go's `exifText`: the ASCII tags people care about from a TIFF/Exif block, and whether GPS tags are there.
pub fn exif_text(t: &[u8]) -> Vec<Vec<u8>> {
    if t.len() < 8 {
        return Vec::new();
    }
    let little = match &t[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Vec::new(),
    };
    let u16_at = |i: usize| if little { le16(&t[i..]) } else { be16(&t[i..]) };
    let u32_at = |i: usize| if little { le32(&t[i..]) } else { be32(&t[i..]) };
    let mut out = Vec::new();
    let mut seen = Vec::new();
    fn walk(
        t: &[u8],
        off: u32,
        depth: u32,
        seen: &mut Vec<u32>,
        out: &mut Vec<Vec<u8>>,
        u16_at: &dyn Fn(usize) -> u16,
        u32_at: &dyn Fn(usize) -> u32,
    ) {
        if depth > 3 || seen.contains(&off) || off as usize + 2 > t.len() {
            return;
        }
        seen.push(off);
        let n = usize::from(u16_at(off as usize));
        for i in 0..n.min(200) {
            let e = off as usize + 2 + i * 12;
            if e + 12 > t.len() {
                return;
            }
            let (tag, typ, count) = (u16_at(e), u16_at(e + 2), u32_at(e + 4));
            let unit: u32 = match typ {
                1 | 2 | 7 => 1,
                3 => 2,
                4 | 9 => 4,
                5 | 10 => 8,
                _ => 0,
            };
            let size = unit.wrapping_mul(count);
            let data: &[u8] = if size <= 4 {
                &t[e + 8..e + 8 + size as usize]
            } else {
                let voff = u32_at(e + 8);
                if voff as usize + size as usize <= t.len() {
                    &t[voff as usize..voff as usize + size as usize]
                } else {
                    &[]
                }
            };
            if tag == 0x8769 || tag == 0xa005 {
                walk(t, u32_at(e + 8), depth + 1, seen, out, u16_at, u32_at);
            } else if tag == 0x8825 {
                out.push(b"GPS: the picture carries GPS tags (a location)".to_vec());
            } else if let (Some(name), 1 | 2 | 7) = (exif_name(tag), typ) {
                let mut v = data.to_vec();
                if tag == 0x9286 && v.len() > 8 {
                    v = v[8..].to_vec();
                }
                if tag >= 0x9c9b {
                    // UTF-16LE: the ASCII characters
                    v = v.chunks_exact(2).filter(|p| p[1] == 0 && p[0] != 0).map(|p| p[0]).collect();
                }
                while v.last() == Some(&0) {
                    v.pop();
                }
                let s = clip_text(&v, 300);
                if !s.is_empty() {
                    out.push(cat(&[name.as_bytes(), b": ", &s]));
                }
            }
        }
    }
    walk(t, u32_at(4), 0, &mut seen, &mut out, &u16_at, &u32_at);
    out
}

static XMP_TAGS: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"(?s-u)<(dc:description|dc:creator|dc:subject|dc:title|xmp:CreatorTool|photoshop:Instructions|dc:rights)[^>]*>(.*?)</")
        .expect("xmp pattern")
});
static XML_TAG: LazyLock<BytesRegex> = LazyLock::new(|| BytesRegex::new(r"(?-u)<[^>]+>").expect("tag pattern"));

/// Go's `xmpText`.
fn xmp_text(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for m in XMP_TAGS.captures_iter(b).take(8) {
        let v = XML_TAG.replace_all(&m[2], &b" "[..]);
        let s = clip_text(&gotext::fields_bytes(&v).join(&b' '), 300);
        if !s.is_empty() {
            out.push(cat(&[b"XMP ", &m[1], b": ", &s]));
        }
    }
    if out.is_empty() {
        out.push(format!("XMP block of {} bytes (no description tags)", b.len()).into_bytes());
    }
    out
}

/// Go's `ParseGIF`.
pub fn parse_gif(b: &[u8]) -> Container {
    let mut c = Container::new();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == 0x21 && b[i + 1] == 0xfe {
            let mut text = Vec::new();
            let mut j = i + 2;
            while j < b.len() && b[j] != 0 {
                let n = usize::from(b[j]);
                if j + 1 + n > b.len() {
                    break;
                }
                text.extend_from_slice(&b[j + 1..j + 1 + n]);
                j += 1 + n;
            }
            c.text.push(cat(&[b"comment: ", &clip_text(&text, 400)]));
        }
        i += 1;
    }
    if let Some(i) = b.iter().rposition(|&x| x == 0x3b) {
        c.end = i as i64 + 1;
    }
    c.parts.push(cat(&[b"GIF ", &b[..6]]));
    c
}

const RIFF_KNOWN: [&[u8]; 18] = [
    b"fmt ", b"data", b"LIST", b"fact", b"VP8 ", b"VP8L", b"VP8X", b"ALPH", b"ANIM", b"ANMF", b"EXIF", b"XMP ", b"ICCP", b"cue ", b"smpl",
    b"bext", b"id3 ", b"PEAK",
];

/// Go's `ParseRIFF`.
pub fn parse_riff(b: &[u8]) -> Container {
    let mut c = Container::new();
    if b.len() < 12 {
        return c;
    }
    c.end = 8 + i64::from(le32(&b[4..]));
    if c.end > b.len() as i64 {
        c.problems.push(format!("the RIFF header claims {} bytes but the file has {}", c.end, b.len()).into_bytes());
        c.end = -1;
    }
    let mut pos = 12usize;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let n = le32(&b[pos + 4..]) as usize;
        if pos + 8 + n > b.len() {
            break;
        }
        let data = &b[pos + 8..pos + 8 + n];
        if id == b"LIST" && data.len() >= 4 && &data[..4] == b"INFO" {
            let mut p = 4;
            while p + 8 <= data.len() {
                let k = &data[p..p + 4];
                let m = le32(&data[p + 4..]) as usize;
                if p + 8 + m > data.len() {
                    break;
                }
                let mut v = &data[p + 8..p + 8 + m];
                while let [rest @ .., 0] = v {
                    v = rest;
                }
                c.text.push(cat(&[k, b": ", &clip_text(v, 300)]));
                p += 8 + m + (m & 1);
            }
        }
        if id == b"fmt " && data.len() >= 16 {
            c.parts.push(
                format!("fmt: format {}, {} channel(s), {} Hz, {} bits", le16(data), le16(&data[2..]), le32(&data[4..]), le16(&data[14..]))
                    .into_bytes(),
            );
        } else {
            c.parts.push(cat(&[gotext::trim_space_bytes(id), format!(" ({n} bytes)").as_bytes()]));
        }
        if !RIFF_KNOWN.contains(&id) && id != b"JUNK" {
            c.unusual.push(format!("chunk {} at {pos}, {n} bytes", gotext::quote_bytes(id)).into_bytes());
        }
        pos += 8 + n + (n & 1);
    }
    c
}

static INTERESTING: LazyLock<Regex> = LazyLock::new(|| {
    crate::goregexp::compile(r"(?i)(flag\{|ctf\{|https?://|www\.|@[a-z0-9-]+\.[a-z]{2,}|passw|secret|\bkey\b|token|-----BEGIN|\.(zip|pdf|exe|sh|py)\b|base64|hidden|steg)")
        .expect("interesting pattern")
});
static BASE64ISH: LazyLock<Regex> = LazyLock::new(|| crate::goregexp::compile(r"^[A-Za-z0-9+/]{24,}={0,2}$").expect("base64 pattern"));

/// Go's `Strings`: the printable ASCII runs of at least `min` bytes worth reading (at most `max`), and how many runs
/// there were.
pub fn strings(b: &[u8], min: usize, max: usize) -> (Vec<Vec<u8>>, usize) {
    let mut worth = Vec::new();
    let mut total = 0;
    let mut flush = |cur: &[u8]| {
        if cur.len() >= min {
            total += 1;
            // only printable ASCII and tabs: always UTF-8
            let s = std::str::from_utf8(cur).unwrap_or_default();
            if worth.len() < max && (INTERESTING.is_match(s) || BASE64ISH.is_match(s)) {
                worth.push(clip_text(cur, 200));
            }
        }
    };
    let mut start = 0;
    for (i, &c) in b.iter().enumerate() {
        if !((0x20..0x7f).contains(&c) || c == b'\t') {
            flush(&b[start..i]);
            start = i + 1;
        }
    }
    flush(&b[start..]);
    (worth, total)
}
