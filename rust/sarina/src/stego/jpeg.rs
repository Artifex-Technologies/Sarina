//! Go's `image/jpeg` decoder, written out from Go 1.27's source: markers and segments with their errors, Huffman tables,
//! baseline and progressive scans (successive approximation, end-of-band runs), restart markers, the integer IDCT,
//! every chroma sampling (the flexible ones expanded as Go does), grey, YCbCr, RGB and CMYK/YCCK, and each picture
//! turned into the pixels DecodeImage reads (YCbCr and CMYK through their `RGBA()` as `color.NRGBAModel` does).

use super::goio::GoErr;
use super::images::{Img, Kind};

fn format_err(m: &str) -> GoErr {
    GoErr::Msg(format!("invalid JPEG format: {m}"))
}

fn unsupported(m: &str) -> GoErr {
    GoErr::Msg(format!("unsupported JPEG feature: {m}"))
}

fn ratio_err() -> GoErr {
    unsupported("luma/chroma subsampling ratio")
}

const UNZIG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56,
    57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

type Block = [i32; 64];

#[derive(Clone, Copy, Default)]
struct Component {
    h: usize,
    v: usize,
    c: u8,
    tq: u8,
    expand_h: usize,
    expand_v: usize,
}

#[derive(Clone)]
struct Huffman {
    n_codes: i32,
    lut: [u16; 256],
    vals: [u8; 256],
    min_codes: [i32; 16],
    max_codes: [i32; 16],
    vals_indices: [i32; 16],
}

