//! Go's `image/png` decoder: chunks with their CRCs and order, IDAT read through a zlib reader over Go's two bufio layers
//! (so a chunk's error surfaces when Go's would), every colour type and bit depth, the five filters, Adam7, tRNS, and
//! palettes that grow to the indices used.

use super::flate::zlib_reader;
use super::goio::{read_full, Bufio, BytesReader, GoErr, GoRead};
use super::images::{Img, Kind, Pal};

fn format_err(m: &str) -> GoErr {
    GoErr::Msg(format!("png: invalid format: {m}"))
}

fn unsupported(m: &str) -> GoErr {
    GoErr::Msg(format!("png: unsupported feature: {m}"))
}

static CRC_TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
    let mut t = [0u32; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    t
});

fn crc_update(crc: u32, b: &[u8]) -> u32 {
    let mut c = !crc;
    for &x in b {
        c = CRC_TABLE[((c ^ u32::from(x)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Stage {
    Start,
    SeenIhdr,
    SeenPlte,
    SeenTrns,
    SeenIdat,
    SeenIend,
}

#[derive(Clone, Copy, PartialEq)]
enum Cb {
    Invalid,
    G1,
    G2,
    G4,
    G8,
    Ga8,
    Tc8,
    P1,
    P2,
    P4,
    P8,
    Tca8,
    G16,
    Ga16,
    Tc16,
    Tca16,
}

fn paletted(cb: Cb) -> bool {
    matches!(cb, Cb::P1 | Cb::P2 | Cb::P4 | Cb::P8)
}

/// The IDAT stream: Go's `decoder.Read`, which runs on into the next IDAT chunk.
struct Idat<'a, 'b> {
    r: &'a mut Bufio<BytesReader<'b>>,
    crc: &'a mut u32,
    idat_length: &'a mut u32,
}

fn verify(r: &mut Bufio<BytesReader>, crc: u32) -> Result<(), GoErr> {
    let mut t = [0u8; 4];
    if let (_, Some(e)) = read_full(r, &mut t) {
        return Err(e);
    }
    if u32::from_be_bytes(t) != crc {
        return Err(format_err("invalid checksum"));
    }
    Ok(())
}

impl GoRead for Idat<'_, '_> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoErr>) {
        if p.is_empty() {
            return (0, None);
        }
        while *self.idat_length == 0 {
            if let Err(e) = verify(self.r, *self.crc) {
                return (0, Some(e));
            }
            let mut t = [0u8; 8];
            if let (_, Some(e)) = read_full(self.r, &mut t) {
                return (0, Some(e));
            }
            *self.idat_length = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
            if &t[4..8] != b"IDAT" {
                return (0, Some(format_err("not enough pixel data")));
            }
            *self.crc = crc_update(0, &t[4..8]);
        }
        let k = p.len().min(*self.idat_length as usize);
        let (n, err) = self.r.read(&mut p[..k]);
        *self.crc = crc_update(*self.crc, &p[..n]);
        *self.idat_length -= n as u32;
        (n, err)
    }
}

const INTERLACING: [(usize, usize, usize, usize); 7] =
    [(8, 8, 0, 0), (8, 8, 4, 0), (4, 8, 0, 4), (4, 4, 2, 0), (2, 4, 0, 2), (2, 2, 1, 0), (1, 2, 0, 1)];

pub struct Decoder<'b> {
    r: Bufio<BytesReader<'b>>,
    crc: u32,
    width: usize,
    height: usize,
    depth: u8,
    /// the palette's underlying 256 entries
    palette: Option<Vec<Pal>>,
    cb: Cb,
    stage: Stage,
    idat_length: u32,
    interlace: u8,
    use_transparent: bool,
    transparent: [u8; 6],
    img: Option<Img>,
}

impl<'b> Decoder<'b> {
    pub fn new(r: Bufio<BytesReader<'b>>) -> Decoder<'b> {
        Decoder {
            r,
            crc: 0,
            width: 0,
            height: 0,
            depth: 0,
            palette: None,
            cb: Cb::Invalid,
            stage: Stage::Start,
            idat_length: 0,
            interlace: 0,
            use_transparent: false,
            transparent: [0; 6],
            img: None,
        }
    }

    fn read_chunk_data(&mut self, n: usize) -> Result<Vec<u8>, GoErr> {
        let mut t = vec![0u8; n];
        let (got, err) = read_full(&mut self.r, &mut t);
        self.crc = crc_update(self.crc, &t[..got]);
        match err {
            Some(e) => Err(e),
            None => Ok(t),
        }
    }

    fn verify_checksum(&mut self) -> Result<(), GoErr> {
        verify(&mut self.r, self.crc)
    }

    fn parse_ihdr(&mut self, length: u32) -> Result<(), GoErr> {
        if length != 13 {
            return Err(format_err("bad IHDR length"));
        }
        let t = self.read_chunk_data(13)?;
        if t[10] != 0 {
            return Err(unsupported("compression method"));
        }
        if t[11] != 0 {
            return Err(unsupported("filter method"));
        }
        if t[12] > 1 {
            return Err(format_err("invalid interlace method"));
        }
        self.interlace = t[12];
        let w = i32::from_be_bytes([t[0], t[1], t[2], t[3]]);
        let h = i32::from_be_bytes([t[4], t[5], t[6], t[7]]);
        if w <= 0 || h <= 0 {
            return Err(format_err("non-positive dimension"));
        }
        self.depth = t[8];
        self.cb = match (t[8], t[9]) {
            (1, 0) => Cb::G1,
            (1, 3) => Cb::P1,
            (2, 0) => Cb::G2,
            (2, 3) => Cb::P2,
            (4, 0) => Cb::G4,
            (4, 3) => Cb::P4,
            (8, 0) => Cb::G8,
            (8, 2) => Cb::Tc8,
            (8, 3) => Cb::P8,
            (8, 4) => Cb::Ga8,
            (8, 6) => Cb::Tca8,
            (16, 0) => Cb::G16,
            (16, 2) => Cb::Tc16,
            (16, 4) => Cb::Ga16,
            (16, 6) => Cb::Tca16,
            _ => Cb::Invalid,
        };
        if self.cb == Cb::Invalid {
            return Err(unsupported(&format!("bit depth {}, color type {}", t[8], t[9])));
        }
        self.width = w as usize;
        self.height = h as usize;
        self.verify_checksum()
    }

    fn parse_plte(&mut self, length: u32) -> Result<(), GoErr> {
        let np = (length / 3) as usize;
        if !length.is_multiple_of(3) || np == 0 || np > 256 || np > 1 << self.depth {
            return Err(format_err("bad PLTE length"));
        }
        let t = self.read_chunk_data(3 * np)?;
        match self.cb {
            Cb::P1 | Cb::P2 | Cb::P4 | Cb::P8 => {
                let mut p = vec![Pal::Rgba([0, 0, 0, 0xff]); 256];
                for i in 0..np {
                    p[i] = Pal::Rgba([t[3 * i], t[3 * i + 1], t[3 * i + 2], 0xff]);
                }
                self.palette = Some(p);
            }
            Cb::Tc8 | Cb::Tca8 | Cb::Tc16 | Cb::Tca16 => {}
            _ => return Err(format_err("PLTE, color type mismatch")),
        }
        self.verify_checksum()
    }

    fn parse_trns(&mut self, length: u32) -> Result<(), GoErr> {
        match self.cb {
            Cb::G1 | Cb::G2 | Cb::G4 | Cb::G8 | Cb::G16 => {
                if length != 2 {
                    return Err(format_err("bad tRNS length"));
                }
                let t = self.read_chunk_data(2)?;
                self.transparent[..2].copy_from_slice(&t);
                self.transparent[1] = self.transparent[1].wrapping_mul(match self.cb {
                    Cb::G1 => 0xff,
                    Cb::G2 => 0x55,
                    Cb::G4 => 0x11,
                    _ => 1,
                });
                self.use_transparent = true;
            }
            Cb::Tc8 | Cb::Tc16 => {
                if length != 6 {
                    return Err(format_err("bad tRNS length"));
                }
                let t = self.read_chunk_data(6)?;
                self.transparent.copy_from_slice(&t);
                self.use_transparent = true;
            }
            Cb::P1 | Cb::P2 | Cb::P4 | Cb::P8 => {
                if length > 256 {
                    return Err(format_err("bad tRNS length"));
                }
                let t = self.read_chunk_data(length as usize)?;
                if let Some(p) = &mut self.palette {
                    for (i, &a) in t.iter().enumerate() {
                        if let Pal::Rgba([r, g, b, _]) = p[i] {
                            p[i] = Pal::Nrgba([r, g, b, a]);
                        }
                    }
                }
            }
            _ => return Err(format_err("tRNS, color type mismatch")),
        }
        self.verify_checksum()
    }

    fn params(&self) -> Params {
        Params {
            cb: self.cb,
            depth: self.depth,
            width: self.width,
            height: self.height,
            use_transparent: self.use_transparent,
            transparent: self.transparent,
            palette: self.palette.clone(),
        }
    }

    fn decode_idat(&mut self) -> Result<Img, GoErr> {
        let params = self.params();
        let interlace = self.interlace;
        let mut crc = self.crc;
        let mut idat_length = self.idat_length;
        let result = (|| -> Result<Img, GoErr> {
            let idat = Idat { r: &mut self.r, crc: &mut crc, idat_length: &mut idat_length };
            let mut z = zlib_reader(Bufio::new(idat))?;
            let img = if interlace == 0 {
                params.read_pass(&mut z, None)?.expect("a whole image")
            } else {
                let mut img = params.new_img(params.width, params.height);
                for (pass, &scan) in INTERLACING.iter().enumerate() {
                    if let Some(p) = params.read_pass(&mut z, Some(pass))? {
                        img.merge(&p, scan);
                    }
                }
                img
            };
            let mut one = [0u8; 1];
            let (mut n, mut err) = (0, None);
            let mut i = 0;
            while n == 0 && err.is_none() {
                if i == 100 {
                    return Err(GoErr::NoProgress);
                }
                (n, err) = z.read(&mut one);
                i += 1;
            }
            if let Some(e) = err {
                if e != GoErr::Eof {
                    return Err(format_err(&e.to_string()));
                }
            }
            if n != 0 || *z.source().rd.idat_length != 0 {
                return Err(format_err("too much pixel data"));
            }
            Ok(img)
        })();
        self.crc = crc;
        self.idat_length = idat_length;
        result
    }
}

/// What a pass needs of the decoder.
struct Params {
    cb: Cb,
    depth: u8,
    width: usize,
    height: usize,
    use_transparent: bool,
    transparent: [u8; 6],
    palette: Option<Vec<Pal>>,
}

impl Params {
    fn kind_and_bits(&self) -> (Kind, usize) {
        match self.cb {
            Cb::G1 | Cb::G2 | Cb::G4 | Cb::G8 => (if self.use_transparent { Kind::Nrgba } else { Kind::Gray }, usize::from(self.depth)),
            Cb::Ga8 => (Kind::Nrgba, 16),
            Cb::Tc8 => (if self.use_transparent { Kind::Nrgba } else { Kind::Rgba }, 24),
            Cb::P1 | Cb::P2 | Cb::P4 | Cb::P8 => (Kind::Paletted, usize::from(self.depth)),
            Cb::Tca8 => (Kind::Nrgba, 32),
            Cb::G16 => (if self.use_transparent { Kind::Nrgba64 } else { Kind::Gray16 }, 16),
            Cb::Ga16 => (Kind::Nrgba64, 32),
            Cb::Tc16 => (if self.use_transparent { Kind::Nrgba64 } else { Kind::Rgba64 }, 48),
            Cb::Tca16 | Cb::Invalid => (Kind::Nrgba64, 64),
        }
    }

    fn new_img(&self, w: usize, h: usize) -> Img {
        let (kind, _) = self.kind_and_bits();
        let mut img = Img::new(kind, 0, 0, w, h);
        if let Some(p) = &self.palette {
            img.palette = p.clone();
        }
        img
    }

    /// Go's `readImagePass` for one pass (or the whole image), from the zlib stream.
    fn read_pass<R: GoRead>(&self, r: &mut R, pass: Option<usize>) -> Result<Option<Img>, GoErr> {
        let (mut width, mut height) = (self.width, self.height);
        if let Some(p) = pass {
            let (xf, yf, xo, yo) = INTERLACING[p];
            width = (width + xf - 1).saturating_sub(xo) / xf;
            height = (height + yf - 1).saturating_sub(yo) / yf;
            if width == 0 || height == 0 {
                return Ok(None);
            }
        }
        let mut img = self.new_img(width, height);
        let (_, bits) = self.kind_and_bits();
        let bpp = bits.div_ceil(8);
        let row = 1 + (bits * width).div_ceil(8);
        let mut cr = vec![0u8; row];
        let mut pr = vec![0u8; row];
        let tr = self.transparent;
        for y in 0..height {
            let (_, err) = read_full(r, &mut cr);
            match err {
                None => {}
                Some(GoErr::Eof | GoErr::UnexpectedEof) => return Err(format_err("not enough pixel data")),
                Some(e) => return Err(e),
            }
            let filter = cr[0];
            let (cdat, pdat) = (&mut cr[1..], &pr[1..]);
            match filter {
                0 => {}
                1 => {
                    for i in bpp..cdat.len() {
                        cdat[i] = cdat[i].wrapping_add(cdat[i - bpp]);
                    }
                }
                2 => {
                    for (c, &p) in cdat.iter_mut().zip(pdat) {
                        *c = c.wrapping_add(p);
                    }
                }
                3 => {
                    for i in 0..bpp.min(cdat.len()) {
                        cdat[i] = cdat[i].wrapping_add(pdat[i] / 2);
                    }
                    for i in bpp..cdat.len() {
                        cdat[i] = cdat[i].wrapping_add(((u16::from(cdat[i - bpp]) + u16::from(pdat[i])) / 2) as u8);
                    }
                }
                4 => paeth(cdat, pdat, bpp),
                _ => return Err(format_err("bad filter type")),
            }
            let cdat: &[u8] = cdat;
            match self.cb {
                Cb::G1 | Cb::G2 | Cb::G4 => {
                    let (per, shift, scale): (usize, u32, u8) = match self.cb {
                        Cb::G1 => (8, 1, 0xff),
                        Cb::G2 => (4, 2, 0x55),
                        _ => (2, 4, 0x11),
                    };
                    for x in (0..width).step_by(per) {
                        let mut b = cdat[x / per];
                        for x2 in 0..per {
                            if x + x2 >= width {
                                break;
                            }
                            let ycol = (b >> (8 - shift)).wrapping_mul(scale);
                            if self.use_transparent {
                                let a = if ycol == tr[1] { 0 } else { 0xff };
                                img.set(x + x2, y, &[ycol, ycol, ycol, a]);
                            } else {
                                img.set(x + x2, y, &[ycol]);
                            }
                            b = b.wrapping_shl(shift);
                        }
                    }
                }
                Cb::G8 => {
                    for (x, &ycol) in cdat[..width].iter().enumerate() {
                        if self.use_transparent {
                            img.set(x, y, &[ycol, ycol, ycol, if ycol == tr[1] { 0 } else { 0xff }]);
                        } else {
                            img.set(x, y, &[ycol]);
                        }
                    }
                }
                Cb::Ga8 => {
                    for x in 0..width {
                        let ycol = cdat[2 * x];
                        img.set(x, y, &[ycol, ycol, ycol, cdat[2 * x + 1]]);
                    }
                }
                Cb::Tc8 => {
                    for x in 0..width {
                        let (r, g, b) = (cdat[3 * x], cdat[3 * x + 1], cdat[3 * x + 2]);
                        let a = if self.use_transparent && r == tr[1] && g == tr[3] && b == tr[5] { 0 } else { 0xff };
                        img.set(x, y, &[r, g, b, a]);
                    }
                }
                Cb::P1 | Cb::P2 | Cb::P4 | Cb::P8 => {
                    let (per, shift): (usize, u32) = match self.cb {
                        Cb::P1 => (8, 1),
                        Cb::P2 => (4, 2),
                        Cb::P4 => (2, 4),
                        _ => (1, 8),
                    };
                    for x in (0..width).step_by(per) {
                        let mut b = cdat[x / per];
                        for x2 in 0..per {
                            if x + x2 >= width {
                                break;
                            }
                            let idx = if shift == 8 { b } else { b >> (8 - shift) };
                            img.set(x + x2, y, &[idx]);
                            b = b.wrapping_shl(shift);
                        }
                    }
                }
                Cb::Tca8 => img.set_row(y, &cdat[..4 * width]),
                Cb::G16 => {
                    for x in 0..width {
                        let (h, l) = (cdat[2 * x], cdat[2 * x + 1]);
                        if self.use_transparent {
                            let a = if h == tr[0] && l == tr[1] { 0 } else { 0xff };
                            img.set(x, y, &[h, l, h, l, h, l, a, a]);
                        } else {
                            img.set(x, y, &[h, l]);
                        }
                    }
                }
                Cb::Ga16 => {
                    for x in 0..width {
                        let c = &cdat[4 * x..4 * x + 4];
                        img.set(x, y, &[c[0], c[1], c[0], c[1], c[0], c[1], c[2], c[3]]);
                    }
                }
                Cb::Tc16 => {
                    for x in 0..width {
                        let c = &cdat[6 * x..6 * x + 6];
                        let a = if self.use_transparent && c == tr { 0 } else { 0xff };
                        img.set(x, y, &[c[0], c[1], c[2], c[3], c[4], c[5], a, a]);
                    }
                }
                Cb::Tca16 => img.set_row(y, &cdat[..8 * width]),
                Cb::Invalid => {}
            }
            std::mem::swap(&mut cr, &mut pr);
        }
        Ok(Some(img))
    }
}

impl<'b> Decoder<'b> {
    fn parse_chunk(&mut self, config_only: bool) -> Result<(), GoErr> {
        let mut t = [0u8; 8];
        if let (_, Some(e)) = read_full(&mut self.r, &mut t) {
            return Err(e);
        }
        let length = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
        self.crc = crc_update(0, &t[4..8]);
        match &t[4..8] {
            b"IHDR" => {
                if self.stage != Stage::Start {
                    return Err(format_err("chunk out of order"));
                }
                self.stage = Stage::SeenIhdr;
                return self.parse_ihdr(length);
            }
            b"PLTE" => {
                if self.stage != Stage::SeenIhdr {
                    return Err(format_err("chunk out of order"));
                }
                self.stage = Stage::SeenPlte;
                return self.parse_plte(length);
            }
            b"tRNS" => {
                let ok = if paletted(self.cb) {
                    self.stage == Stage::SeenPlte
                } else if matches!(self.cb, Cb::Tc8 | Cb::Tc16) {
                    self.stage == Stage::SeenIhdr || self.stage == Stage::SeenPlte
                } else {
                    self.stage == Stage::SeenIhdr
                };
                if !ok {
                    return Err(format_err("chunk out of order"));
                }
                self.stage = Stage::SeenTrns;
                return self.parse_trns(length);
            }
            b"IDAT" => {
                if self.stage < Stage::SeenIhdr || self.stage > Stage::SeenIdat || (self.stage == Stage::SeenIhdr && paletted(self.cb)) {
                    return Err(format_err("chunk out of order"));
                }
                if self.stage != Stage::SeenIdat {
                    self.stage = Stage::SeenIdat;
                    if config_only {
                        return Ok(());
                    }
                    self.idat_length = length;
                    self.img = Some(self.decode_idat()?);
                    return self.verify_checksum();
                }
            }
            b"IEND" => {
                if self.stage != Stage::SeenIdat {
                    return Err(format_err("chunk out of order"));
                }
                self.stage = Stage::SeenIend;
                if length != 0 {
                    return Err(format_err("bad IEND length"));
                }
                return self.verify_checksum();
            }
            _ => {}
        }
        if length > 0x7fff_ffff {
            return Err(format_err(&format!("Bad chunk length: {length}")));
        }
        let mut left = length as usize;
        while left > 0 {
            let k = left.min(4096);
            self.read_chunk_data(k)?;
            left -= k;
        }
        self.verify_checksum()
    }

    fn check_header(&mut self) -> Result<(), GoErr> {
        let mut t = [0u8; 8];
        if let (_, Some(e)) = read_full(&mut self.r, &mut t) {
            return Err(e);
        }
        if &t != b"\x89PNG\r\n\x1a\n" {
            return Err(format_err("not a PNG file"));
        }
        Ok(())
    }

    /// Go's `png.Decode`.
    pub fn decode(mut self) -> Result<Img, GoErr> {
        let eof = |e: GoErr| if e == GoErr::Eof { GoErr::UnexpectedEof } else { e };
        self.check_header().map_err(eof)?;
        while self.stage != Stage::SeenIend {
            self.parse_chunk(false).map_err(eof)?;
        }
        Ok(self.img.expect("IEND follows IDAT"))
    }

    /// Go's `png.DecodeConfig`: (width, height).
    pub fn decode_config(mut self) -> Result<(usize, usize), GoErr> {
        let eof = |e: GoErr| if e == GoErr::Eof { GoErr::UnexpectedEof } else { e };
        self.check_header().map_err(eof)?;
        loop {
            self.parse_chunk(true).map_err(eof)?;
            if paletted(self.cb) {
                if self.stage >= Stage::SeenTrns {
                    break;
                }
            } else if self.stage >= Stage::SeenIhdr {
                break;
            }
        }
        Ok((self.width, self.height))
    }
}

fn paeth(cdat: &mut [u8], pdat: &[u8], bpp: usize) {
    for i in 0..bpp {
        let (mut a, mut c) = (0i32, 0i32);
        let mut j = i;
        while j < cdat.len() {
            let b = i32::from(pdat[j]);
            let pa0 = b - c;
            let pb0 = a - c;
            let pc = (pa0 + pb0).abs();
            let (pa, pb) = (pa0.abs(), pb0.abs());
            if pa <= pb && pa <= pc {
            } else if pb <= pc {
                a = b;
            } else {
                a = c;
            }
            a += i32::from(cdat[j]);
            a &= 0xff;
            cdat[j] = a as u8;
            c = b;
            j += bpp;
        }
    }
}
