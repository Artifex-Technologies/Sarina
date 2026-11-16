//! Go: `cryptotools/modern.go`: crypto_modern on certificates, certificate requests and public keys in PEM (crypto/x509's
//! parsers ported, [`super::x509`] and [`super::pubkey`]), SSH public keys (type, size, fingerprint), JWTs (header,
//! claims, expiry; an HMAC, RSA, RSA-PSS or ECDSA signature verified with the key given), PGP blocks (gpg's packet list,
//! never decrypting or importing), and private keys refused (testdata/parity/modern.json, modernkeys.json,
//! modernx509.json).

use super::decode::{base64_raw, base64_std};
use super::hash::{hmac, sum, Algo};
use super::pem;
use super::pubkey::{self, PubKey};
use super::to_upper;
use super::x509;
use crate::goany::Any;
use crate::gotext::{self, quote};
use regex::Regex;
use std::sync::LazyLock;

const URL64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn re(p: &str) -> Regex {
    crate::goregexp::compile(p).expect("modern pattern")
}
static PEM_RE: LazyLock<Regex> = LazyLock::new(|| re(r"-----BEGIN ([A-Z0-9 ]+)-----"));
static JWT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{2,}\.[A-Za-z0-9_-]*$"));
static SSH_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(ssh-(rsa|ed25519|dss)|ecdsa-sha2-nistp(256|384|521)) [A-Za-z0-9+/=]+"));

/// Go's `pemLabel`: the first whole header's label, or `fallback`.
fn pem_label<'a>(t: &'a str, fallback: &'a str) -> &'a str {
    PEM_RE.captures(t).and_then(|m| m.get(1)).map_or(fallback, |m| m.as_str())
}

/// Go's `Modern`; None only for a certificate (or request) in PEM with a URI name that url.Parse refuses, whose error text
/// would need net/url's (docs/RUST_PORT.md "Differences").
/// `now` is (Unix seconds, nanoseconds).
pub fn modern(input: &str, key: &str, now: (i64, u32)) -> Option<Result<Vec<u8>, String>> {
    let t = gotext::trim_space(input);
    if t.is_empty() {
        return Some(Err("nothing to look at".into()));
    }
    if t.contains("-----BEGIN PGP") {
        return Some(Ok(pgp_info(t)));
    }
    if t.contains("PRIVATE KEY-----") {
        return Some(Ok(format!("This is a PRIVATE key ({}). I do not parse, print or load private keys: keep it out of the conversation, and if it was pasted somewhere it should not have been, treat it as exposed and make a new one.", pem_label(t, "PRIVATE KEY")).into_bytes()));
    }
    if t.contains("-----BEGIN") {
        return pem_info(t, now).map(|r| r.map(String::into_bytes));
    }
    if SSH_RE.is_match(t) {
        return Some(ssh_info(t).map(String::into_bytes));
    }
    if JWT_RE.is_match(t) {
        return Some(jwt_info(t, key, now).map(String::into_bytes));
    }
    // Go tries the base64 of a DER certificate last
    let joined: String = gotext::fields(t).concat();
    if let Ok(der) = base64_std(joined.as_bytes()) {
        if let Ok(c) = x509::parse_certificate(&der) {
            return Some(Ok(cert_info(&c, now, 1, 1).into_bytes()));
        }
    }
    Some(Err("not a certificate, public key, SSH key, JWT or PGP block that I recognise; crypto_identify says what it might be".into()))
}

