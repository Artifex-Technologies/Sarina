//! Go: `cryptotools/modern.go`, the part without X.509: crypto_modern on SSH public keys (type, size, fingerprint), JWTs
//! (header, claims, expiry; an HMAC signature verified with the secret given), PGP blocks (gpg's packet list, never
//! decrypting or importing), and private keys refused. Certificates, public keys in PEM and RS/ES/PS signatures need
//! Go's crypto/x509 and come in the next part: [`modern_without_x509`] answers None for them
//! (testdata/parity/modern.json).

use super::decode::{base64_raw, base64_std};
use super::hash::{hmac, Algo};
use super::to_upper;
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

/// Go's `Modern` but for what needs crypto/x509: None for a PEM block that is not a private key or PGP, for anything that
/// is not an SSH key, JWT or PGP block but decodes as base64 (Go then tries it as a DER certificate), and for a JWT
/// checked with RS/ES/PS.
/// `now` is (Unix seconds, nanoseconds).
pub fn modern_without_x509(input: &str, key: &str, now: (i64, u32)) -> Option<Result<Vec<u8>, String>> {
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
        return None;
    }
    if SSH_RE.is_match(t) {
        return Some(ssh_info(t).map(String::into_bytes));
    }
    if JWT_RE.is_match(t) {
        return jwt_info(t, key, now).map(|r| r.map(String::into_bytes));
    }
    // Go tries the base64 of a DER certificate last: only what decodes needs crypto/x509 to tell
    let joined: String = gotext::fields(t).concat();
    if base64_std(joined.as_bytes()).is_ok() {
        return None;
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

/// Go's `jwtInfo` (None: an RS/ES/PS signature to check, which needs crypto/x509).
fn jwt_info(t: &str, key: &str, now: (i64, u32)) -> Option<Result<String, String>> {
    let parts: Vec<&str> = t.split('.').collect();
    let dec = |s: &str| base64_raw(s.trim_end_matches('=').as_bytes(), URL64);
    let hdr = match dec(parts[0]) {
        Ok(h) => h,
        Err(e) => return Some(Err(format!("the header is not base64url: {e}"))),
    };
    let body = match dec(parts[1]) {
        Ok(b) => b,
        Err(e) => return Some(Err(format!("the payload is not base64url: {e}"))),
    };
    let Some(h) = json_map(&hdr) else { return Some(Err("the header is not JSON".into())) };
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
        let (ok, how) = verify_jwt(&parts, &alg, key)?;
        b += &format!("  Signature: {} ({how}).\n", if ok { "VERIFIED with the key given" } else { "does NOT verify with the key given" });
    }
    Some(Ok(b))
}

/// Go's `verifyJWT` for HMAC (None: RS/ES/PS, which need crypto/x509).
fn verify_jwt(parts: &[&str], alg: &str, key: &str) -> Option<(bool, String)> {
    let not_mine = || Some((false, format!("algorithm {alg} is not one I verify")));
    if alg.len() < 2 {
        return not_mine();
    }
    let Ok(sig) = base64_raw(parts[2].trim_end_matches('=').as_bytes(), URL64) else {
        return Some((false, "the signature is not base64url".into()));
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
            Some((mac == sig, "HMAC with the shared secret".into()))
        }
        Some("RS" | "ES" | "PS") => None,
        _ => not_mine(),
    }
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
