//! Go's crypto/x509 certificates and certificate requests, as far as crypto_modern shows them: `ParseCertificate` (every
//! check and error text of crypto/x509's parser, extensions included), `ParseCertificateRequest` (through encoding/asn1,
//! with its texts), pkix.Name's String, the signature algorithm's name, and the validity times as Go reads them.

use super::asn1::{tag, unmarshal, Der, Int, Ty, Val, P};
use super::pubkey::{self, PubKey};
use crate::gotext;

/// A time as Go's `time.Time` from DER: the instant and the fields in its own zone.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Time {
    pub unix: i64,
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    /// the zone's offset in seconds east of UTC
    off: i64,
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count from 1970-01-01.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_in(m: u32, y: i64) -> u32 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl Time {
    fn new(year: i64, month: u32, day: u32, hour: u32, min: u32, sec: u32, off: i64) -> Time {
        let unix = days_from_civil(year, month, day) * 86400 + i64::from(hour * 3600 + min * 60 + sec) - off;
        Time { unix, year, month, day, hour, min, sec, off }
    }

    /// Go's `UTC().Format("2006-01-02")`.
    pub(crate) fn utc_date(&self) -> String {
        let (y, m, d) = civil_from_days(self.unix.div_euclid(86400));
        format!("{y:04}-{m:02}-{d:02}")
    }

    /// Go's asn1.Marshal of the time: UTCTime from 1950 to 2049, GeneralizedTime otherwise.
    fn marshal(&self) -> Vec<u8> {
        let utc = (1950..2050).contains(&self.year);
        let mut s = if utc { format!("{:02}", self.year % 100) } else { format!("{:04}", self.year) };
        s += &format!("{:02}{:02}{:02}{:02}{:02}", self.month, self.day, self.hour, self.min, self.sec);
        if self.off / 60 == 0 {
            s.push('Z');
        } else {
            let m = (self.off / 60).abs();
            s += &format!("{}{:02}{:02}", if self.off > 0 { '+' } else { '-' }, m / 60, m % 60);
        }
        let mut out = vec![if utc { tag::UTC_TIME } else { tag::GENERALIZED_TIME }, s.len() as u8];
        out.extend(s.bytes());
        out
    }
}

