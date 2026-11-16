//! Go's `image/gif` decoder for the first frame (what `image.Decode` returns) with its LZW reader (`compress/lzw`), its
//! sub-block reader and every error text.

use super::goio::{read_full, Bufio, BytesReader, GoByteRead, GoErr};
use super::images::{Img, Kind, Pal};

type R<'b> = Bufio<BytesReader<'b>>;

fn full(r: &mut R, n: usize) -> Result<Vec<u8>, GoErr> {
    let mut b = vec![0u8; n];
    match read_full(r, &mut b) {
        (_, None) => Ok(b),
        (_, Some(GoErr::Eof)) => Err(GoErr::UnexpectedEof),
        (_, Some(e)) => Err(e),
    }
}

fn byte(r: &mut R) -> Result<u8, GoErr> {
    r.read_byte().map_err(|e| if e == GoErr::Eof { GoErr::UnexpectedEof } else { e })
}

fn msg(m: String) -> GoErr {
    GoErr::Msg(m)
}

/// Go's `blockReader`: the bytes of the data sub-blocks.
struct Blocks<'a, 'b> {
    r: &'a mut R<'b>,
    buf: Vec<u8>,
    i: usize,
    err: Option<GoErr>,
}

impl Blocks<'_, '_> {
    fn fill(&mut self) {
        if self.err.is_some() {
            return;
        }
        let n = match byte(self.r) {
            Ok(0) => {
                self.err = Some(GoErr::Eof);
                self.buf.clear();
                return;
            }
            Ok(n) => n,
            Err(e) => {
                self.err = Some(e);
                self.buf.clear();
                return;
            }
        };
        self.i = 0;
        match full(self.r, usize::from(n)) {
            Ok(b) => self.buf = b,
            Err(e) => {
                self.err = Some(e);
                self.buf.clear();
            }
        }
    }

    fn read_byte(&mut self) -> Result<u8, GoErr> {
        if self.i >= self.buf.len() {
            self.fill();
            if let Some(e) = &self.err {
                return Err(e.clone());
            }
        }
        let c = self.buf[self.i];
        self.i += 1;
        Ok(c)
    }

    /// Go's `close`: what follows the LZW data in the sub-blocks.
    fn close(&mut self) -> Result<(), GoErr> {
        match &self.err {
            Some(GoErr::Eof) => return Ok(()),
            Some(e) => return Err(e.clone()),
            None => {}
        }
        if self.i >= self.buf.len() {
            self.fill();
            match &self.err {
                Some(GoErr::Eof) => return Ok(()),
                Some(e) => return Err(e.clone()),
                None if self.buf.len() > 1 => return Err(too_much()),
                None => {}
            }
        }
        self.fill();
        match &self.err {
            Some(GoErr::Eof) => Ok(()),
            Some(e) => Err(e.clone()),
            None => Err(too_much()),
        }
    }
}

fn too_much() -> GoErr {
    msg("gif: too much image data".into())
}

const INVALID: u16 = 0xffff;

/// Go's `lzw.Reader` (LSB order), decoding a code at a time.
struct Lzw {
    bits: u32,
    n_bits: u32,
    width: u32,
    lit_width: u32,
    clear: u16,
    eof: u16,
    hi: u16,
    overflow: u16,
    last: u16,
    suffix: Vec<u8>,
    prefix: Vec<u16>,
    pending: Vec<u8>,
    err: Option<GoErr>,
}

impl Lzw {
    fn new(lit: u32) -> Lzw {
        let clear = 1u16 << lit;
        Lzw {
            bits: 0,
            n_bits: 0,
            width: 1 + lit,
            lit_width: lit,
            clear,
            eof: clear + 1,
            hi: clear + 1,
            overflow: 1 << (1 + lit),
            last: INVALID,
            suffix: vec![0; 1 << 12],
            prefix: vec![0; 1 << 12],
            pending: Vec::new(),
            err: None,
        }
    }

