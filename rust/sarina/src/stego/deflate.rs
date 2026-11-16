//! Go 1.27's `compress/flate` compressor at its default level (6: `fastEncL6`, the token histograms, the bit writer's
//! choice between stored, fixed, dynamic and reused tables, Go's length-limited Huffman codes) and `compress/zlib`'s
//! writer, written out from Go's source so a PNG Sarina writes is Go's byte for byte. Each `Write` the bit writer makes
//! is kept as its own piece: the PNG encoder's bufio turns them into IDAT chunks.

use super::enc_tables::*;

const LENGTH_SHIFT: u32 = 22;
const OFFSET_MASK: u32 = (1 << LENGTH_SHIFT) - 1;
const MATCH_TYPE: u32 = 1 << 30;
const MATCH_OFFSET_ONLY_MASK: u32 = 0xffff;
const MAX_STORE_BLOCK_SIZE: usize = 65535;
const LITERAL_COUNT: usize = 286;
const OFFSET_CODE_COUNT: usize = 30;
const END_BLOCK_MARKER: usize = 256;
const LENGTH_CODES_START: usize = 257;
const CODEGEN_CODE_COUNT: usize = 19;
const BAD_CODE: u8 = 255;
const MAX_PREDEFINED_TOKENS: u16 = 250;
const BUFFER_FLUSH_SIZE: u8 = 246;
const LENGTH_EXTRA_BITS_MIN_CODE: u8 = 8;
const OFFSET_EXTRA_BITS_MIN_CODE: u32 = 4;
const CODEGEN_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn offset_code(off: u32) -> u32 {
    if off < 256 {
        OFFSET_CODES[off as usize]
    } else {
        OFFSET_CODES14[((off >> 7) & 0xff) as usize]
    }
}

/// Go's `tokens`.
struct Tokens {
    extra_hist: [u16; 32],
    off_hist: [u16; 32],
    lit_hist: [u16; 256],
    n_filled: i32,
    n: u16,
    tokens: Vec<u32>,
}

impl Tokens {
    fn new() -> Tokens {
        Tokens { extra_hist: [0; 32], off_hist: [0; 32], lit_hist: [0; 256], n_filled: 0, n: 0, tokens: vec![0; 65536] }
    }

    fn reset(&mut self) {
        if self.n == 0 {
            return;
        }
        self.n = 0;
        self.n_filled = 0;
        self.lit_hist = [0; 256];
        self.extra_hist = [0; 32];
        self.off_hist = [0; 32];
    }

    fn add_literal(&mut self, lit: u8) {
        self.tokens[usize::from(self.n)] = u32::from(lit);
        self.lit_hist[usize::from(lit)] = self.lit_hist[usize::from(lit)].wrapping_add(1);
        self.n = self.n.wrapping_add(1);
    }

    fn add_match_long(&mut self, mut xlength: i32, mut xoffset: u32) {
        let oc = offset_code(xoffset);
        xoffset |= oc << 16;
        while xlength > 0 {
            let mut xl = xlength;
            if xl > 258 {
                xl = if xl > 258 + 3 { 258 } else { 258 - 3 };
            }
            xlength -= xl;
            xl -= 3;
            let lc = usize::from(LENGTH_CODES1[(xl as u32 & 0xff) as usize]);
            self.extra_hist[lc] = self.extra_hist[lc].wrapping_add(1);
            self.off_hist[(oc & 31) as usize] = self.off_hist[(oc & 31) as usize].wrapping_add(1);
            self.tokens[usize::from(self.n)] = MATCH_TYPE | (xl as u32) << LENGTH_SHIFT | xoffset;
            self.n = self.n.wrapping_add(1);
        }
    }

    fn add_eob(&mut self) {
        self.tokens[usize::from(self.n)] = END_BLOCK_MARKER as u32;
        self.extra_hist[0] = self.extra_hist[0].wrapping_add(1);
        self.n = self.n.wrapping_add(1);
    }

    fn estimated_bits(&self) -> i64 {
        let mut shannon = 0f32;
        let mut bits = 0i64;
        let mut n_matches = 0i64;
        let total = i64::from(self.n) + i64::from(self.n_filled);
        if total > 0 {
            let inv_total = 1.0 / total as f32;
            for &v in &self.lit_hist {
                if v > 0 {
                    let n = f32::from(v);
                    shannon += (-m_fast_log2(n * inv_total)).clamp(1.0, 15.0) * n;
                }
            }
            shannon += 15.0;
            for (i, &v) in self.extra_hist[1..LITERAL_COUNT - 256].iter().enumerate() {
                if v > 0 {
                    let n = f32::from(v);
                    shannon += (-m_fast_log2(n * inv_total)).clamp(1.0, 15.0) * n;
                    bits += i64::from(LENGTH_EXTRA_BITS[i & 31]) * i64::from(v);
                    n_matches += i64::from(v);
                }
            }
        }
        if n_matches > 0 {
            let inv_total = 1.0 / n_matches as f32;
            for (i, &v) in self.off_hist[..OFFSET_CODE_COUNT].iter().enumerate() {
                if v > 0 {
                    let n = f32::from(v);
                    shannon += (-m_fast_log2(n * inv_total)).clamp(1.0, 15.0) * n;
                    bits += i64::from(OFFSET_EXTRA_BITS[i & 31]) * i64::from(v);
                }
            }
        }
        shannon as i64 + bits
    }
}

/// Go's `mFastLog2`.
fn m_fast_log2(val: f32) -> f32 {
    let mut ux = val.to_bits() as i32;
    let mut log2 = (((ux >> 23) & 255) - 128) as f32;
    ux &= -0x7f80_0001;
    ux += 127 << 23;
    let uval = f32::from_bits(ux as u32);
    log2 += ((-0.344_848_43) * uval + 2.024_665_8) * uval - 0.674_877_6;
    log2
}

/// A Huffman code: its length in the low byte, the bit-reversed code above.
type Hcode = u32;

fn hcode(code: u16, len: u8) -> Hcode {
    u32::from(len) | u32::from(code) << 8
}

fn hlen(h: Hcode) -> u8 {
    h as u8
}