impl Default for Huffman {
    fn default() -> Huffman {
        Huffman { n_codes: 0, lut: [0; 256], vals: [0; 256], min_codes: [0; 16], max_codes: [0; 16], vals_indices: [0; 16] }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Ratio {
    R444,
    R422,
    R420,
    R440,
    R411,
    R410,
}

/// image.YCbCr's planes.
struct Ycc {
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
    y_stride: usize,
    c_stride: usize,
    ratio: Ratio,
}

impl Ycc {
    fn new(w: usize, h: usize, ratio: Ratio) -> Ycc {
        let (cw, ch) = match ratio {
            Ratio::R444 => (w, h),
            Ratio::R422 => (w.div_ceil(2), h),
            Ratio::R420 => (w.div_ceil(2), h.div_ceil(2)),
            Ratio::R440 => (w, h.div_ceil(2)),
            Ratio::R411 => (w.div_ceil(4), h),
            Ratio::R410 => (w.div_ceil(4), h.div_ceil(2)),
        };
        Ycc { y: vec![0; w * h], cb: vec![0; cw * ch], cr: vec![0; cw * ch], y_stride: w, c_stride: cw, ratio }
    }

    fn c_offset(&self, x: usize, y: usize) -> usize {
        match self.ratio {
            Ratio::R422 => y * self.c_stride + x / 2,
            Ratio::R420 => (y / 2) * self.c_stride + x / 2,
            Ratio::R440 => (y / 2) * self.c_stride + x,
            Ratio::R411 => y * self.c_stride + x / 4,
            Ratio::R410 => (y / 2) * self.c_stride + x / 4,
            Ratio::R444 => y * self.c_stride + x,
        }
    }
}

#[derive(Default)]
struct Bits {
    a: u32,
    m: u32,
    n: i32,
}

pub struct Decoder<'b> {
    data: &'b [u8],
    i: usize,
    n_unreadable: usize,
    bits: Bits,
    width: usize,
    height: usize,
    img1: Option<(Vec<u8>, usize)>,
    img3: Option<Ycc>,
    black: Option<(Vec<u8>, usize)>,
    flex: bool,
    max_h: usize,
    max_v: usize,
    ri: usize,
    n_comp: usize,
    baseline: bool,
    progressive: bool,
    jfif: bool,
    adobe_valid: bool,
    adobe_transform: u8,
    eob_run: u16,
    comp: [Component; 4],
    prog_coeffs: [Option<Vec<Block>>; 4],
    huff: [[Huffman; 4]; 2],
    quant: [Block; 4],
    tmp: [u8; 128],
}

const SHORT_HUFFMAN: &str = "short Huffman data";
const MISSING_FF00: &str = "missing 0xff00 sequence";

fn is(e: &GoErr, m: &str) -> bool {
    *e == format_err(m)
}

impl<'b> Decoder<'b> {
    pub fn new(data: &'b [u8]) -> Decoder<'b> {
        Decoder {
            data,
            i: 0,
            n_unreadable: 0,
            bits: Bits::default(),
            width: 0,
            height: 0,
            img1: None,
            img3: None,
            black: None,
            flex: false,
            max_h: 0,
            max_v: 0,
            ri: 0,
            n_comp: 0,
            baseline: false,
            progressive: false,
            jfif: false,
            adobe_valid: false,
            adobe_transform: 0,
            eob_run: 0,
            comp: [Component::default(); 4],
            prog_coeffs: [None, None, None, None],
            huff: Default::default(),
            quant: [[0; 64]; 4],
            tmp: [0; 128],
        }
    }

    fn unread_byte_stuffed_byte(&mut self) {
        self.i -= self.n_unreadable;
        self.n_unreadable = 0;
        if self.bits.n >= 8 {
            self.bits.a >>= 8;
            self.bits.n -= 8;
            self.bits.m >>= 8;
        }
    }

    fn read_byte(&mut self) -> Result<u8, GoErr> {
        let x = *self.data.get(self.i).ok_or(GoErr::UnexpectedEof)?;
        self.i += 1;
        self.n_unreadable = 0;
        Ok(x)
    }

    fn read_byte_stuffed_byte(&mut self) -> Result<u8, GoErr> {
        self.n_unreadable = 0;
        let x = self.read_byte()?;
        self.n_unreadable = 1;
        if x != 0xff {
            return Ok(x);
        }
        let y = self.read_byte()?;
        self.n_unreadable = 2;
        if y != 0 {
            return Err(format_err(MISSING_FF00));
        }
        Ok(0xff)
    }

    fn settle_unreadable(&mut self) {
        if self.n_unreadable != 0 {
            if self.bits.n >= 8 {
                self.unread_byte_stuffed_byte();
            }
            self.n_unreadable = 0;
        }
    }

    fn read_full(&mut self, n: usize) -> Result<[u8; 128], GoErr> {
        self.settle_unreadable();
        if self.i + n > self.data.len() {
            self.i = self.data.len();
            return Err(GoErr::UnexpectedEof);
        }
        let mut t = [0u8; 128];
        t[..n].copy_from_slice(&self.data[self.i..self.i + n]);
        self.i += n;
        Ok(t)
    }

    fn ignore(&mut self, n: usize) -> Result<(), GoErr> {
        self.settle_unreadable();
        if self.i + n > self.data.len() {
            self.i = self.data.len();
            return Err(GoErr::UnexpectedEof);
        }
        self.i += n;
        Ok(())
    }

    fn ensure_n_bits(&mut self, n: i32) -> Result<(), GoErr> {
        loop {
            let c = match self.read_byte_stuffed_byte() {
                Ok(c) => c,
                Err(GoErr::UnexpectedEof) => return Err(format_err(SHORT_HUFFMAN)),
                Err(e) => return Err(e),
            };
            self.bits.a = self.bits.a << 8 | u32::from(c);
            self.bits.n += 8;
            self.bits.m = if self.bits.m == 0 { 1 << 7 } else { self.bits.m << 8 };
            if self.bits.n >= n {
                return Ok(());
            }
        }
    }

    fn receive_extend(&mut self, t: u8) -> Result<i32, GoErr> {
        if self.bits.n < i32::from(t) {
            self.ensure_n_bits(i32::from(t))?;
        }
        self.bits.n -= i32::from(t);
        self.bits.m = self.bits.m.checked_shr(u32::from(t)).unwrap_or(0);
        let s = 1i32.wrapping_shl(u32::from(t));
        let mut x = (self.bits.a.checked_shr(self.bits.n as u32).unwrap_or(0) as i32) & s.wrapping_sub(1);
        let sign = x.wrapping_shr(u32::from(t).wrapping_sub(1)).wrapping_sub(1);
        x = x.wrapping_add(sign & ((-1i32).wrapping_shl(u32::from(t))).wrapping_add(1));
        Ok(x)
    }

    fn process_dht(&mut self, mut n: i64) -> Result<(), GoErr> {
        while n > 0 {
            if n < 17 {
                return Err(format_err("DHT has wrong length"));
            }
            let t = self.read_full(17)?;
            let tc = t[0] >> 4;
            if tc > 1 {
                return Err(format_err("bad Tc value"));
            }
            let th = t[0] & 0x0f;
            if th > 3 || (self.baseline && th > 1) {
                return Err(format_err("bad Th value"));
            }
            let mut n_codes = [0i32; 16];
            let mut total = 0;
            for (i, c) in n_codes.iter_mut().enumerate() {
                *c = i32::from(t[i + 1]);
                total += *c;
            }
            let h = &mut self.huff[usize::from(tc)][usize::from(th)];
            h.n_codes = total;
            if total == 0 {
                return Err(format_err("Huffman table has zero length"));
            }
            if total > 256 {
                return Err(format_err("Huffman table has excessive length"));
            }
            n -= i64::from(total) + 17;
            if n < 0 {
                return Err(format_err("DHT has wrong length"));
            }
            self.settle_unreadable();
            let k = total as usize;
            if self.i + k > self.data.len() {
                self.i = self.data.len();
                return Err(GoErr::UnexpectedEof);
            }
            let h = &mut self.huff[usize::from(tc)][usize::from(th)];
            h.vals[..k].copy_from_slice(&self.data[self.i..self.i + k]);
            self.i += k;
            h.lut = [0; 256];
            let (mut x, mut code) = (0usize, 0u32);
            for i in 0..8u32 {
                code <<= 1;
                for _ in 0..n_codes[i as usize] {
                    let base = (code << (7 - i)) as u8;
                    let lut_value = u16::from(h.vals[x]) << 8 | (2 + i) as u16;
                    for k in 0..(1u32 << (7 - i)) {
                        h.lut[usize::from(base | k as u8)] = lut_value;
                    }
                    code += 1;
                    x += 1;
                }
            }
            let (mut c, mut index) = (0i32, 0i32);
            for (i, &nc) in n_codes.iter().enumerate() {
                if nc == 0 {
                    h.min_codes[i] = -1;
                    h.max_codes[i] = -1;
                    h.vals_indices[i] = -1;
                } else {
                    h.min_codes[i] = c;
                    h.max_codes[i] = c + nc - 1;
                    h.vals_indices[i] = index;
                    c += nc;
                    index += nc;
                }
                c <<= 1;
            }
        }
        Ok(())
    }

    fn decode_huffman(&mut self, tc: usize, th: usize) -> Result<u8, GoErr> {
        if self.huff[tc][th].n_codes == 0 {
            return Err(format_err("uninitialized Huffman table"));
        }
        let mut slow = false;
        if self.bits.n < 8 {
            if let Err(e) = self.ensure_n_bits(8) {
                if !is(&e, MISSING_FF00) && !is(&e, SHORT_HUFFMAN) {
                    return Err(e);
                }
                if self.n_unreadable != 0 {
                    self.unread_byte_stuffed_byte();
                }
                slow = true;
            }
        }
        if !slow {
            let v = self.huff[tc][th].lut[((self.bits.a >> (self.bits.n - 8) as u32) & 0xff) as usize];
            if v != 0 {
                let n = (v & 0xff) - 1;
                self.bits.n -= i32::from(n);
                self.bits.m >>= n;
                return Ok((v >> 8) as u8);
            }
        }
        let mut code = 0i32;
        for i in 0..16 {
            if self.bits.n == 0 {
                self.ensure_n_bits(1)?;
            }
            if self.bits.a & self.bits.m != 0 {
                code |= 1;
            }
            self.bits.n -= 1;
            self.bits.m >>= 1;
            let h = &self.huff[tc][th];
            if code <= h.max_codes[i] {
                return Ok(h.vals[(h.vals_indices[i] + code - h.min_codes[i]) as usize]);
            }
            code <<= 1;
        }
        Err(format_err("bad Huffman code"))
    }

    fn decode_bit(&mut self) -> Result<bool, GoErr> {
        if self.bits.n == 0 {
            self.ensure_n_bits(1)?;
        }
        let ret = self.bits.a & self.bits.m != 0;
        self.bits.n -= 1;
        self.bits.m >>= 1;
        Ok(ret)
    }

    fn decode_bits(&mut self, n: i32) -> Result<u32, GoErr> {
        if self.bits.n < n {
            self.ensure_n_bits(n)?;
        }
        let ret = self.bits.a.checked_shr((self.bits.n - n) as u32).unwrap_or(0) & (1u32 << n as u32).wrapping_sub(1);
        self.bits.n -= n;
        self.bits.m = self.bits.m.checked_shr(n as u32).unwrap_or(0);
        Ok(ret)
    }

    fn process_sof(&mut self, n: usize) -> Result<(), GoErr> {
        if self.n_comp != 0 {
            return Err(format_err("multiple SOF markers"));
        }
        self.n_comp = match n {
            9 => 1,
            15 => 3,
            18 => 4,
            _ => return Err(unsupported("number of components")),
        };
        let t = self.read_full(n)?;
        if t[0] != 8 {
            return Err(unsupported("precision"));
        }
        self.height = usize::from(t[1]) << 8 | usize::from(t[2]);
        self.width = usize::from(t[3]) << 8 | usize::from(t[4]);
        if usize::from(t[5]) != self.n_comp {
            return Err(format_err("SOF has wrong length"));
        }
        for i in 0..self.n_comp {
            self.comp[i].c = t[6 + 3 * i];
            if (0..i).any(|j| self.comp[j].c == self.comp[i].c) {
                return Err(format_err("repeated component identifier"));
            }
            self.comp[i].tq = t[8 + 3 * i];
            if self.comp[i].tq > 3 {
                return Err(format_err("bad Tq value"));
            }
            let hv = t[7 + 3 * i];
            let (mut h, mut v) = (usize::from(hv >> 4), usize::from(hv & 0x0f));
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                return Err(format_err("luma/chroma subsampling ratio"));
            }
            if h == 3 || v == 3 {
                return Err(ratio_err());
            }
            match self.n_comp {
                1 => (h, v) = (1, 1),
                4 => match i {
                    0 if hv != 0x11 && hv != 0x22 => return Err(ratio_err()),
                    1 | 2 if hv != 0x11 => return Err(ratio_err()),
                    3 if self.comp[0].h != h || self.comp[0].v != v => return Err(ratio_err()),
                    _ => {}
                },
                _ => {}
            }
            self.max_h = self.max_h.max(h);
            self.max_v = self.max_v.max(v);
            self.comp[i].h = h;
            self.comp[i].v = v;
        }
        if self.n_comp == 3 && (0..3).any(|i| !self.max_h.is_multiple_of(self.comp[i].h) || !self.max_v.is_multiple_of(self.comp[i].v)) {
            return Err(ratio_err());
        }
        for i in 0..self.n_comp {
            self.comp[i].expand_h = self.max_h / self.comp[i].h;
            self.comp[i].expand_v = self.max_v / self.comp[i].v;
        }
        Ok(())
    }

