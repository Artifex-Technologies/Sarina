//! testdata/parity/modernkeys.json: crypto_modern on public keys in PEM, the PEM framing, and JWTs signed RS/PS/ES
//! (go/cryptotools/keys_parity_test.go).

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

fn run(input: &str, key: &str) -> Vec<Value> {
    match modern::modern(input, key, (1_700_000_000, 0)) {
        Some(Ok(o)) => vec![text(&o), json!("")],
        Some(Err(e)) => vec![json!(""), json!(e)],
        None => vec![json!("needs crypto/x509"), json!("")],
    }
}

#[test]
fn modern_keys_match_go() {
    let mut d = Check::new();
    for c in common::cases("modernkeys") {
        for r in c["out"].as_array().unwrap() {
            let r = r.as_array().unwrap();
            let got = match c["fn"].as_str().unwrap() {
                "pem" => {
                    let (label, h) = (r[0].as_str().unwrap(), r[1].as_str().unwrap());
                    let der: Vec<u8> = (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect();
                    [vec![r[0].clone(), r[1].clone()], run(&pem_of(label, &der), "")].concat()
                }
                "raw" => [vec![r[0].clone()], run(r[0].as_str().unwrap(), "")].concat(),
                _ => [vec![r[0].clone(), r[1].clone()], run(r[0].as_str().unwrap(), r[1].as_str().unwrap())].concat(),
            };
            d.eq(format!("{} {:.80}", c["fn"], r[0].to_string()), &Value::Array(got), &Value::Array(r.clone()));
        }
    }
    d.done("modernkeys");
}
