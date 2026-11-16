//! testdata/parity/stegotext.json: stego's text checks and file-type checks (go/stego/textmagic_parity_test.go).

mod common;
use common::{bytes, cases, Check};
use sarina::stego::{magic, text};
use serde_json::{json, Value};

fn txt(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => json!(s),
        Err(_) => json!({ "hex": b.iter().map(|c| format!("{c:02x}")).collect::<String>() }),
    }
}

#[test]
fn stegotext_matches_go() {
    let mut d = Check::new();
    for c in cases("stegotext") {
        for r in c["out"].as_array().unwrap() {
            let s = bytes(&r[0]);
            let what = format!("{} {:.40}", c["fn"], String::from_utf8_lossy(&s));
            match c["fn"].as_str().unwrap() {
                "hidden" => d.eq(what, &txt(&text::find_hidden(&s).describe()), &r[1]),
                _ => {
                    let k = magic::detect(&s);
                    let exts = if k.exts.is_empty() { Value::Null } else { json!(k.exts) };
                    let emb: Vec<Value> = magic::find_embedded(&s, 12).iter().map(|e| json!([e.offset, txt(&e.kind)])).collect();
                    let emb = if emb.is_empty() { Value::Null } else { json!(emb) };
                    d.eq(what, &json!([r[0], k.name, exts, emb]), r);
                }
            }
        }
    }
    d.done("stegotext");
}