    fn process_dqt(&mut self, mut n: i64) -> Result<(), GoErr> {
        while n > 0 {
            n -= 1;
            let x = self.read_byte()?;
            let tq = usize::from(x & 0x0f);
            if tq > 3 {
                return Err(format_err("bad Tq value"));
            }
            match x >> 4 {
                0 => {
                    if n < 64 {
                        break;
                    }
                    n -= 64;
                    let t = self.read_full(64)?;
                    for i in 0..64 {
                        self.quant[tq][i] = i32::from(t[i]);
                    }
                }
                1 => {
                    if n < 128 {
                        break;
                    }
                    n -= 128;
                    let t = self.read_full(128)?;
                    for i in 0..64 {
                        self.quant[tq][i] = i32::from(t[2 * i]) << 8 | i32::from(t[2 * i + 1]);
                    }
                }
                _ => return Err(format_err("bad Pq value")),
            }
        }
        if n != 0 {
            return Err(format_err("DQT has wrong length"));
        }
        Ok(())
    }

    fn make_img(&mut self, mxx: usize, myy: usize) {
        if self.n_comp == 1 {
            self.img1 = Some((vec![0; 64 * mxx * myy], 8 * mxx));
            return;
        }
        let mut ratio = Ratio::R444;
        let (c1, c2, c0) = (self.comp[1], self.comp[2], self.comp[0]);
        if c1.h != c2.h || c1.v != c2.v || self.max_h != c0.h || self.max_v != c0.v {
            self.flex = true;
        } else {
            ratio = match (self.max_h / c1.h) << 4 | (self.max_v / c1.v) {
                0x11 => Ratio::R444,
                0x12 => Ratio::R440,
                0x21 => Ratio::R422,
                0x22 => Ratio::R420,
                0x41 => Ratio::R411,
                0x42 => Ratio::R410,
                _ => {
                    self.flex = true;
                    Ratio::R444
                }
            };
        }
        self.img3 = Some(Ycc::new(8 * self.max_h * mxx, 8 * self.max_v * myy, ratio));
        if self.n_comp == 4 {
            let (h3, v3) = (self.comp[3].h, self.comp[3].v);
            self.black = Some((vec![0; 8 * h3 * mxx * 8 * v3 * myy], 8 * h3 * mxx));
        }
    }

