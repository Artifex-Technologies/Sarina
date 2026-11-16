//! testdata/parity/stegolsb.json: stego's image decoding (Go's image/png and image/gif with their errors), the LSB
//! statistics, extraction, the payload search, Diff, and math.Lgamma (go/stego/lsb_parity_test.go).

mod common;
use common::{lsb_describe as describe, short_sum as sum, Check};
use sarina::stego::lsb;
use serde_json::json;

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn stego_lsb_matches_go() {
    let mut d = Check::new();
    for c in common::cases("stegolsb") {
        let rows = c["out"].as_array().unwrap();
        match c["fn"].as_str().unwrap() {
            "decode" => {
                for r in rows {
                    let got = describe(&unhex(r[1].as_str().unwrap()));
                    d.eq(format!("{} {:.50}", r[0], r[1].as_str().unwrap()), &got, &r[2]);
                }
            }
            "diff" => {
                let imgs: Vec<Vec<u8>> = c["in"].as_array().unwrap().iter().map(|h| unhex(h.as_str().unwrap())).collect();
                let by_sum = |s: &str| imgs.iter().find(|b| sum(b) == s).expect("an input of the case").clone();
                for r in rows {
                    let (a, b) = (by_sum(r[0].as_str().unwrap()), by_sum(r[1].as_str().unwrap()));
                    let pa = lsb::decode_image(&a).unwrap();
                    let pb = lsb::decode_image(&b).unwrap();
                    let got = match lsb::diff(&pa, &pb) {
                        Ok(s) => json!([r[0], r[1], s, ""]),
                        Err(e) => json!([r[0], r[1], "", e]),
                    };
                    d.eq(format!("diff {} {}", r[0], r[1]), &got, r);
                }
            }
            _ => {
                for r in rows {
                    let x = f64::from_bits(r[0].as_u64().unwrap());
                    let got = json!([r[0], sarina::gomath::lgamma(x).to_bits()]);
                    d.eq(format!("lgamma {x}"), &got, r);
                }
            }
        }
    }
    d.done("stegolsb");
}
