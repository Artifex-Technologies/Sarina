//! Go: `stego/meta.go`: what a container's own structure says (PNG chunks with their text, zlib-compressed through
//! [`inflate`], JPEG segments with Exif and XMP, GIF comments, RIFF chunks with LIST/INFO), where the format ends and
//! what follows, and the printable strings worth reading (testdata/parity/stegometa.json).

use super::clip_text;
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

/// Bits from the input as Go's flate reads them: whole bytes, only when a step needs more bits.
struct Bits<'a> {
    input: &'a [u8],
    pos: usize,
    b: u32,
    nb: u32,
}

impl Bits<'_> {
    /// Have at least n bits buffered, or the input ran out.
    fn need(&mut self, n: u32) -> Result<(), ()> {
        while self.nb < n {
            let x = *self.input.get(self.pos).ok_or(())?;
            self.pos += 1;
            self.b |= u32::from(x) << self.nb;
            self.nb += 8;
        }
        Ok(())
    }

    fn take(&mut self, n: u32) -> Result<u32, ()> {
        self.need(n)?;
        let v = self.b & ((1u32 << n) - 1);
        self.b = if n == 32 { 0 } else { self.b >> n };
        self.nb -= n;
        Ok(v)
    }
}

/// A canonical Huffman code as Go's `huffmanDecoder` accepts it.
struct Huff {
    /// number of codes of each length
    count: [u16; 16],
    /// symbols in canonical order
    symbols: Vec<u16>,
    /// the fewest bits Go reads before looking a symbol up
    min: u32,
}

impl Huff {
    /// Go's `huffmanDecoder.init`: None for an over-subscribed or incomplete code (a single code of length 1 is allowed;
    /// an empty one is allowed and fails when used).
    fn new(lengths: &[u8]) -> Option<Huff> {
        let mut count = [0u16; 16];
        let (mut min, mut max) = (0u32, 0u32);
        for &n in lengths {
            if n == 0 {
                continue;
            }
            if min == 0 || u32::from(n) < min {
                min = u32::from(n);
            }
            max = max.max(u32::from(n));
            count[usize::from(n)] += 1;
        }
        if max != 0 {
            let mut code = 0u32;
            for i in min..=max {
                code <<= 1;
                code += u32::from(count[i as usize]);
            }
            if code != 1 << max && !(code == 1 && max == 1) {
                return None;
            }
        }
        let mut symbols = Vec::new();
        for len in 1..16u8 {
            for (s, &n) in lengths.iter().enumerate() {
                if n == len {
                    symbols.push(s as u16);
                }
            }
        }
        Some(Huff { count, symbols, min })
    }

    /// Go's `huffSym`: at least `min` bits are read first; a code not in the table is corrupt input.
    fn decode(&self, bits: &mut Bits) -> Result<u16, ()> {
        bits.need(self.min)?;
        if self.min == 0 {
            return Err(());
        }
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(())
    }
}

/// Go's `inflate`: a zlib stream (compress/zlib, compress/flate) read through a 1 MiB limit with io.ReadAll: the
/// first MiB when the stream gives more before any error, else only a whole stream with the right checksum.
pub fn inflate(b: &[u8]) -> Option<Vec<u8>> {
    inflate_stream(b).ok()
}

