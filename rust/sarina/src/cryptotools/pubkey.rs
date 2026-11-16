//! Go's crypto/x509 public keys (`ParsePKIXPublicKey`, `ParsePKCS1PublicKey`, `parsePublicKey`) with its error texts,
//! and the signature checks crypto_modern makes with them: RSA PKCS#1 v1.5 and PSS (crypto/rsa's refusals included:
//! keys under 1024 bits, an even modulus or exponent) and ECDSA on P-224, P-256, P-384 and P-521.

use super::asn1::{right_align, tag, unmarshal, Der, Int, Ty, Val, P};
use super::bigint::Nat;
use super::hash::{sum, Algo};

/// A NIST prime curve y² = x³ - 3x + b.
pub(crate) struct Curve {
    pub name: &'static str,
    /// The names its errors use ("P256", and "P256 element" or "P384Element").
    short: &'static str,
    element: &'static str,
    pub bits: usize,
    p: &'static str,
    n: &'static str,
    b: &'static str,
    gx: &'static str,
    gy: &'static str,
}

pub(crate) static CURVES: [Curve; 4] = [
    Curve {
        name: "P-224",
        short: "P224",
        element: "P224Element",
        bits: 224,
        p: "ffffffffffffffffffffffffffffffff000000000000000000000001",
        n: "ffffffffffffffffffffffffffff16a2e0b8f03e13dd29455c5c2a3d",
        b: "b4050a850c04b3abf54132565044b0b7d7bfd8ba270b39432355ffb4",
        gx: "b70e0cbd6bb4bf7f321390b94a03c1d356c21122343280d6115c1d21",
        gy: "bd376388b5f723fb4c22dfe6cd4375a05a07476444d5819985007e34",
    },
    Curve {
        name: "P-256",
        short: "P256",
        element: "P256 element",
        bits: 256,
        p: "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
        n: "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551",
        b: "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
        gx: "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
        gy: "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
    },
    Curve {
        name: "P-384",
        short: "P384",
        element: "P384Element",
        bits: 384,
        p: "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
        n: "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
        b: "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
        gx: "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
        gy: "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
    },
    Curve {
        name: "P-521",
        short: "P521",
        element: "P521Element",
        bits: 521,
        p: "01ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        n: "01fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa51868783bf2f966b7fcc0148f709a5d03bb5c9b8899c47aebb6fb71e91386409",
        b: "0051953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8b489918ef109e156193951ec7e937b1652c0bd3bb1bf073573df883d2c34f1ef451fd46b503f00",
        gx: "00c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe75928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66",
        gy: "011839296a789a3bc0045c8a5fb42c7d1bd998f54449579b446817afbd17273e662c97ee72995ef42640c550b9013fad0761353c7086a272c24088be94769fd16650",
    },
];

fn hex(s: &str) -> Nat {
    Nat::from_be(&(0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("curve constant")).collect::<Vec<_>>())
}

impl Curve {
    fn size(&self) -> usize {
        self.bits.div_ceil(8)
    }
}

/// A point in Jacobian coordinates (Z = 0 is the point at infinity).
#[derive(Clone)]
struct Jac {
    x: Nat,
    y: Nat,
    z: Nat,
}

struct Field {
    p: Nat,
}

impl Field {
    fn add(&self, a: &Nat, b: &Nat) -> Nat {
        let s = a.add(b);
        if s >= self.p {
            s.sub(&self.p)
        } else {
            s
        }
    }
    fn sub(&self, a: &Nat, b: &Nat) -> Nat {
        if a >= b {
            a.sub(b)
        } else {
            a.add(&self.p).sub(b)
        }
    }
    fn mul(&self, a: &Nat, b: &Nat) -> Nat {
        a.mul_mod(b, &self.p)
    }
    fn small(&self, k: u64, a: &Nat) -> Nat {
        a.mul_mod(&Nat::from_u64(k), &self.p)
    }