    fn process_sos(&mut self, n: usize) -> Result<(), GoErr> {
        if self.n_comp == 0 {
            return Err(format_err("missing SOF marker"));
        }
        if n < 6 || 4 + 2 * self.n_comp < n || !n.is_multiple_of(2) {
            return Err(format_err("SOS has wrong length"));
        }
        let t = self.read_full(n)?;
        let n_comp = usize::from(t[0]);
        if n != 4 + 2 * n_comp {
            return Err(format_err("SOS length inconsistent with number of components"));
        }
        let mut scan = [(0usize, 0usize, 0usize); 4];
        let mut total_hv = 0;
        for i in 0..n_comp {
            let cs = t[1 + 2 * i];
            let Some(ci) = (0..self.n_comp).rev().find(|&j| self.comp[j].c == cs) else {
                return Err(format_err("unknown component selector"));
            };
            scan[i].0 = ci;
            if (0..i).any(|j| scan[j].0 == ci) {
                return Err(format_err("repeated component selector"));
            }
            total_hv += self.comp[ci].h * self.comp[ci].v;
            scan[i].1 = usize::from(t[2 + 2 * i] >> 4);
            if scan[i].1 > 3 || (self.baseline && scan[i].1 > 1) {
                return Err(format_err("bad Td value"));
            }
            scan[i].2 = usize::from(t[2 + 2 * i] & 0x0f);
            if scan[i].2 > 3 || (self.baseline && scan[i].2 > 1) {
                return Err(format_err("bad Ta value"));
            }
        }
        if self.n_comp > 1 && total_hv > 10 {
            return Err(format_err("total sampling factors too large"));
        }
        let (mut zig_start, mut zig_end, mut ah, mut al) = (0i32, 63i32, 0u32, 0u32);
        if self.progressive {
            zig_start = i32::from(t[1 + 2 * n_comp]);
            zig_end = i32::from(t[2 + 2 * n_comp]);
            ah = u32::from(t[3 + 2 * n_comp] >> 4);
            al = u32::from(t[3 + 2 * n_comp] & 0x0f);
            if (zig_start == 0 && zig_end != 0) || zig_start > zig_end || 64 <= zig_end {
                return Err(format_err("bad spectral selection bounds"));
            }
            if zig_start != 0 && n_comp != 1 {
                return Err(format_err("progressive AC coefficients for more than one component"));
            }
            if ah != 0 && ah != al + 1 {
                return Err(format_err("bad successive approximation values"));
            }
        }
        let mxx = self.width.div_ceil(8 * self.max_h);
        let myy = self.height.div_ceil(8 * self.max_v);
        if self.img1.is_none() && self.img3.is_none() {
            self.make_img(mxx, myy);
        }
        if self.progressive {
            for s in scan.iter().take(n_comp) {
                let ci = s.0;
                if self.prog_coeffs[ci].is_none() {
                    self.prog_coeffs[ci] = Some(vec![[0; 64]; mxx * myy * self.comp[ci].h * self.comp[ci].v]);
                }
            }
        }
        self.bits = Bits::default();
        let (mut mcu, mut expected_rst) = (0usize, 0xd0u8);
        let mut dc = [0i32; 4];
        let mut block_count = 0usize;
        for my in 0..myy {
            for mx in 0..mxx {
                for s in scan.iter().take(n_comp) {
                    let (ci, td, ta) = *s;
                    let (hi, vi) = (self.comp[ci].h, self.comp[ci].v);
                    for j in 0..hi * vi {
                        let (bx, by);
                        if n_comp != 1 {
                            bx = hi * mx + j % hi;
                            by = vi * my + j / hi;
                        } else {
                            let q = mxx * hi;
                            bx = block_count % q;
                            by = block_count / q;
                            block_count += 1;
                            if bx * 8 >= self.width || by * 8 >= self.height {
                                continue;
                            }
                        }
                        let idx = by * mxx * hi + bx;
                        let mut b: Block = if self.progressive { self.prog_coeffs[ci].as_ref().expect("allocated")[idx] } else { [0; 64] };
                        if ah != 0 {
                            self.refine(&mut b, ta, zig_start, zig_end, 1 << al)?;
                        } else {
                            let mut zig = zig_start;
                            if zig == 0 {
                                zig += 1;
                                let value = self.decode_huffman(0, td)?;
                                if value > 16 {
                                    return Err(unsupported("excessive DC component"));
                                }
                                let delta = self.receive_extend(value)?;
                                dc[ci] = dc[ci].wrapping_add(delta);
                                b[0] = dc[ci].wrapping_shl(al);
                            }
                            if zig <= zig_end && self.eob_run > 0 {
                                self.eob_run -= 1;
                            } else {
                                while zig <= zig_end {
                                    let value = self.decode_huffman(1, ta)?;
                                    let (val0, val1) = (value >> 4, value & 0x0f);
                                    if val1 != 0 {
                                        zig += i32::from(val0);
                                        if zig > zig_end {
                                            break;
                                        }
                                        let ac = self.receive_extend(val1)?;
                                        b[UNZIG[zig as usize]] = ac.wrapping_shl(al);
                                    } else {
                                        if val0 != 0x0f {
                                            self.eob_run = 1u16 << val0;
                                            if val0 != 0 {
                                                let bits = self.decode_bits(i32::from(val0))?;
                                                self.eob_run |= bits as u16;
                                            }
                                            self.eob_run = self.eob_run.wrapping_sub(1);
                                            break;
                                        }
                                        zig += 0x0f;
                                    }
                                    zig += 1;
                                }
                            }
                        }
                        if self.progressive {
                            self.prog_coeffs[ci].as_mut().expect("allocated")[idx] = b;
                            continue;
                        }
                        self.reconstruct_block(&mut b, bx, by, ci)?;
                    }
                }
                mcu += 1;
                if self.ri > 0 && mcu % self.ri == 0 && mcu < mxx * myy {
                    let t = self.read_full(2)?;
                    self.tmp[0] = t[0];
                    self.tmp[1] = t[1];
                    if t[0] != 0xff || t[1] != expected_rst {
                        self.find_rst(expected_rst)?;
                    }
                    expected_rst += 1;
                    if expected_rst == 0xd8 {
                        expected_rst = 0xd0;
                    }
                    self.bits = Bits::default();
                    dc = [0; 4];
                    self.eob_run = 0;
                }
            }
        }
        Ok(())
    }