    /// Decodes until some output is pending or an error (Go's io.EOF at the end code) is set.
    fn step(&mut self, src: &mut Blocks) {
        while self.pending.is_empty() && self.err.is_none() {
            while self.n_bits < self.width {
                match src.read_byte() {
                    Ok(x) => {
                        self.bits |= u32::from(x) << self.n_bits;
                        self.n_bits += 8;
                    }
                    Err(e) => {
                        self.err = Some(if e == GoErr::Eof { GoErr::UnexpectedEof } else { e });
                        return;
                    }
                }
            }
            let code = (self.bits & ((1 << self.width) - 1)) as u16;
            self.bits >>= self.width;
            self.n_bits -= self.width;
            if code < self.clear {
                self.pending.push(code as u8);
                if self.last != INVALID {
                    self.suffix[usize::from(self.hi)] = code as u8;
                    self.prefix[usize::from(self.hi)] = self.last;
                }
            } else if code == self.clear {
                self.width = 1 + self.lit_width;
                self.hi = self.eof;
                self.overflow = 1 << self.width;
                self.last = INVALID;
                continue;
            } else if code == self.eof {
                self.err = Some(GoErr::Eof);
                return;
            } else if code <= self.hi {
                let mut out = Vec::new();
                let mut c = code;
                if code == self.hi && self.last != INVALID {
                    c = self.last;
                    while c >= self.clear {
                        c = self.prefix[usize::from(c)];
                    }
                    out.push(c as u8);
                    c = self.last;
                }
                while c >= self.clear {
                    out.push(self.suffix[usize::from(c)]);
                    c = self.prefix[usize::from(c)];
                }
                out.push(c as u8);
                out.reverse();
                self.pending.extend(out);
                if self.last != INVALID {
                    self.suffix[usize::from(self.hi)] = c as u8;
                    self.prefix[usize::from(self.hi)] = self.last;
                }
            } else {
                self.err = Some(msg("lzw: invalid code".into()));
                return;
            }
            self.last = code;
            self.hi += 1;
            if self.hi >= self.overflow {
                if self.width == 12 {
                    self.last = INVALID;
                    self.hi -= 1;
                } else {
                    self.width += 1;
                    self.overflow = 1 << self.width;
                }
            }
        }
    }

    /// Go's `io.ReadFull(lzwr, buf)`: (bytes read, error).
    fn read_full(&mut self, src: &mut Blocks, buf: &mut [u8]) -> (usize, Option<GoErr>) {
        let mut n = 0;
        while n < buf.len() {
            if self.pending.is_empty() {
                self.step(src);
            }
            if self.pending.is_empty() {
                let e = self.err.clone().unwrap_or(GoErr::Eof);
                return (n, Some(if n > 0 && e == GoErr::Eof { GoErr::UnexpectedEof } else { e }));
            }
            let k = self.pending.len().min(buf.len() - n);
            buf[n..n + k].copy_from_slice(&self.pending[..k]);
            self.pending.drain(..k);
            n += k;
        }
        (n, None)
    }
}

struct Decoder {
    width: usize,
    height: usize,
    global: Option<Vec<Pal>>,
    transparent: Option<u8>,
    image: Option<Img>,
}

fn color_table(r: &mut R, fields: u8) -> Result<Vec<Pal>, GoErr> {
    let n = 1usize << (1 + (fields & 7));
    let t = full(r, 3 * n).map_err(|e| msg(format!("gif: reading color table: {e}")))?;
    Ok(t.chunks(3).map(|c| Pal::Rgba([c[0], c[1], c[2], 0xff])).collect())
}

fn header(r: &mut R) -> Result<Decoder, GoErr> {
    let t = full(r, 13).map_err(|e| msg(format!("gif: reading header: {e}")))?;
    if &t[..6] != b"GIF87a" && &t[..6] != b"GIF89a" {
        return Err(msg(format!("gif: can't recognize format {}", crate::gotext::quote_bytes(&t[..6]))));
    }
    let mut d = Decoder {
        width: usize::from(t[6]) | usize::from(t[7]) << 8,
        height: usize::from(t[8]) | usize::from(t[9]) << 8,
        global: None,
        transparent: None,
        image: None,
    };
    if t[10] & 0x80 != 0 {
        d.global = Some(color_table(r, t[10])?);
    }
    Ok(d)
}

/// Go's `readBlock`: a sub-block's length (0 at the terminator).
fn block(r: &mut R) -> Result<usize, GoErr> {
    let n = byte(r)?;
    if n == 0 {
        return Ok(0);
    }
    full(r, usize::from(n))?;
    Ok(usize::from(n))
}