fn inflate_stream(b: &[u8]) -> Result<Vec<u8>, ()> {
    if b.len() < 2 || b[0] & 0x0f != 8 || b[0] >> 4 > 7 || !be16(b).is_multiple_of(31) {
        return Err(());
    }
    let mut pos = 2;
    if b[1] & 0x20 != 0 {
        // a preset dictionary: none is given, so only the empty one's checksum (1) passes
        if b.len() < 6 || be32(&b[2..]) != 1 {
            return Err(());
        }
        pos = 6;
    }
    let mut bits = Bits { input: b, pos, b: 0, nb: 0 };
    let mut out: Vec<u8> = Vec::new();
    // done as soon as the limit is out: 1 MiB is a whole number of flate's 32 KiB windows, so Go's reader gets that much
    // before flate decodes on, and never sees an error or a checksum that follows
    let full = |out: &Vec<u8>| out.len() >= INFLATE_LIMIT;
    loop {
        let header = bits.take(3)?;
        let final_block = header & 1 == 1;
        match header >> 1 {
            0 => {
                bits.b = 0;
                bits.nb = 0;
                let p = bits.pos;
                if p + 4 > b.len() {
                    return Err(());
                }
                let (n, nn) = (le16(&b[p..]), le16(&b[p + 2..]));
                if n != !nn {
                    return Err(());
                }
                bits.pos += 4;
                for _ in 0..n {
                    let x = *b.get(bits.pos).ok_or(())?;
                    bits.pos += 1;
                    out.push(x);
                    if full(&out) {
                        out.truncate(INFLATE_LIMIT);
                        return Ok(out);
                    }
                }
            }
            t @ (1 | 2) => {
                let (lit, dist) = if t == 1 {
                    let mut l = [8u8; 288];
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    (Huff::new(&l).ok_or(())?, None)
                } else {
                    let nlit = bits.take(5)? as usize + 257;
                    if nlit > 286 {
                        return Err(());
                    }
                    let ndist = bits.take(5)? as usize + 1;
                    if ndist > 30 {
                        return Err(());
                    }
                    let nclen = bits.take(4)? as usize + 4;
                    const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                    let mut cl = [0u8; 19];
                    for &i in ORDER.iter().take(nclen) {
                        cl[i] = bits.take(3)? as u8;
                    }
                    let h1 = Huff::new(&cl).ok_or(())?;
                    let mut lens = vec![0u8; nlit + ndist];
                    let mut i = 0;
                    while i < lens.len() {
                        let x = h1.decode(&mut bits)?;
                        if x < 16 {
                            lens[i] = x as u8;
                            i += 1;
                            continue;
                        }
                        let (rep, nb, v) = match x {
                            16 => {
                                if i == 0 {
                                    return Err(());
                                }
                                (3, 2, lens[i - 1])
                            }
                            17 => (3, 3, 0),
                            _ => (11, 7, 0),
                        };
                        let rep = rep + bits.take(nb)? as usize;
                        if i + rep > lens.len() {
                            return Err(());
                        }
                        lens[i..i + rep].fill(v);
                        i += rep;
                    }
                    let mut lit = Huff::new(&lens[..nlit]).ok_or(())?;
                    let dist = Huff::new(&lens[nlit..]).ok_or(())?;
                    // Go reads at least the end-of-block code's length for a literal or length
                    lit.min = lit.min.max(u32::from(lens[256]));
                    (lit, Some(dist))
                };
                loop {
                    let v = lit.decode(&mut bits)?;
                    let (length, extra) = match v {
                        0..=255 => {
                            out.push(v as u8);
                            if full(&out) {
                                out.truncate(INFLATE_LIMIT);
                                return Ok(out);
                            }
                            continue;
                        }
                        256 => break,
                        257..=264 => (v as usize - 254, 0),
                        265..=268 => (v as usize * 2 - (265 * 2 - 11), 1),
                        269..=272 => (v as usize * 4 - (269 * 4 - 19), 2),
                        273..=276 => (v as usize * 8 - (273 * 8 - 35), 3),
                        277..=280 => (v as usize * 16 - (277 * 16 - 67), 4),
                        281..=284 => (v as usize * 32 - (281 * 32 - 131), 5),
                        285 => (258, 0),
                        _ => return Err(()),
                    };
                    let length = length + bits.take(extra)? as usize;
                    let d = match &dist {
                        None => (bits.take(5)? as u8).reverse_bits() >> 3,
                        Some(h) => h.decode(&mut bits)? as u8,
                    };
                    let d = match d {
                        0..=3 => usize::from(d) + 1,
                        4..=29 => {
                            let nb = u32::from(d - 2) >> 1;
                            let extra = (u32::from(d) & 1) << nb | bits.take(nb)?;
                            (1usize << (nb + 1)) + 1 + extra as usize
                        }
                        _ => return Err(()),
                    };
                    if d > out.len().min(32768) {
                        return Err(());
                    }
                    for _ in 0..length {
                        out.push(out[out.len() - d]);
                        if full(&out) {
                            out.truncate(INFLATE_LIMIT);
                            return Ok(out);
                        }
                    }
                }
            }
            _ => return Err(()),
        }
        if final_block {
            break;
        }
    }
    // the Adler-32 after the last byte the deflate data touched
    let p = bits.pos;
    if p + 4 > b.len() {
        return Err(());
    }
    let (mut a, mut s) = (1u32, 0u32);
    for &x in &out {
        a = (a + u32::from(x)) % 65521;
        s = (s + a) % 65521;
    }
    if be32(&b[p..]) != (s << 16 | a) {
        return Err(());
    }
    Ok(out)
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
