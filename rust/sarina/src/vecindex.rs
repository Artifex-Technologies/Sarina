//! Go: `go/vecindex`: the 2/4-bit compressed vector index (TurboQuant: normalise, rotate with random signs, Walsh-Hadamard
//! blocks and shuffles, quantise every coordinate to the Lloyd-Max levels of a normal curve) and its file format
//! (`SVEC1`). The rotation comes from the saved seed through Go's math/rand (`gorand`), the levels from Go's amd64
//! `math.Exp` (`gomath`), and all arithmetic is float32 or float64 exactly where Go's is, so an index Go saved searches
//! the same here (testdata/parity/vecindex.json).
//!
//! On purpose, not as Go: a dimension of exactly 65,536 passes Go's check and then never ends (its block loop appends
//! blocks of size 0 until memory runs out); here it is refused as an unsupported dimension. And a file's vector count is
//! checked against the bytes that follow before anything is allocated (Go allocates the count's slices first: 2^32
//! vectors pass its size check and cost about 52 GB); the error is the same either way.

use crate::gomath;
use crate::gorand::Rand;
use std::collections::HashMap;
use std::sync::OnceLock;

const ROUNDS: usize = 3;
const MAGIC: &[u8] = b"SVEC1\n";

/// An index of vectors of one dimension at 2 or 4 bits per number.
#[derive(Clone, Debug)]
pub struct Index {
    dim: usize,
    cells: usize,
    blocks: Vec<usize>,
    perm: [Vec<i32>; ROUNDS],
    bits: usize,
    seed: i64,
    signs: [Vec<f32>; ROUNDS],
    book: Vec<f32>,
    cuts: Vec<f32>,
    per: usize,
    codes: Vec<u8>,
    norms: Vec<f32>,
    ids: Vec<u64>,
    slot: HashMap<u64, usize>,
}

/// One search result: the id and the approximate inner product.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub id: u64,
    pub score: f32,
}

/// The n best levels for a standard normal variable (Go's lloydMax: the classic iteration on a fine grid, with Go's Exp).
pub fn lloyd_max(n: usize) -> Vec<f64> {
    let mut lv: Vec<f64> = (0..n).map(|i| -2.5 + 5.0 * i as f64 / (n - 1) as f64).collect();
    const STEPS: usize = 40000;
    let (lo, hi) = (-6.0f64, 6.0f64);
    let xs: Vec<f64> = (0..STEPS).map(|i| lo + (hi - lo) * (i as f64 + 0.5) / STEPS as f64).collect();
    let w: Vec<f64> = xs.iter().map(|&x| gomath::exp(-x * x / 2.0)).collect();
    for _ in 0..200 {
        let mut sum = vec![0.0f64; n];
        let mut wt = vec![0.0f64; n];
        for (i, &x) in xs.iter().enumerate() {
            let mut c = 0;
            while c < n - 1 && x > (lv[c] + lv[c + 1]) / 2.0 {
                c += 1;
            }
            sum[c] += w[i] * x;
            wt[c] += w[i];
        }
        for c in 0..n {
            if wt[c] > 0.0 {
                lv[c] = sum[c] / wt[c];
            }
        }
    }
    lv
}

/// The levels for 2 and 4 bits, made once (Go makes them for every index; they never change on one machine).
fn levels(bits: usize) -> &'static [f64] {
    static L2: OnceLock<Vec<f64>> = OnceLock::new();
    static L4: OnceLock<Vec<f64>> = OnceLock::new();
    if bits == 2 {
        L2.get_or_init(|| lloyd_max(4))
    } else {
        L4.get_or_init(|| lloyd_max(16))
    }
}

