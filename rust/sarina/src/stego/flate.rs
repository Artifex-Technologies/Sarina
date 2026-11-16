//! Go's `compress/flate` decompressor and `compress/zlib` reader, written out from Go's source so that what is read
//! from the source, when, and every error (with its offset) are Go's: PNG's IDAT reader and its two bufio layers see the
//! same reads as Go's would.

use super::goio::{read_full, GoByteRead, GoErr, GoRead};

/// What flate and zlib read from: Go's `flate.Reader`.
pub trait Source: GoRead + GoByteRead {}
impl<T: GoRead + GoByteRead> Source for T {}

const CHUNK_BITS: u32 = 9;
const NUM_CHUNKS: usize = 1 << CHUNK_BITS;
const COUNT_MASK: u32 = 15;
const VALUE_SHIFT: u32 = 4;

/// Go's `huffmanDecoder`.
#[derive(Clone)]
struct Huffman {
    min: u32,
    chunks: Vec<u32>,
    links: Vec<Vec<u32>>,
    link_mask: u32,
}

impl Huffman {
    fn empty() -> Huffman {
        Huffman { min: 0, chunks: vec![0; NUM_CHUNKS], links: Vec::new(), link_mask: 0 }
    }

    /// Go's `init`: false for an over-subscribed or incomplete code.
    fn init(&mut self, lengths: &[u32]) -> bool {
        if self.min != 0 {
            *self = Huffman::empty();
        }
        let mut count = [0usize; 16];
        let (mut min, mut max) = (0u32, 0u32);
        for &n in lengths {
            if n == 0 {
                continue;
            }
            if min == 0 || n < min {
                min = n;
            }
            max = max.max(n);
            count[n as usize] += 1;
        }
        if max == 0 {
            return true;
        }
        let mut code = 0usize;
        let mut nextcode = [0usize; 16];
        for i in min..=max {
            code <<= 1;
            nextcode[i as usize] = code;
            code += count[i as usize];
        }
        if code != 1 << max && !(code == 1 && max == 1) {
            return false;
        }
        self.min = min;
        if max > CHUNK_BITS {
            let num_links = 1usize << (max - CHUNK_BITS);
            self.link_mask = (num_links - 1) as u32;
            let link = nextcode[CHUNK_BITS as usize + 1] >> 1;
            self.links = vec![Vec::new(); NUM_CHUNKS - link];
            for j in link..NUM_CHUNKS {
                let reverse = ((j as u16).reverse_bits() >> (16 - CHUNK_BITS)) as usize;
                let off = j - link;
                self.chunks[reverse] = (off << VALUE_SHIFT) as u32 | (CHUNK_BITS + 1);
                self.links[off] = vec![0; num_links];
            }
        }
        for (i, &n) in lengths.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let code = nextcode[n as usize];
            nextcode[n as usize] += 1;
            let chunk = (i << VALUE_SHIFT) as u32 | n;
            let reverse = ((code as u16).reverse_bits() >> (16 - n)) as usize;
            if n <= CHUNK_BITS {
                let mut off = reverse;
                while off < NUM_CHUNKS {
                    self.chunks[off] = chunk;
                    off += 1 << n;
                }
            } else {
                let j = reverse & (NUM_CHUNKS - 1);
                let value = (self.chunks[j] >> VALUE_SHIFT) as usize;
                let linktab = &mut self.links[value];
                let mut off = reverse >> CHUNK_BITS;
                while off < linktab.len() {
                    linktab[off] = chunk;
                    off += 1 << (n - CHUNK_BITS);
                }
            }
        }
        true
    }
}

fn fixed() -> Huffman {
    let mut bits = [8u32; 288];
    bits[144..256].fill(9);
    bits[256..280].fill(7);
    let mut h = Huffman::empty();
    h.init(&bits);
    h
}

fn corrupt(offset: i64) -> GoErr {
    GoErr::Msg(format!("flate: corrupt input before offset {offset}"))
}

