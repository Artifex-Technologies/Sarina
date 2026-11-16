//! DER as Go reads it, in its two readers: `cryptobyte` (crypto/x509's parser; true or false, no reasons) as [`Der`], and
//! `encoding/asn1`'s Unmarshal (its reasons word for word) for the few structures crypto/x509 still reads through it, as
//! [`unmarshal`] over a [`Ty`] schema.

use super::bigint::Nat;

/// A signed big integer as DER holds it: the sign and the magnitude.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Int {
    pub neg: bool,
    pub mag: Nat,
}

impl Int {
    /// Go's `Sign() <= 0`.
    pub(crate) fn not_positive(&self) -> bool {
        self.neg || self.mag.is_zero()
    }

    /// Go's big.Int String.
    pub(crate) fn decimal(&self) -> String {
        format!("{}{}", if self.neg { "-" } else { "" }, self.mag.decimal())
    }
}

/// Two's-complement bytes (already checked minimal and non-empty).
fn int_from(b: &[u8]) -> Int {
    if b[0] & 0x80 == 0 {
        return Int { neg: false, mag: Nat::from_be(b) };
    }
    let inv: Vec<u8> = b.iter().map(|x| !x).collect();
    Int { neg: true, mag: Nat::from_be(&inv).add(&Nat::from_u64(1)) }
}

/// Go's `checkASN1Integer` (cryptobyte) and `checkInteger` (encoding/asn1) agree.
fn minimal_int(b: &[u8]) -> bool {
    !(b.is_empty() || b.len() > 1 && (b[0] == 0 && b[1] & 0x80 == 0 || b[0] == 0xff && b[1] & 0x80 == 0x80))
}

fn signed64(b: &[u8]) -> i64 {
    let mut v: i64 = 0;
    for &x in b {
        v = (v << 8) | i64::from(x);
    }
    if b.len() < 8 && !b.is_empty() {
        let sh = 64 - 8 * b.len() as u32;
        v = (v << sh) >> sh;
    }
    v
}

pub(crate) mod tag {
    pub const BOOLEAN: u8 = 1;
    pub const INTEGER: u8 = 2;
    pub const BIT_STRING: u8 = 3;
    pub const OCTET_STRING: u8 = 4;
    pub const NULL: u8 = 5;
    pub const OID: u8 = 6;
    pub const UTF8: u8 = 12;
    pub const SEQUENCE: u8 = 0x30;
    pub const SET: u8 = 0x31;
    pub const PRINTABLE: u8 = 19;
    pub const T61: u8 = 20;
    pub const IA5: u8 = 22;
    pub const UTC_TIME: u8 = 23;
    pub const GENERALIZED_TIME: u8 = 24;
    pub const NUMERIC: u8 = 18;
    pub const BMP: u8 = 30;
}

/// cryptobyte.String: a cursor over DER.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Der<'a>(pub &'a [u8]);

impl<'a> Der<'a> {
    pub(crate) fn empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Go's `readASN1`: (tag, the element with or without its header). Nothing is consumed on failure.
    fn read_any_inner(&mut self, skip_header: bool) -> Option<(u8, &'a [u8])> {
        let s = self.0;
        if s.len() < 2 {
            return None;
        }
        let (t, lb) = (s[0], s[1]);
        if t & 0x1f == 0x1f {
            return None;
        }
        let (length, header) = if lb & 0x80 == 0 {
            (usize::from(lb) + 2, 2usize)
        } else {
            let ll = usize::from(lb & 0x7f);
            if ll == 0 || ll > 4 || s.len() < 2 + ll {
                return None;
            }
            let mut l32: u64 = 0;
            for &b in &s[2..2 + ll] {
                l32 = (l32 << 8) | u64::from(b);
            }
            if l32 < 128 || l32 >> ((ll - 1) * 8) == 0 {
                return None;
            }
            (2 + ll + l32 as usize, 2 + ll)
        };
        if s.len() < length {
            return None;
        }
        self.0 = &s[length..];
        Some((t, if skip_header { &s[header..length] } else { &s[..length] }))
    }

