//! Go: `cryptotools/hash.go` and `HashReport`: crypto_hash's digests (MD5, SHA-1, SHA-2, SHA-3, CRC-32), HMACs, and the
//! comparison with an expected digest. MD5 and SHA-1 come from RustCrypto's crates and SHA-2 from `sha2`; SHA-3
//! (Keccak-f[1600]), CRC-32 (IEEE) and HMAC are written out here. All proved against Go's (testdata/parity/cryptohash.json).

use crate::gotext;
use sha2::Digest as _;

/// Go's `Algorithms`.
pub const ALGORITHMS: [&str; 9] = ["md5", "sha1", "sha224", "sha256", "sha384", "sha512", "sha3-256", "sha3-512", "crc32"];

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Algo {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha3_256,
    Sha3_512,
    Crc32,
}

/// Go's `newHash`: the algorithm a name means ("" is sha256).
fn algo(name: &str) -> Result<Algo, String> {
    Ok(match gotext::to_lower(&name.replace('_', "-")).as_str() {
        "md5" => Algo::Md5,
        "sha1" | "sha-1" => Algo::Sha1,
        "sha224" | "sha-224" => Algo::Sha224,
        "sha256" | "sha-256" | "" => Algo::Sha256,
        "sha384" | "sha-384" => Algo::Sha384,
        "sha512" | "sha-512" => Algo::Sha512,
        "sha3-256" | "sha3" => Algo::Sha3_256,
        "sha3-512" => Algo::Sha3_512,
        "crc32" => Algo::Crc32,
        _ => return Err(format!("unknown algorithm {} (one of {})", gotext::quote(name), ALGORITHMS.join(", "))),
    })
}

/// The block size HMAC pads its key to (Go's `BlockSize()`; SHA-3's is its rate).
fn block_size(a: Algo) -> usize {
    match a {
        Algo::Sha384 | Algo::Sha512 => 128,
        Algo::Sha3_256 => 136,
        Algo::Sha3_512 => 72,
        _ => 64,
    }
}

pub(crate) fn sum(a: Algo, data: &[u8]) -> Vec<u8> {
    match a {
        Algo::Md5 => md5::Md5::digest(data).to_vec(),
        Algo::Sha1 => sha1::Sha1::digest(data).to_vec(),
        Algo::Sha224 => sha2::Sha224::digest(data).to_vec(),
        Algo::Sha256 => sha2::Sha256::digest(data).to_vec(),
        Algo::Sha384 => sha2::Sha384::digest(data).to_vec(),
        Algo::Sha512 => sha2::Sha512::digest(data).to_vec(),
        Algo::Sha3_256 => sha3(data, 136, 32),
        Algo::Sha3_512 => sha3(data, 72, 64),
        Algo::Crc32 => crc32(data).to_be_bytes().to_vec(),
    }
}

/// HMAC (RFC 2104) over `a`, as Go's crypto/hmac: a key longer than the block is hashed first.
pub(crate) fn hmac(a: Algo, key: &[u8], data: &[u8]) -> Vec<u8> {
    let bs = block_size(a);
    let mut k = if key.len() > bs { sum(a, key) } else { key.to_vec() };
    k.resize(bs, 0);
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(data);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sum(a, &inner));
    sum(a, &outer)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Go's `Digest`: the hex digest of data (an HMAC with a key).
pub fn digest(name: &str, data: &[u8], key: &[u8]) -> Result<String, String> {
    let a = algo(name)?;
    if !key.is_empty() {
        if gotext::equal_fold(name, "crc32") {
            return Err("CRC32 is a checksum, not a hash: it has no HMAC".into());
        }
        return Ok(hex(&hmac(a, key, data)));
    }
    Ok(hex(&sum(a, data)))
}

/// Go's `stripSep`: spaces, newlines, tabs, returns, colons, commas and hyphens taken out.
pub fn strip_sep(s: &str) -> String {
    s.chars().filter(|r| !matches!(r, ' ' | '\n' | '\t' | '\r' | ':' | ',' | '-')).collect()
}

