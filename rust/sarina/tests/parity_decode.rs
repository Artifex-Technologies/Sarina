//! testdata/parity/decode.json: crypto_decode (go/cryptotools/decode_parity_test.go): the encodings a string looks like,
//! each decoder with Go's error texts, the automatic layered decoding and its description.

mod common;
use common::{bytes, cases, Check};
use sarina::cryptotools::decode;
use serde_json::{json, Value};

fn text(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => json!(s),
        Err(_) => json!({ "hex": b.iter().map(|c| format!("{c:02x}")).collect::<String>() }),
    }
}

#[test]
fn decode_matches_go() {
    let mut d = Check::new();
    for c in cases("decode") {
        for r in c["out"].as_array().unwrap() {
            match c["fn"].as_str().unwrap() {
                "looks" => {
                    let s = r[0].as_str().unwrap();
                    let l: Vec<&str> = decode::ENCODINGS.iter().copied().filter(|e| decode::looks(e, s.as_bytes())).collect();
                    let want = if r[1].is_null() { json!([]) } else { r[1].clone() };
                    d.eq(format!("looks {s:?}"), &json!(l), &want);
                }
                "one" => {
                    let (enc, s) = (r[0].as_str().unwrap(), r[1].as_str().unwrap());
                    let err = r[3].as_str().unwrap();
                    match decode::decode_one(enc, s.as_bytes()) {
                        // Go's partial output beside an error is never used: only the error is compared then
                        Err(e) => d.eq(format!("one {enc} {s:?}"), &json!(e), &json!(err)),
                        Ok(out) => d.eq(format!("one {enc} {s:?}"), &json!([text(&out), ""]), &json!([r[2], err])),
                    }
                }
                "auto" => {
                    let s = bytes(&r[0]);
                    let depth = r[1].as_u64().unwrap() as usize;
                    let got = decode::describe(decode::decode_auto(&s, depth).as_ref());
                    d.eq(format!("auto {:.50} depth {depth}", String::from_utf8_lossy(&s)), &text(&got), &r[2]);
                }
                f => panic!("unknown fn {f}"),
            }
        }
    }
    d.done("decode");
}