    pub(crate) fn read_any(&mut self) -> Option<(u8, Der<'a>)> {
        self.read_any_inner(true).map(|(t, b)| (t, Der(b)))
    }

    pub(crate) fn read_any_element(&mut self) -> Option<(u8, &'a [u8])> {
        self.read_any_inner(false)
    }

    /// Go's `ReadASN1`: the contents of an element with this tag (a different tag consumes the element and fails).
    pub(crate) fn read(&mut self, t: u8) -> Option<Der<'a>> {
        match self.read_any()? {
            (got, d) if got == t => Some(d),
            _ => None,
        }
    }

    pub(crate) fn read_element(&mut self, t: u8) -> Option<&'a [u8]> {
        match self.read_any_element()? {
            (got, d) if got == t => Some(d),
            _ => None,
        }
    }

    pub(crate) fn peek(&self, t: u8) -> bool {
        self.0.first() == Some(&t)
    }

    pub(crate) fn read_big_int(&mut self) -> Option<Int> {
        let b = self.read(tag::INTEGER)?;
        minimal_int(b.0).then(|| int_from(b.0))
    }

    pub(crate) fn read_i64(&mut self) -> Option<i64> {
        self.read_i64_tag(tag::INTEGER)
    }

    pub(crate) fn read_i64_tag(&mut self, t: u8) -> Option<i64> {
        let b = self.read(t)?;
        (minimal_int(b.0) && b.0.len() <= 8).then(|| signed64(b.0))
    }

    /// Go's `readASN1Uint64` into a uint.
    pub(crate) fn read_u64(&mut self) -> Option<u64> {
        let b = self.read(tag::INTEGER)?.0;
        if !minimal_int(b) || b.len() > 9 || b.len() == 9 && b[0] != 0 || b[0] & 0x80 != 0 {
            return None;
        }
        Some(b.iter().fold(0u64, |v, &x| (v << 8) | u64::from(x)))
    }

    /// Go's `ReadOptionalASN1Integer` into an int with a default.
    pub(crate) fn read_optional_i64(&mut self, t: u8, default: i64) -> Option<i64> {
        if !self.peek(t) {
            return Some(default);
        }
        let mut inner = self.read(t)?;
        let v = inner.read_i64()?;
        inner.empty().then_some(v)
    }

    pub(crate) fn read_oid(&mut self) -> Option<Vec<i64>> {
        let b = self.read(tag::OID)?.0;
        if b.is_empty() {
            return None;
        }
        let mut cur = b;
        let first = base128(&mut cur)?;
        let mut out = if first < 80 { vec![first / 40, first % 40] } else { vec![2, first - 80] };
        while !cur.is_empty() {
            out.push(base128(&mut cur)?);
        }
        Some(out)
    }

    pub(crate) fn read_bool(&mut self) -> Option<bool> {
        let b = self.read(tag::BOOLEAN)?.0;
        match b {
            [0] => Some(false),
            [0xff] => Some(true),
            _ => None,
        }
    }

    /// Go's `ReadASN1BitString`: (bytes, bit length).
    pub(crate) fn read_bit_string(&mut self) -> Option<(&'a [u8], usize)> {
        let b = self.read(tag::BIT_STRING)?.0;
        if b.is_empty() {
            return None;
        }
        let pad = b[0];
        if pad > 7 || b.len() == 1 && pad > 0 || b[b.len() - 1] & ((1u8 << pad) - 1) != 0 {
            return None;
        }
        Some((&b[1..], (b.len() - 1) * 8 - usize::from(pad)))
    }
}

/// cryptobyte's `readBase128Int`.
fn base128(s: &mut &[u8]) -> Option<i64> {
    let mut ret: i64 = 0;
    let mut i = 0;
    while let Some((&b, rest)) = s.split_first() {
        if i == 5 || ret >= 1 << (31 - 7) {
            return None;
        }
        ret <<= 7;
        *s = rest;
        if i == 0 && b == 0x80 {
            return None;
        }
        ret |= i64::from(b & 0x7f);
        if b & 0x80 == 0 {
            return Some(ret);
        }
        i += 1;
    }
    None
}