/// Go's `sshInfo`.
fn ssh_info(t: &str) -> Result<String, String> {
    let fields = gotext::fields(t);
    let blob = base64_std(fields[1].as_bytes()).map_err(|e| format!("the key's base64 body is broken: {e}"))?;
    fn read(b: &[u8]) -> Option<(&[u8], &[u8])> {
        if b.len() < 4 {
            return None;
        }
        let n = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
        if b.len() - 4 < n {
            return None;
        }
        Some((&b[4..4 + n], &b[4 + n..]))
    }
    let Some((typ, rest)) = read(&blob).filter(|(typ, _)| *typ == fields[0].as_bytes()) else {
        return Err("the key's body does not match its type prefix".into());
    };
    let _ = typ;
    let size = match fields[0] {
        "ssh-rsa" => match read(rest).and_then(|(_, rest2)| read(rest2)) {
            Some((n, _)) => {
                let n: Vec<u8> = n.iter().copied().skip_while(|&b| b == 0).collect();
                let bits = if n.is_empty() { 0 } else { (n.len() - 1) * 8 + (8 - n[0].leading_zeros() as usize) };
                format!("{bits} bits")
            }
            None => String::new(),
        },
        "ssh-ed25519" => "256 bits".into(),
        "ssh-dss" => "DSA (obsolete: OpenSSH no longer accepts it by default)".into(),
        other => other.strip_prefix("ecdsa-sha2-").unwrap_or(other).into(),
    };
    let comment = if fields.len() > 2 { fields[2..].join(" ") } else { String::new() };
    use sha2::Digest;
    let fp = sha2::Sha256::digest(&blob);
    let fp = base64_encode(&fp);
    Ok(format!(
        "SSH public key: {}, {size}. Fingerprint SHA256:{}. Comment: {}.\nA public key can be shared; it identifies the holder of the matching private key, which this is not.",
        fields[0],
        fp.trim_end_matches('='),
        quote(&comment)
    ))
}

