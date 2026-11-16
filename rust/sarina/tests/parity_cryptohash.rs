//! testdata/parity/cryptohash.json: crypto_hash's digests, HMACs and comparisons (go/cryptotools/hash_parity_test.go).

mod common;
use common::{bytes, cases, Check};
use sarina::cryptotools::hash;
use serde_json::json;

#[test]
fn cryptohash_matches_go() {
    let mut d = Check::new();
    let all = cases("cryptohash");
    let inputs = all.iter().find(|c| c["fn"] == "inputs").unwrap();
    let datas: Vec<Vec<u8>> = inputs["in"].as_array().unwrap().iter().map(bytes).collect();
    let keys: Vec<Vec<u8>> = inputs["out"].as_array().unwrap().iter().map(bytes).collect();
    for c in &all {
        match c["fn"].as_str().unwrap() {
            "inputs" => {}
            "digest" => {
                for r in c["out"].as_array().unwrap() {
                    let a = r[0].as_str().unwrap();
                    let (i, j) = (r[1].as_u64().unwrap() as usize, r[2].as_u64().unwrap() as usize);
                    let (got, err) = match hash::digest(a, &datas[i], &keys[j]) {
                        Ok(g) => (g, String::new()),
                        Err(e) => (String::new(), e),
                    };
                    d.eq(format!("digest {a:?} data {i} key {j}"), &json!([a, i, j, got, err]), r);
                }
            }
            "report" => {
                for r in c["out"].as_array().unwrap() {
                    let s = |k: usize| r[k].as_str().unwrap();
                    let got = hash::hash_report(s(0).as_bytes(), s(1), s(2), s(3), s(4));
                    d.eq(format!("report {r}"), &json!([s(0), s(1), s(2), s(3), s(4), got]), r);
                }
            }
            f => panic!("unknown fn {f}"),
        }
    }
    d.done("cryptohash");
}