fn no_eof(e: GoErr) -> GoErr {
    if e == GoErr::Eof {
        GoErr::UnexpectedEof
    } else {
        e
    }
}

const WINDOW: usize = 32768;
const CODE_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

#[derive(Clone, Copy, PartialEq)]
enum Step {
    NextBlock,
    Huffman,
    CopyData,
}

/// Go's `decompressor` (flate.NewReader).
pub struct Flate<S> {
    pub r: S,
    roffset: i64,
    b: u32,
    nb: u32,
    h1: Huffman,
    h2: Huffman,
    fixed: Huffman,
    /// which decoder the block uses: fixed, or the block's own (with or without distance codes)
    use_fixed: bool,
    bits: Vec<u32>,
    codebits: [u32; 19],
    hist: Vec<u8>,
    wr: usize,
    rd: usize,
    full: bool,
    to_read: (usize, usize),
    final_block: bool,
    err: Option<GoErr>,
    step: Step,
    state_dict: bool,
    copy_len: usize,
    copy_dist: usize,
}

impl<S: Source> Flate<S> {
    pub fn new(r: S) -> Flate<S> {
        Flate {
            r,
            roffset: 0,
            b: 0,
            nb: 0,
            h1: Huffman::empty(),
            h2: Huffman::empty(),
            fixed: fixed(),
            use_fixed: false,
            bits: vec![0; 286 + 30],
            codebits: [0; 19],
            hist: vec![0; WINDOW],
            wr: 0,
            rd: 0,
            full: false,
            to_read: (0, 0),
            final_block: false,
            err: None,
            step: Step::NextBlock,
            state_dict: false,
            copy_len: 0,
            copy_dist: 0,
        }
    }

    fn read_flush(&mut self) {
        self.to_read = (self.rd, self.wr);
        self.rd = self.wr;
        if self.wr == WINDOW {
            self.wr = 0;
            self.rd = 0;
            self.full = true;
        }
    }

    fn hist_size(&self) -> usize {
        if self.full {
            WINDOW
        } else {
            self.wr
        }
    }

    fn more_bits(&mut self) -> Result<(), GoErr> {
        let c = self.r.read_byte().map_err(no_eof)?;
        self.roffset += 1;
        self.b |= u32::from(c) << self.nb;
        self.nb += 8;
        Ok(())
    }

    fn need(&mut self, n: u32) -> Result<(), GoErr> {
        while self.nb < n {
            self.more_bits()?;
        }
        Ok(())
    }

    fn huff_sym(&mut self, which: u8) -> Result<u32, GoErr> {
        let h = match which {
            0 => &self.fixed,
            1 => &self.h1,
            _ => &self.h2,
        };
        let mut n = h.min;
        let (mut nb, mut b) = (self.nb, self.b);
        loop {
            while nb < n {
                let c = match self.r.read_byte() {
                    Ok(c) => c,
                    Err(e) => {
                        self.b = b;
                        self.nb = nb;
                        return Err(no_eof(e));
                    }
                };
                self.roffset += 1;
                b |= u32::from(c) << (nb & 31);
                nb += 8;
            }
            let h = match which {
                0 => &self.fixed,
                1 => &self.h1,
                _ => &self.h2,
            };
            let mut chunk = h.chunks[(b & (NUM_CHUNKS as u32 - 1)) as usize];
            n = chunk & COUNT_MASK;
            if n > CHUNK_BITS {
                chunk = h.links[(chunk >> VALUE_SHIFT) as usize][((b >> CHUNK_BITS) & h.link_mask) as usize];
                n = chunk & COUNT_MASK;
            }
            if n <= nb {
                if n == 0 {
                    self.b = b;
                    self.nb = nb;
                    let e = corrupt(self.roffset);
                    self.err = Some(e.clone());
                    return Err(e);
                }
                self.b = b >> (n & 31);
                self.nb = nb - n;
                return Ok(chunk >> VALUE_SHIFT);
            }
        }
    }

