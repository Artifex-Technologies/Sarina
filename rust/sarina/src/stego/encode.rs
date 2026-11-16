//! Go's image encoders for an `*image.NRGBA`, as `Embed` and `CleanImage` use them: image/png's (its row filters
//! chosen as Go chooses, zlib at the default level through [`super::deflate`], IDAT chunks cut by its 32 KiB bufio)
//! and image/jpeg's (4:2:0 at a quality, its forward DCT and tables), byte for byte.

use super::deflate::ZlibWriter;
use super::enc_tables::*;

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = !0u32;
    for p in parts {
        for &x in *p {
            c ^= u32::from(x);
            for _ in 0..8 {
                c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
    }
    !c
}

fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[name, data]).to_be_bytes());
}

fn abs8(d: u8) -> usize {
    if d < 128 {
        usize::from(d)
    } else {
        256 - usize::from(d)
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let pc = i32::from(c);
    let pa = i32::from(b) - pc;
    let pb = i32::from(a) - pc;
    let pc = (pa + pb).abs();
    let (pa, pb) = (pa.abs(), pb.abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Go's png `filter`: which of the five filtered rows (`cr[1..]`, computed here) to write.
fn filter(cr: &mut [Vec<u8>; 5], pr: &[u8], bpp: usize) -> usize {
    let n = cr[0].len() - 1;
    let (c0, rest) = cr.split_at_mut(1);
    let cdat0 = &c0[0][1..];
    let pdat = &pr[1..];
    let (c1, rest) = rest.split_at_mut(1);
    let (c2, rest) = rest.split_at_mut(1);
    let (c3, c4) = rest.split_at_mut(1);
    let (cdat1, cdat2, cdat3, cdat4) = (&mut c1[0][1..], &mut c2[0][1..], &mut c3[0][1..], &mut c4[0][1..]);
    let mut sum = 0;
    for i in 0..n {
        cdat2[i] = cdat0[i].wrapping_sub(pdat[i]);
        sum += abs8(cdat2[i]);
    }
    let mut best = sum;
    let mut f = 2;
    sum = 0;
    for i in 0..bpp.min(n) {
        cdat4[i] = cdat0[i].wrapping_sub(pdat[i]);
        sum += abs8(cdat4[i]);
    }
    for i in bpp..n {
        cdat4[i] = cdat0[i].wrapping_sub(paeth(cdat0[i - bpp], pdat[i], pdat[i - bpp]));
        sum += abs8(cdat4[i]);
        if sum >= best {
            break;
        }
    }
    if sum < best {
        best = sum;
        f = 4;
    }
    sum = 0;
    for &c in cdat0.iter() {
        sum += abs8(c);
        if sum >= best {
            break;
        }
    }
    if sum < best {
        best = sum;
        f = 0;
    }
    sum = 0;
    for i in 0..bpp.min(n) {
        cdat1[i] = cdat0[i];
        sum += abs8(cdat1[i]);
    }
    for i in bpp..n {
        cdat1[i] = cdat0[i].wrapping_sub(cdat0[i - bpp]);
        sum += abs8(cdat1[i]);
        if sum >= best {
            break;
        }
    }
    if sum < best {
        best = sum;
        f = 1;
    }
    sum = 0;
    for i in 0..bpp.min(n) {
        cdat3[i] = cdat0[i].wrapping_sub(pdat[i] / 2);
        sum += abs8(cdat3[i]);
    }
    for i in bpp..n {
        cdat3[i] = cdat0[i].wrapping_sub(((u16::from(cdat0[i - bpp]) + u16::from(pdat[i])) / 2) as u8);
        sum += abs8(cdat3[i]);
        if sum >= best {
            break;
        }
    }
    if sum < best {
        f = 3;
    }
    f
}

/// Go's `png.Encode` of an NRGBA picture (w x h, 4 bytes a pixel): RGB when every alpha is 255, RGBA otherwise.
pub fn png_nrgba(w: usize, h: usize, pix: &[u8]) -> Vec<u8> {
    let opaque = pix.chunks(4).all(|p| p[3] == 0xff);
    let bpp = if opaque { 3 } else { 4 };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if opaque { 2 } else { 6 }, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let sz = 1 + bpp * w;
    let mut cr: [Vec<u8>; 5] = std::array::from_fn(|i| {
        let mut v = vec![0u8; sz];
        v[0] = i as u8;
        v
    });
    let mut pr = vec![0u8; sz];
    let mut z = ZlibWriter::new();
    for y in 0..h {
        let row = &pix[y * 4 * w..(y + 1) * 4 * w];
        if opaque {
            for (x, p) in row.chunks(4).enumerate() {
                cr[0][1 + 3 * x..4 + 3 * x].copy_from_slice(&p[..3]);
            }
        } else {
            cr[0][1..].copy_from_slice(row);
        }
        let f = filter(&mut cr, &pr, bpp);
        z.write(&cr[f]);
        std::mem::swap(&mut pr, &mut cr[0]);
    }
    // the zlib stream through Go's bufio.Writer of 32 KiB: each Flush, or each large write into an empty buffer, is an
    // IDAT chunk
    const SIZE: usize = 1 << 15;
    let mut buf: Vec<u8> = Vec::with_capacity(SIZE);
    for piece in z.close() {
        let mut p = &piece[..];
        while p.len() > SIZE - buf.len() {
            if buf.is_empty() {
                chunk(&mut out, b"IDAT", p);
                p = &[];
            } else {
                let n = SIZE - buf.len();
                buf.extend_from_slice(&p[..n]);
                chunk(&mut out, b"IDAT", &buf);
                buf.clear();
                p = &p[n..];
            }
        }
        buf.extend_from_slice(p);
    }
    if !buf.is_empty() {
        chunk(&mut out, b"IDAT", &buf);
    }
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Go's jpeg `encoder`: the entropy-coded output with its 0xff stuffing.
struct Jpeg {
    out: Vec<u8>,
    bits: u32,
    n_bits: u32,
    quant: [[u8; 64]; 2],
    luts: [Vec<u32>; 4],
}

const UNZIG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56,
    57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

fn spec(i: usize) -> (&'static [u8; 16], &'static [u8]) {
    match i {
        0 => (&HUFF_COUNT_0, &HUFF_VALUE_0),
        1 => (&HUFF_COUNT_1, &HUFF_VALUE_1),
        2 => (&HUFF_COUNT_2, &HUFF_VALUE_2),
        _ => (&HUFF_COUNT_3, &HUFF_VALUE_3),
    }
}

fn lut(i: usize) -> Vec<u32> {
    let (count, value) = spec(i);
    let max = value.iter().copied().max().map_or(0, usize::from);
    let mut h = vec![0u32; max + 1];
    let (mut code, mut k) = (0u32, 0);
    for (i, &c) in count.iter().enumerate() {
        let n_bits = (i as u32 + 1) << 24;
        for _ in 0..c {
            h[usize::from(value[k])] = n_bits | code;
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    h
}

fn div(a: i32, b: i32) -> i32 {
    if a >= 0 {
        (a + (b >> 1)) / b
    } else {
        -((-a + (b >> 1)) / b)
    }
}

const fn fix(x: u64, bits: u32) -> i32 {
    ((x + (1 << (59 - bits))) >> (60 - bits)) as i32
}

const COS1: u64 = 1130768441178740757;
const SIN1: u64 = 224923827593068887;
const COS3: u64 = 958619196450722178;
const SIN3: u64 = 640528868967736374;
const SQRT2: u64 = 1630477228166597777;
const SQRT2_COS6: u64 = 623956622067911264;
const SQRT2_SIN6: u64 = 1506364539328854985;

fn dct_box(x0: i32, x1: i32, kcos: i32, ksin: i32) -> (i32, i32) {
    let ksum = kcos.wrapping_mul(x0.wrapping_add(x1));
    (ksum.wrapping_add(ksin.wrapping_sub(kcos).wrapping_mul(x1)), ksum.wrapping_sub(kcos.wrapping_add(ksin).wrapping_mul(x0)))
}

/// Go 1.27's forward DCT (image/jpeg/dct.go): columns, then rows.
fn fdct(b: &mut [i32; 64]) {
    for i in 0..8 {
        let g = |r: usize| b[r * 8 + i];
        let (mut x0, mut x1, mut x2, mut x3, mut x4, mut x5, mut x6, mut x7) = (g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7));
        (x0, x7) = (x0.wrapping_add(x7), x0.wrapping_sub(x7));
        (x1, x6) = (x1.wrapping_add(x6), x1.wrapping_sub(x6));
        (x2, x5) = (x2.wrapping_add(x5), x2.wrapping_sub(x5));
        (x3, x4) = (x3.wrapping_add(x4), x3.wrapping_sub(x4));
        (x4, x7) = dct_box(x4, x7, fix(COS3, 18), fix(SIN3, 18));
        (x5, x6) = dct_box(x5, x6, fix(COS1, 18), fix(SIN1, 18));
        (x0, x3) = (x0.wrapping_add(x3), x0.wrapping_sub(x3));
        (x1, x2) = (x1.wrapping_add(x2), x1.wrapping_sub(x2));
        (x2, x3) = dct_box(x2, x3, fix(SQRT2_COS6, 18), fix(SQRT2_SIN6, 18));
        (x0, x1) = (x0.wrapping_add(x1), x0.wrapping_sub(x1));
        b[i] = (x0.wrapping_sub(128 * 8)).wrapping_shl(18);
        b[4 * 8 + i] = x1.wrapping_shl(18);
        b[2 * 8 + i] = x2;
        b[6 * 8 + i] = x3;
        (x4, x6) = (x4.wrapping_add(x6), x4.wrapping_sub(x6));
        (x7, x5) = (x7.wrapping_add(x5), x7.wrapping_sub(x5));
        x5 = (x5 >> 12).wrapping_mul(fix(SQRT2, 12));
        x6 = (x6 >> 12).wrapping_mul(fix(SQRT2, 12));
        (x7, x4) = (x7.wrapping_add(x4), x7.wrapping_sub(x4));
        b[8 + i] = x7;
        b[3 * 8 + i] = x5;
        b[5 * 8 + i] = x6;
        b[7 * 8 + i] = x4;
    }
    for i in 0..8 {
        let x = &mut b[8 * i..8 * i + 8];
        let (mut x0, mut x1, mut x2, mut x3, mut x4, mut x5, mut x6, mut x7) = (x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]);
        (x0, x7) = (x0.wrapping_add(x7), x0.wrapping_sub(x7));
        (x1, x6) = (x1.wrapping_add(x6), x1.wrapping_sub(x6));
        (x2, x5) = (x2.wrapping_add(x5), x2.wrapping_sub(x5));
        (x3, x4) = (x3.wrapping_add(x4), x3.wrapping_sub(x4));
        (x4, x7) = dct_box(x4 >> 14, x7 >> 14, fix(COS3, 14), fix(SIN3, 14));
        (x5, x6) = dct_box(x5 >> 14, x6 >> 14, fix(COS1, 14), fix(SIN1, 14));
        (x0, x3) = (x0.wrapping_add(x3), x0.wrapping_sub(x3));
        (x1, x2) = (x1.wrapping_add(x2), x1.wrapping_sub(x2));
        (x2, x3) = dct_box(x2 >> 14, x3 >> 14, fix(SQRT2_COS6, 14), fix(SQRT2_SIN6, 14));
        (x0, x1) = (x0.wrapping_add(x1), x0.wrapping_sub(x1));
        (x4, x6) = (x4.wrapping_add(x6), x4.wrapping_sub(x6));
        (x7, x5) = (x7.wrapping_add(x5), x7.wrapping_sub(x5));
        x5 = (x5 >> 14).wrapping_mul(fix(SQRT2, 14));
        x6 = (x6 >> 14).wrapping_mul(fix(SQRT2, 14));
        (x7, x4) = (x7.wrapping_add(x4), x7.wrapping_sub(x4));
        let r = |v: i32| v.wrapping_add(1 << 17) >> 18;
        x.copy_from_slice(&[r(x0), r(x7), r(x2), r(x5), r(x1), r(x6), r(x3), r(x4)]);
    }
}

impl Jpeg {
    fn emit(&mut self, bits: u32, n_bits: u32) {
        let n_bits = n_bits + self.n_bits;
        let mut bits = (bits << (32 - n_bits)) | self.bits;
        let mut n = n_bits;
        while n >= 8 {
            let b = (bits >> 24) as u8;
            self.out.push(b);
            if b == 0xff {
                self.out.push(0);
            }
            bits <<= 8;
            n -= 8;
        }
        self.bits = bits;
        self.n_bits = n;
    }

    fn emit_huff(&mut self, h: usize, value: i32) {
        let x = self.luts[h][value as usize];
        self.emit(x & ((1 << 24) - 1), x >> 24);
    }

    fn emit_huff_rle(&mut self, h: usize, run: i32, value: i32) {
        let (a, b) = if value < 0 { (-value, value - 1) } else { (value, value) };
        let n_bits = if a < 0x100 { u32::from(BIT_COUNT[a as usize]) } else { 8 + u32::from(BIT_COUNT[(a >> 8) as usize]) };
        self.emit_huff(h, run << 4 | n_bits as i32);
        if n_bits > 0 {
            self.emit(b as u32 & ((1 << n_bits) - 1), n_bits);
        }
    }

    fn write_block(&mut self, b: &mut [i32; 64], q: usize, prev_dc: i32) -> i32 {
        fdct(b);
        let dc = div(b[0], 8 * i32::from(self.quant[q][0]));
        self.emit_huff_rle(2 * q, 0, dc - prev_dc);
        let h = 2 * q + 1;
        let mut run = 0;
        for zig in 1..64 {
            let ac = div(b[UNZIG[zig]], 8 * i32::from(self.quant[q][zig]));
            if ac == 0 {
                run += 1;
            } else {
                while run > 15 {
                    self.emit_huff(h, 0xf0);
                    run -= 16;
                }
                self.emit_huff_rle(h, run, ac);
                run = 0;
            }
        }
        if run > 0 {
            self.emit_huff(h, 0);
        }
        dc
    }
}

/// color's `RGBToYCbCr`.
fn rgb_to_ycbcr(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let (r1, g1, b1) = (i32::from(r), i32::from(g), i32::from(b));
    let yy = (19595 * r1 + 38470 * g1 + 7471 * b1 + (1 << 15)) >> 16;
    let fix = |v: i32| if (v as u32) & 0xff00_0000 == 0 { (v >> 16) as u8 } else { !(v >> 31) as u8 };
    (yy as u8, fix(-11056 * r1 - 21712 * g1 + 32768 * b1 + (257 << 15)), fix(32768 * r1 - 27440 * g1 - 5328 * b1 + (257 << 15)))
}

/// Go's `jpeg.Encode` of an NRGBA picture at a quality (1-100), through the generic `toYCbCr` (each pixel's
/// premultiplied colour).
pub fn jpeg_nrgba(w: usize, h: usize, pix: &[u8], quality: i32) -> Vec<u8> {
    let quality = quality.clamp(1, 100);
    let scale = if quality < 50 { 5000 / quality } else { 200 - quality * 2 };
    let mut e = Jpeg { out: vec![0xff, 0xd8], bits: 0, n_bits: 0, quant: [[0; 64]; 2], luts: std::array::from_fn(lut) };
    for (i, q) in [&UNSCALED_QUANT_0, &UNSCALED_QUANT_1].iter().enumerate() {
        for j in 0..64 {
            e.quant[i][j] = ((i32::from(q[j]) * scale + 50) / 100).clamp(1, 255) as u8;
        }
    }
    let marker = |out: &mut Vec<u8>, m: u8, len: usize| out.extend_from_slice(&[0xff, m, (len >> 8) as u8, (len & 0xff) as u8]);
    marker(&mut e.out, 0xdb, 2 + 2 * 65);
    for i in 0..2 {
        e.out.push(i as u8);
        let q = e.quant[i];
        e.out.extend_from_slice(&q);
    }
    marker(&mut e.out, 0xc0, 8 + 3 * 3);
    e.out.extend_from_slice(&[8, (h >> 8) as u8, (h & 0xff) as u8, (w >> 8) as u8, (w & 0xff) as u8, 3]);
    for i in 0..3 {
        e.out.extend_from_slice(&[i as u8 + 1, [0x22, 0x11, 0x11][i], [0, 1, 1][i]]);
    }
    let dht_len = 2 + (0..4).map(|i| 1 + 16 + spec(i).1.len()).sum::<usize>();
    marker(&mut e.out, 0xc4, dht_len);
    for i in 0..4 {
        e.out.push([0x00, 0x10, 0x01, 0x11][i]);
        let (c, v) = spec(i);
        e.out.extend_from_slice(c);
        e.out.extend_from_slice(v);
    }
    e.out.extend_from_slice(&[0xff, 0xda, 0x00, 0x0c, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00]);
    // each pixel as `m.At(x, y).RGBA() >> 8` of an NRGBA: the colour premultiplied by its alpha
    let at = |x: usize, y: usize| {
        let p = &pix[(y * w + x) * 4..(y * w + x) * 4 + 4];
        let a = u32::from(p[3]) * 0x101;
        let f = |c: u8| ((u32::from(c) * 0x101 * a / 0xffff) >> 8) as u8;
        (f(p[0]), f(p[1]), f(p[2]))
    };
    let (mut prev_y, mut prev_cb, mut prev_cr) = (0, 0, 0);
    let mut b = [0i32; 64];
    let mut cb = [[0i32; 64]; 4];
    let mut cr = [[0i32; 64]; 4];
    for y in (0..h).step_by(16) {
        for x in (0..w).step_by(16) {
            for i in 0..4 {
                let (px, py) = (x + (i & 1) * 8, y + (i & 2) * 4);
                for j in 0..8 {
                    for k in 0..8 {
                        let (r, g, bl) = at((px + k).min(w - 1), (py + j).min(h - 1));
                        let (yy, ccb, ccr) = rgb_to_ycbcr(r, g, bl);
                        b[8 * j + k] = i32::from(yy);
                        cb[i][8 * j + k] = i32::from(ccb);
                        cr[i][8 * j + k] = i32::from(ccr);
                    }
                }
                prev_y = e.write_block(&mut b, 0, prev_y);
            }
            scale_blocks(&mut b, &cb);
            prev_cb = e.write_block(&mut b, 1, prev_cb);
            scale_blocks(&mut b, &cr);
            prev_cr = e.write_block(&mut b, 1, prev_cr);
        }
    }
    e.emit(0x7f, 7);
    e.out.extend_from_slice(&[0xff, 0xd9]);
    e.out
}

fn scale_blocks(dst: &mut [i32; 64], src: &[[i32; 64]; 4]) {
    for (i, s) in src.iter().enumerate() {
        let off = (i & 2) << 4 | (i & 1) << 2;
        for y in 0..4 {
            for x in 0..4 {
                let j = 16 * y + 2 * x;
                dst[8 * y + x + off] = (s[j] + s[j + 1] + s[j + 8] + s[j + 9] + 2) >> 2;
            }
        }
    }
}
