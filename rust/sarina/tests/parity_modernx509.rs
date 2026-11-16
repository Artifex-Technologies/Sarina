//! testdata/parity/modernx509.json: crypto_modern on certificates and certificate requests (crypto/x509's parsers: every
//! extension, truncations and byte changes), bundles, the base64 DER form, and JWTs checked with a certificate
//! (go/cryptotools/x509_parity_test.go).

mod common;
use common::{text, Check};
use sarina::cryptotools::modern;
use serde_json::{json, Value};

/// Go's `pem.EncodeToMemory` for a block without headers.
fn pem_of(label: &str, der: &[u8]) -> String {
    const STD: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for c in der.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |v, (i, &x)| v | u32::from(x) << (16 - 8 * i));
        for i in 0..4 {
            b64.push(if i <= c.len() { STD[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let mut s = format!("-----BEGIN {label}-----\n");
    for line in b64.as_bytes().chunks(64) {
        s += std::str::from_utf8(line).unwrap();
        s.push('\n');
    }
    s + &format!("-----END {label}-----\n")
}

/// The answer as the fixture holds it.
fn run(input: &str, key: &str) -> Vec<Value> {
    match modern::modern(input, key, (1_700_000_000, 0)) {
        Ok(o) => vec![text(&o), json!("")],
        Err(e) => vec![json!(""), json!(e)],
    }
}

#[test]
fn modern_x509_matches_go() {
    let mut d = Check::new();
    for c in common::cases("modernx509") {
        for r in c["out"].as_array().unwrap() {
            let r = r.as_array().unwrap();
            let got = match c["fn"].as_str().unwrap() {
                "cert" => {
                    let (label, h) = (r[0].as_str().unwrap(), r[1].as_str().unwrap());
                    let der: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
                    [vec![r[0].clone(), r[1].clone()], run(&pem_of(label, &der), "")].concat()
                }
                "raw" => [vec![r[0].clone()], run(r[0].as_str().unwrap(), "")].concat(),
                "uri" => {
                    let s = r[0].as_str().unwrap();
                    let parsed = match sarina::gourl::parse_host_or_error(s) {
                        Ok(h) => json!({ "host": String::from_utf8_lossy(&h) }),
                        Err(e) => json!({ "err": e }),
                    };
                    vec![r[0].clone(), parsed, json!(sarina::gonetip::parses_as_ip(s))]
                }
                _ => [vec![r[0].clone(), r[1].clone()], run(r[0].as_str().unwrap(), r[1].as_str().unwrap())].concat(),
            };
            d.eq(
                format!("{} {}", c["fn"], &r[1].to_string()[..r[1].to_string().len().min(90)]),
                &Value::Array(got),
                &Value::Array(r.clone()),
            );
        }
    }
    d.done("modernx509");
}