fn reverse_bits(x: u16, b: u8) -> u16 {
    (x << ((16 - b) & 15)).reverse_bits()
}

/// Go's `huffmanEncoder`.
#[derive(Clone)]
struct HuffEnc {
    codes: Vec<Hcode>,
    bit_count: [i32; 17],
    /// (literal, frequency)
    freqcache: Vec<(u16, u16)>,
}

impl HuffEnc {
    fn new(size: usize) -> HuffEnc {
        HuffEnc { codes: vec![0; size], bit_count: [0; 17], freqcache: vec![(0, 0); LITERAL_COUNT + 1] }
    }

    fn bit_length(&self, freq: &[u16]) -> i64 {
        freq.iter().enumerate().filter(|&(_, &f)| f != 0).map(|(i, &f)| i64::from(f) * i64::from(hlen(self.codes[i]))).sum()
    }

    fn can_encode_len(&self, freq: &[u16]) -> i64 {
        let mut total = 0i64;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 {
                let code = self.codes[i];
                if code == 0 {
                    return i64::from(i32::MAX);
                }
                total += i64::from(f) * i64::from(hlen(code));
            }
        }
        total
    }

    fn bit_counts(&mut self, count: usize, mut max_bits: i32) -> Vec<i32> {
        let n = count as i32;
        self.freqcache[count] = (u16::MAX, u16::MAX);
        let list = &self.freqcache;
        if max_bits > n - 1 {
            max_bits = n - 1;
        }
        #[derive(Clone, Copy, Default)]
        struct Level {
            level: i32,
            last_freq: i32,
            next_char_freq: i32,
            next_pair_freq: i32,
            needed: i32,
        }
        let mut levels = [Level::default(); 16];
        let mut leaf_counts = [[0i32; 16]; 16];
        for level in 1..=max_bits {
            levels[level as usize] = Level {
                level,
                last_freq: i32::from(list[1].1),
                next_char_freq: i32::from(list[2].1),
                next_pair_freq: i32::from(list[0].1) + i32::from(list[1].1),
                needed: 0,
            };
            leaf_counts[level as usize][level as usize] = 2;
            if level == 1 {
                levels[1].next_pair_freq = i32::MAX;
            }
        }
        levels[max_bits as usize].needed = 2 * n - 4;
        let mut level = max_bits as usize;
        while level < 16 {
            if levels[level].next_pair_freq == i32::MAX && levels[level].next_char_freq == i32::MAX {
                levels[level].needed = 0;
                levels[level + 1].next_pair_freq = i32::MAX;
                level += 1;
                continue;
            }
            let prev_freq = levels[level].last_freq;
            if levels[level].next_char_freq < levels[level].next_pair_freq {
                let nn = leaf_counts[level][level] + 1;
                levels[level].last_freq = levels[level].next_char_freq;
                leaf_counts[level][level] = nn;
                let e = list[nn as usize];
                levels[level].next_char_freq = if e.0 < u16::MAX { i32::from(e.1) } else { i32::MAX };
            } else {
                levels[level].last_freq = levels[level].next_pair_freq;
                let save = leaf_counts[level][level];
                leaf_counts[level] = leaf_counts[level - 1];
                leaf_counts[level][level] = save;
                let l = levels[level].level as usize;
                levels[l - 1].needed = 2;
            }
            levels[level].needed -= 1;
            if levels[level].needed == 0 {
                if levels[level].level == max_bits {
                    break;
                }
                let l = levels[level].level as usize;
                levels[l + 1].next_pair_freq = prev_freq + levels[level].last_freq;
                level += 1;
            } else {
                while levels[level - 1].needed > 0 {
                    level -= 1;
                }
            }
        }
        assert_eq!(leaf_counts[max_bits as usize][max_bits as usize], n, "leafCounts[maxBits][maxBits] != n");
        let counts = leaf_counts[max_bits as usize];
        for (bits, level) in (1..=max_bits as usize).rev().enumerate() {
            self.bit_count[bits + 1] = counts[level] - counts[level - 1];
        }
        self.bit_count[..=max_bits as usize].to_vec()
    }

    fn assign_encoding_and_size(&mut self, bit_count: &[i32], count: usize) {
        let mut code = 0u16;
        let mut len = count;
        for (n, &bits) in bit_count.iter().enumerate() {
            code = code.wrapping_shl(1);
            if n == 0 || bits == 0 {
                continue;
            }
            let bits = bits as usize;
            let chunk = &mut self.freqcache[len - bits..len];
            chunk.sort_by_key(|&(lit, _)| lit);
            for &(lit, _) in chunk.iter() {
                self.codes[usize::from(lit)] = hcode(reverse_bits(code, n as u8), n as u8);
                code = code.wrapping_add(1);
            }
            len -= bits;
        }
    }

    fn generate(&mut self, freq: &[u16], max_bits: i32) {
        let mut count = 0;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 {
                self.freqcache[count] = (i as u16, f);
                count += 1;
            } else {
                self.codes[i] = 0;
            }
        }
        self.freqcache[count] = (0, 0);
        if count <= 2 {
            for i in 0..count {
                let lit = self.freqcache[i].0;
                self.codes[usize::from(lit)] = hcode(i as u16, 1);
            }
            return;
        }
        self.freqcache[..count].sort_by_key(|&(lit, f)| (i64::from(f) << 10) + i64::from(lit));
        let bit_count = self.bit_counts(count, max_bits);
        self.assign_encoding_and_size(&bit_count, count);
    }
}

fn fixed_literal() -> HuffEnc {
    let mut h = HuffEnc::new(LITERAL_COUNT);
    for ch in 0..LITERAL_COUNT as u16 {
        let (bits, size) = match ch {
            0..=143 => (ch + 48, 8),
            144..=255 => (ch + 400 - 144, 9),
            256..=279 => (ch - 256, 7),
            _ => (ch + 192 - 280, 8),
        };
        h.codes[usize::from(ch)] = hcode(reverse_bits(bits, size), size);
    }
    h
}

fn fixed_offset() -> HuffEnc {
    let mut h = HuffEnc::new(30);
    for ch in 0..30u16 {
        h.codes[usize::from(ch)] = hcode(reverse_bits(ch, 5), 5);
    }
    h
}

