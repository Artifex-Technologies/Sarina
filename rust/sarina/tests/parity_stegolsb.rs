//! testdata/parity/stegolsb.json: stego's image decoding (Go's image/png and image/gif with their errors), the LSB
//! statistics, extraction, the payload search, Diff, and math.Lgamma (go/stego/lsb_parity_test.go).

mod common;
use common::Check;
use sarina::stego::lsb;
use serde_json::{json, Value};

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect()
}

fn sum(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b)[..8].iter().map(|x| format!("{x:02x}")).collect()
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn describe(b: &[u8]) -> Value {
    let p = match lsb::decode_image(b) {
        None => return json!("a JPEG: Go decodes it"),
        Some(Err(e)) => return json!({ "err": e }),
        Some(Ok(p)) => p,
    };
    let found: Vec<Value> =
        lsb::search_lsb(&p, 4096).iter().map(|f| json!([f.order, f.bits, f.msb_first, f.length, f.why, hexs(&f.data)])).collect();
    let mut ex = Vec::new();
    let palette: (&str, &[i32]) = ("palette index", &[-1]);
    for (_, chans) in std::iter::once(palette).chain(lsb::ORDERS) {
        for bits in [1, 2, 4, 9] {
            ex.push(json!(hexs(&lsb::extract_bits(&p, chans, bits, bits % 2 == 0, 40))));
        }
    }
    json!({
        "w": p.w, "h": p.h, "format": p.format, "alpha": p.alpha, "rgba": sum(&p.rgba),
        "indices": p.indices.as_ref().map_or(String::new(), |i| sum(i)),
        "describe": lsb::describe(&lsb::analyse(&p)), "found": if found.is_empty() { Value::Null } else { Value::Array(found) }, "extract": ex,
    })
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
                    let pa = lsb::decode_image(&a).unwrap().unwrap();
                    let pb = lsb::decode_image(&b).unwrap().unwrap();
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
