//! testdata/parity/trust.json: Go's base64 reading, ed25519.Verify on edge cases (small-order and non-canonical keys,
//! S+L), whole trust folders asked about files on a made-up tree, and Heimdall's lock.

mod common;

use ring::signature::{Ed25519KeyPair, KeyPair};
use sarina::trust::{contained, std_base64, verify, Checker, ENV_CONTAINED};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn key(seed: &[u8]) -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(seed).unwrap()
}

fn seed() -> Vec<u8> {
    Sha256::digest(b"sarina trust parity").to_vec()
}

// L, the group order, little-endian
const L: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0x10,
];

/// The Go test's sigOp.
fn sig_op(op: &str, body: &[u8]) -> Vec<u8> {
    let k = key(&seed());
    let mut sig = k.sign(body).as_ref().to_vec();
    match op {
        "flip" => sig[5] ^= 1,
        "flip-s" => sig[40] ^= 0x10,
        "s-plus-l" => {
            let mut carry = 0u16;
            for i in 0..32 {
                let s = sig[32 + i] as u16 + L[i] as u16 + carry;
                sig[32 + i] = s as u8;
                carry = s >> 8;
            }
        }
        "other-key" => sig = key(&[0u8; 32]).sign(body).as_ref().to_vec(),
        "short" => sig.truncate(63),
        "long" => sig.push(0),
        "other-body" => {
            let mut b = body.to_vec();
            b.push(b' ');
            sig = k.sign(&b).as_ref().to_vec();
        }
        "zero" => sig = vec![0; 64],
        _ => {}
    }
    sig
}

fn sig_text_op(op: &str, s: &str) -> String {
    match op {
        "crlf-end" => format!("{s}\r\n"),
        "crlf-in" => format!("{}\r\n{}", &s[..20], &s[20..]),
        _ => format!("x{s}"),
    }
}

fn b64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for j in 0..4 {
            out.push(if j <= c.len() { A[(n >> (18 - 6 * j)) as usize & 63] as char } else { '=' });
        }
    }
    out
}

fn tree(root: &Path) {
    for (name, body) in [("files/a.txt", "alpha\n"), ("files/b.txt", "beta\n"), ("files/empty", ""), ("files/sub/c.md", "# c\n")] {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    std::fs::create_dir_all(root.join("files/dir")).unwrap();
    std::os::unix::fs::symlink(root.join("files/a.txt"), root.join("files/link")).unwrap();
}

#[test]
fn trust_matches_go() {
    let mut check = common::Check::new();
    let base = std::env::temp_dir().join(format!("sarina-trust-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("root");
    tree(&root);
    let root_s = root.to_str().unwrap().to_string();
    let wd = std::env::current_dir().unwrap().to_str().unwrap().to_string();
    let cases = common::cases("trust");
    let queries: Vec<String> = cases.iter().find(|c| c["fn"] == "queries").unwrap()["in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q.as_str().unwrap().to_string())
        .collect();
    // the fixture's key is the one this seed gives
    assert_eq!(b64(key(&seed()).public_key().as_ref()), "oW2dpGYdSfnjzRcbkv+2M+XiGq8AEKXNm1hDtVIEqt0=");
    for (n, c) in cases.iter().enumerate() {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "base64" => json!(std_base64(&common::bytes(input)).map_or("error".to_string(), |b| hex(&b))),
            "verify" => {
                let p: Vec<Vec<u8>> = input.as_array().unwrap().iter().map(|x| unhex(x.as_str().unwrap())).collect();
                json!(verify(&p[0], &p[1], &p[2]))
            }
            "checker" => {
                let dir = base.join(format!("trust{n}"));
                std::fs::create_dir_all(&dir).unwrap();
                if let Some(m) = input["mode"].as_str() {
                    std::fs::write(dir.join("mode"), m).unwrap();
                }
                if let Some(p) = input["pub"].as_str() {
                    std::fs::write(dir.join("owner.pub"), p).unwrap();
                }
                let mut body = Vec::new();
                if !input["manifest"].is_null() {
                    let tmpl = common::bytes(&input["manifest"]);
                    body = String::from_utf8_lossy(&tmpl).replace("{ROOT}", &root_s).into_bytes();
                    if std::str::from_utf8(&tmpl).is_err() {
                        // keep the bytes that are not UTF-8: replace {ROOT} bytewise
                        let mut out = Vec::new();
                        let mut rest = &tmpl[..];
                        while let Some(i) = rest.windows(6).position(|w| w == b"{ROOT}") {
                            out.extend_from_slice(&rest[..i]);
                            out.extend_from_slice(root_s.as_bytes());
                            rest = &rest[i + 6..];
                        }
                        out.extend_from_slice(rest);
                        body = out;
                    }
                    std::fs::write(dir.join("manifest.json"), &body).unwrap();
                }
                if let Some(op) = input["sig"].as_str() {
                    let mut s = b64(&sig_op(op, &body));
                    if let Some(t) = input["sigText"].as_str() {
                        s = sig_text_op(t, &s);
                    }
                    std::fs::write(dir.join("manifest.sig"), s).unwrap();
                }
                let ch = Checker::new(dir.to_str().unwrap().as_bytes());
                let refuse: Vec<String> = queries
                    .iter()
                    .map(|q| ch.refuse(q.replace("{ROOT}", &root_s).as_bytes()).replace(&root_s, "{ROOT}").replace(&wd, "{CWD}"))
                    .collect();
                json!({"enforcing": ch.enforcing(), "refuse": refuse})
            }
            "queries" => continue,
            "contained" => {
                let p = base.join(format!("lock{n}"));
                match input {
                    Value::Null => {}
                    Value::String(s) if s == "folder" => std::fs::create_dir_all(&p).unwrap(),
                    _ => std::fs::write(&p, common::bytes(input)).unwrap(),
                }
                let ps = p.to_str().unwrap().to_string();
                let (on, why) = contained(&|k| if k == ENV_CONTAINED { ps.clone() } else { String::new() });
                json!([on, why])
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&base);
    check.done("trust");
}