    fn refine(&mut self, b: &mut Block, ta: usize, zig_start: i32, zig_end: i32, delta: i32) -> Result<(), GoErr> {
        if zig_start == 0 {
            if self.decode_bit()? {
                b[0] |= delta;
            }
            return Ok(());
        }
        let mut zig = zig_start;
        if self.eob_run == 0 {
            while zig <= zig_end {
                let mut z = 0i32;
                let value = self.decode_huffman(1, ta)?;
                let (val0, val1) = (value >> 4, value & 0x0f);
                match val1 {
                    0 => {
                        if val0 != 0x0f {
                            self.eob_run = 1u16 << val0;
                            if val0 != 0 {
                                let bits = self.decode_bits(i32::from(val0))?;
                                self.eob_run |= bits as u16;
                            }
                            break;
                        }
                    }
                    1 => {
                        z = delta;
                        if !self.decode_bit()? {
                            z = -z;
                        }
                    }
                    _ => return Err(format_err("unexpected Huffman code")),
                }
                zig = self.refine_non_zeroes(b, zig, zig_end, i32::from(val0), delta)?;
                if zig > zig_end {
                    return Err(format_err("too many coefficients"));
                }
                if z != 0 {
                    b[UNZIG[zig as usize]] = z;
                }
                zig += 1;
            }
        }
        if self.eob_run > 0 {
            self.eob_run -= 1;
            self.refine_non_zeroes(b, zig, zig_end, -1, delta)?;
        }
        Ok(())
    }

    fn refine_non_zeroes(&mut self, b: &mut Block, mut zig: i32, zig_end: i32, mut nz: i32, delta: i32) -> Result<i32, GoErr> {
        while zig <= zig_end {
            let u = UNZIG[zig as usize];
            if b[u] == 0 {
                if nz == 0 {
                    break;
                }
                nz -= 1;
                zig += 1;
                continue;
            }
            if !self.decode_bit()? {
                zig += 1;
                continue;
            }
            if b[u] >= 0 {
                b[u] = b[u].wrapping_add(delta);
            } else {
                b[u] = b[u].wrapping_sub(delta);
            }
            zig += 1;
        }
        Ok(zig)
    }

    fn reconstruct_progressive(&mut self) -> Result<(), GoErr> {
        let mxx = self.width.div_ceil(8 * self.max_h);
        for i in 0..self.n_comp {
            let Some(mut coeffs) = self.prog_coeffs[i].take() else { continue };
            let v = 8 * self.max_v / self.comp[i].v;
            let h = 8 * self.max_h / self.comp[i].h;
            let stride = mxx * self.comp[i].h;
            let mut by = 0;
            while by * v < self.height {
                let mut bx = 0;
                while bx * h < self.width {
                    self.reconstruct_block(&mut coeffs[by * stride + bx], bx, by, i)?;
                    bx += 1;
                }
                by += 1;
            }
            self.prog_coeffs[i] = Some(coeffs);
        }
        Ok(())
    }

