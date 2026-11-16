//! testdata/parity/stegometa.json: stego's container readers (PNG, JPEG, GIF, RIFF), Trailing, Strings and inflate
//! (go/stego/meta_parity_test.go).

mod common;
use common::{bytes, text, Check};
use sarina::stego::meta;
use serde_json::{json, Value};

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

fn texts(v: &[Vec<u8>]) -> Value {
    Value::Array(v.iter().map(|t| text(t)).collect())
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// The stored-block stream the Go side describes instead of writing out: n bytes of 's' in stored blocks.
fn stored(n: usize, good: bool) -> Vec<u8> {
    let mut d = vec![0x78, 0x01];
    let mut left = n;
    while left > 0 {
        let k = left.min(65535);
        left -= k;
        d.extend([u8::from(left == 0), k as u8, (k >> 8) as u8, !(k as u8), !((k >> 8) as u8)]);
        d.extend(std::iter::repeat_n(b's', k));
    }
    let sum = if good {
        let (mut a, mut s) = (1u32, 0u32);
        for _ in 0..n {
            a = (a + u32::from(b's')) % 65521;
            s = (s + a) % 65521;
        }
        s << 16 | a
    } else {
        0
    };
    d.extend(sum.to_be_bytes());
    d
}

#[test]
fn stego_meta_matches_go() {
    let mut d = Check::new();
    for c in common::cases("stegometa") {
        for r in c["out"].as_array().unwrap() {
            let r = r.as_array().unwrap();
            match c["fn"].as_str().unwrap() {
                "inflate" => {
                    let input = match &r[0] {
                        Value::String(h) => unhex(h),
                        o => stored(o["stored"].as_u64().unwrap() as usize, o["good"].as_bool().unwrap()),
                    };
                    // what Go's caller sees: whether it worked, and then the bytes
                    let got = match meta::inflate(&input) {
                        Some(out) => json!([true, out.len(), sha256_hex(&out)]),
                        None => json!([false]),
                    };
                    let want = if r[1] == true { json!([true, r[2], r[3]]) } else { json!([false]) };
                    d.eq(format!("inflate {:.60}", r[0].to_string()), &got, &want);
                }
                "parse" => {
                    let b = unhex(r[1].as_str().unwrap());
                    let k = match r[0].as_str().unwrap() {
                        "png" => meta::parse_png(&b),
                        "jpeg" => meta::parse_jpeg(&b),
                        "gif" => meta::parse_gif(&b),
                        _ => meta::parse_riff(&b),
                    };
                    let got = json!([
                        r[0],
                        r[1],
                        texts(&k.parts),
                        texts(&k.text),
                        texts(&k.unusual),
                        k.end,
                        texts(&k.problems),
                        k.trailing(&b).len()
                    ]);
                    d.eq(format!("{} {:.60}", r[0], r[1].as_str().unwrap()), &got, &Value::Array(r.clone()));
                }
                _ => {
                    let s = bytes(&r[0]);
                    let (w, t) = meta::strings(&s, 8, 15);
                    let (w2, t2) = meta::strings(&s, 4, 2);
                    let got = json!([r[0], texts(&w), t, texts(&w2), t2]);
                    d.eq(format!("strings {:.60}", r[0].to_string()), &got, &Value::Array(r.clone()));
                }
            }
        }
    }
    d.done("stegometa");
}