    fn double(&self, q: &Jac) -> Jac {
        if q.z.is_zero() || q.y.is_zero() {
            return Jac { x: Nat::from_u64(1), y: Nat::from_u64(1), z: Nat::zero() };
        }
        let delta = self.mul(&q.z, &q.z);
        let gamma = self.mul(&q.y, &q.y);
        let beta = self.mul(&q.x, &gamma);
        let alpha = self.small(3, &self.mul(&self.sub(&q.x, &delta), &self.add(&q.x, &delta)));
        let x3 = self.sub(&self.mul(&alpha, &alpha), &self.small(8, &beta));
        let yz = self.add(&q.y, &q.z);
        let z3 = self.sub(&self.sub(&self.mul(&yz, &yz), &gamma), &delta);
        let y3 = self.sub(&self.mul(&alpha, &self.sub(&self.small(4, &beta), &x3)), &self.small(8, &self.mul(&gamma, &gamma)));
        Jac { x: x3, y: y3, z: z3 }
    }

    fn add_points(&self, a: &Jac, b: &Jac) -> Jac {
        if a.z.is_zero() {
            return b.clone();
        }
        if b.z.is_zero() {
            return a.clone();
        }
        let z1z1 = self.mul(&a.z, &a.z);
        let z2z2 = self.mul(&b.z, &b.z);
        let u1 = self.mul(&a.x, &z2z2);
        let u2 = self.mul(&b.x, &z1z1);
        let s1 = self.mul(&self.mul(&a.y, &b.z), &z2z2);
        let s2 = self.mul(&self.mul(&b.y, &a.z), &z1z1);
        let h = self.sub(&u2, &u1);
        let r = self.small(2, &self.sub(&s2, &s1));
        if h.is_zero() {
            if r.is_zero() {
                return self.double(a);
            }
            return Jac { x: Nat::from_u64(1), y: Nat::from_u64(1), z: Nat::zero() };
        }
        let h2 = self.small(2, &h);
        let i = self.mul(&h2, &h2);
        let j = self.mul(&h, &i);
        let v = self.mul(&u1, &i);
        let x3 = self.sub(&self.sub(&self.mul(&r, &r), &j), &self.small(2, &v));
        let y3 = self.sub(&self.mul(&r, &self.sub(&v, &x3)), &self.small(2, &self.mul(&s1, &j)));
        let zs = self.add(&a.z, &b.z);
        let z3 = self.mul(&self.sub(&self.sub(&self.mul(&zs, &zs), &z1z1), &z2z2), &h);
        Jac { x: x3, y: y3, z: z3 }
    }

    fn scalar(&self, k: &Nat, q: &Jac) -> Jac {
        let mut acc = Jac { x: Nat::from_u64(1), y: Nat::from_u64(1), z: Nat::zero() };
        for i in (0..k.bit_len()).rev() {
            acc = self.double(&acc);
            if k.bit(i) {
                acc = self.add_points(&acc, q);
            }
        }
        acc
    }

    /// The affine x, or None at infinity.
    fn affine_x(&self, q: &Jac) -> Option<Nat> {
        if q.z.is_zero() {
            return None;
        }
        let two = Nat::from_u64(2);
        let zinv = q.z.pow_mod(&self.p.sub(&two), &self.p);
        Some(self.mul(&q.x, &self.mul(&zinv, &zinv)))
    }
}