/// Go's time.Parse with "20060102150405Z0700" (or the UTCTime layouts) and the round trip cryptobyte checks: digits in
/// range, a real date, "Z" or a non-zero ±hhmm offset of at most 24 hours, nothing after it.
fn parse_time(b: &[u8], year_digits: usize, with_secs: bool) -> Option<Time> {
    let n = year_digits + 8 + if with_secs { 2 } else { 0 };
    if b.len() < n || !b[..n].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let num = |i: usize, w: usize| b[i..i + w].iter().fold(0u32, |v, &c| v * 10 + u32::from(c - b'0'));
    let mut year = i64::from(num(0, year_digits));
    if year_digits == 2 {
        year += if year >= 69 { 1900 } else { 2000 };
    }
    let p = year_digits;
    let (month, day, hour, min) = (num(p, 2), num(p + 2, 2), num(p + 4, 2), num(p + 6, 2));
    let sec = if with_secs { num(p + 8, 2) } else { 0 };
    if !(1..=12).contains(&month) || hour > 23 || min > 59 || sec > 59 || day < 1 || day > days_in(month, year) {
        return None;
    }
    let off = match &b[n..] {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, m1, m2] if [h1, h2, m1, m2].iter().all(|c| c.is_ascii_digit()) => {
            let (h, m) = (i64::from((h1 - b'0') * 10 + (h2 - b'0')), i64::from((m1 - b'0') * 10 + (m2 - b'0')));
            if h > 24 || m > 59 || h == 0 && m == 0 {
                return None;
            }
            (h * 3600 + m * 60) * if *sign == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    Some(Time::new(year, month, day, hour, min, sec, off))
}

fn utc_time(b: &[u8]) -> Option<Time> {
    let t = parse_time(b, 2, true).or_else(|| parse_time(b, 2, false))?;
    Some(if t.year >= 2050 { Time::new(t.year - 100, t.month, t.day, t.hour, t.min, t.sec, t.off) } else { t })
}

/// x509's `readASN1Time`.
fn read_time(d: &mut Der) -> Result<Time, String> {
    if d.peek(tag::UTC_TIME) {
        return d.read(tag::UTC_TIME).and_then(|b| utc_time(b.0)).ok_or_else(|| "x509: malformed UTCTime".into());
    }
    if d.peek(tag::GENERALIZED_TIME) {
        return d
            .read(tag::GENERALIZED_TIME)
            .and_then(|b| parse_time(b.0, 4, true))
            .ok_or_else(|| "x509: malformed GeneralizedTime".into());
    }
    Err("x509: unsupported time format".into())
}

/// A name attribute's value as x509's `readASN1Any` reads it.
#[derive(Clone, Debug)]
enum Value {
    Str(String),
    Null,
    /// time.Time
    Time(Time),
    /// any other kind: its DER as asn1.Marshal writes it back (the element itself)
    Der(Vec<u8>),
}

fn is_printable(b: u8) -> bool {
    b.is_ascii_alphanumeric() || (b'\''..=b')').contains(&b) || (b'+'..=b'/').contains(&b) || b" :=?*&".contains(&b)
}

/// x509's `parseASN1String`.
fn asn1_string(t: u8, v: &[u8]) -> Result<String, String> {
    match t {
        tag::T61 => Ok(v.iter().map(|&b| char::from(b)).collect()),
        tag::PRINTABLE => v
            .iter()
            .all(|&b| is_printable(b))
            .then(|| String::from_utf8_lossy(v).into_owned())
            .ok_or_else(|| "invalid PrintableString".into()),
        tag::UTF8 => std::str::from_utf8(v).map(str::to_owned).map_err(|_| "invalid UTF-8 string".into()),
        tag::BMP => {
            if !v.len().is_multiple_of(2) {
                return Err("invalid BMPString".into());
            }
            let v = if v.len() >= 2 && v[v.len() - 1] == 0 && v[v.len() - 2] == 0 { &v[..v.len() - 2] } else { v };
            let mut s = String::new();
            for c in v.chunks(2) {
                let p = u16::from(c[0]) << 8 | u16::from(c[1]);
                if p == 0xfffe || p == 0xffff || (0xfdd0..=0xfdef).contains(&p) || (0xd800..=0xdfff).contains(&p) {
                    return Err("invalid BMPString".into());
                }
                s.push(char::from_u32(u32::from(p)).unwrap_or('\u{fffd}'));
            }
            Ok(s)
        }
        tag::IA5 => v.is_ascii().then(|| String::from_utf8_lossy(v).into_owned()).ok_or_else(|| "invalid IA5String".into()),
        tag::NUMERIC => v
            .iter()
            .all(|&b| b.is_ascii_digit() || b == b' ')
            .then(|| String::from_utf8_lossy(v).into_owned())
            .ok_or_else(|| "invalid NumericString".into()),
        _ => Err(format!("unsupported string type: {t}")),
    }
}

/// x509's `readASN1Any`.
fn read_any(d: &mut Der) -> Result<Value, String> {
    let (t, full) = d.read_any_element().ok_or("invalid ASN.1 element")?;
    let mut fv = Der(full);
    let bad = |what: &str| format!("invalid ASN.1 {what}");
    match t {
        tag::T61 | tag::PRINTABLE | tag::UTF8 | tag::BMP | tag::IA5 | tag::NUMERIC => {
            let raw = fv.read(t).ok_or("invalid ASN.1 element")?;
            asn1_string(t, raw.0).map(Value::Str)
        }
        tag::INTEGER => fv.read_i64().map(|_| Value::Der(full.to_vec())).ok_or_else(|| bad("integer")),
        tag::BIT_STRING => fv.read_bit_string().map(|_| Value::Der(full.to_vec())).ok_or_else(|| bad("BIT STRING")),
        tag::OCTET_STRING => fv.read(tag::OCTET_STRING).map(|_| Value::Der(full.to_vec())).ok_or_else(|| bad("OCTET STRING")),
        tag::OID => fv.read_oid().map(|_| Value::Der(full.to_vec())).ok_or_else(|| bad("OBJECT IDENTIFIER")),
        tag::UTC_TIME | tag::GENERALIZED_TIME => read_time(&mut fv).map(Value::Time),
        tag::BOOLEAN => fv.read_bool().map(|_| Value::Der(full.to_vec())).ok_or_else(|| bad("BOOLEAN")),
        tag::NULL => Ok(Value::Null),
        _ => fv.read_any().map(|_| Value::Der(full.to_vec())).ok_or_else(|| "invalid ASN.1 element".into()),
    }
}

/// A distinguished name: its attributes in order (pkix.Name's Names).
#[derive(Clone, Debug, Default)]
pub(crate) struct Name(Vec<(Vec<i64>, Value)>);

/// x509's `parseName`.
fn parse_name(raw: &[u8]) -> Result<Name, String> {
    let mut raw = Der(raw).read(tag::SEQUENCE).ok_or("x509: invalid RDNSequence")?;
    let mut out = Vec::new();
    while !raw.empty() {
        let mut set = raw.read(tag::SET).ok_or("x509: invalid RDNSequence")?;
        while !set.empty() {
            let mut atv = set.read(tag::SEQUENCE).ok_or("x509: invalid RDNSequence: invalid attribute")?;
            let t = atv.read_oid().ok_or("x509: invalid RDNSequence: invalid attribute type")?;
            let v = read_any(&mut atv).map_err(|e| format!("x509: invalid RDNSequence: invalid attribute value: {e}"))?;
            out.push((t, v));
        }
    }
    Ok(Name(out))
}

fn oid_string(o: &[i64]) -> String {
    o.iter().map(i64::to_string).collect::<Vec<_>>().join(".")
}

/// The pkix attribute an OID 2.5.4.n names, by n, in ToRDNSequence's order.
const STANDARD: [(i64, &str); 9] =
    [(6, "C"), (8, "ST"), (7, "L"), (9, "STREET"), (17, "POSTALCODE"), (10, "O"), (11, "OU"), (3, "CN"), (5, "SERIALNUMBER")];

fn standard(o: &[i64]) -> Option<i64> {
    match o {
        [2, 5, 4, n] if STANDARD.iter().any(|(k, _)| k == n) => Some(*n),
        _ => None,
    }
}

impl Name {
    /// Go's pkix `Name.String` after `FillFromRDNSequence`.
    pub(crate) fn string(&self) -> String {
        // each RDN: (type, value) pairs
        let mut rdns: Vec<Vec<(Vec<i64>, Value)>> = self.0.iter().filter(|(t, _)| standard(t).is_none()).map(|a| vec![a.clone()]).collect();
        for (n, _) in STANDARD {
            let values: Vec<&String> = self
                .0
                .iter()
                .filter_map(|(t, v)| match (standard(t), v) {
                    (Some(k), Value::Str(s)) if k == n => Some(s),
                    _ => None,
                })
                .collect();
            let values: Vec<&String> =
                if n == 3 || n == 5 { values.last().into_iter().copied().filter(|s| !s.is_empty()).collect() } else { values };
            if !values.is_empty() {
                rdns.push(values.into_iter().map(|s| (vec![2, 5, 4, n], Value::Str(s.clone()))).collect());
            }
        }
        let mut out = String::new();
        for (i, rdn) in rdns.iter().rev().enumerate() {
            if i > 0 {
                out.push(',');
            }
            for (j, (t, v)) in rdn.iter().enumerate() {
                if j > 0 {
                    out.push('+');
                }
                let oid = oid_string(t);
                let name = standard(t).and_then(|n| STANDARD.iter().find(|(k, _)| *k == n)).map(|(_, s)| (*s).to_owned());
                let value = match v {
                    Value::Str(s) => s.clone(),
                    Value::Der(d) if name.is_none() => {
                        out += &format!("{oid}=#{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>());
                        continue;
                    }
                    Value::Time(tm) if name.is_none() => {
                        out += &format!("{oid}=#{}", tm.marshal().iter().map(|b| format!("{b:02x}")).collect::<String>());
                        continue;
                    }
                    Value::Null => "<nil>".into(),
                    // a standard attribute with a value that is not a string never reaches here (FillFromRDNSequence)
                    _ => String::new(),
                };
                out += &name.unwrap_or(oid);
                out.push('=');
                let last = value.len().saturating_sub(1);
                for (k, c) in value.char_indices() {
                    let esc = match c {
                        ',' | '+' | '"' | '\\' | '<' | '>' | ';' => true,
                        ' ' => k == 0 || k == last,
                        '#' => k == 0,
                        _ => false,
                    };
                    if esc {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
        }
        out
    }
}

const SIG_ALGS: [(&[i64], &str); 19] = [
    (&[1, 2, 840, 113549, 1, 1, 4], "MD5-RSA"),
    (&[1, 2, 840, 113549, 1, 1, 5], "SHA1-RSA"),
    (&[1, 3, 14, 3, 2, 29], "SHA1-RSA"),
    (&[1, 2, 840, 113549, 1, 1, 11], "SHA256-RSA"),
    (&[1, 2, 840, 113549, 1, 1, 12], "SHA384-RSA"),
    (&[1, 2, 840, 113549, 1, 1, 13], "SHA512-RSA"),
    (&[1, 2, 840, 113549, 1, 1, 10], "SHA256-RSAPSS"),
    (&[1, 2, 840, 113549, 1, 1, 10], "SHA384-RSAPSS"),
    (&[1, 2, 840, 113549, 1, 1, 10], "SHA512-RSAPSS"),
    (&[1, 2, 840, 10040, 4, 3], "DSA-SHA1"),
    (&[2, 16, 840, 1, 101, 3, 4, 3, 2], "DSA-SHA256"),
    (&[1, 2, 840, 10045, 4, 1], "ECDSA-SHA1"),
    (&[1, 2, 840, 10045, 4, 3, 2], "ECDSA-SHA256"),
    (&[1, 2, 840, 10045, 4, 3, 3], "ECDSA-SHA384"),
    (&[1, 2, 840, 10045, 4, 3, 4], "ECDSA-SHA512"),
    (&[1, 3, 101, 112], "Ed25519"),
    (&[2, 16, 840, 1, 101, 3, 4, 3, 17], "ML-DSA-44"),
    (&[2, 16, 840, 1, 101, 3, 4, 3, 18], "ML-DSA-65"),
    (&[2, 16, 840, 1, 101, 3, 4, 3, 19], "ML-DSA-87"),
];
const RSA_PSS: &[i64] = &[1, 2, 840, 113549, 1, 1, 10];
const SHA2: [&[i64]; 3] = [&[2, 16, 840, 1, 101, 3, 4, 2, 1], &[2, 16, 840, 1, 101, 3, 4, 2, 2], &[2, 16, 840, 1, 101, 3, 4, 2, 3]];

fn ai_fields(v: &Val) -> Option<(&[i64], &[u8])> {
    let Val::Struct(f) = v else { return None };
    let Val::Oid(o) = &f[0] else { return None };
    Some((o, if let Val::Raw(p) = &f[1] { p } else { &[] }))
}

/// Go's `getSignatureAlgorithmFromAI` and the algorithm's `String`: "0" for one it does not know.
fn sig_alg_name(oid: &[i64], params: &[u8]) -> &'static str {
    let pure =
        [&[1, 3, 101, 112][..], &[2, 16, 840, 1, 101, 3, 4, 3, 17], &[2, 16, 840, 1, 101, 3, 4, 3, 18], &[2, 16, 840, 1, 101, 3, 4, 3, 19]];
    if pure.contains(&oid) && !params.is_empty() {
        return "0";
    }
    if oid != RSA_PSS {
        return SIG_ALGS.iter().find(|(o, _)| *o == oid).map_or("0", |(_, n)| n);
    }
    let explicit = |n: usize| P { explicit: true, tag: Some(n), ..P::default() };
    let pss = Ty::Struct(
        "pssParameters",
        vec![
            (pubkey::ai_ty(), explicit(0)),
            (pubkey::ai_ty(), explicit(1)),
            (Ty::Int, explicit(2)),
            (Ty::Int, P { optional: true, default: Some(1), ..explicit(3) }),
        ],
    );
    let Ok((Val::Struct(f), _)) = unmarshal(params, &pss) else { return "0" };
    let (Some((hash, hash_params)), Some((mgf, mgf_params)), Val::Int(salt), Val::Int(trailer)) =
        (ai_fields(&f[0]), ai_fields(&f[1]), &f[2], &f[3])
    else {
        return "0";
    };
    let Ok((mgf_hash, _)) = unmarshal(mgf_params, &pubkey::ai_ty()) else { return "0" };
    let Some((mgf_hash, mgf_hash_params)) = ai_fields(&mgf_hash) else { return "0" };
    let null_or_none = |p: &[u8]| p.is_empty() || p == [5, 0];
    if !null_or_none(hash_params)
        || mgf != [1, 2, 840, 113549, 1, 1, 8]
        || mgf_hash != hash
        || !null_or_none(mgf_hash_params)
        || *trailer != 1
    {
        return "0";
    }
    match (SHA2.iter().position(|o| *o == hash), salt) {
        (Some(0), 32) => "SHA256-RSAPSS",
        (Some(1), 48) => "SHA384-RSAPSS",
        (Some(2), 64) => "SHA512-RSAPSS",
        _ => "0",
    }
}

/// Go's `getPublicKeyAlgorithmFromOID` is not Unknown (RSA, DSA, ECDSA, Ed25519, ML-DSA).
fn known_key_algorithm(oid: &[i64]) -> bool {
    [&[1, 2, 840, 113549, 1, 1, 1][..], &[1, 2, 840, 10040, 4, 1], &[1, 2, 840, 10045, 2, 1], &[1, 3, 101, 112]].contains(&oid)
        || matches!(oid, [2, 16, 840, 1, 101, 3, 4, 3, 17..=19])
}

/// What crypto_modern shows of a certificate.
pub(crate) struct Cert {
    pub raw: Vec<u8>,
    pub serial: Int,
    pub sig_alg: &'static str,
    pub issuer: Name,
    pub subject: Name,
    pub not_before: Time,
    pub not_after: Time,
    /// None: an algorithm Go does not parse (`%T` of nil)
    pub key: Option<PubKey>,
    pub dns: Vec<String>,
    pub ips: Vec<Vec<u8>>,
    pub emails: Vec<String>,
    pub is_ca: bool,
    /// the ExtKeyUsage values, as Go numbers them
    pub eku: Vec<u8>,
}

fn x(m: &str) -> String {
    format!("x509: {m}")
}

fn parse_ai(mut d: Der) -> Result<(Vec<i64>, Vec<u8>), String> {
    let oid = d.read_oid().ok_or_else(|| x("malformed OID"))?;
    if d.empty() {
        return Ok((oid, Vec::new()));
    }
    let (_, params) = d.read_any_element().ok_or_else(|| x("malformed parameters"))?;
    Ok((oid, params.to_vec()))
}

/// Go's `domainNameValid`.
fn domain_name_valid(s: &[u8], constraint: bool) -> bool {
    if s.is_empty() {
        return true;
    }
    if s[s.len() - 1] == b'.' {
        return false;
    }
    let s = if constraint && s[0] == b'.' { &s[1..] } else { s };
    let mut last_dot: isize = -1;
    for i in 0..=s.len() {
        if i < s.len() && !(33..=126).contains(&s[i]) {
            return false;
        }
        if i == s.len() || s[i] == b'.' {
            let label = i as isize - (last_dot + 1);
            if label == 0 {
                return false;
            }
            last_dot = i as isize;
        }
    }
    true
}

/// Go's `parseRFC2821Mailbox`, as a yes or no.
fn rfc2821_mailbox(s: &[u8]) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut local = Vec::new();
    let mut i;
    if s[0] == b'"' {
        i = 1;
        loop {
            let Some(&c) = s.get(i) else { return false };
            i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let Some(&n) = s.get(i) else { return false };
                    if n == 11 || n == 12 || (1..=9).contains(&n) || (14..=127).contains(&n) {
                        local.push(n);
                        i += 1;
                    } else {
                        return false;
                    }
                }
                11 | 12 | 32 | 33 | 127 | 1..=8 | 14..=31 | 35..=91 | 93..=126 => local.push(c),
                _ => return false,
            }
        }
    } else {
        i = 0;
        while i < s.len() {
            let mut c = s[i];
            if c == b'\\' {
                i += 1;
                if i == s.len() {
                    return false;
                }
                // Go falls through to append the escaped byte itself
                c = b'a';
            }
            if c.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~.".contains(&c) {
                local.push(s[i]);
                i += 1;
            } else {
                break;
            }
        }
        if local.is_empty() || local[0] == b'.' || local[local.len() - 1] == b'.' || local.windows(2).any(|w| w == b"..") {
            return false;
        }
    }
    if s.get(i) != Some(&b'@') {
        return false;
    }
    let domain = &s[i + 1..];
    domain_name_valid(domain, false) && !domain.contains(&b'@')
}

