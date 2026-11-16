//! testdata/parity/stegojpeg.json: Go's image/jpeg decoder through stego's DecodeImage, on the files in
//! testdata/parity/jpeg and on every truncation and a byte change at every position of three of them
//! (go/stego/jpeg_parity_test.go).

mod common;
use common::{lsb_describe, short_sum, Check};
use sarina::stego::lsb;
use serde_json::{json, Value};

fn brief(b: &[u8]) -> Value {
    match lsb::decode_image(b) {
        Err(e) => json!(e),
        Ok(p) => json!([p.w, p.h, short_sum(&p.rgba)]),
    }
}

#[test]
fn stego_jpeg_matches_go() {
    let dir = format!("{}/../../testdata/parity/jpeg", env!("CARGO_MANIFEST_DIR"));
    let mut d = Check::new();
    for c in common::cases("stegojpeg") {
        let rows = c["out"].as_array().unwrap();
        if c["fn"] == "file" {
            for r in rows {
                let b = std::fs::read(format!("{dir}/{}", r[0].as_str().unwrap())).unwrap();
                d.eq(format!("file {}", r[0]), &lsb_describe(&b), &r[1]);
            }
            continue;
        }
        let name = c["in"].as_str().unwrap();
        let b = std::fs::read(format!("{dir}/{name}")).unwrap();
        for r in rows {
            let i = r[0].as_u64().unwrap() as usize;
            let mut changed = b.clone();
            changed[i] ^= 0x41;
            let got = json!([i, brief(&changed), brief(&b[..i])]);
            d.eq(format!("{name} at {i}"), &got, r);
        }
    }
    d.done("stegojpeg");
}