fn huff_offset() -> HuffEnc {
    let mut freq = [0u16; 30];
    freq[0] = 1;
    let mut h = HuffEnc::new(OFFSET_CODE_COUNT);
    h.generate(&freq, 15);
    h
}

fn store_le64(b: &mut [u8], v: u64) {
    b[..8].copy_from_slice(&v.to_le_bytes());
}

/// Go's `huffmanBitWriter`; `out` holds each Write it makes.
struct BitWriter {
    out: Vec<Vec<u8>>,
    bits: u64,
    nbits: u8,
    nbytes: u8,
    wrote_huffman: bool,
    literal_encoding: HuffEnc,
    tmp_lit_encoding: HuffEnc,
    offset_encoding: HuffEnc,
    codegen_encoding: HuffEnc,
    fixed_lit: HuffEnc,
    fixed_off: HuffEnc,
    huff_off: HuffEnc,
    prev_header: i64,
    log_new_table_penalty: u32,
    bytes: [u8; 264],
    literal_freq: [u16; LENGTH_CODES_START + 32],
    offset_freq: [u16; 32],
    codegen_freq: [u16; CODEGEN_CODE_COUNT],
    codegen: [u8; LITERAL_COUNT + OFFSET_CODE_COUNT + 1],
}

impl BitWriter {
    fn new() -> BitWriter {
        BitWriter {
            out: Vec::new(),
            bits: 0,
            nbits: 0,
            nbytes: 0,
            wrote_huffman: false,
            literal_encoding: HuffEnc::new(LITERAL_COUNT),
            tmp_lit_encoding: HuffEnc::new(LITERAL_COUNT),
            offset_encoding: HuffEnc::new(OFFSET_CODE_COUNT),
            codegen_encoding: HuffEnc::new(CODEGEN_CODE_COUNT),
            fixed_lit: fixed_literal(),
            fixed_off: fixed_offset(),
            huff_off: huff_offset(),
            prev_header: 0,
            log_new_table_penalty: 7,
            bytes: [0; 264],
            literal_freq: [0; LENGTH_CODES_START + 32],
            offset_freq: [0; 32],
            codegen_freq: [0; CODEGEN_CODE_COUNT],
            codegen: [0; LITERAL_COUNT + OFFSET_CODE_COUNT + 1],
        }
    }

    fn write(&mut self, b: &[u8]) {
        self.out.push(b.to_vec());
    }

    fn can_reuse(&self, t: &Tokens) -> bool {
        let b = &self.offset_encoding.codes;
        if t.off_hist[..OFFSET_CODE_COUNT].iter().enumerate().any(|(i, &v)| v != 0 && b[i] == 0) {
            return false;
        }
        let b = &self.literal_encoding.codes;
        if t.extra_hist[..LITERAL_COUNT - 256].iter().enumerate().any(|(i, &v)| v != 0 && b[256 + i] == 0) {
            return false;
        }
        !t.lit_hist.iter().enumerate().any(|(i, &v)| v != 0 && b[i] == 0)
    }

    fn flush(&mut self) {
        if self.prev_header > 0 {
            self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
            self.prev_header = 0;
        }
        let mut n = usize::from(self.nbytes);
        while self.nbits != 0 {
            self.bytes[n] = self.bits as u8;
            self.bits >>= 8;
            self.nbits = self.nbits.saturating_sub(8);
            n += 1;
        }
        self.bits = 0;
        if n > 0 {
            let b = self.bytes[..n].to_vec();
            self.write(&b);
        }
        self.nbytes = 0;
    }