type Sans = (Vec<String>, Vec<String>, Vec<Vec<u8>>, usize);

/// x509's `parseSANExtension`: (DNS names, emails, IPs, how many URIs).
fn parse_san(der: &[u8]) -> Result<Sans, String> {
    let mut d = Der(der).read(tag::SEQUENCE).ok_or_else(|| x("invalid subject alternative names"))?;
    let (mut dns, mut emails, mut ips, mut uris) = (Vec::new(), Vec::new(), Vec::new(), 0);
    while !d.empty() {
        let (t, san) = d.read_any().ok_or_else(|| x("invalid subject alternative name"))?;
        let data = san.0;
        let text = || String::from_utf8_lossy(data).into_owned();
        match t ^ 0x80 {
            1 => {
                if !data.is_ascii() {
                    return Err(x("SAN rfc822Name is malformed"));
                }
                emails.push(text());
            }
            2 => {
                if !data.is_ascii() {
                    return Err(x("SAN dNSName is malformed"));
                }
                dns.push(text());
            }
            6 => {
                if !data.is_ascii() {
                    return Err(x("SAN uniformResourceIdentifier is malformed"));
                }
                let s = text();
                let host = crate::gourl::parse_host_or_error(&s).map_err(|e| x(&format!("cannot parse URI {}: {e}", gotext::quote(&s))))?;
                if !host.is_empty() && !domain_name_valid(&host, false) {
                    return Err(x(&format!("cannot parse URI {}: invalid domain", gotext::quote(&s))));
                }
                uris += 1;
            }
            7 => match data.len() {
                16 if data[..10].iter().all(|&b| b == 0) && data[10] == 0xff && data[11] == 0xff => {
                    return Err(x("SAN iPAddress contains IPv4-mapped IPv6 address"));
                }
                4 | 16 => ips.push(data.to_vec()),
                n => return Err(x(&format!("cannot parse IP address of length {n}"))),
            },
            _ => {}
        }
    }
    Ok((dns, emails, ips, uris))
}