    fn next_block(&mut self) {
        if let Err(e) = self.need(3) {
            self.err = Some(e);
            return;
        }
        self.final_block = self.b & 1 == 1;
        self.b >>= 1;
        let typ = self.b & 3;
        self.b >>= 2;
        self.nb -= 3;
        match typ {
            0 => self.data_block(),
            1 => {
                self.use_fixed = true;
                self.state_dict = false;
                self.huffman_block();
            }
            2 => {
                if let Err(e) = self.read_huffman() {
                    self.err = Some(e);
                    return;
                }
                self.use_fixed = false;
                self.state_dict = false;
                self.huffman_block();
            }
            _ => self.err = Some(corrupt(self.roffset)),
        }
    }

    fn read_huffman(&mut self) -> Result<(), GoErr> {
        self.need(14)?;
        let nlit = (self.b & 0x1f) as usize + 257;
        if nlit > 286 {
            return Err(corrupt(self.roffset));
        }
        self.b >>= 5;
        let ndist = (self.b & 0x1f) as usize + 1;
        if ndist > 30 {
            return Err(corrupt(self.roffset));
        }
        self.b >>= 5;
        let nclen = (self.b & 0xf) as usize + 4;
        self.b >>= 4;
        self.nb -= 14;
        for &i in CODE_ORDER.iter().take(nclen) {
            self.need(3)?;
            self.codebits[i] = self.b & 7;
            self.b >>= 3;
            self.nb -= 3;
        }
        for &i in CODE_ORDER.iter().skip(nclen) {
            self.codebits[i] = 0;
        }
        let codebits = self.codebits;
        if !self.h1.init(&codebits) {
            return Err(corrupt(self.roffset));
        }
        let n = nlit + ndist;
        let mut i = 0;
        while i < n {
            let x = self.huff_sym(1)?;
            if x < 16 {
                self.bits[i] = x;
                i += 1;
                continue;
            }
            let (rep, nb, b) = match x {
                16 => {
                    if i == 0 {
                        return Err(corrupt(self.roffset));
                    }
                    (3, 2, self.bits[i - 1])
                }
                17 => (3, 3, 0),
                18 => (11, 7, 0),
                _ => return Err(GoErr::Msg("flate: internal error: unexpected length code".into())),
            };
            self.need(nb)?;
            let rep = rep + (self.b & ((1 << nb) - 1)) as usize;
            self.b >>= nb;
            self.nb -= nb;
            if i + rep > n {
                return Err(corrupt(self.roffset));
            }
            self.bits[i..i + rep].fill(b);
            i += rep;
        }
        let (lit, dist) = (self.bits[..nlit].to_vec(), self.bits[nlit..n].to_vec());
        if !self.h1.init(&lit) || !self.h2.init(&dist) {
            return Err(corrupt(self.roffset));
        }
        if self.h1.min < self.bits[256] {
            self.h1.min = self.bits[256];
        }
        Ok(())
    }

    fn write_byte(&mut self, c: u8) {
        self.hist[self.wr] = c;
        self.wr += 1;
    }

    /// Go's `tryWriteCopy`, then `writeCopy`: how many bytes were written.
    fn write_copy(&mut self, dist: usize, length: usize) -> usize {
        let dst_base = self.wr;
        let end = (dst_base + length).min(WINDOW);
        let mut dst = dst_base;
        let mut src = dst as isize - dist as isize;
        if src < 0 {
            src += WINDOW as isize;
            let mut s = src as usize;
            while dst < end && s < WINDOW {
                self.hist[dst] = self.hist[s];
                dst += 1;
                s += 1;
            }
            src = 0;
        }
        let mut s = src as usize;
        while dst < end {
            self.hist[dst] = self.hist[s];
            dst += 1;
            s += 1;
        }
        self.wr = dst;
        dst - dst_base
    }