/// Base64 (standard alphabet, padded).
fn base64_encode(b: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let v = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Go's `json.Unmarshal(b, &m)` into `map[string]any`: Some(the map; None for JSON null), or None on an error.
fn json_map(b: &[u8]) -> Option<Option<std::collections::BTreeMap<String, Any>>> {
    let (v, err) = crate::gojson::unmarshal_any(b);
    if err.is_some() {
        return None;
    }
    match &v {
        Any::Object(m) => Some(Some(m.clone())),
        Any::Null => Some(None),
        _ => None,
    }
}

/// Go's `compactJSON`: an object's keys, sorted, with their values as json.Marshal writes them; else the bytes.
fn compact_json(b: &[u8]) -> String {
    match json_map(b) {
        Some(m) => m
            .unwrap_or_default()
            .iter()
            .map(|(k, v)| format!("{k}: {}", crate::gojson::marshal_any(v).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", "),
        None => gotext::lossy(b),
    }
}

/// Go's conversion of a float64 to int64 on amd64.
fn go_int(x: f64) -> i64 {
    if x.is_nan() || x >= 9.223_372_036_854_776e18 || x < -9.223_372_036_854_776e18 {
        i64::MIN
    } else {
        x as i64
    }
}

/// Go's `time.Unix(s, 0).UTC().Format(time.RFC3339)` (the year padded to four digits, with its sign when negative).
fn rfc3339(secs: i64) -> String {
    let days = (secs as i128).div_euclid(86_400);
    let rem = (secs as i128).rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i128::from(m <= 2);
    let year = if y < 0 { format!("-{:04}", -y) } else { format!("{y:04}") };
    format!("{year}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Go's `jwtInfo`.
fn jwt_info(t: &str, key: &str, now: (i64, u32)) -> Result<String, String> {
    let parts: Vec<&str> = t.split('.').collect();
    let dec = |s: &str| base64_raw(s.trim_end_matches('=').as_bytes(), URL64);
    let hdr = match dec(parts[0]) {
        Ok(h) => h,
        Err(e) => return Err(format!("the header is not base64url: {e}")),
    };
    let body = match dec(parts[1]) {
        Ok(b) => b,
        Err(e) => return Err(format!("the payload is not base64url: {e}")),
    };
    let Some(h) = json_map(&hdr) else { return Err("the header is not JSON".into()) };
    let mut b = String::from("JWT (JSON Web Token)\n");
    b += &format!("  Header: {}\n", compact_json(&hdr));
    match json_map(&body) {
        Some(p) => {
            b += &format!("  Payload: {}\n", compact_json(&body));
            let p = p.unwrap_or_default();
            for claim in ["iat", "nbf", "exp"] {
                if let Some(Any::Number(v)) = p.get(claim) {
                    let ts = go_int(*v);
                    let mut note = "";
                    if claim == "exp" {
                        // now.After(ts): ts has no fraction of a second
                        note = if now.0 > ts || (now.0 == ts && now.1 > 0) { " (EXPIRED)" } else { " (not expired)" };
                    }
                    b += &format!("  {claim}: {}{note}\n", rfc3339(ts));
                }
            }
        }
        None => b += &format!("  Payload: not JSON ({} bytes)\n", body.len()),
    }
    let alg = match h.as_ref().and_then(|h| h.get("alg")) {
        Some(Any::String(s)) => s.clone(),
        _ => String::new(),
    };
    if gotext::equal_fold(&alg, "none") {
        b += "  Signature: alg is \"none\": the token carries NO signature. Anyone can forge it; a server that accepts alg none is broken.\n";
    } else if parts.len() < 3 || parts[2].is_empty() {
        b += &format!("  Signature: missing (alg {alg}): the token is unsigned or truncated.\n");
    } else if key.is_empty() {
        let what = if alg.starts_with("HS") { "shared secret" } else { "issuer's public key" };
        b += &format!("  Signature: present (alg {alg}) but NOT VERIFIED: that needs the {what}. Decoding a JWT proves nothing about who issued it; treat the claims as unverified.\n");
    } else {
        let (ok, how) = verify_jwt(&parts, &alg, key);
        b += &format!("  Signature: {} ({how}).\n", if ok { "VERIFIED with the key given" } else { "does NOT verify with the key given" });
    }
    Ok(b)
}

/// Go's `verifyJWT`.
fn verify_jwt(parts: &[&str], alg: &str, key: &str) -> (bool, String) {
    let not_mine = || (false, format!("algorithm {alg} is not one I verify"));
    if alg.len() < 2 {
        return not_mine();
    }
    let Ok(sig) = base64_raw(parts[2].trim_end_matches('=').as_bytes(), URL64) else {
        return (false, "the signature is not base64url".into());
    };
    // Go slices the bytes: a family or size cut inside a character never matches
    let size = alg.get(2..).map(to_upper);
    let algo = match size.as_deref() {
        Some("256") => Algo::Sha256,
        Some("384") => Algo::Sha384,
        Some("512") => Algo::Sha512,
        _ => return not_mine(),
    };
    match alg.get(..2).map(to_upper).as_deref() {
        Some("HS") => {
            let signed = format!("{}.{}", parts[0], parts[1]);
            let mac = hmac(algo, key.as_bytes(), signed.as_bytes());
            (mac == sig, "HMAC with the shared secret".into())
        }
        Some(fam @ ("RS" | "ES" | "PS")) => {
            let (Some(block), _) = pem::decode(key.as_bytes()) else {
                return (false, "the key is not a PEM public key".into());
            };
            let pub_key = match pubkey::parse_pkix(&block.bytes) {
                Ok(k) => Some(k),
                Err(_) => match x509::parse_certificate(&block.bytes) {
                    Ok(c) => c.key,
                    Err(_) => return (false, "the PEM is not a public key or certificate".into()),
                },
            };
            let signed = format!("{}.{}", parts[0], parts[1]);
            let digest = sum(algo, signed.as_bytes());
            match pub_key {
                Some(PubKey::Rsa { n, e }) if fam == "PS" => {
                    (pubkey::rsa_pss(&n.mag, e, algo, &digest, &sig), "RSA-PSS with the public key".into())
                }
                Some(PubKey::Rsa { n, e }) => (pubkey::rsa_pkcs1v15(&n.mag, e, algo, &digest, &sig), "RSA with the public key".into()),
                Some(PubKey::Ecdsa { curve, x, y }) => {
                    let n = curve.bits.div_ceil(8);
                    if sig.len() != 2 * n {
                        return (false, "ECDSA signature has the wrong length".into());
                    }
                    (pubkey::ecdsa_verify(curve, &x, &y, &digest, &sig[..n], &sig[n..]), "ECDSA with the public key".into())
                }
                Some(_) | None => (false, format!("the key type does not fit {alg}")),
            }
        }
        _ => not_mine(),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Go's `keyDesc` of a certificate's key (`%T` of nil for an algorithm Go leaves unparsed).
fn desc(k: &Option<PubKey>) -> String {
    k.as_ref().map_or_else(|| "<nil>".into(), pubkey::key_desc)
}

/// Go's `durationWords(now.Sub(t))` or `t.Sub(now)` (`later` - `earlier`), with time.Duration's saturation.
fn duration_words(later: (i64, u32), earlier: (i64, u32)) -> String {
    let ns = (i128::from(later.0) - i128::from(earlier.0)) * 1_000_000_000 + i128::from(later.1) - i128::from(earlier.1);
    let d = ns.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
    const HOUR: i64 = 3_600_000_000_000;
    let hours = (d / HOUR) as f64 + (d % HOUR) as f64 / 3.6e12;
    let days = (hours / 24.0) as i64;
    if days >= 365 {
        format!("{} years", days / 365)
    } else if days >= 1 {
        format!("{days} days")
    } else {
        format!("{} hours", hours as i64)
    }
}

/// Go's `certInfo`.
fn cert_info(c: &x509::Cert, now: (i64, u32), n: usize, total: usize) -> String {
    let (subject, issuer) = (c.subject.string(), c.issuer.string());
    let mut b = format!("Certificate {n} of {total}\n");
    b += &format!(
        "  Subject: {subject}\n  Issuer: {issuer}{}\n",
        if subject == issuer { " (self-signed: subject and issuer are the same)" } else { "" }
    );
    if c.dns.len() + c.ips.len() + c.emails.len() > 0 {
        let sans: Vec<String> =
            c.dns.iter().cloned().chain(c.ips.iter().map(|ip| x509::ip_string(ip))).chain(c.emails.iter().cloned()).collect();
        b += &format!("  Names (SANs): {}\n", sans.join(", "));
    }
    let (nb, na) = ((c.not_before.unix, 0), (c.not_after.unix, 0));
    let state = if now < nb {
        "NOT YET valid".to_owned()
    } else if now > na {
        format!("EXPIRED {} ago", duration_words(now, na))
    } else {
        format!("valid now, expires in {}", duration_words(na, now))
    };
    b += &format!("  Valid: {} to {} ({state})\n", c.not_before.utc_date(), c.not_after.utc_date());
    let weak = matches!(c.sig_alg, "SHA1-RSA" | "MD5-RSA" | "ECDSA-SHA1");
    b += &format!(
        "  Key: {}; signature: {}{}\n",
        desc(&c.key),
        c.sig_alg,
        if weak { " (SHA-1 or MD5 signatures are no longer trusted by browsers)" } else { "" }
    );
    b += &format!("  Serial: {}; CA: {}; SHA-256 fingerprint: {}\n", c.serial.decimal(), c.is_ca, hex(&sum(Algo::Sha256, &c.raw)));
    if !c.eku.is_empty() {
        let usages: Vec<&str> =
            c.eku.iter().map(|u| ["", "server", "client", "code signing", "email"].get(usize::from(*u)).copied().unwrap_or("")).collect();
        b += &format!("  For: {}\n", usages.join(", "));
    }
    b += "  Trust: not checked against any root store here; this is what the certificate says about itself.\n";
    b
}

/// Go's `pemInfo` (None: a certificate with a URI name url.Parse refuses, whose error text needs net/url's).
fn pem_info(t: &str, now: (i64, u32)) -> Option<Result<String, String>> {
    let total = t.matches("-----BEGIN").count();
    let mut rest = t.as_bytes();
    let mut b = String::new();
    let mut n = 0;
    while let (Some(block), r) = pem::decode(rest) {
        rest = r;
        n += 1;
        let kind = String::from_utf8_lossy(&block.kind);
        match kind.as_ref() {
            "CERTIFICATE" => match x509::parse_certificate(&block.bytes) {
                Ok(c) => b += &cert_info(&c, now, n, total),
                Err(e) if e == x509::NEEDS_GO => return None,
                Err(e) => b += &format!("Certificate {n} of {total}: cannot parse it ({e}).\n"),
            },
            "PUBLIC KEY" | "RSA PUBLIC KEY" => {
                let parsed = if kind == "RSA PUBLIC KEY" { pubkey::parse_pkcs1(&block.bytes) } else { pubkey::parse_pkix(&block.bytes) };
                match parsed {
                    Ok(k) => {
                        b += &format!(
                            "Public key: {}. SHA-256 of its DER: {}.\n",
                            pubkey::key_desc(&k),
                            hex(&sum(Algo::Sha256, &block.bytes))
                        )
                    }
                    Err(e) => b += &format!("Public key: cannot parse it ({e}).\n"),
                }
            }
            "CERTIFICATE REQUEST" => match x509::parse_request(&block.bytes) {
                Ok(r) => {
                    b += &format!(
                        "Certificate signing request for {} (SANs: {}), key {}, signature {}.\n",
                        r.subject.string(),
                        r.dns.join(", "),
                        desc(&r.key),
                        r.sig_alg
                    )
                }
                Err(e) if e == x509::NEEDS_GO => return None,
                Err(e) => b += &format!("Certificate request: cannot parse it ({e}).\n"),
            },
            _ => b += &format!("PEM block of type {kind} ({} bytes): not something I parse.\n", block.bytes.len()),
        }
    }
    if n == 0 {
        return Some(Err("the PEM block does not decode (missing lines or a broken base64 body)".into()));
    }
    Some(Ok(b))
}

/// Go's `pgpInfo`: gpg's packet list of the block (10 s at most), or the header alone without gpg.
fn pgp_info(t: &str) -> Vec<u8> {
    let kind = pem_label(t, "PGP");
    let Ok(gpg) = crate::tools::exec::look_path_go("gpg") else {
        return format!("PGP armored block: {kind}. gpg is not installed, so I cannot list its packets (key ids, algorithms); the header is all I can say.").into_bytes();
    };
    let (out, errb) = run_with_stdin(&gpg, &["--batch", "--no-tty", "--list-packets", "--list-only"], t.as_bytes());
    let mut text = gotext::trim_space_bytes(&out).to_vec();
    if text.is_empty() {
        text = gotext::trim_space_bytes(&errb).to_vec();
    }
    if text.len() > 3000 {
        text.truncate(3000);
        text.extend_from_slice("\n[…]".as_bytes());
    }
    let mut b = format!("PGP armored block: {kind}. Packets as gpg lists them (nothing was decrypted or imported):\n").into_bytes();
    b.extend_from_slice(&text);
    b
}

/// Go's `exec.CommandContext(10 s)` with standard input from `input` and both outputs kept.
fn run_with_stdin(path: &str, args: &[&str], input: &[u8]) -> (Vec<u8>, Vec<u8>) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(path).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() else {
        return (Vec::new(), Vec::new());
    };
    let mut stdin = child.stdin.take();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(s) = stdin.as_mut() {
            let _ = s.write_all(&input);
        }
    });
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut out);
        }
        out
    });
    let mut errb = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let err_reader = std::thread::spawn(move || {
        let mut e = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut e);
        }
        e
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    let _ = writer.join();
    let out = reader.join().unwrap_or_default();
    errb.extend(err_reader.join().unwrap_or_default());
    (out, errb)
}
