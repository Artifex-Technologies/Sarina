//! Go: `stego/wav.go` and the LSB judge of `lsb.go`: a PCM WAV's samples, the message their lowest bits may carry
//! (a file signature, a 32-bit length then text, or text), and how often the lowest bit is set.

use super::magic::detect;

/// The most samples read (Go's `MaxPixels`, shared with the images).
pub const MAX_PIXELS: usize = 40_000_000;

/// A PCM WAV (Go's `WAV`): interleaved samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wav {
    pub channels: i64,
    pub rate: i64,
    pub bits: i64,
    pub samples: Vec<i32>,
}

fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Go's `ParseWAV`: the RIFF chunks walked (a chunk running past the end is cut there), PCM of 8 to 32 bits read.
pub fn parse_wav(b: &[u8]) -> Result<Wav, String> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut w = Wav::default();
    let mut pos = 12usize;
    let mut data: Option<&[u8]> = None;
    let mut format = 0i64;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        // Go's int(uint32): no overflow on 64 bits
        let mut n = le32(&b[pos + 4..]) as usize;
        if pos + 8 + n > b.len() {
            n = b.len() - pos - 8;
        }
        let chunk = &b[pos + 8..pos + 8 + n];
        match id {
            b"fmt " if chunk.len() >= 16 => {
                format = le16(chunk) as i64;
                w.channels = le16(&chunk[2..]) as i64;
                w.rate = le32(&chunk[4..]) as i64;
                w.bits = le16(&chunk[14..]) as i64;
            }
            b"data" => data = Some(chunk),
            _ => {}
        }
        pos += 8 + n + (n & 1);
    }
    if format != 1 && format != 0xfffe {
        return Err(format!("WAV format {format} is compressed: I read only PCM"));
    }
    let Some(data) = data.filter(|_| w.bits != 0) else { return Err("no PCM data chunk".into()) };
    let bytes_per = w.bits / 8;
    if !(1..=4).contains(&bytes_per) {
        return Err(format!("{}-bit samples are not something I read", w.bits));
    }
    let bytes_per = bytes_per as usize;
    let count = (data.len() / bytes_per).min(MAX_PIXELS);
    w.samples = data
        .chunks(bytes_per)
        .take(count)
        .map(|s| match bytes_per {
            1 => s[0] as i32,
            2 => le16(s) as i16 as i32,
            3 => (s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16) as i32,
            _ => le32(s) as i32,
        })
        .collect();
    Ok(w)
}

impl Wav {
    /// Go's `LSBBytes`: the samples' lowest bits packed into bytes (first bit highest or lowest), at most `max`.
    pub fn lsb_bytes(&self, msb_first: bool, max: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let (mut cur, mut have) = (0u64, 0u64);
        for &s in &self.samples {
            let bit = (s as u32 as u64) & 1;
            if msb_first {
                cur = cur << 1 | bit;
            } else {
                cur |= bit << have;
            }
            have += 1;
            if have == 8 {
                out.push(cur as u8);
                cur = 0;
                have = 0;
                if out.len() >= max {
                    break;
                }
            }
        }
        out
    }

    /// Go's `LSBStats`: the share of samples whose lowest bit is set, and how many there are.
    pub fn lsb_stats(&self) -> (f64, usize) {
        if self.samples.is_empty() {
            return (0.0, 0);
        }
        let ones = self.samples.iter().filter(|&&s| s & 1 == 1).count();
        (ones as f64 / self.samples.len() as f64, self.samples.len())
    }
}

/// What the lowest bits held (Go's `Found`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub order: String,
    pub bits: i64,
    pub msb_first: bool,
    pub length: i64,
    pub why: String,
    pub data: Vec<u8>,
}

/// Go's `textLike`: four bytes or more, 95 % printable ASCII or whitespace.
pub fn text_like(b: &[u8]) -> bool {
    if b.len() < 4 {
        return false;
    }
    let ok = b.iter().filter(|&&c| (0x20..0x7f).contains(&c) || matches!(c, b'\n' | b'\r' | b'\t')).count();
    ok as f64 >= 0.95 * b.len() as f64
}

/// Go's `judge`: whether extracted bytes are a message (a file signature, a 32-bit length then text, or text up to four
/// zero bytes).
pub fn judge(data: &[u8]) -> Option<Found> {
    if data.len() < 8 {
        return None;
    }
    let k = detect(data);
    if !matches!(k.name, "unknown" | "text" | "script") {
        return Some(Found { why: format!("starts like a {} file", k.name), data: data.to_vec(), ..Found::default() });
    }
    for le in [false, true] {
        let n = if le {
            u32::from_le_bytes([data[0], data[1], data[2], data[3]])
        } else {
            u32::from_be_bytes([data[0], data[1], data[2], data[3]])
        };
        let n = n as usize;
        if n >= 4 && n <= data.len() - 4 && text_like(&data[4..4 + n]) {
            return Some(Found {
                length: n as i64,
                why: format!("a 32-bit length ({n}) followed by that much text"),
                data: data[4..4 + n].to_vec(),
                ..Found::default()
            });
        }
    }
    let end = data.windows(4).position(|w| w == [0, 0, 0, 0]).unwrap_or(data.len());
    if end >= 8 && text_like(&data[..end]) {
        return Some(Found { why: "reads as text".into(), data: data[..end].to_vec(), ..Found::default() });
    }
    None
}

/// Go's `SearchWAV`: the samples' lowest bits read first-bit-highest, then lowest, each judged.
pub fn search_wav(w: &Wav, max_bytes: usize) -> Vec<Found> {
    let mut found = Vec::new();
    for msb in [true, false] {
        if let Some(mut f) = judge(&w.lsb_bytes(msb, max_bytes)) {
            f.order = "sample LSB".into();
            f.msb_first = msb;
            f.bits = 1;
            found.push(f);
        }
    }
    found
}