    fn huffman_block(&mut self) {
        let which = if self.use_fixed { 0 } else { 1 };
        loop {
            if !self.state_dict {
                let v = match self.huff_sym(which) {
                    Ok(v) => v,
                    Err(e) => {
                        self.err = Some(e);
                        return;
                    }
                };
                let (mut length, n) = match v {
                    0..=255 => {
                        self.write_byte(v as u8);
                        if self.wr == WINDOW {
                            self.read_flush();
                            self.step = Step::Huffman;
                            self.state_dict = false;
                            return;
                        }
                        continue;
                    }
                    256 => {
                        self.finish_block();
                        return;
                    }
                    257..=264 => (v as usize - 254, 0),
                    265..=268 => (v as usize * 2 - (265 * 2 - 11), 1),
                    269..=272 => (v as usize * 4 - (269 * 4 - 19), 2),
                    273..=276 => (v as usize * 8 - (273 * 8 - 35), 3),
                    277..=280 => (v as usize * 16 - (277 * 16 - 67), 4),
                    281..=284 => (v as usize * 32 - (281 * 32 - 131), 5),
                    285 => (258, 0),
                    _ => {
                        self.err = Some(corrupt(self.roffset));
                        return;
                    }
                };
                if n > 0 {
                    if let Err(e) = self.need(n) {
                        self.err = Some(e);
                        return;
                    }
                    length += (self.b & ((1 << n) - 1)) as usize;
                    self.b >>= n;
                    self.nb -= n;
                }
                let mut dist = if self.use_fixed {
                    if let Err(e) = self.need(5) {
                        self.err = Some(e);
                        return;
                    }
                    let d = ((self.b & 0x1f) as u8).reverse_bits() >> 3;
                    self.b >>= 5;
                    self.nb -= 5;
                    d as usize
                } else {
                    match self.huff_sym(2) {
                        Ok(d) => d as usize,
                        Err(e) => {
                            self.err = Some(e);
                            return;
                        }
                    }
                };
                match dist {
                    0..=3 => dist += 1,
                    4..=29 => {
                        let nb = ((dist - 2) >> 1) as u32;
                        let mut extra = (dist & 1) << nb;
                        if let Err(e) = self.need(nb) {
                            self.err = Some(e);
                            return;
                        }
                        extra |= (self.b & ((1 << nb) - 1)) as usize;
                        self.b >>= nb;
                        self.nb -= nb;
                        dist = (1 << (nb + 1)) + 1 + extra;
                    }
                    _ => {
                        self.err = Some(corrupt(self.roffset));
                        return;
                    }
                }
                if dist > self.hist_size() {
                    self.err = Some(corrupt(self.roffset));
                    return;
                }
                self.copy_len = length;
                self.copy_dist = dist;
            }
            let cnt = self.write_copy(self.copy_dist, self.copy_len);
            self.copy_len -= cnt;
            if self.wr == WINDOW || self.copy_len > 0 {
                self.read_flush();
                self.step = Step::Huffman;
                self.state_dict = true;
                return;
            }
            self.state_dict = false;
        }
    }

    fn data_block(&mut self) {
        self.nb = 0;
        self.b = 0;
        let mut buf = [0u8; 4];
        let (nr, err) = read_full(&mut self.r, &mut buf);
        self.roffset += nr as i64;
        if let Some(e) = err {
            self.err = Some(no_eof(e));
            return;
        }
        let n = usize::from(buf[0]) | usize::from(buf[1]) << 8;
        let nn = u16::from(buf[2]) | u16::from(buf[3]) << 8;
        if nn != !(n as u16) {
            self.err = Some(corrupt(self.roffset));
            return;
        }
        if n == 0 {
            self.read_flush();
            self.finish_block();
            return;
        }
        self.copy_len = n;
        self.copy_data();
    }