/// Go's `Compare`: whether two digests are the same (separators and case ignored), and where they differ.
pub fn compare(got: &str, expected: &str) -> (bool, String) {
    let (a, b) = (gotext::to_lower(&strip_sep(got)), gotext::to_lower(&strip_sep(expected)));
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return (false, format!("different lengths ({} and {} hex digits): not the same kind of digest", a.len(), b.len()));
    }
    if a == b {
        return (true, "identical".into());
    }
    match a.iter().zip(b).position(|(x, y)| x != y) {
        Some(i) => (false, format!("they differ from hex digit {} on (byte {})", i + 1, i / 2)),
        None => (false, "they differ".into()),
    }
}

/// Go's `WhichMatches`: the algorithms whose digest of data is the expected one, sorted.
pub fn which_matches(data: &[u8], expected: &str) -> Vec<String> {
    let mut hits: Vec<String> =
        ALGORITHMS.iter().filter(|a| digest(a, data, &[]).is_ok_and(|d| compare(&d, expected).0)).map(|a| a.to_string()).collect();
    hits.sort();
    hits
}

/// Go's `HashReport`: crypto_hash's answer.
pub fn hash_report(data: &[u8], src: &str, algo: &str, key: &str, expected: &str) -> String {
    let algo = if algo.is_empty() { "sha256" } else { algo };
    let d = match digest(algo, data, key.as_bytes()) {
        Ok(d) => d,
        Err(e) => return e,
    };
    let mut what = super::to_upper(algo);
    if !key.is_empty() {
        what = format!("HMAC-{what}");
    }
    let mut b = format!("{what} of the {src} ({} bytes): {d}\n", data.len());
    if !expected.is_empty() {
        let (same, detail) = compare(&d, expected);
        b += &format!("Compared with the expected digest: {} ({detail}).\n", if same { "MATCH" } else { "NO MATCH" });
        if !same && key.is_empty() {
            let hits = which_matches(data, expected);
            if !hits.is_empty() {
                b += &format!("The expected digest IS the {} of this data: it matches with that algorithm.\n", hits.join("/"));
            } else {
                b += "No algorithm I know gives the expected digest for this data: the data differs from what was hashed (or a key was involved).\n";
            }
        }
    }
    b
}

// ---- CRC-32 (IEEE) ------------------------------------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 == 1 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

// ---- SHA-3 (FIPS 202) ---------------------------------------------------------------------------------------------------

const RC: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];
const ROT: [u32; 25] = [0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14];

fn keccak_f(a: &mut [u64; 25]) {
    for rc in RC {
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = a[x] ^ a[x + 5] ^ a[x + 10] ^ a[x + 15] ^ a[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                a[x + 5 * y] ^= d;
            }
        }
        let mut b = [0u64; 25];
        for x in 0..5 {
            for y in 0..5 {
                b[y + 5 * ((2 * x + 3 * y) % 5)] = a[x + 5 * y].rotate_left(ROT[x + 5 * y]);
            }
        }
        for x in 0..5 {
            for y in 0..5 {
                a[x + 5 * y] = b[x + 5 * y] ^ (!b[(x + 1) % 5 + 5 * y] & b[(x + 2) % 5 + 5 * y]);
            }
        }
        a[0] ^= rc;
    }
}

fn sha3(data: &[u8], rate: usize, out: usize) -> Vec<u8> {
    let mut st = [0u64; 25];
    let mut msg = data.to_vec();
    msg.push(0x06);
    while !msg.len().is_multiple_of(rate) {
        msg.push(0);
    }
    let last = msg.len() - 1;
    msg[last] |= 0x80;
    for block in msg.chunks(rate) {
        for (i, w) in block.chunks(8).enumerate() {
            st[i] ^= u64::from_le_bytes(w.try_into().unwrap_or([0; 8]));
        }
        keccak_f(&mut st);
    }
    st.iter().flat_map(|w| w.to_le_bytes()).take(out).collect()
}