/// x509's `parseNameConstraintsExtension`: an error or nothing (the constraints themselves are not shown).
fn check_name_constraints(value: &[u8]) -> Result<(), String> {
    let invalid = || x("invalid NameConstraints extension");
    let mut outer = Der(value);
    let mut top = outer.read(tag::SEQUENCE).ok_or_else(invalid)?;
    if !outer.empty() {
        return Err(invalid());
    }
    let mut opt = |t: u8| -> Result<Option<Der>, String> {
        if !top.peek(t) {
            return Ok(None);
        }
        top.read(t).map(Some).ok_or_else(invalid)
    };
    let permitted = opt(0xa0)?;
    let excluded = opt(0xa1)?;
    if !top.empty() {
        return Err(invalid());
    }
    let len = |d: &Option<Der>| d.map_or(0, |d| d.0.len());
    if permitted.is_none() && excluded.is_none() || len(&permitted) == 0 && len(&excluded) == 0 {
        return Err(x("empty name constraints extension"));
    }
    for mut sub in [permitted, excluded].into_iter().flatten() {
        while !sub.empty() {
            let (t, value) = sub.read(tag::SEQUENCE).and_then(|mut s| s.read_any()).ok_or_else(invalid)?;
            let v = value.0;
            let s = String::from_utf8_lossy(v);
            let ia5 = || -> Result<(), String> {
                if v.is_ascii() {
                    Ok(())
                } else {
                    Err(x(&format!("invalid constraint value: x509: {} cannot be encoded as an IA5String", gotext::quote(&s))))
                }
            };
            match t {
                0x82 => {
                    ia5()?;
                    if !domain_name_valid(v, true) {
                        return Err(x(&format!("failed to parse dnsName constraint {}", gotext::quote(&s))));
                    }
                }
                0x87 => {
                    let (ip, mask) = match v.len() {
                        8 => v.split_at(4),
                        32 => v.split_at(16),
                        l => return Err(x(&format!("IP constraint contained value of length {l}"))),
                    };
                    let mut seen_zero = false;
                    for &b in mask {
                        if seen_zero {
                            if b != 0 {
                                return Err(x(&format!(
                                    "IP constraint contained invalid mask {}",
                                    mask.iter().map(|b| format!("{b:02x}")).collect::<String>()
                                )));
                            }
                        } else if [0x00, 0x80, 0xc0, 0xe0, 0xf0, 0xf8, 0xfc, 0xfe].contains(&b) {
                            seen_zero = true;
                        } else if b != 0xff {
                            return Err(x(&format!(
                                "IP constraint contained invalid mask {}",
                                mask.iter().map(|b| format!("{b:02x}")).collect::<String>()
                            )));
                        }
                    }
                    if ip.len() == 16 && ip[..10].iter().all(|&b| b == 0) && ip[10] == 0xff && ip[11] == 0xff {
                        return Err(x("IP constraint contained IPv4-mapped IPv6 address"));
                    }
                }
                0x81 => {
                    ia5()?;
                    let ok = if v.contains(&b'@') { rfc2821_mailbox(v) } else { domain_name_valid(v, true) };
                    if !ok {
                        return Err(x(&format!("failed to parse rfc822Name constraint {}", gotext::quote(&s))));
                    }
                }
                0x86 => {
                    ia5()?;
                    if crate::gonetip::parses_as_ip(&s) {
                        return Err(x(&format!("failed to parse URI constraint {}: cannot be IP address", gotext::quote(&s))));
                    }
                    if !domain_name_valid(v, true) {
                        return Err(x(&format!("failed to parse URI constraint {}", gotext::quote(&s))));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Go's `newOIDFromDER` accepts the bytes.
fn oid_der_ok(der: &[u8]) -> bool {
    if der.is_empty() || der[der.len() - 1] & 0x80 != 0 {
        return false;
    }
    let mut start = 0;
    for (i, &v) in der.iter().enumerate() {
        if i == start && v == 0x80 {
            return false;
        }
        if v & 0x80 == 0 {
            start = i + 1;
        }
    }
    true
}

const EKU: [&[i64]; 14] = [
    &[2, 5, 29, 37, 0],
    &[1, 3, 6, 1, 5, 5, 7, 3, 1],
    &[1, 3, 6, 1, 5, 5, 7, 3, 2],
    &[1, 3, 6, 1, 5, 5, 7, 3, 3],
    &[1, 3, 6, 1, 5, 5, 7, 3, 4],
    &[1, 3, 6, 1, 5, 5, 7, 3, 5],
    &[1, 3, 6, 1, 5, 5, 7, 3, 6],
    &[1, 3, 6, 1, 5, 5, 7, 3, 7],
    &[1, 3, 6, 1, 5, 5, 7, 3, 8],
    &[1, 3, 6, 1, 5, 5, 7, 3, 9],
    &[1, 3, 6, 1, 4, 1, 311, 10, 3, 3],
    &[2, 16, 840, 1, 113730, 4, 1],
    &[1, 3, 6, 1, 4, 1, 311, 2, 1, 22],
    &[1, 3, 6, 1, 4, 1, 311, 61, 1, 1],
];

/// x509's `processExtensions` for one extension.
fn process_extension(c: &mut Cert, id: &[i64], critical: bool, value: &[u8]) -> Result<(), String> {
    let v = || Der(value);
    match id {
        [2, 5, 29, n] => match n {
            15 => {
                v().read_bit_string().ok_or_else(|| x("invalid key usage"))?;
            }
            19 => {
                let bad = || x("invalid basic constraints");
                let mut d = v().read(tag::SEQUENCE).ok_or_else(bad)?;
                let mut is_ca = false;
                if d.peek(tag::BOOLEAN) {
                    is_ca = d.read_bool().ok_or_else(bad)?;
                }
                if d.peek(tag::INTEGER) && !d.read_u64().is_some_and(|m| m <= i64::MAX as u64) {
                    return Err(bad());
                }
                c.is_ca = is_ca;
            }
            17 => {
                let (dns, emails, ips, _) = parse_san(value)?;
                (c.dns, c.emails, c.ips) = (dns, emails, ips);
            }
            30 => check_name_constraints(value)?,
            31 => {
                let mut d = v().read(tag::SEQUENCE).ok_or_else(|| x("invalid CRL distribution points"))?;
                let bad = || x("invalid CRL distribution point");
                while !d.empty() {
                    let mut dp = d.read(tag::SEQUENCE).ok_or_else(bad)?;
                    if !dp.peek(0xa0) {
                        continue;
                    }
                    let mut name = dp.read(0xa0).ok_or_else(bad)?.read(0xa0).ok_or_else(bad)?;
                    while !name.empty() && name.peek(0x86) {
                        name.read(0x86).ok_or_else(bad)?;
                    }
                }
            }
            35 => {
                if critical {
                    return Err(x("authority key identifier incorrectly marked critical"));
                }
                let bad = || x("invalid authority key identifier");
                let mut akid = v().read(tag::SEQUENCE).ok_or_else(bad)?;
                if akid.peek(0x80) {
                    akid.read(0x80).ok_or_else(bad)?;
                }
            }
            36 => {
                let bad = || x("invalid policy constraints extension");
                let mut d = v().read(tag::SEQUENCE).ok_or_else(bad)?;
                for t in [0x80, 0x81] {
                    if d.peek(t) {
                        d.read_i64_tag(t).ok_or_else(bad)?;
                    }
                }
            }
            37 => {
                let bad = || x("invalid extended key usages");
                let mut d = v().read(tag::SEQUENCE).ok_or_else(bad)?;
                let mut eku = Vec::new();
                while !d.empty() {
                    let o = d.read_oid().ok_or_else(bad)?;
                    if let Some(i) = EKU.iter().position(|e| *e == o.as_slice()) {
                        eku.push(i as u8);
                    }
                }
                c.eku = eku;
            }
            14 => {
                if critical {
                    return Err(x("subject key identifier incorrectly marked critical"));
                }
                v().read(tag::OCTET_STRING).ok_or_else(|| x("invalid subject key identifier"))?;
            }
            32 => {
                let bad = || x("invalid certificate policies");
                let mut d = v().read(tag::SEQUENCE).ok_or_else(bad)?;
                let mut seen: Vec<&[u8]> = Vec::new();
                while !d.empty() {
                    let o = d.read(tag::SEQUENCE).and_then(|mut cp| cp.read(tag::OID)).ok_or_else(bad)?;
                    if seen.contains(&o.0) || !oid_der_ok(o.0) {
                        return Err(bad());
                    }
                    seen.push(o.0);
                }
            }
            33 => {
                let bad = || x("invalid policy mappings extension");
                let mut d = v().read(tag::SEQUENCE).ok_or_else(bad)?;
                while !d.empty() {
                    let mut s = d.read(tag::SEQUENCE).ok_or_else(bad)?;
                    s.read(tag::OID).and_then(|_| s.read(tag::OID)).ok_or_else(bad)?;
                }
            }
            54 => {
                v().read_i64().ok_or_else(|| x("invalid inhibit any policy extension"))?;
            }
            _ => {}
        },
        [1, 3, 6, 1, 5, 5, 7, 1, 1] => {
            if critical {
                return Err(x("authority info access incorrectly marked critical"));
            }
            let bad = || x("invalid authority info access");
            let mut d = v().read(tag::SEQUENCE).ok_or_else(bad)?;
            while !d.empty() {
                let mut aia = d.read(tag::SEQUENCE).ok_or_else(bad)?;
                aia.read_oid().ok_or_else(bad)?;
                if aia.peek(0x86) {
                    aia.read(0x86).ok_or_else(bad)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Go's `x509.ParseCertificate`.
pub(crate) fn parse_certificate(der: &[u8]) -> Result<Cert, String> {
    let malformed = |m: &str| x(&format!("malformed {m}"));
    let mut input = Der(der);
    let raw = input.read_element(tag::SEQUENCE).ok_or_else(|| malformed("certificate"))?;
    let mut input = Der(raw).read(tag::SEQUENCE).ok_or_else(|| malformed("certificate"))?;
    let tbs_el = input.read_element(tag::SEQUENCE).ok_or_else(|| malformed("tbs certificate"))?;
    let mut tbs = Der(tbs_el).read(tag::SEQUENCE).ok_or_else(|| malformed("tbs certificate"))?;
    let version = tbs.read_optional_i64(0xa0, 0).filter(|v| *v >= 0).ok_or_else(|| malformed("version"))? + 1;
    if version > 3 {
        return Err(x("invalid version"));
    }
    let serial = tbs.read_big_int().ok_or_else(|| malformed("serial number"))?;
    if serial.neg {
        return Err(x("negative serial number"));
    }
    let sig_ai = tbs.read_element(tag::SEQUENCE).ok_or_else(|| malformed("signature algorithm identifier"))?;
    let sig_ai = Der(sig_ai).read(tag::SEQUENCE).ok_or_else(|| malformed("signature algorithm identifier"))?;
    let outer = input.read(tag::SEQUENCE).ok_or_else(|| malformed("algorithm identifier"))?;
    if outer.0 != sig_ai.0 {
        return Err(x("inner and outer signature algorithm identifiers don't match"));
    }
    let (sig_oid, sig_params) = parse_ai(sig_ai)?;
    let sig_alg = sig_alg_name(&sig_oid, &sig_params);
    let issuer = tbs.read_element(tag::SEQUENCE).ok_or_else(|| malformed("issuer"))?;
    let issuer = parse_name(issuer)?;
    let mut validity = tbs.read(tag::SEQUENCE).ok_or_else(|| malformed("validity"))?;
    let not_before = read_time(&mut validity)?;
    let not_after = read_time(&mut validity)?;
    let subject = tbs.read_element(tag::SEQUENCE).ok_or_else(|| malformed("issuer"))?;
    let subject = parse_name(subject)?;
    let spki = tbs.read_element(tag::SEQUENCE).ok_or_else(|| malformed("spki"))?;
    let mut spki = Der(spki).read(tag::SEQUENCE).ok_or_else(|| malformed("spki"))?;
    let pk_ai = spki.read(tag::SEQUENCE).ok_or_else(|| malformed("public key algorithm identifier"))?;
    let (pk_oid, pk_params) = parse_ai(pk_ai)?;
    let (key_bytes, key_bits) = spki.read_bit_string().ok_or_else(|| malformed("subjectPublicKey"))?;
    let key = if known_key_algorithm(&pk_oid) { Some(pubkey::parse_public_key(&pk_oid, &pk_params, key_bytes, key_bits)?) } else { None };
    let mut c = Cert {
        raw: raw.to_vec(),
        serial,
        sig_alg,
        issuer,
        subject,
        not_before,
        not_after,
        key,
        dns: Vec::new(),
        ips: Vec::new(),
        emails: Vec::new(),
        is_ca: false,
        eku: Vec::new(),
    };
    if version > 1 {
        for (t, what) in [(0x81, "issuerUniqueID"), (0x82, "subjectUniqueID")] {
            if tbs.peek(t) && tbs.read(t).is_none() {
                return Err(malformed(what));
            }
        }
        if version == 3 && tbs.peek(0xa3) {
            let mut exts = tbs.read(0xa3).and_then(|mut e| e.read(tag::SEQUENCE)).ok_or_else(|| malformed("extensions"))?;
            let mut list: Vec<(Vec<i64>, bool, &[u8])> = Vec::new();
            while !exts.empty() {
                let mut e = exts.read(tag::SEQUENCE).ok_or_else(|| malformed("extension"))?;
                let id = e.read_oid().ok_or_else(|| x("malformed extension OID field"))?;
                let mut critical = false;
                if e.peek(tag::BOOLEAN) {
                    critical = e.read_bool().ok_or_else(|| x("malformed extension critical field"))?;
                }
                let value = e.read(tag::OCTET_STRING).ok_or_else(|| x("malformed extension value field"))?;
                if list.iter().any(|(o, ..)| *o == id) {
                    return Err(x(&format!("certificate contains duplicate extension with OID {}", gotext::quote(&oid_string(&id)))));
                }
                list.push((id, critical, value.0));
            }
            for (id, critical, value) in list {
                process_extension(&mut c, &id, critical, value)?;
            }
        }
    }
    input.read_bit_string().ok_or_else(|| malformed("signature"))?;
    if der.len() != raw.len() {
        return Err(x("trailing data"));
    }
    Ok(c)
}

/// What crypto_modern shows of a certificate request.
pub(crate) struct Request {
    pub subject: Name,
    pub dns: Vec<String>,
    pub key: Option<PubKey>,
    pub sig_alg: &'static str,
}

fn csr_ty() -> Ty {
    let d = P::default;
    Ty::Struct(
        "certificateRequest",
        vec![
            (
                Ty::Struct(
                    "tbsCertificateRequest",
                    vec![
                        (Ty::Int, d()),
                        (Ty::RawValue, d()),
                        (pubkey::spki_ty(), d()),
                        (Ty::Slice(Box::new(Ty::RawValue)), P { tag: Some(0), ..d() }),
                    ],
                ),
                d(),
            ),
            (Ty::Struct("", vec![(Ty::Oid, d()), (Ty::RawValue, P { optional: true, ..d() })]), d()),
            (Ty::BitString, d()),
        ],
    )
}

/// Go's `x509.ParseCertificateRequest`.
pub(crate) fn parse_request(der: &[u8]) -> Result<Request, String> {
    let (v, rest) = unmarshal(der, &csr_ty())?;
    if !rest.is_empty() {
        return Err("asn1: syntax error: trailing data".into());
    }
    let Val::Struct(top) = v else { unreachable!("certificateRequest is a struct") };
    let (Val::Struct(tbs), Some((sig_oid, sig_params))) = (&top[0], ai_fields(&top[1])) else { unreachable!("certificateRequest fields") };
    let sig_alg = sig_alg_name(sig_oid, sig_params);
    let (Val::Raw(subject), spki, Val::Slice(attrs)) = (&tbs[1], tbs[2].clone(), &tbs[3]) else {
        unreachable!("tbsCertificateRequest fields")
    };
    let (oid, params, key, bits) = pubkey::spki_fields(spki);
    let key = if known_key_algorithm(&oid) { Some(pubkey::parse_public_key(&oid, &params, &key, bits)?) } else { None };
    let subject = parse_name(subject)?;
    // parseCSRExtensions
    let attr_ty =
        Ty::Struct("pkcs10Attribute", vec![(Ty::Oid, P::default()), (Ty::Slice(Box::new(Ty::RawValue)), P { set: true, ..P::default() })]);
    let ext_ty = Ty::Slice(Box::new(Ty::Struct(
        "Extension",
        vec![(Ty::Oid, P::default()), (Ty::Bool, P { optional: true, ..P::default() }), (Ty::Octets, P::default())],
    )));
    let mut seen = Vec::new();
    let mut exts = Vec::new();
    for a in attrs {
        let Val::Raw(full) = a else { continue };
        let Ok((Val::Struct(f), rest)) = unmarshal(full, &attr_ty) else { continue };
        let (Val::Oid(id), Val::Slice(values)) = (&f[0], &f[1]) else { continue };
        if !rest.is_empty() || values.is_empty() || *id != [1, 2, 840, 113549, 1, 9, 14] {
            continue;
        }
        let Val::Raw(first) = &values[0] else { continue };
        let (Val::Slice(list), _) = unmarshal(first, &ext_ty)? else { unreachable!("a slice") };
        for e in list {
            let Val::Struct(f) = e else { continue };
            let (Val::Oid(id), Val::Octets(value)) = (&f[0], &f[2]) else { continue };
            let s = oid_string(id);
            if seen.contains(&s) {
                return Err(x("certificate request contains duplicate requested extensions"));
            }
            seen.push(s);
            exts.push((id.clone(), value.clone()));
        }
    }
    let mut dns = Vec::new();
    for (id, value) in exts {
        if id == [2, 5, 29, 17] {
            dns = parse_san(&value)?.0;
        }
    }
    Ok(Request { subject, dns, key, sig_alg })
}

/// Go's `ip.String()` for 4 or 16 bytes (an IPv4-mapped address never gets here).
pub(crate) fn ip_string(b: &[u8]) -> String {
    match <[u8; 4]>::try_from(b) {
        Ok(v4) => std::net::Ipv4Addr::from(v4).to_string(),
        Err(_) => <[u8; 16]>::try_from(b).map(|v6| std::net::Ipv6Addr::from(v6).to_string()).unwrap_or_default(),
    }
}