fn extension(r: &mut R, d: &mut Decoder) -> Result<(), GoErr> {
    let ext = byte(r).map_err(|e| msg(format!("gif: reading extension: {e}")))?;
    let size = match ext {
        0x01 => 13,
        0xf9 => {
            let t = full(r, 6).map_err(|e| msg(format!("gif: can't read graphic control: {e}")))?;
            if t[0] != 4 {
                return Err(msg(format!("gif: invalid graphic control extension block size: {}", t[0])));
            }
            if t[1] & 1 != 0 {
                d.transparent = Some(t[4]);
            }
            if t[5] != 0 {
                return Err(msg(format!("gif: invalid graphic control extension block terminator: {}", t[5])));
            }
            return Ok(());
        }
        0xfe => 0,
        0xff => usize::from(byte(r).map_err(|e| msg(format!("gif: reading extension: {e}")))?),
        _ => return Err(msg(format!("gif: unknown extension 0x{ext:02x}"))),
    };
    let mut data = Vec::new();
    if size > 0 {
        data = full(r, size).map_err(|e| msg(format!("gif: reading extension: {e}")))?;
    }
    let ext_err = |e: GoErr| msg(format!("gif: reading extension: {e}"));
    if ext == 0xff && data == b"NETSCAPE2.0" && block(r).map_err(ext_err)? == 0 {
        return Ok(());
    }
    while block(r).map_err(ext_err)? != 0 {}
    Ok(())
}

fn image_descriptor(r: &mut R, d: &mut Decoder) -> Result<(), GoErr> {
    let t = full(r, 9).map_err(|e| msg(format!("gif: can't read image descriptor: {e}")))?;
    let at = |i: usize| usize::from(t[i]) | usize::from(t[i + 1]) << 8;
    let (left, top, width, height, fields) = (at(0), at(2), at(4), at(6), t[8]);
    if left + width > d.width || top + height > d.height {
        return Err(msg("gif: frame bounds larger than image bounds".into()));
    }
    let mut m = Img::new(Kind::Paletted, left, top, width, height);
    let local = fields & 0x80 != 0;
    let mut palette = if local { color_table(r, fields)? } else { d.global.clone().ok_or_else(|| msg("gif: no color table".into()))? };
    if let Some(ti) = d.transparent {
        let ti = usize::from(ti);
        if ti >= palette.len() {
            palette.resize(ti + 1, Pal::Rgba([0, 0, 0, 0]));
        }
        palette[ti] = Pal::Rgba([0, 0, 0, 0]);
    }
    let lit = byte(r).map_err(|e| msg(format!("gif: reading image data: {e}")))?;
    if !(2..=8).contains(&lit) {
        return Err(msg(format!("gif: pixel size in decode out of range: {lit}")));
    }
    let mut br = Blocks { r, buf: Vec::new(), i: 0, err: None };
    let mut lzw = Lzw::new(u32::from(lit));
    let mut pix = vec![0u8; width * height];
    match lzw.read_full(&mut br, &mut pix) {
        (_, None) => {}
        (_, Some(GoErr::Eof | GoErr::UnexpectedEof)) => return Err(msg("gif: not enough image data".into())),
        (_, Some(e)) => return Err(msg(format!("gif: reading image data: {e}"))),
    }
    let mut one = [0u8; 1];
    match lzw.read_full(&mut br, &mut one) {
        (0, Some(GoErr::Eof | GoErr::UnexpectedEof)) => {}
        (0, Some(e)) => return Err(msg(format!("gif: reading image data: {e}"))),
        _ => return Err(too_much()),
    }
    match br.close() {
        Ok(()) => {}
        Err(e) if e == too_much() => return Err(e),
        Err(e) => return Err(msg(format!("gif: reading image data: {e}"))),
    }
    if palette.len() < 256 && pix.iter().any(|&p| usize::from(p) >= palette.len()) {
        return Err(msg("gif: invalid pixel value".into()));
    }
    if fields & 0x40 != 0 {
        let mut n = vec![0u8; width * height];
        let mut off = 0;
        for (skip, start) in [(8, 0), (8, 4), (4, 2), (2, 1)] {
            let mut y = start;
            while y < height {
                n[y * width..y * width + width].copy_from_slice(&pix[off..off + width]);
                off += width;
                y += skip;
            }
        }
        pix = n;
    }
    m.pix = pix;
    m.palette = palette;
    d.image = Some(m);
    Ok(())
}

/// Go's `gif.DecodeConfig`: the screen's (width, height).
pub fn decode_config(mut r: R) -> Result<(usize, usize), GoErr> {
    let d = header(&mut r)?;
    Ok((d.width, d.height))
}

/// Go's `gif.Decode`: the first frame.
pub fn decode(mut r: R) -> Result<Img, GoErr> {
    let mut d = header(&mut r)?;
    loop {
        let c = byte(&mut r).map_err(|e| msg(format!("gif: reading frames: {e}")))?;
        match c {
            0x21 => extension(&mut r, &mut d)?,
            0x2c => {
                image_descriptor(&mut r, &mut d)?;
                if let Some(m) = d.image.take() {
                    return Ok(m);
                }
            }
            0x3b => return Err(msg("gif: missing image data".into())),
            _ => return Err(msg(format!("gif: unknown block type: 0x{c:02x}"))),
        }
    }
}