/// A parsed public key, as far as crypto_modern tells them apart.
pub(crate) enum PubKey {
    Rsa {
        n: Int,
        e: i64,
    },
    Ecdsa {
        curve: &'static Curve,
        x: Nat,
        y: Nat,
    },
    Ed25519,
    /// Another kind Go parses, by its `%T` name.
    Other(&'static str),
}

/// Go's `keyDesc`.
pub(crate) fn key_desc(k: &PubKey) -> String {
    match k {
        PubKey::Rsa { n, .. } => {
            let bits = n.mag.bit_len();
            format!("RSA {bits} bits{}", if bits < 2048 { " (under 2048: too small for today)" } else { "" })
        }
        PubKey::Ecdsa { curve, .. } => format!("ECDSA {}", curve.name),
        PubKey::Ed25519 => "Ed25519".into(),
        PubKey::Other(t) => (*t).into(),
    }
}

const RSA: &[i64] = &[1, 2, 840, 113549, 1, 1, 1];
const DSA: &[i64] = &[1, 2, 840, 10040, 4, 1];
const EC: &[i64] = &[1, 2, 840, 10045, 2, 1];
const X25519: &[i64] = &[1, 3, 101, 110];
const ED25519: &[i64] = &[1, 3, 101, 112];
const MLDSA: [&[i64]; 3] = [&[2, 16, 840, 1, 101, 3, 4, 3, 17], &[2, 16, 840, 1, 101, 3, 4, 3, 18], &[2, 16, 840, 1, 101, 3, 4, 3, 19]];
const MLDSA_SIZES: [usize; 3] = [1312, 1952, 2592];
const CURVE_OIDS: [&[i64]; 4] = [&[1, 3, 132, 0, 33], &[1, 2, 840, 10045, 3, 1, 7], &[1, 3, 132, 0, 34], &[1, 3, 132, 0, 35]];

fn x509(m: &str) -> String {
    format!("x509: {m}")
}

/// Go's `ecdsa.ParseUncompressedPublicKey` (through nistec's SetBytes).
fn parse_point(c: &'static Curve, data: &[u8]) -> Result<PubKey, String> {
    if data.first() != Some(&4) {
        return Err("ecdsa: invalid uncompressed public key".into());
    }
    let size = c.size();
    if data.len() != 1 + 2 * size {
        return Err(format!("invalid {} point encoding", c.short));
    }
    let p = hex(c.p);
    let (x, y) = (Nat::from_be(&data[1..1 + size]), Nat::from_be(&data[1 + size..]));
    if x >= p || y >= p {
        return Err(format!("invalid {} encoding", c.element));
    }
    let f = Field { p };
    let rhs = f.add(&f.sub(&f.mul(&f.mul(&x, &x), &x), &f.small(3, &x)), &hex(c.b));
    if f.mul(&y, &y) != rhs {
        return Err(format!("{} point not on curve", c.short));
    }
    Ok(PubKey::Ecdsa { curve: c, x, y })
}

/// Go's `parsePublicKey`: the algorithm's OID, its parameters' full bytes (empty when absent) and the key's bits.
pub(crate) fn parse_public_key(oid: &[i64], params: &[u8], key: &[u8], bit_len: usize) -> Result<PubKey, String> {
    let data = right_align(key, bit_len);
    if oid == RSA {
        if params != [5, 0] {
            return Err(x509("RSA key missing NULL parameters"));
        }
        let mut der = Der(&data);
        let mut der = der.read(tag::SEQUENCE).ok_or_else(|| x509("invalid RSA public key"))?;
        let n = der.read_big_int().ok_or_else(|| x509("invalid RSA modulus"))?;
        let e = der.read_i64().ok_or_else(|| x509("invalid RSA public exponent"))?;
        if n.not_positive() {
            return Err(x509("RSA modulus is not a positive number"));
        }
        if e <= 0 {
            return Err(x509("RSA public exponent is not a positive number"));
        }
        return Ok(PubKey::Rsa { n, e });
    }
    if oid == EC {
        let curve_oid = Der(params).read_oid().ok_or_else(|| x509("invalid ECDSA parameters"))?;
        let Some(i) = CURVE_OIDS.iter().position(|o| *o == curve_oid.as_slice()) else {
            return Err(x509("unsupported elliptic curve"));
        };
        return parse_point(&CURVES[i], &data);
    }
    if oid == ED25519 {
        if !params.is_empty() {
            return Err(x509("Ed25519 key encoded with illegal parameters"));
        }
        if data.len() != 32 {
            return Err(x509("wrong Ed25519 public key size"));
        }
        return Ok(PubKey::Ed25519);
    }
    if let Some(i) = MLDSA.iter().position(|o| *o == oid) {
        if !params.is_empty() {
            return Err(x509("ML-DSA key encoded with illegal parameters"));
        }
        if data.len() != MLDSA_SIZES[i] {
            return Err("mldsa: invalid public key length".into());
        }
        return Ok(PubKey::Other("*mldsa.PublicKey"));
    }
    if oid == X25519 {
        if !params.is_empty() {
            return Err(x509("X25519 key encoded with illegal parameters"));
        }
        if data.len() != 32 {
            return Err("crypto/ecdh: invalid public key".into());
        }
        return Ok(PubKey::Other("*ecdh.PublicKey"));
    }
    if oid == DSA {
        let y = Der(&data).read_big_int().ok_or_else(|| x509("invalid DSA public key"))?;
        let mut pd = Der(params);
        let pqg = pd.read(tag::SEQUENCE).and_then(|mut s| Some([s.read_big_int()?, s.read_big_int()?, s.read_big_int()?]));
        let Some(pqg) = pqg else {
            return Err(x509("invalid DSA parameters"));
        };
        if y.not_positive() || pqg.iter().any(Int::not_positive) {
            return Err(x509("zero or negative DSA parameter"));
        }
        return Ok(PubKey::Other("*dsa.PublicKey"));
    }
    Err(x509("unknown public key algorithm"))
}

/// pkix.AlgorithmIdentifier.
pub(crate) fn ai_ty() -> Ty {
    Ty::Struct("AlgorithmIdentifier", vec![(Ty::Oid, P::default()), (Ty::RawValue, P { optional: true, ..P::default() })])
}

/// x509's publicKeyInfo.
pub(crate) fn spki_ty() -> Ty {
    Ty::Struct("publicKeyInfo", vec![(ai_ty(), P::default()), (Ty::BitString, P::default())])
}

fn pkcs1_ty() -> Ty {
    Ty::Struct("pkcs1PublicKey", vec![(Ty::BigInt, P::default()), (Ty::Int, P::default())])
}

/// The publicKeyInfo fields: (algorithm OID, parameters' full bytes, key bytes, bit length).
pub(crate) type Spki = (Vec<i64>, Vec<u8>, Vec<u8>, usize);

pub(crate) fn spki_fields(v: Val) -> Spki {
    let Val::Struct(f) = v else { unreachable!("publicKeyInfo is a struct") };
    let mut f = f.into_iter();
    let (Some(Val::Struct(ai)), Some(Val::BitString(key, bits))) = (f.next(), f.next()) else { unreachable!("publicKeyInfo fields") };
    let mut ai = ai.into_iter();
    let Some(Val::Oid(oid)) = ai.next() else { unreachable!("AlgorithmIdentifier's OID") };
    let params = match ai.next() {
        Some(Val::Raw(b)) => b,
        _ => Vec::new(),
    };
    (oid, params, key, bits)
}

/// Go's `x509.ParsePKIXPublicKey`.
pub(crate) fn parse_pkix(der: &[u8]) -> Result<PubKey, String> {
    let (v, rest) = match unmarshal(der, &spki_ty()) {
        Ok(x) => x,
        Err(e) => {
            if unmarshal(der, &pkcs1_ty()).is_ok() {
                return Err(x509("failed to parse public key (use ParsePKCS1PublicKey instead for this key format)"));
            }
            return Err(e);
        }
    };
    if !rest.is_empty() {
        return Err(x509("trailing data after ASN.1 of public-key"));
    }
    let (oid, params, key, bits) = spki_fields(v);
    parse_public_key(&oid, &params, &key, bits)
}

/// Go's `x509.ParsePKCS1PublicKey`.
pub(crate) fn parse_pkcs1(der: &[u8]) -> Result<PubKey, String> {
    let (v, rest) = match unmarshal(der, &pkcs1_ty()) {
        Ok(x) => x,
        Err(e) => {
            if unmarshal(der, &spki_ty()).is_ok() {
                return Err(x509("failed to parse public key (use ParsePKIXPublicKey instead for this key format)"));
            }
            return Err(e);
        }
    };
    if !rest.is_empty() {
        return Err("asn1: syntax error: trailing data".into());
    }
    let Val::Struct(f) = v else { unreachable!("pkcs1PublicKey is a struct") };
    let (Val::BigInt(n), Val::Int(e)) = (&f[0], &f[1]) else { unreachable!("pkcs1PublicKey fields") };
    if n.not_positive() || *e <= 0 {
        return Err(x509("public key contains zero or negative value"));
    }
    if *e > (1 << 31) - 1 {
        return Err(x509("public key contains large public exponent"));
    }
    Ok(PubKey::Rsa { n: n.clone(), e: *e })
}

/// The DigestInfo prefix PKCS#1 v1.5 puts before a digest.
fn digest_prefix(a: Algo) -> &'static [u8] {
    match a {
        Algo::Sha384 => &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30],
        Algo::Sha512 => &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40],
        _ => &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20],
    }
}