    fn reconstruct_block(&mut self, b: &mut Block, mut bx: usize, mut by: usize, ci: usize) -> Result<(), GoErr> {
        let qt = self.quant[usize::from(self.comp[ci].tq)];
        for zig in 0..64 {
            b[UNZIG[zig]] = b[UNZIG[zig]].wrapping_mul(qt[zig]);
        }
        idct(b);
        let (mut h, mut v) = (0, 0);
        if self.flex {
            h = self.comp[ci].expand_h;
            v = self.comp[ci].expand_v;
            bx *= h;
            by *= v;
        }
        let (dst, stride): (&mut Vec<u8>, usize) = if self.n_comp == 1 {
            let (p, s) = self.img1.as_mut().expect("made");
            (p, *s)
        } else {
            let img = self.img3.as_mut().expect("made");
            match ci {
                0 => (&mut img.y, img.y_stride),
                1 => (&mut img.cb, img.c_stride),
                2 => (&mut img.cr, img.c_stride),
                3 => {
                    let (p, s) = self.black.as_mut().expect("made");
                    (p, *s)
                }
                _ => return Err(unsupported("too many components")),
            }
        };
        let base = 8 * (by * stride + bx);
        let clamp = |x: i32| x.wrapping_add(128).clamp(0, 255) as u8;
        if self.flex {
            for y in 0..8 {
                for x in 0..8 {
                    let val = clamp(b[y * 8 + x]);
                    for yy in 0..v {
                        for xx in 0..h {
                            dst[base + (y * v + yy) * stride + x * h + xx] = val;
                        }
                    }
                }
            }
            return Ok(());
        }
        for y in 0..8 {
            for x in 0..8 {
                dst[base + y * stride + x] = clamp(b[y * 8 + x]);
            }
        }
        Ok(())
    }

    fn find_rst(&mut self, expected: u8) -> Result<(), GoErr> {
        loop {
            let mut i = 0;
            if self.tmp[0] == 0xff {
                if self.tmp[1] == expected {
                    return Ok(());
                } else if self.tmp[1] == 0xff {
                    i = 1;
                } else if self.tmp[1] != 0 {
                    return Err(format_err("bad RST marker"));
                }
            } else if self.tmp[1] == 0xff {
                self.tmp[0] = 0xff;
                i = 1;
            }
            let t = self.read_full(2 - i)?;
            self.tmp[i..2].copy_from_slice(&t[..2 - i]);
        }
    }

    fn is_rgb(&self) -> bool {
        if self.jfif {
            return false;
        }
        if self.adobe_valid && self.adobe_transform == 0 {
            return true;
        }
        self.comp[0].c == b'R' && self.comp[1].c == b'G' && self.comp[2].c == b'B'
    }

    /// Go's `decode` up to the end of its marker loop: true when the end-of-image marker ended it (configOnly also stops
    /// at the first scan, or after a JFIF file's frame header).
    fn run(&mut self, config_only: bool) -> Result<bool, GoErr> {
        let t = self.read_full(2)?;
        if t[0] != 0xff || t[1] != 0xd8 {
            return Err(format_err("missing SOI marker"));
        }
        loop {
            let t = self.read_full(2)?;
            let (mut t0, mut t1) = (t[0], t[1]);
            while t0 != 0xff {
                t0 = t1;
                t1 = self.read_byte()?;
            }
            let mut marker = t1;
            if marker == 0 {
                continue;
            }
            while marker == 0xff {
                marker = self.read_byte()?;
            }
            if marker == 0xd9 {
                break;
            }
            if (0xd0..=0xd7).contains(&marker) {
                continue;
            }
            let t = self.read_full(2)?;
            let n = (i64::from(t[0]) << 8) + i64::from(t[1]) - 2;
            if n < 0 {
                return Err(format_err("short segment length"));
            }
            let nu = n as usize;
            match marker {
                0xc0..=0xc2 => {
                    self.baseline = marker == 0xc0;
                    self.progressive = marker == 0xc2;
                    let r = self.process_sof(nu);
                    if config_only && self.jfif {
                        return r.map(|()| false);
                    }
                    r?;
                }
                0xc4 if config_only => self.ignore(nu)?,
                0xc4 => self.process_dht(n)?,
                0xdb if config_only => self.ignore(nu)?,
                0xdb => self.process_dqt(n)?,
                0xda if config_only => return Ok(false),
                0xda => self.process_sos(nu)?,
                0xdd if config_only => self.ignore(nu)?,
                0xdd => {
                    if nu != 2 {
                        return Err(format_err("DRI has wrong length"));
                    }
                    let t = self.read_full(2)?;
                    self.ri = usize::from(t[0]) << 8 | usize::from(t[1]);
                }
                0xe0 => {
                    if nu < 5 {
                        self.ignore(nu)?;
                    } else {
                        let t = self.read_full(5)?;
                        self.jfif = &t[..5] == b"JFIF\x00";
                        self.ignore(nu - 5)?;
                    }
                }
                0xee => {
                    if nu < 12 {
                        self.ignore(nu)?;
                    } else {
                        let t = self.read_full(12)?;
                        if &t[..5] == b"Adobe" {
                            self.adobe_valid = true;
                            self.adobe_transform = t[11];
                        }
                        self.ignore(nu - 12)?;
                    }
                }
                0xe1..=0xef | 0xfe => self.ignore(nu)?,
                m if m < 0xc0 => return Err(format_err("unknown marker")),
                _ => return Err(unsupported("unknown marker")),
            }
        }
        Ok(true)
    }