    fn copy_data(&mut self) {
        let want = (WINDOW - self.wr).min(self.copy_len);
        let wr = self.wr;
        let (cnt, err) = read_full(&mut self.r, &mut self.hist[wr..wr + want]);
        self.roffset += cnt as i64;
        self.copy_len -= cnt;
        self.wr += cnt;
        if let Some(e) = err {
            self.err = Some(no_eof(e));
            return;
        }
        if self.wr == WINDOW || self.copy_len > 0 {
            self.read_flush();
            self.step = Step::CopyData;
            return;
        }
        self.finish_block();
    }

    fn finish_block(&mut self) {
        if self.final_block {
            if self.wr > self.rd {
                self.read_flush();
            }
            self.err = Some(GoErr::Eof);
        }
        self.step = Step::NextBlock;
    }
}

impl<S: Source> GoRead for Flate<S> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoErr>) {
        loop {
            let (from, to) = self.to_read;
            if to > from {
                let n = p.len().min(to - from);
                p[..n].copy_from_slice(&self.hist[from..from + n]);
                self.to_read.0 += n;
                if self.to_read.0 == to {
                    return (n, self.err.clone());
                }
                return (n, None);
            }
            if let Some(e) = &self.err {
                return (0, Some(e.clone()));
            }
            match self.step {
                Step::NextBlock => self.next_block(),
                Step::Huffman => self.huffman_block(),
                Step::CopyData => self.copy_data(),
            }
            if self.err.is_some() && self.to_read.0 == self.to_read.1 {
                self.read_flush();
            }
        }
    }
}

/// Go's `zlib.Reader`.
pub struct Zlib<S> {
    flate: Flate<S>,
    a: u32,
    s: u32,
    err: Option<GoErr>,
}

/// Go's `zlib.NewReader`: the header read and checked (no dictionary is given, so only the empty one's id passes).
pub fn zlib_reader<S: Source>(mut r: S) -> Result<Zlib<S>, GoErr> {
    let mut h = [0u8; 2];
    if let (_, Some(e)) = read_full(&mut r, &mut h) {
        return Err(no_eof(e));
    }
    if h[0] & 0x0f != 8 || h[0] >> 4 > 7 || !u16::from_be_bytes(h).is_multiple_of(31) {
        return Err(GoErr::Msg("zlib: invalid header".into()));
    }
    if h[1] & 0x20 != 0 {
        let mut d = [0u8; 4];
        if let (_, Some(e)) = read_full(&mut r, &mut d) {
            return Err(no_eof(e));
        }
        if u32::from_be_bytes(d) != 1 {
            return Err(GoErr::Msg("zlib: invalid dictionary".into()));
        }
    }
    Ok(Zlib { flate: Flate::new(r), a: 1, s: 0, err: None })
}

impl<S: Source> Zlib<S> {
    /// The source underneath (PNG's IDAT reader, to ask what is left).
    pub fn source(&self) -> &S {
        &self.flate.r
    }
}

impl<S: Source> GoRead for Zlib<S> {
    fn read(&mut self, p: &mut [u8]) -> (usize, Option<GoErr>) {
        if let Some(e) = &self.err {
            return (0, Some(e.clone()));
        }
        let (n, err) = self.flate.read(p);
        for &x in &p[..n] {
            self.a = (self.a + u32::from(x)) % 65521;
            self.s = (self.s + self.a) % 65521;
        }
        self.err = err;
        if self.err != Some(GoErr::Eof) {
            return (n, self.err.clone());
        }
        let mut c = [0u8; 4];
        if let (_, Some(e)) = read_full(&mut self.flate.r, &mut c) {
            self.err = Some(no_eof(e));
            return (n, self.err.clone());
        }
        if u32::from_be_bytes(c) != (self.s << 16 | self.a) {
            self.err = Some(GoErr::Msg("zlib: invalid checksum".into()));
            return (n, self.err.clone());
        }
        (n, Some(GoErr::Eof))
    }
}
