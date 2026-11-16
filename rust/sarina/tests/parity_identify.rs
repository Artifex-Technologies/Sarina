//! testdata/parity/identify.json: crypto_identify's report (go/cryptotools/identify_parity_test.go) on every kind of
//! string and file bytes it names.

mod common;
use common::{bytes, cases, Check};
use sarina::cryptotools::identify;
use serde_json::{json, Value};

fn text(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => json!(s),
        Err(_) => json!({ "hex": b.iter().map(|c| format!("{c:02x}")).collect::<String>() }),
    }
}

#[test]
fn identify_matches_go() {
    let mut d = Check::new();
    for c in cases("identify") {
        for r in c["out"].as_array().unwrap() {
            let s = bytes(&r[0]);
            d.eq(format!("identify {:.60}", String::from_utf8_lossy(&s)), &text(&identify::identify_report(&s, "string")), &r[1]);
        }
    }
    d.done("identify");
}