    /// Go's `jpeg.DecodeConfig`: (width, height).
    pub fn decode_config(mut self) -> Result<(usize, usize), GoErr> {
        if self.run(true)? {
            // Go's decode goes on to its end, where no scan was decoded
            return Err(format_err("missing SOS marker"));
        }
        if self.n_comp == 0 {
            return Err(format_err("missing SOF marker"));
        }
        Ok((self.width, self.height))
    }

    /// Go's `jpeg.Decode`, as the pixels DecodeImage reads from each image type.
    pub fn decode(mut self) -> Result<Img, GoErr> {
        self.run(false)?;
        if self.progressive {
            self.reconstruct_progressive()?;
        }
        let (w, h) = (self.width, self.height);
        if let Some((pix, stride)) = &self.img1 {
            let mut img = Img::new(Kind::Gray, 0, 0, w, h);
            for y in 0..h {
                img.pix[y * w..y * w + w].copy_from_slice(&pix[y * stride..y * stride + w]);
            }
            return Ok(img);
        }
        let Some(ycc) = &self.img3 else {
            return Err(format_err("missing SOS marker"));
        };
        let mut img = Img::new(Kind::Nrgba, 0, 0, w, h);
        if let Some((black, bstride)) = &self.black {
            if !self.adobe_valid {
                return Err(unsupported("unknown color model: 4-component JPEG doesn't have Adobe APP14 metadata"));
            }
            for y in 0..h {
                for x in 0..w {
                    let (c, m, yel, k) = if self.adobe_transform != 0 {
                        let (r, g, b) = ycbcr_to_rgb(ycc.y[y * ycc.y_stride + x], ycc.cb[ycc.c_offset(x, y)], ycc.cr[ycc.c_offset(x, y)]);
                        (r, g, b, 255 - black[y * bstride + x])
                    } else {
                        let sub = |t: usize| self.comp[t].h != self.comp[0].h || self.comp[t].v != self.comp[0].v;
                        let at = |t: usize, src: &[u8], stride: usize| {
                            let (sx, sy) = if sub(t) { (x / 2, y / 2) } else { (x, y) };
                            255 - src[sy * stride + sx]
                        };
                        (at(0, &ycc.y, ycc.y_stride), at(1, &ycc.cb, ycc.c_stride), at(2, &ycc.cr, ycc.c_stride), at(3, black, *bstride))
                    };
                    let wk = 0xffff - u32::from(k) * 0x101;
                    let f = |v: u8| (((0xffff - u32::from(v) * 0x101) * wk / 0xffff) >> 8) as u8;
                    img.set(x, y, &[f(c), f(m), f(yel), 0xff]);
                }
            }
            return Ok(img);
        }
        if self.is_rgb() {
            let (h0, h1, h2) = (self.comp[0].h, self.comp[1].h, self.comp[2].h);
            let (v0, v1, v2) = (self.comp[0].v, self.comp[1].v, self.comp[2].v);
            if h1 != h2 || h0 % h1 != 0 || v1 != v2 || v0 % v1 != 0 {
                return Err(ratio_err());
            }
            let c_scale = h0 / h1;
            let mut img = Img::new(Kind::Rgba, 0, 0, w, h);
            for y in 0..h {
                let co = ycc.c_offset(0, y);
                for i in 0..w {
                    img.set(i, y, &[ycc.y[y * ycc.y_stride + i], ycc.cb[co + i / c_scale], ycc.cr[co + i / c_scale], 255]);
                }
            }
            return Ok(img);
        }
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = ycbcr_rgba(ycc.y[y * ycc.y_stride + x], ycc.cb[ycc.c_offset(x, y)], ycc.cr[ycc.c_offset(x, y)]);
                img.set(x, y, &[(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8, 0xff]);
            }
        }
        Ok(img)
    }
}

/// color.YCbCr's `RGBA()` (16 bits per channel).
fn ycbcr_rgba(y: u8, cb: u8, cr: u8) -> (u32, u32, u32) {
    let yy1 = i32::from(y) * 0x10101;
    let (cb1, cr1) = (i32::from(cb) - 128, i32::from(cr) - 128);
    let fix = |v: i32| if (v as u32) & 0xff00_0000 == 0 { (v >> 8) as u32 } else { (!(v >> 31) & 0xffff) as u32 };
    (fix(yy1 + 91881 * cr1), fix(yy1 - 22554 * cb1 - 46802 * cr1), fix(yy1 + 116130 * cb1))
}

/// color's `YCbCrToRGB` (8 bits), as imageutil.DrawYCbCr inlines it.
fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
    let yy1 = i32::from(y) * 0x10101;
    let (cb1, cr1) = (i32::from(cb) - 128, i32::from(cr) - 128);
    let fix = |v: i32| if (v as u32) & 0xff00_0000 == 0 { (v >> 16) as u8 } else { !(v >> 31) as u8 };
    (fix(yy1 + 91881 * cr1), fix(yy1 - 22554 * cb1 - 46802 * cr1), fix(yy1 + 116130 * cb1))
}