/// crypto/rsa's checks before any arithmetic, and `encrypt`: sig^e mod n as `size` bytes, or None.
fn rsa_open(n: &Nat, e: i64, sig: &[u8]) -> Option<Vec<u8>> {
    if n.bit_len() < 1024 || !n.is_odd() || e < 2 || e & 1 == 0 || e > (1 << 31) - 1 {
        return None;
    }
    let size = n.bit_len().div_ceil(8);
    let m = Nat::from_be(sig);
    if sig.len() != size || &m >= n {
        return None;
    }
    Some(m.pow_mod(&Nat::from_u64(e as u64), n).to_be(size))
}

/// Go's `rsa.VerifyPKCS1v15(key, hash, digest, sig) == nil`.
pub(crate) fn rsa_pkcs1v15(n: &Nat, e: i64, a: Algo, digest: &[u8], sig: &[u8]) -> bool {
    let Some(em) = rsa_open(n, e, sig) else {
        return false;
    };
    let prefix = digest_prefix(a);
    let k = em.len();
    if k < prefix.len() + digest.len() + 2 + 8 + 1 {
        return false;
    }
    let mut want = vec![0u8; k];
    want[1] = 1;
    for b in &mut want[2..k - prefix.len() - digest.len() - 1] {
        *b = 0xff;
    }
    want[k - prefix.len() - digest.len()..k - digest.len()].copy_from_slice(prefix);
    want[k - digest.len()..].copy_from_slice(digest);
    em == want
}