impl Index {
    /// Go's `New`: an empty index; `seed` fixes the rotation (a saved index keeps it).
    pub fn new(dim: usize, bits: usize, seed: i64) -> Result<Index, String> {
        if !(2..1 << 16).contains(&dim) {
            return Err(format!("vecindex: dimension {dim} is not supported"));
        }
        if bits != 2 && bits != 4 {
            return Err("vecindex: bits must be 2 or 4".into());
        }
        let per = (dim * bits).div_ceil(8);
        let mut blocks = Vec::new();
        let (mut rest, mut b) = (dim, 1usize << 15);
        while rest > 0 {
            if rest >= b {
                blocks.push(b);
                rest -= b;
            }
            b >>= 1;
        }
        let mut rng = Rand::new(seed);
        let mut signs: [Vec<f32>; ROUNDS] = Default::default();
        let mut perm: [Vec<i32>; ROUNDS] = Default::default();
        for r in 0..ROUNDS {
            signs[r] = (0..dim).map(|_| (1 - 2 * rng.intn(2)) as f32).collect();
            perm[r] = rng.perm(dim).into_iter().map(|v| v as i32).collect();
        }
        let scale = 1.0 / (dim as f64).sqrt();
        let book: Vec<f32> = levels(bits).iter().map(|&v| (v * scale) as f32).collect();
        let cuts = (0..book.len() - 1).map(|i| (book[i] + book[i + 1]) / 2.0).collect();
        Ok(Index {
            dim,
            cells: per * 8 / bits,
            blocks,
            perm,
            bits,
            seed,
            signs,
            book,
            cuts,
            per,
            codes: Vec::new(),
            norms: Vec::new(),
            ids: Vec::new(),
            slot: HashMap::new(),
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn bits(&self) -> usize {
        self.bits
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// About how much memory the stored vectors take.
    pub fn bytes(&self) -> usize {
        self.codes.len() + 4 * self.norms.len() + 8 * self.ids.len()
    }

    /// Spins the first `dim` places of v: signs, a Walsh-Hadamard transform in each block, a shuffle; three times.
    fn rotate(&self, v: &mut [f32]) {
        let mut tmp = vec![0f32; self.dim];
        for r in 0..ROUNDS {
            for (x, s) in v[..self.dim].iter_mut().zip(&self.signs[r]) {
                *x *= s;
            }
            let mut off = 0;
            for &b in &self.blocks {
                let blk = &mut v[off..off + b];
                let mut h = 1;
                while h < b {
                    let mut i = 0;
                    while i < b {
                        for j in i..i + h {
                            let (a, c) = (blk[j], blk[j + h]);
                            blk[j] = a + c;
                            blk[j + h] = a - c;
                        }
                        i += h << 1;
                    }
                    h <<= 1;
                }
                let sc = (1.0 / (b as f64).sqrt()) as f32;
                for x in blk.iter_mut() {
                    *x *= sc;
                }
                off += b;
            }
            if self.blocks.len() > 1 || r < ROUNDS - 1 {
                for i in 0..self.dim {
                    tmp[i] = v[self.perm[r][i] as usize];
                }
                v[..self.dim].copy_from_slice(&tmp);
            }
        }
    }

    /// The rotation's parts and the levels (blocks, bytes per vector, cells, levels, cuts, signs, shuffles): what the
    /// parity test compares with Go's.
    #[allow(clippy::type_complexity)]
    pub fn rotation(&self) -> (&[usize], usize, usize, &[f32], &[f32], &[Vec<f32>; ROUNDS], &[Vec<i32>; ROUNDS]) {
        (&self.blocks, self.per, self.cells, &self.book, &self.cuts, &self.signs, &self.perm)
    }

    /// v (the dimension's numbers, padded to whole bytes) after the rotation.
    pub fn rotated(&self, v: &[f32]) -> Vec<f32> {
        let mut buf = vec![0f32; self.cells];
        buf[..v.len().min(self.dim)].copy_from_slice(&v[..v.len().min(self.dim)]);
        self.rotate(&mut buf);
        buf
    }

    /// Go's sort.Search over the cuts: the first level whose upper cut is at least c.
    fn level(&self, c: f32) -> usize {
        self.cuts.partition_point(|&x| x < c)
    }

    /// Stores v under id (an existing id is replaced).
    pub fn add(&mut self, id: u64, v: &[f32]) -> Result<(), String> {
        if v.len() != self.dim {
            return Err(format!("vecindex: vector has {} numbers, the index wants {}", v.len(), self.dim));
        }
        let mut buf = vec![0f32; self.cells];
        let mut n2 = 0f64;
        for (i, &f) in v.iter().enumerate() {
            if !f.is_finite() {
                return Err("vecindex: vector has a NaN or infinite number".into());
            }
            buf[i] = f;
            n2 += f as f64 * f as f64;
        }
        let norm = n2.sqrt() as f32;
        if norm > 0.0 {
            let inv = 1.0 / norm;
            for x in buf.iter_mut() {
                *x *= inv;
            }
        }
        self.rotate(&mut buf);
        let mut code = vec![0u8; self.per];
        for (i, &c) in buf.iter().take(self.dim).enumerate() {
            let l = self.level(c) as u8;
            if self.bits == 4 {
                code[i >> 1] |= l << (4 * (i & 1));
            } else {
                code[i >> 2] |= l << (2 * (i & 3));
            }
        }
        if let Some(&s) = self.slot.get(&id) {
            self.codes[s * self.per..(s + 1) * self.per].copy_from_slice(&code);
            self.norms[s] = norm;
            return Ok(());
        }
        self.slot.insert(id, self.ids.len());
        self.ids.push(id);
        self.norms.push(norm);
        self.codes.extend_from_slice(&code);
        Ok(())
    }

    /// Deletes id (false if it was not there); the last vector takes its place.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(s) = self.slot.remove(&id) else { return false };
        let last = self.ids.len() - 1;
        if s != last {
            self.codes.copy_within(last * self.per..(last + 1) * self.per, s * self.per);
            self.norms[s] = self.norms[last];
            self.ids[s] = self.ids[last];
            self.slot.insert(self.ids[s], s);
        }
        self.codes.truncate(last * self.per);
        self.norms.truncate(last);
        self.ids.truncate(last);
        true
    }

    /// Up to k best matches by inner product, best first; `allow`, when given, limits the search to those ids. Ties come
    /// out in the order Go's container/heap leaves them.
    pub fn search(&self, q: &[f32], k: usize, allow: Option<&dyn Fn(u64) -> bool>) -> Result<Vec<Hit>, String> {
        if q.len() != self.dim {
            return Err(format!("vecindex: query has {} numbers, the index wants {}", q.len(), self.dim));
        }
        if k < 1 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0f32; self.cells];
        buf[..self.dim].copy_from_slice(q);
        self.rotate(&mut buf);
        let nl = 1 << self.bits;
        let mut lut = vec![0f32; self.cells * nl];
        for i in 0..self.cells {
            for l in 0..nl {
                lut[i * nl + l] = buf[i] * self.book[l];
            }
        }
        let mut h: Vec<Hit> = Vec::new();
        for s in 0..self.ids.len() {
            if allow.is_some_and(|a| !a(self.ids[s])) {
                continue;
            }
            let code = &self.codes[s * self.per..(s + 1) * self.per];
            let mut sum = 0f32;
            if self.bits == 4 {
                for (i, &b) in code.iter().enumerate() {
                    sum += lut[(2 * i) * 16 + (b & 15) as usize] + lut[(2 * i + 1) * 16 + (b >> 4) as usize];
                }
            } else {
                for (i, &b) in code.iter().enumerate() {
                    let o = 4 * i;
                    sum += lut[o * 4 + (b & 3) as usize]
                        + lut[(o + 1) * 4 + (b >> 2 & 3) as usize]
                        + lut[(o + 2) * 4 + (b >> 4 & 3) as usize]
                        + lut[(o + 3) * 4 + (b >> 6) as usize];
                }
            }
            let sc = sum * self.norms[s];
            if h.len() < k {
                h.push(Hit { id: self.ids[s], score: sc });
                let n = h.len() - 1;
                heap_up(&mut h, n);
            } else if sc > h[0].score {
                h[0] = Hit { id: self.ids[s], score: sc };
                let n = h.len();
                if !heap_down(&mut h, 0, n) {
                    heap_up(&mut h, 0);
                }
            }
        }
        let mut out = vec![Hit { id: 0, score: 0.0 }; h.len()];
        for i in (0..out.len()).rev() {
            let n = h.len() - 1;
            h.swap(0, n);
            heap_down(&mut h, 0, n);
            out[i] = h.pop().unwrap_or(Hit { id: 0, score: 0.0 });
        }
        Ok(out)
    }

    /// The file Go's `Save` writes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAGIC.len() + 24 + self.ids.len() * 12 + self.codes.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.dim as u32).to_le_bytes());
        out.extend_from_slice(&(self.bits as u32).to_le_bytes());
        out.extend_from_slice(&self.seed.to_le_bytes());
        out.extend_from_slice(&(self.ids.len() as u64).to_le_bytes());
        for id in &self.ids {
            out.extend_from_slice(&id.to_le_bytes());
        }
        for n in &self.norms {
            out.extend_from_slice(&n.to_bits().to_le_bytes());
        }
        out.extend_from_slice(&self.codes);
        out
    }

    /// Go's `Save`: a temporary file in the same folder (0600), then moved into place.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let tmp = dir.join(format!(".vec-{}-{nanos}", std::process::id()));
        let result = (|| {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(&self.to_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Go's `Load` on a file's bytes, with Go's errors ("EOF", "unexpected EOF" as io.ReadFull gives them).
    pub fn from_bytes(data: &[u8]) -> Result<Index, String> {
        if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
            return Err("vecindex: not an index file".into());
        }
        let mut at = MAGIC.len();
        let mut take = |n: usize| -> Result<&[u8], String> {
            let rest = &data[at..];
            if n > 0 && rest.is_empty() {
                return Err("EOF".into());
            }
            if rest.len() < n {
                return Err("unexpected EOF".into());
            }
            at += n;
            Ok(&rest[..n])
        };
        let dim = u32::from_le_bytes(take(4)?.try_into().unwrap_or_default());
        let bits = u32::from_le_bytes(take(4)?.try_into().unwrap_or_default());
        let seed = i64::from_le_bytes(take(8)?.try_into().unwrap_or_default());
        let n = u64::from_le_bytes(take(8)?.try_into().unwrap_or_default());
        let mut x = Index::new(dim as usize, bits as usize, seed)?;
        if n > 1 << 32 || n * x.per as u64 > 1 << 36 {
            return Err(format!("vecindex: implausible size {n}"));
        }
        let n = n as usize;
        x.ids = take(n * 8)?.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap_or_default())).collect();
        x.norms = take(n * 4)?.chunks_exact(4).map(|c| f32::from_bits(u32::from_le_bytes(c.try_into().unwrap_or_default()))).collect();
        x.codes = take(n * x.per)?.to_vec();
        for (i, &id) in x.ids.iter().enumerate() {
            x.slot.insert(id, i);
        }
        Ok(x)
    }

    /// Go's `Load`.
    pub fn load(path: &std::path::Path) -> Result<Index, String> {
        let data = std::fs::read(path).map_err(|e| crate::goerr::path("open", path.as_os_str().as_encoded_bytes(), &e))?;
        Index::from_bytes(&data)
    }
}

// Go's container/heap on a min-heap of hits by score.
fn heap_up(h: &mut [Hit], mut j: usize) {
    while j > 0 {
        let i = (j - 1) / 2;
        if h[j].score.partial_cmp(&h[i].score) != Some(std::cmp::Ordering::Less) {
            break;
        }
        h.swap(i, j);
        j = i;
    }
}

fn heap_down(h: &mut [Hit], i0: usize, n: usize) -> bool {
    let mut i = i0;
    loop {
        let j1 = 2 * i + 1;
        if j1 >= n {
            break;
        }
        let mut j = j1;
        let j2 = j1 + 1;
        if j2 < n && h[j2].score < h[j1].score {
            j = j2;
        }
        if h[j].score.partial_cmp(&h[i].score) != Some(std::cmp::Ordering::Less) {
            break;
        }
        h.swap(i, j);
        i = j;
    }
    i > i0
}