    fn write_bits(&mut self, b: i32, nb: u8) {
        self.bits |= (b as u32 as u64) << (self.nbits & 63);
        self.nbits += nb;
        if self.nbits >= 48 {
            self.flush_bits();
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        let mut n = usize::from(self.nbytes);
        while self.nbits != 0 {
            self.bytes[n] = self.bits as u8;
            self.bits >>= 8;
            self.nbits -= 8;
            n += 1;
        }
        if n != 0 {
            let b = self.bytes[..n].to_vec();
            self.write(&b);
        }
        self.nbytes = 0;
        self.write(bytes);
    }

    fn generate_codegen(&mut self, num_literals: usize, num_offsets: usize, use_huff_off: bool) {
        self.codegen_freq = [0; CODEGEN_CODE_COUNT];
        for i in 0..num_literals {
            self.codegen[i] = hlen(self.literal_encoding.codes[i]);
        }
        for i in 0..num_offsets {
            let off = if use_huff_off { &self.huff_off } else { &self.offset_encoding };
            self.codegen[num_literals + i] = hlen(off.codes[i]);
        }
        self.codegen[num_literals + num_offsets] = BAD_CODE;
        let codegen = &mut self.codegen;
        let freq = &mut self.codegen_freq;
        let mut size = codegen[0];
        let mut count: i32 = 1;
        let mut out = 0;
        let mut in_index = 1;
        while size != BAD_CODE {
            let next = codegen[in_index];
            in_index += 1;
            if next == size {
                count += 1;
                continue;
            }
            if size != 0 {
                codegen[out] = size;
                out += 1;
                freq[usize::from(size)] += 1;
                count -= 1;
                while count >= 3 {
                    let n = count.min(6);
                    codegen[out] = 16;
                    codegen[out + 1] = (n - 3) as u8;
                    out += 2;
                    freq[16] += 1;
                    count -= n;
                }
            } else {
                while count >= 11 {
                    let n = count.min(138);
                    codegen[out] = 18;
                    codegen[out + 1] = (n - 11) as u8;
                    out += 2;
                    freq[18] += 1;
                    count -= n;
                }
                if count >= 3 {
                    codegen[out] = 17;
                    codegen[out + 1] = (count - 3) as u8;
                    out += 2;
                    freq[17] += 1;
                    count = 0;
                }
            }
            count -= 1;
            while count >= 0 {
                codegen[out] = size;
                out += 1;
                freq[usize::from(size)] += 1;
                count -= 1;
            }
            size = next;
            count = 1;
        }
        codegen[out] = BAD_CODE;
    }

    fn codegens(&self) -> usize {
        let mut n = self.codegen_freq.len();
        while n > 4 && self.codegen_freq[CODEGEN_ORDER[n - 1]] == 0 {
            n -= 1;
        }
        n
    }

    fn header_size(&self) -> (i64, usize) {
        let n = self.codegens();
        let f = |i: usize| i64::from(self.codegen_freq[i]);
        (3 + 5 + 5 + 4 + 3 * n as i64 + self.codegen_encoding.bit_length(&self.codegen_freq) + f(16) * 2 + f(17) * 3 + f(18) * 7, n)
    }

    fn dynamic_reuse_size(&self) -> i64 {
        self.literal_encoding.bit_length(&self.literal_freq) + self.offset_encoding.bit_length(&self.offset_freq)
    }

    fn dynamic_size(&self, extra_bits: i64) -> (i64, usize) {
        let (header, n) = self.header_size();
        (header + self.literal_encoding.bit_length(&self.literal_freq) + self.offset_encoding.bit_length(&self.offset_freq) + extra_bits, n)
    }

    fn extra_bit_size(&self) -> i64 {
        let mut total = 0i64;
        for (i, &n) in self.literal_freq[257..LITERAL_COUNT].iter().enumerate() {
            total += i64::from(n) * i64::from(LENGTH_EXTRA_BITS[i & 31]);
        }
        for (i, &n) in self.offset_freq[..OFFSET_CODE_COUNT].iter().enumerate() {
            total += i64::from(n) * i64::from(OFFSET_EXTRA_BITS[i & 31]);
        }
        total
    }

    fn fixed_size(&self, extra_bits: i64) -> i64 {
        3 + self.fixed_lit.bit_length(&self.literal_freq[..LITERAL_COUNT]) + self.fixed_off.bit_length(&self.offset_freq[..30]) + extra_bits
    }

    fn write_code(&mut self, c: Hcode) {
        self.bits |= u64::from(c >> 8) << (self.nbits & 63);
        self.nbits += hlen(c);
        if self.nbits >= 48 {
            self.flush_bits();
        }
    }

    fn flush_bits(&mut self) {
        let bits = self.bits;
        self.bits >>= 48;
        self.nbits -= 48;
        let mut n = self.nbytes;
        store_le64(&mut self.bytes[usize::from(n)..], bits);
        n += 6;
        if n >= BUFFER_FLUSH_SIZE {
            let b = self.bytes[..usize::from(n)].to_vec();
            self.write(&b);
            n = 0;
        }
        self.nbytes = n;
    }

    fn write_dynamic_header(&mut self, num_literals: usize, num_offsets: usize, num_codegens: usize, eof: bool) {
        self.write_bits(if eof { 5 } else { 4 }, 3);
        self.write_bits((num_literals - 257) as i32, 5);
        self.write_bits((num_offsets - 1) as i32, 5);
        self.write_bits((num_codegens - 4) as i32, 4);
        for &o in CODEGEN_ORDER.iter().take(num_codegens) {
            let v = hlen(self.codegen_encoding.codes[o]);
            self.write_bits(i32::from(v), 3);
        }
        let mut i = 0;
        loop {
            let word = self.codegen[i];
            i += 1;
            if word == BAD_CODE {
                break;
            }
            self.write_code(self.codegen_encoding.codes[usize::from(word)]);
            let nb = match word {
                16 => 2,
                17 => 3,
                18 => 7,
                _ => continue,
            };
            self.write_bits(i32::from(self.codegen[i]), nb);
            i += 1;
        }
    }

    fn write_stored_header(&mut self, length: usize, eof: bool) {
        if self.prev_header > 0 {
            self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
            self.prev_header = 0;
        }
        if length == 0 && eof {
            self.write_fixed_header(eof);
            self.write_bits(0, 7);
            self.flush();
            return;
        }
        self.write_bits(i32::from(eof), 3);
        self.flush();
        self.write_bits(length as i32, 16);
        self.write_bits(i32::from(!(length as u16)), 16);
    }

    fn write_fixed_header(&mut self, eof: bool) {
        if self.prev_header > 0 {
            self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
            self.prev_header = 0;
        }
        self.write_bits(if eof { 3 } else { 2 }, 3);
    }

    fn index_tokens(&mut self, t: &Tokens) -> (usize, usize) {
        self.literal_freq[..256].copy_from_slice(&t.lit_hist);
        self.literal_freq[256..288].copy_from_slice(&t.extra_hist);
        self.offset_freq = t.off_hist;
        if t.n == 0 {
            return (0, 0);
        }
        let mut num_literals = self.literal_freq.len();
        while self.literal_freq[num_literals - 1] == 0 {
            num_literals -= 1;
        }
        let mut num_offsets = self.offset_freq.len();
        while num_offsets > 0 && self.offset_freq[num_offsets - 1] == 0 {
            num_offsets -= 1;
        }
        if num_offsets == 0 {
            self.offset_freq[0] = 1;
            num_offsets = 1;
        }
        (num_literals, num_offsets)
    }

    fn generate(&mut self) {
        let lf = self.literal_freq;
        let of = self.offset_freq;
        self.literal_encoding.generate(&lf[..LITERAL_COUNT], 15);
        self.offset_encoding.generate(&of[..OFFSET_CODE_COUNT], 15);
    }

    fn generate_codegen_encoding(&mut self) {
        let f = self.codegen_freq;
        self.codegen_encoding.generate(&f, 7);
    }

    fn write_block_dynamic(&mut self, tokens: &mut Tokens, eof: bool, input: &[u8], sync: bool) {
        let sync = sync || eof;
        if sync {
            tokens.add_eob();
        } else {
            tokens.extra_hist[0] = 1;
        }
        if (self.wrote_huffman || eof) && self.prev_header > 0 {
            self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
            self.prev_header = 0;
            self.wrote_huffman = false;
        }
        if self.prev_header > 0 && !self.can_reuse(tokens) {
            self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
            self.prev_header = 0;
        }
        let (num_literals, num_offsets) = self.index_tokens(tokens);
        let mut extra_bits = 0;
        let storable = input.len() <= MAX_STORE_BLOCK_SIZE;
        let ssize = if storable { (input.len() as i64 + 5) * 8 } else { 0 };
        if storable || self.prev_header > 0 {
            extra_bits = self.extra_bit_size();
        }
        let mut size;
        if self.prev_header > 0 {
            let mut new_size = self.prev_header + tokens.estimated_bits();
            new_size += i64::from(hlen(self.literal_encoding.codes[END_BLOCK_MARKER])) + (new_size >> self.log_new_table_penalty);
            let reuse_size = self.dynamic_reuse_size() + extra_bits;
            if new_size < reuse_size {
                self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
                size = new_size;
                self.prev_header = 0;
            } else {
                size = reuse_size;
            }
            if tokens.n < MAX_PREDEFINED_TOKENS {
                let pre_size = self.fixed_size(extra_bits) + 7;
                if pre_size < size {
                    if storable && ssize <= size {
                        self.write_stored_header(input.len(), eof);
                        self.write_bytes(input);
                        return;
                    }
                    self.write_fixed_header(eof);
                    if !sync {
                        tokens.add_eob();
                    }
                    let (lit, off) = (self.fixed_lit.codes.clone(), self.fixed_off.codes.clone());
                    self.write_tokens(&tokens.tokens[..usize::from(tokens.n)], &lit, &off);
                    return;
                }
            }
            if storable && ssize <= size {
                self.write_stored_header(input.len(), eof);
                self.write_bytes(input);
                return;
            }
        }
        if self.prev_header == 0 {
            self.literal_freq[END_BLOCK_MARKER] = 1;
            self.generate();
            self.generate_codegen(num_literals, num_offsets, false);
            self.generate_codegen_encoding();
            let (s, num_codegens) = self.dynamic_size(extra_bits);
            size = s;
            if tokens.n < MAX_PREDEFINED_TOKENS {
                let pre_size = self.fixed_size(extra_bits);
                if pre_size <= size {
                    if storable && ssize <= pre_size {
                        self.write_stored_header(input.len(), eof);
                        self.write_bytes(input);
                        return;
                    }
                    self.write_fixed_header(eof);
                    if !sync {
                        tokens.add_eob();
                    }
                    let (lit, off) = (self.fixed_lit.codes.clone(), self.fixed_off.codes.clone());
                    self.write_tokens(&tokens.tokens[..usize::from(tokens.n)], &lit, &off);
                    return;
                }
            }
            if storable && ssize <= size {
                self.write_stored_header(input.len(), eof);
                self.write_bytes(input);
                return;
            }
            self.write_dynamic_header(num_literals, num_offsets, num_codegens, eof);
            if !sync {
                self.prev_header = self.header_size().0;
            }
            self.wrote_huffman = false;
        }
        if sync {
            self.prev_header = 0;
        }
        let (lit, off) = (self.literal_encoding.codes.clone(), self.offset_encoding.codes.clone());
        self.write_tokens(&tokens.tokens[..usize::from(tokens.n)], &lit, &off);
    }

    fn put(&mut self, bits: &mut u64, nbits: &mut u8, nbytes: &mut u8, v: u64, n: u8) {
        *bits |= v << (*nbits & 63);
        *nbits += n;
        if *nbits >= 48 {
            store_le64(&mut self.bytes[usize::from(*nbytes)..], *bits);
            *bits >>= 48;
            *nbits -= 48;
            *nbytes += 6;
            if *nbytes >= BUFFER_FLUSH_SIZE {
                let b = self.bytes[..usize::from(*nbytes)].to_vec();
                self.write(&b);
                *nbytes = 0;
            }
        }
    }

    fn write_tokens(&mut self, tokens: &[u32], len_codes: &[Hcode], off_codes: &[Hcode]) {
        if tokens.is_empty() {
            return;
        }
        let (tokens, defer_eob) = match tokens.split_last() {
            Some((&t, rest)) if t == END_BLOCK_MARKER as u32 => (rest, true),
            _ => (tokens, false),
        };
        let (mut bits, mut nbits, mut nbytes) = (self.bits, self.nbits, self.nbytes);
        for &t in tokens {
            if t < 256 {
                let c = len_codes[t as usize];
                self.put(&mut bits, &mut nbits, &mut nbytes, u64::from(c >> 8), hlen(c));
                continue;
            }
            let length = (t >> LENGTH_SHIFT) as u8;
            let len_code = LENGTH_CODES[usize::from(length)] & 31;
            let c = len_codes[LENGTH_CODES_START + usize::from(len_code)];
            self.put(&mut bits, &mut nbits, &mut nbytes, u64::from(c >> 8), hlen(c));
            if len_code >= LENGTH_EXTRA_BITS_MIN_CODE {
                let extra = u64::from(length.wrapping_sub(LENGTH_BASE[usize::from(len_code)]));
                self.put(&mut bits, &mut nbits, &mut nbytes, extra, LENGTH_EXTRA_BITS[usize::from(len_code)]);
            }
            let offset = t & OFFSET_MASK;
            let off_code = (offset >> 16) & 31;
            let c = off_codes[off_code as usize];
            self.put(&mut bits, &mut nbits, &mut nbytes, u64::from(c >> 8), hlen(c));
            if off_code >= OFFSET_EXTRA_BITS_MIN_CODE {
                let comb = OFFSET_COMBINED[off_code as usize];
                self.put(
                    &mut bits,
                    &mut nbits,
                    &mut nbytes,
                    u64::from(offset.wrapping_sub(comb >> 8) & MATCH_OFFSET_ONLY_MASK),
                    comb as u8,
                );
            }
        }
        self.bits = bits;
        self.nbits = nbits;
        self.nbytes = nbytes;
        if defer_eob {
            self.write_code(len_codes[END_BLOCK_MARKER]);
        }
    }

    fn write_block_huff(&mut self, eof: bool, mut input: &[u8], sync: bool) {
        self.literal_freq = [0; LENGTH_CODES_START + 32];
        if !self.wrote_huffman {
            self.offset_freq = [0; 32];
        }
        for &b in input {
            self.literal_freq[usize::from(b)] = self.literal_freq[usize::from(b)].wrapping_add(1);
        }
        let storable = input.len() <= MAX_STORE_BLOCK_SIZE;
        let ssize = if storable { (input.len() as i64 + 5) * 8 } else { 0 };
        if storable && input.len() > 1024 {
            let mut abs = 0f64;
            let avg = input.len() as f64 / 256.0;
            let max = (input.len() * 2) as f64;
            for &v in &self.literal_freq[..256] {
                let diff = f64::from(v) - avg;
                abs += diff * diff;
                if abs >= max {
                    break;
                }
            }
            if abs < max {
                self.write_stored_header(input.len(), eof);
                self.write_bytes(input);
                return;
            }
        }
        self.literal_freq[END_BLOCK_MARKER] = 1;
        let lf = self.literal_freq;
        self.tmp_lit_encoding.generate(&lf[..END_BLOCK_MARKER + 1], 15);
        let mut est_bits = self.tmp_lit_encoding.can_encode_len(&lf[..END_BLOCK_MARKER + 1]);
        if est_bits < i64::from(i32::MAX) {
            est_bits += self.prev_header;
            if self.prev_header == 0 {
                est_bits += 70 * 8;
            }
            est_bits += est_bits >> self.log_new_table_penalty;
        }
        if storable && ssize <= est_bits {
            self.write_stored_header(input.len(), eof);
            self.write_bytes(input);
            return;
        }
        if self.prev_header > 0 {
            let reuse = self.literal_encoding.can_encode_len(&lf[..256]);
            if est_bits < reuse {
                self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
                self.prev_header = 0;
            }
        }
        if self.prev_header == 0 {
            std::mem::swap(&mut self.literal_encoding, &mut self.tmp_lit_encoding);
            self.generate_codegen(END_BLOCK_MARKER + 1, 1, true);
            self.generate_codegen_encoding();
            let n = self.codegens();
            self.write_dynamic_header(END_BLOCK_MARKER + 1, 1, n, eof);
            self.wrote_huffman = true;
            self.prev_header = self.header_size().0;
        }
        let encoding: Vec<Hcode> = self.literal_encoding.codes[..256].to_vec();
        let (mut bits, mut nbits, mut nbytes) = (self.bits, self.nbits, self.nbytes);
        while input.len() > 3 {
            if nbits >= 8 {
                let n = nbits >> 3;
                store_le64(&mut self.bytes[usize::from(nbytes)..], bits);
                bits >>= (u32::from(n) * 8) & 63;
                nbits -= n * 8;
                nbytes += n;
            }
            if nbytes >= BUFFER_FLUSH_SIZE {
                let b = self.bytes[..usize::from(nbytes)].to_vec();
                self.write(&b);
                nbytes = 0;
            }
            let (a, b) = (encoding[usize::from(input[0])], encoding[usize::from(input[1])]);
            bits |= u64::from(a >> 8) << (nbits & 63);
            bits |= u64::from(b >> 8) << (nbits.wrapping_add(hlen(a)) & 63);
            let c = encoding[usize::from(input[2])];
            nbits += hlen(b) + hlen(a);
            bits |= u64::from(c >> 8) << (nbits & 63);
            nbits += hlen(c);
            input = &input[3..];
        }
        for &t in input {
            if nbits >= 48 {
                store_le64(&mut self.bytes[usize::from(nbytes)..], bits);
                bits >>= 48;
                nbits -= 48;
                nbytes += 6;
                if nbytes >= BUFFER_FLUSH_SIZE {
                    let b = self.bytes[..usize::from(nbytes)].to_vec();
                    self.write(&b);
                    nbytes = 0;
                }
            }
            let c = encoding[usize::from(t)];
            bits |= u64::from(c >> 8) << (nbits & 63);
            nbits += hlen(c);
        }
        self.bits = bits;
        self.nbits = nbits;
        self.nbytes = nbytes;
        if self.nbits >= 48 {
            self.flush_bits();
        }
        if eof || sync {
            self.write_code(self.literal_encoding.codes[END_BLOCK_MARKER]);
            self.prev_header = 0;
            self.wrote_huffman = false;
        }
    }
}

const TABLE_BITS: u8 = 15;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const MAX_MATCH_OFFSET: i32 = 1 << 15;
const ALLOC_HISTORY: usize = MAX_STORE_BLOCK_SIZE * 5;
const BUFFER_RESET: i32 = ((1i64 << 31) - ALLOC_HISTORY as i64 - MAX_STORE_BLOCK_SIZE as i64 - 1) as i32;

fn hash_len(u: u64, b: u8, n: u8) -> usize {
    match n {
        7 => ((u << (64 - 56)).wrapping_mul(58_295_818_150_454_627) >> (64 - b)) as u32 as usize,
        _ => ((u as u32).wrapping_mul(2_654_435_761) >> (32 - b)) as usize,
    }
}

fn load32(b: &[u8], i: i32) -> u32 {
    let i = i as usize;
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn load64(b: &[u8], i: i32) -> u64 {
    let i = i as usize;
    u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"))
}

fn match_len(a: &[u8], b: &[u8]) -> i32 {
    let mut n = 0;
    let mut left = a.len();
    while left >= 8 {
        let diff = u64::from_le_bytes(a[n..n + 8].try_into().expect("8")) ^ u64::from_le_bytes(b[n..n + 8].try_into().expect("8"));
        if diff != 0 {
            return (n + (diff.trailing_zeros() >> 3) as usize) as i32;
        }
        n += 8;
        left -= 8;
    }
    n += a[n..].iter().zip(&b[n..]).take_while(|(x, y)| x == y).count();
    n as i32
}

/// Go's `fastEncL6`.
struct L6 {
    hist: Vec<u8>,
    hist_cap: usize,
    cur: i32,
    table: Vec<i32>,
    b_table: Vec<(i32, i32)>,
}

impl L6 {
    fn new() -> L6 {
        L6 {
            hist: Vec::new(),
            hist_cap: 0,
            cur: MAX_STORE_BLOCK_SIZE as i32,
            table: vec![0; TABLE_SIZE],
            b_table: vec![(0, 0); TABLE_SIZE],
        }
    }

    fn add_block(&mut self, src: &[u8]) -> i32 {
        if self.hist.len() + src.len() > self.hist_cap {
            if self.hist_cap == 0 {
                self.hist_cap = ALLOC_HISTORY;
            } else {
                let offset = self.hist.len() as i32 - MAX_MATCH_OFFSET;
                self.hist.copy_within(offset as usize..offset as usize + MAX_MATCH_OFFSET as usize, 0);
                self.cur += offset;
                self.hist.truncate(MAX_MATCH_OFFSET as usize);
            }
        }
        let s = self.hist.len() as i32;
        self.hist.extend_from_slice(src);
        s
    }

    fn reset(&mut self) {
        if self.hist_cap < ALLOC_HISTORY {
            self.hist_cap = ALLOC_HISTORY;
        }
        if self.cur <= BUFFER_RESET {
            self.cur += MAX_MATCH_OFFSET + self.hist.len() as i32;
        }
        self.hist.clear();
    }

    fn match_len_limited(&self, s: i32, t: i32) -> i32 {
        let (s, t) = (s as usize, t as usize);
        let a = &self.hist[s..(s + 258 - 4).min(self.hist.len())];
        match_len(a, &self.hist[t..])
    }

    fn match_len_long(&self, s: i32, t: i32) -> i32 {
        match_len(&self.hist[s as usize..], &self.hist[t as usize..])
    }

    fn encode(&mut self, dst: &mut Tokens, src_in: &[u8]) {
        const INPUT_MARGIN: i32 = 12 - 1;
        const MIN_NON_LITERAL_BLOCK_SIZE: usize = 1 + 1 + INPUT_MARGIN as usize;
        while self.cur >= BUFFER_RESET {
            if self.hist.is_empty() {
                self.table.fill(0);
                self.b_table.fill((0, 0));
                self.cur = MAX_MATCH_OFFSET;
                break;
            }
            let min_off = self.cur + self.hist.len() as i32 - MAX_MATCH_OFFSET;
            for v in self.table.iter_mut() {
                *v = if *v <= min_off { 0 } else { *v - self.cur + MAX_MATCH_OFFSET };
            }
            for v in self.b_table.iter_mut() {
                if v.0 <= min_off {
                    *v = (0, 0);
                } else {
                    v.0 = v.0 - self.cur + MAX_MATCH_OFFSET;
                    v.1 = if v.1 <= min_off { 0 } else { v.1 - self.cur + MAX_MATCH_OFFSET };
                }
            }
            self.cur = MAX_MATCH_OFFSET;
        }
        let mut s = self.add_block(src_in);
        if src_in.len() < MIN_NON_LITERAL_BLOCK_SIZE {
            dst.n = src_in.len() as u16;
            return;
        }
        let src_len = self.hist.len() as i32;
        let mut next_emit = s;
        let s_limit = src_len - INPUT_MARGIN;
        let mut cv = load64(&self.hist, s);
        let mut repeat = 1i32;
        'outer: loop {
            let mut next_s = s;
            let mut l: i32 = 0;
            let mut t: i32;
            loop {
                let mut next_hash_s = hash_len(cv, TABLE_BITS, 4);
                let mut next_hash_l = hash_len(cv, TABLE_BITS, 7);
                s = next_s;
                next_s = s + 1 + ((s - next_emit) >> 7);
                if next_s > s_limit {
                    break 'outer;
                }
                let s_candidate = self.table[next_hash_s];
                let mut l_candidate = self.b_table[next_hash_l];
                let next = load64(&self.hist, next_s);
                let entry = s + self.cur;
                self.table[next_hash_s] = entry;
                let e = &mut self.b_table[next_hash_l];
                *e = (entry, e.0);
                next_hash_s = hash_len(next, TABLE_BITS, 4);
                next_hash_l = hash_len(next, TABLE_BITS, 7);
                t = l_candidate.0 - self.cur;
                if s - t < MAX_MATCH_OFFSET {
                    if cv as u32 == load32(&self.hist, t) {
                        self.table[next_hash_s] = next_s + self.cur;
                        let e = &mut self.b_table[next_hash_l];
                        *e = (next_s + self.cur, e.0);
                        let t2 = l_candidate.1 - self.cur;
                        if s - t2 < MAX_MATCH_OFFSET && cv as u32 == load32(&self.hist, t2) {
                            l = self.match_len_limited(s + 4, t + 4) + 4;
                            let ml1 = self.match_len_limited(s + 4, t2 + 4) + 4;
                            if ml1 > l {
                                t = t2;
                                l = ml1;
                                break;
                            }
                        }
                        break;
                    }
                    t = l_candidate.1 - self.cur;
                    if s - t < MAX_MATCH_OFFSET && cv as u32 == load32(&self.hist, t) {
                        self.table[next_hash_s] = next_s + self.cur;
                        let e = &mut self.b_table[next_hash_l];
                        *e = (next_s + self.cur, e.0);
                        break;
                    }
                }
                t = s_candidate - self.cur;
                if s - t < MAX_MATCH_OFFSET && cv as u32 == load32(&self.hist, t) {
                    l = self.match_len_limited(s + 4, t + 4) + 4;
                    l_candidate = self.b_table[next_hash_l];
                    self.table[next_hash_s] = next_s + self.cur;
                    let e = &mut self.b_table[next_hash_l];
                    *e = (next_s + self.cur, e.0);
                    let mut t2 = s - repeat + 1;
                    if load32(&self.hist, t2) == (cv >> 8) as u32 {
                        let ml = self.match_len_limited(s + 4 + 1, t2 + 4) + 4;
                        if ml > l {
                            t = t2;
                            l = ml;
                            s += 1;
                            break;
                        }
                    }
                    t2 = l_candidate.0 - self.cur;
                    if next_s - t2 < MAX_MATCH_OFFSET {
                        if load32(&self.hist, t2) == next as u32 {
                            let ml = self.match_len_limited(next_s + 4, t2 + 4) + 4;
                            if ml > l {
                                t = t2;
                                s = next_s;
                                l = ml;
                            }
                        }
                        t2 = l_candidate.1 - self.cur;
                        if next_s - t2 < MAX_MATCH_OFFSET && load32(&self.hist, t2) == next as u32 {
                            let ml = self.match_len_limited(next_s + 4, t2 + 4) + 4;
                            if ml > l {
                                t = t2;
                                s = next_s;
                                l = ml;
                                break;
                            }
                        }
                    }
                    break;
                }
                cv = next;
            }
            if l == 0 {
                l = self.match_len_long(s + 4, t + 4) + 4;
            } else if l == 258 {
                l += self.match_len_long(s + l, t + l);
            }
            let s_at = s + l;
            if s_at < s_limit {
                let e = self.b_table[hash_len(load64(&self.hist, s_at), TABLE_BITS, 7)];
                let mut t2 = e.0 - self.cur - l + 2;
                let s2 = s + 2;
                let off = s2 - t2;
                if off < MAX_MATCH_OFFSET {
                    if off > 0 && t2 >= 0 {
                        let l2 = self.match_len_long(s2, t2);
                        if l2 > l {
                            t = t2;
                            l = l2;
                            s = s2;
                        }
                    }
                    t2 = e.1 - self.cur - l + 2;
                    let off = s2 - t2;
                    if off > 0 && off < MAX_MATCH_OFFSET && t2 >= 0 {
                        let l2 = self.match_len_long(s2, t2);
                        if l2 > l {
                            t = t2;
                            l = l2;
                            s = s2;
                        }
                    }
                }
            }
            while t > 0 && s > next_emit && self.hist[(t - 1) as usize] == self.hist[(s - 1) as usize] {
                s -= 1;
                t -= 1;
                l += 1;
            }
            if next_emit < s {
                for i in next_emit..s {
                    dst.add_literal(self.hist[i as usize]);
                }
            }
            dst.add_match_long(l, (s - t - 1) as u32);
            repeat = s - t;
            s += l;
            next_emit = s;
            if next_s >= s {
                s = next_s + 1;
            }
            if s >= s_limit {
                let mut i = next_s + 1;
                while i < src_len - 8 {
                    let cv = load64(&self.hist, i);
                    self.table[hash_len(cv, TABLE_BITS, 4)] = i + self.cur;
                    let e = &mut self.b_table[hash_len(cv, TABLE_BITS, 7)];
                    *e = (i + self.cur, e.0);
                    i += 2;
                }
                break 'outer;
            }
            let mut i = next_s + 1;
            while i < s - 1 {
                let cv = load64(&self.hist, i);
                let t = i + self.cur;
                let h1 = hash_len(cv, TABLE_BITS, 7);
                let h2 = hash_len(cv >> 8, TABLE_BITS, 7);
                self.table[hash_len(cv, TABLE_BITS, 4)] = t;
                let e = &mut self.b_table[h1];
                *e = (t, e.0);
                let e = &mut self.b_table[h2];
                *e = (t + 1, e.0);
                i += 2;
            }
            cv = load64(&self.hist, s);
        }
        if (next_emit as usize) < self.hist.len() {
            if dst.n == 0 {
                return;
            }
            for &c in &self.hist[next_emit as usize..] {
                dst.add_literal(c);
            }
        }
    }
}

/// Go's `zlib.NewWriter` (default level) over a `compressor`: each piece Go's writers hand on, in order.
pub struct ZlibWriter {
    w: BitWriter,
    window: Vec<u8>,
    window_end: usize,
    tokens: Tokens,
    fast: L6,
    wrote_header: bool,
    a: u32,
    s: u32,
}

impl Default for ZlibWriter {
    fn default() -> ZlibWriter {
        ZlibWriter::new()
    }
}

impl ZlibWriter {
    pub fn new() -> ZlibWriter {
        ZlibWriter {
            w: BitWriter::new(),
            window: vec![0; MAX_STORE_BLOCK_SIZE],
            window_end: 0,
            tokens: Tokens::new(),
            fast: L6::new(),
            wrote_header: false,
            a: 1,
            s: 0,
        }
    }