/// Go's `rsa.VerifyPSS(key, hash, digest, sig, nil) == nil` (the salt length found from the signature).
pub(crate) fn rsa_pss(n: &Nat, e: i64, a: Algo, digest: &[u8], sig: &[u8]) -> bool {
    let Some(mut em) = rsa_open(n, e, sig) else {
        return false;
    };
    let em_bits = n.bit_len() - 1;
    let em_len = em_bits.div_ceil(8);
    while em.len() > em_len {
        if em[0] != 0 {
            return false;
        }
        em.remove(0);
    }
    let h_len = digest.len();
    if em_len < h_len + 1 || em[em_len - 1] != 0xbc {
        return false;
    }
    let (db, rest) = em.split_at_mut(em_len - h_len - 1);
    let h = &rest[..h_len];
    let mask = 0xffu8 >> (8 * em_len - em_bits);
    if db[0] & !mask != 0 {
        return false;
    }
    let mut counter = 0u32;
    let mut done = 0;
    while done < db.len() {
        let mut seed = h.to_vec();
        seed.extend_from_slice(&counter.to_be_bytes());
        for x in sum(a, &seed) {
            if done == db.len() {
                break;
            }
            db[done] ^= x;
            done += 1;
        }
        counter += 1;
    }
    db[0] &= mask;
    let Some(ps_len) = db.iter().position(|&b| b == 1) else {
        return false;
    };
    if db[..ps_len].iter().any(|&b| b != 0) {
        return false;
    }
    let salt = &db[ps_len + 1..];
    let mut m = vec![0u8; 8];
    m.extend_from_slice(digest);
    m.extend_from_slice(salt);
    sum(a, &m) == h
}

/// Go's `ecdsa.Verify(key, digest, r, s)` with r and s from the signature's two halves.
pub(crate) fn ecdsa_verify(c: &Curve, qx: &Nat, qy: &Nat, digest: &[u8], r: &[u8], s: &[u8]) -> bool {
    let n = hex(c.n);
    let (r, s) = (Nat::from_be(r), Nat::from_be(s));
    if r.is_zero() || s.is_zero() || r >= n || s >= n {
        return false;
    }
    let size = n.bit_len().div_ceil(8);
    let mut e = Nat::from_be(&digest[..digest.len().min(size)]);
    if digest.len() >= size {
        let excess = size * 8 - n.bit_len();
        if excess > 0 {
            e = e.divrem(&Nat::from_u64(1 << excess)).0;
        }
    }
    let e = e.rem(&n);
    let w = s.pow_mod(&n.sub(&Nat::from_u64(2)), &n);
    let (u1, u2) = (e.mul_mod(&w, &n), r.mul_mod(&w, &n));
    let f = Field { p: hex(c.p) };
    let one = Nat::from_u64(1);
    let g = Jac { x: hex(c.gx), y: hex(c.gy), z: one.clone() };
    let q = Jac { x: qx.clone(), y: qy.clone(), z: one };
    let sum = f.add_points(&f.scalar(&u1, &g), &f.scalar(&u2, &q));
    f.affine_x(&sum).is_some_and(|x| x.rem(&n) == r)
}