/// asn1.BitString's RightAlign.
pub(crate) fn right_align(bytes: &[u8], bit_len: usize) -> Vec<u8> {
    let shift = 8 - (bit_len % 8) as u32;
    if shift == 8 || bytes.is_empty() {
        return bytes.to_vec();
    }
    let mut a = vec![0u8; bytes.len()];
    a[0] = bytes[0] >> shift;
    for i in 1..bytes.len() {
        a[i] = (bytes[i - 1] << (8 - shift)) | (bytes[i] >> shift);
    }
    a
}

// ---- encoding/asn1's Unmarshal, for fixed structures ----

/// A Go type encoding/asn1 fills: the field kinds crypto/x509's structures use.
pub(crate) enum Ty {
    /// A struct: its Go name ("" for an anonymous one) and its fields with their tags (an asn1.RawContent field 0 left out).
    Struct(&'static str, Vec<(Ty, P)>),
    BigInt,
    Int,
    Bool,
    Oid,
    BitString,
    /// []byte
    Octets,
    RawValue,
    /// A slice of the element type.
    Slice(Box<Ty>),
}

/// A field's `asn1:"..."` parameters.
#[derive(Clone, Copy, Default)]
pub(crate) struct P {
    pub optional: bool,
    pub explicit: bool,
    pub tag: Option<usize>,
    pub set: bool,
    pub default: Option<i64>,
}

/// What Unmarshal filled in.
#[derive(Debug, Clone)]
pub(crate) enum Val {
    Absent,
    Struct(Vec<Val>),
    BigInt(Int),
    Int(i64),
    /// a BOOLEAN read (crypto_modern never needs its value)
    Bool,
    Oid(Vec<i64>),
    BitString(Vec<u8>, usize),
    Octets(Vec<u8>),
    /// A RawValue's full bytes.
    Raw(Vec<u8>),
    Slice(Vec<Val>),
}

fn syntax(m: &str) -> String {
    format!("asn1: syntax error: {m}")
}

fn structural(m: &str) -> String {
    format!("asn1: structure error: {m}")
}

struct TagLen {
    class: usize,
    tag: usize,
    length: usize,
    compound: bool,
}

/// encoding/asn1's `parseBase128Int`.
fn go_base128(b: &[u8], mut off: usize) -> Result<(i64, usize), String> {
    let mut ret: i64 = 0;
    let mut shifted = 0;
    while off < b.len() {
        if shifted == 5 {
            return Err(structural("base 128 integer too large"));
        }
        ret <<= 7;
        let x = b[off];
        if shifted == 0 && x == 0x80 {
            return Err(syntax("integer is not minimally encoded"));
        }
        ret |= i64::from(x & 0x7f);
        off += 1;
        if x & 0x80 == 0 {
            if ret > i64::from(i32::MAX) {
                return Err(structural("base 128 integer too large"));
            }
            return Ok((ret, off));
        }
        shifted += 1;
    }
    Err(syntax("truncated base 128 integer"))
}

fn tag_and_length(b: &[u8], mut off: usize) -> Result<(TagLen, usize), String> {
    let x = b[off];
    off += 1;
    let mut t = TagLen { class: usize::from(x >> 6), tag: usize::from(x & 0x1f), length: 0, compound: x & 0x20 == 0x20 };
    if t.tag == 0x1f {
        let (v, o) = go_base128(b, off)?;
        off = o;
        if v < 0x1f {
            return Err(syntax("non-minimal tag"));
        }
        t.tag = v as usize;
    }
    if off >= b.len() {
        return Err(syntax("truncated tag or length"));
    }
    let x = b[off];
    off += 1;
    if x & 0x80 == 0 {
        t.length = usize::from(x & 0x7f);
    } else {
        let n = usize::from(x & 0x7f);
        if n == 0 {
            return Err(syntax("indefinite length found (not DER)"));
        }
        for _ in 0..n {
            if off >= b.len() {
                return Err(syntax("truncated tag or length"));
            }
            let y = b[off];
            off += 1;
            if t.length >= 1 << 23 {
                return Err(structural("length too large"));
            }
            t.length = (t.length << 8) | usize::from(y);
            if t.length == 0 {
                return Err(structural("superfluous leading zeros in length"));
            }
        }
        if t.length < 0x80 {
            return Err(structural("non-minimal length"));
        }
    }
    Ok((t, off))
}

fn oid_go(b: &[u8]) -> Result<Vec<i64>, String> {
    if b.is_empty() {
        return Err(syntax("zero length OBJECT IDENTIFIER"));
    }
    let (v, mut off) = go_base128(b, 0)?;
    let mut out = if v < 80 { vec![v / 40, v % 40] } else { vec![2, v - 80] };
    while off < b.len() {
        let (v, o) = go_base128(b, off)?;
        out.push(v);
        off = o;
    }
    Ok(out)
}

/// Go's `getUniversalType`: (matches any tag, the universal tag, constructed, the type's name for errors).
fn universal(ty: &Ty) -> (bool, usize, bool, &'static str) {
    match ty {
        Ty::Struct(n, ..) => (false, 16, true, n),
        Ty::BigInt => (false, 2, false, ""),
        Ty::Int => (false, 2, false, "int"),
        Ty::Bool => (false, 1, false, "bool"),
        Ty::Oid => (false, 6, false, "ObjectIdentifier"),
        Ty::BitString => (false, 3, false, "BitString"),
        Ty::Octets => (false, 4, false, ""),
        Ty::RawValue => (true, 0, false, "RawValue"),
        Ty::Slice(_) => (false, 16, true, ""),
    }
}

fn absent(p: &P) -> Val {
    p.default.map_or(Val::Absent, Val::Int)
}

fn field(ty: &Ty, p: &P, b: &[u8], init: usize) -> Result<(usize, Val), String> {
    if init == b.len() {
        return if p.optional { Ok((init, absent(p))) } else { Err(syntax("sequence truncated")) };
    }
    let (mut t, mut o) = tag_and_length(b, init)?;
    if p.explicit {
        if o == b.len() {
            return Err(structural("explicit tag has no child"));
        }
        if t.class == 2 && Some(t.tag) == p.tag && (t.length == 0 || t.compound) {
            if matches!(ty, Ty::RawValue) {
            } else if t.length > 0 {
                (t, o) = tag_and_length(b, o)?;
            } else {
                return Err(structural("zero length explicit tag was not an asn1.Flag"));
            }
        } else if p.optional {
            return Ok((init, absent(p)));
        } else {
            return Err(structural("explicitly tagged member didn't match"));
        }
    }
    let (any, mut want, compound, name) = universal(ty);
    if p.set {
        want = 17;
    }
    let (mut class, mut match_tag) = (0, any);
    if !p.explicit {
        if let Some(tag) = p.tag {
            (class, want, match_tag) = (2, tag, false);
        }
    }
    if !match_tag && (t.class != class || t.tag != want) || !any && t.compound != compound {
        if p.optional {
            return Ok((init, absent(p)));
        }
        let ptr = |x: bool| if x { "0xADDR" } else { "<nil>" };
        return Err(structural(&format!(
            "tags don't match ({want} vs {{class:{} tag:{} length:{} isCompound:{}}}) {{optional:{} explicit:{} application:false private:false defaultValue:{} tag:{} stringType:0 timeType:0 set:{} omitEmpty:false}} {name} @{o}",
            t.class, t.tag, t.length, t.compound, p.optional, p.explicit, ptr(p.default.is_some()), ptr(p.tag.is_some()), p.set
        )));
    }
    if o.checked_add(t.length).is_none_or(|end| end > b.len()) {
        return Err(syntax("data truncated"));
    }
    let inner = &b[o..o + t.length];
    o += t.length;
    let int = |inner: &[u8]| -> Result<(), String> {
        if inner.is_empty() {
            return Err(structural("empty integer"));
        }
        if !minimal_int(inner) {
            return Err(structural("integer not minimally-encoded"));
        }
        Ok(())
    };
    let v = match ty {
        Ty::RawValue => Val::Raw(b[init..o].to_vec()),
        Ty::Oid => Val::Oid(oid_go(inner)?),
        Ty::Octets => Val::Octets(inner.to_vec()),
        Ty::Bool => match inner {
            [0] | [0xff] => Val::Bool,
            _ => return Err(syntax("invalid boolean")),
        },
        Ty::BitString => {
            if inner.is_empty() {
                return Err(syntax("zero length BIT STRING"));
            }
            let pad = inner[0];
            if pad > 7 || inner.len() == 1 && pad > 0 || inner[inner.len() - 1] & ((1u8 << pad.min(7)) - 1) != 0 {
                return Err(syntax("invalid padding bits in BIT STRING"));
            }
            Val::BitString(inner[1..].to_vec(), (inner.len() - 1) * 8 - usize::from(pad))
        }
        Ty::BigInt => {
            int(inner)?;
            Val::BigInt(int_from(inner))
        }
        Ty::Int => {
            int(inner)?;
            if inner.len() > 8 {
                return Err(structural("integer too large"));
            }
            Val::Int(signed64(inner))
        }
        Ty::Struct(_, fields) => {
            let mut io = 0;
            let mut vals = Vec::new();
            for (fty, fp) in fields {
                let (n, v) = field(fty, fp, inner, io)?;
                io = n;
                vals.push(v);
            }
            Val::Struct(vals)
        }
        Ty::Slice(elem) => Val::Slice(sequence_of(inner, elem)?),
    };
    Ok((o, v))
}

/// Go's `parseSequenceOf`.
fn sequence_of(b: &[u8], elem: &Ty) -> Result<Vec<Val>, String> {
    let (any, want, compound, _) = universal(elem);
    let mut n = 0;
    let mut off = 0;
    while off < b.len() {
        let (mut t, o) = tag_and_length(b, off)?;
        t.tag = match t.tag {
            22 | 27 | 20 | 12 | 18 | 30 => 19,
            24 | 23 => 23,
            x => x,
        };
        if !any && (t.class != 0 || t.compound != compound || t.tag != want) {
            return Err(structural("sequence tag mismatch"));
        }
        if o.checked_add(t.length).is_none_or(|end| end > b.len()) {
            return Err(syntax("truncated sequence"));
        }
        off = o + t.length;
        n += 1;
    }
    let mut out = Vec::new();
    let mut off = 0;
    for _ in 0..n {
        let (o, v) = field(elem, &P::default(), b, off)?;
        off = o;
        out.push(v);
    }
    Ok(out)
}

/// Go's `asn1.Unmarshal(b, &v)`: the value and the bytes left after it.
pub(crate) fn unmarshal<'b>(b: &'b [u8], ty: &Ty) -> Result<(Val, &'b [u8]), String> {
    let (o, v) = field(ty, &P::default(), b, 0)?;
    Ok((v, &b[o..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_follow_go() {
        assert_eq!(Der(&[2, 1, 0xff]).read_big_int().map(|i| i.decimal()), Some("-1".into()));
        assert_eq!(Der(&[2, 2, 0xff, 0x00]).read_big_int().map(|i| i.decimal()), Some("-256".into()));
        assert_eq!(Der(&[2, 2, 0, 0x7f]).read_big_int(), None);
        assert_eq!(Der(&[2, 2, 0, 0x80]).read_i64(), Some(128));
        assert_eq!(Der(&[2, 1, 0x80]).read_u64(), None);
        assert_eq!(right_align(&[0xab, 0xc0], 10), vec![0x02, 0xaf]);
        let mut d = Der(&[0x30, 0x81, 0x05, 0, 0, 0, 0, 0]);
        assert!(d.read_any().is_none(), "a long form for a short length");
    }
}