    fn header(&mut self) {
        if !self.wrote_header {
            self.wrote_header = true;
            self.w.write(&[0x78, 0x9c]);
        }
    }

    /// Go's `zlib.Writer.Write`.
    pub fn write(&mut self, p: &[u8]) {
        self.header();
        if p.is_empty() {
            return;
        }
        let mut b = p;
        while !b.is_empty() {
            if self.window_end == self.window.len() {
                self.step(false);
            }
            let n = (self.window.len() - self.window_end).min(b.len());
            self.window[self.window_end..self.window_end + n].copy_from_slice(&b[..n]);
            self.window_end += n;
            b = &b[n..];
        }
        for &x in p {
            self.a = (self.a + u32::from(x)) % 65521;
            self.s = (self.s + self.a) % 65521;
        }
    }

    /// Go's `deflateFast`.
    fn step(&mut self, sync: bool) {
        if self.window_end < self.window.len() {
            if !sync {
                return;
            }
            if self.window_end < 128 {
                if self.window_end == 0 {
                    return;
                }
                if self.window_end <= 32 {
                    let data = self.window[..self.window_end].to_vec();
                    self.w.write_stored_header(data.len(), false);
                    self.w.write_bytes(&data);
                } else {
                    let data = self.window[..self.window_end].to_vec();
                    self.w.write_block_huff(false, &data, true);
                }
                self.tokens.reset();
                self.window_end = 0;
                self.fast.reset();
                return;
            }
        }
        let data = self.window[..self.window_end].to_vec();
        self.fast.encode(&mut self.tokens, &data);
        let end = self.window_end as i32;
        if self.tokens.n == 0 {
            self.w.write_stored_header(data.len(), false);
            self.w.write_bytes(&data);
        } else if i32::from(self.tokens.n) > end - (end >> 4) {
            self.w.write_block_huff(false, &data, sync);
        } else {
            self.w.write_block_dynamic(&mut self.tokens, false, &data, sync);
        }
        self.tokens.reset();
        self.window_end = 0;
    }

    /// Go's `zlib.Writer.Close`: the pieces written, in order.
    pub fn close(mut self) -> Vec<Vec<u8>> {
        self.header();
        self.step(true);
        self.w.write_stored_header(0, true);
        self.w.flush();
        let sum = (self.s << 16 | self.a).to_be_bytes();
        self.w.write(&sum);
        self.w.out
    }
}