const fn fix(x: u64, bits: u32) -> i32 {
    ((x + (1 << (59 - bits))) >> (60 - bits)) as i32
}

const COS1: u64 = 1130768441178740757;
const SIN1: u64 = 224923827593068887;
const COS3: u64 = 958619196450722178;
const SIN3: u64 = 640528868967736374;
const SQRT2INV: u64 = 815238614083298888;
const SQRT2INV_COS6: u64 = 311978311033955632;
const SQRT2INV_SIN6: u64 = 753182269664427492;

fn dct_box(x0: i32, x1: i32, kcos: i32, ksin: i32) -> (i32, i32) {
    let ksum = kcos.wrapping_mul(x0.wrapping_add(x1));
    (ksum.wrapping_add(ksin.wrapping_sub(kcos).wrapping_mul(x1)), ksum.wrapping_sub(kcos.wrapping_add(ksin).wrapping_mul(x0)))
}

/// Go 1.27's integer IDCT (image/jpeg/dct.go), rows then columns, with int32 wrapping as Go's.
fn idct(b: &mut Block) {
    for i in 0..8 {
        let x = &mut b[8 * i..8 * i + 8];
        let (mut x0, mut x7, mut x2, mut x5, mut x1, mut x6, mut x3, mut x4) = (x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]);
        x0 = x0.wrapping_shl(17);
        x1 = x1.wrapping_shl(17);
        (x0, x1) = (x0.wrapping_add(x1), x0.wrapping_sub(x1));
        (x2, x3) = dct_box(x2, x3, fix(SQRT2INV_COS6, 18), -fix(SQRT2INV_SIN6, 18));
        (x1, x2) = (x1.wrapping_add(x2), x1.wrapping_sub(x2));
        (x0, x3) = (x0.wrapping_add(x3), x0.wrapping_sub(x3));
        x4 = x4.wrapping_shl(7);
        x7 = x7.wrapping_shl(7);
        (x7, x4) = (x7.wrapping_add(x4), x7.wrapping_sub(x4));
        x6 = x6.wrapping_mul(fix(SQRT2INV, 8));
        x5 = x5.wrapping_mul(fix(SQRT2INV, 8));
        (x7, x5) = (x7.wrapping_add(x5), x7.wrapping_sub(x5));
        (x4, x6) = (x4.wrapping_add(x6), x4.wrapping_sub(x6));
        (x4, x7) = dct_box(x4 >> 2, x7 >> 2, fix(COS3, 12), -fix(SIN3, 12));
        (x5, x6) = dct_box(x5 >> 2, x6 >> 2, fix(COS1, 12), -fix(SIN1, 12));
        (x0, x7) = (x0.wrapping_add(x7), x0.wrapping_sub(x7));
        (x1, x6) = (x1.wrapping_add(x6), x1.wrapping_sub(x6));
        (x2, x5) = (x2.wrapping_add(x5), x2.wrapping_sub(x5));
        (x3, x4) = (x3.wrapping_add(x4), x3.wrapping_sub(x4));
        x.copy_from_slice(&[x0, x1, x2, x3, x4, x5, x6, x7]);
    }
    for i in 0..8 {
        let g = |r: usize| b[r * 8 + i];
        let (mut x0, mut x7, mut x2, mut x5, mut x1, mut x6, mut x3, mut x4) = (g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7));
        x0 = x0.wrapping_add(1 << 19);
        (x0, x1) = (x0.wrapping_add(x1) >> 2, x0.wrapping_sub(x1) >> 2);
        (x2, x3) = dct_box(x2 >> 13, x3 >> 13, fix(SQRT2INV_COS6, 12), -fix(SQRT2INV_SIN6, 12));
        (x1, x2) = (x1.wrapping_add(x2), x1.wrapping_sub(x2));
        (x0, x3) = (x0.wrapping_add(x3), x0.wrapping_sub(x3));
        (x7, x4) = (x7.wrapping_add(x4), x7.wrapping_sub(x4));
        x5 = (x5 >> 13).wrapping_mul(fix(SQRT2INV, 14));
        x6 = (x6 >> 13).wrapping_mul(fix(SQRT2INV, 14));
        (x7, x5) = (x7.wrapping_add(x5), x7.wrapping_sub(x5));
        (x4, x6) = (x4.wrapping_add(x6), x4.wrapping_sub(x6));
        (x4, x7) = dct_box(x4 >> 14, x7 >> 14, fix(COS3, 12), -fix(SIN3, 12));
        (x5, x6) = dct_box(x5 >> 14, x6 >> 14, fix(COS1, 12), -fix(SIN1, 12));
        (x0, x7) = (x0.wrapping_add(x7), x0.wrapping_sub(x7));
        (x1, x6) = (x1.wrapping_add(x6), x1.wrapping_sub(x6));
        (x2, x5) = (x2.wrapping_add(x5), x2.wrapping_sub(x5));
        (x3, x4) = (x3.wrapping_add(x4), x3.wrapping_sub(x4));
        for (r, v) in [x0, x1, x2, x3, x4, x5, x6, x7].into_iter().enumerate() {
            b[r * 8 + i] = v >> 18;
        }
    }
}
