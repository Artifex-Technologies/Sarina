//! testdata/parity/sessionsearch.json: Search over a made-up session folder (files ordered by their modification time),
//! and Go's amd64 math.Log.

mod common;

use sarina::gomath;
use sarina::sessionsearch::{search, Hit};
use serde_json::{json, Value};
use std::time::{Duration, UNIX_EPOCH};

fn hits_out(hs: &[Hit]) -> Value {
    json!(hs
        .iter()
        .map(|h| json!([h.session_id, h.role, h.turn_index, common::text(h.snippet.as_bytes()), format!("{:x}", h.score.to_bits())]))
        .collect::<Vec<_>>())
}

#[test]
fn sessionsearch_matches_go() {
    let mut check = common::Check::new();
    let dir = std::env::temp_dir().join(format!("sarina-search-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("folder.json")).unwrap();
    // 2026-01-01T00:00:00Z
    let epoch = UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    let cases = common::cases("sessionsearch");
    for f in cases.iter().find(|c| c["fn"] == "files").unwrap()["in"].as_array().unwrap() {
        let p = dir.join(f[0].as_str().unwrap());
        std::fs::write(&p, common::bytes(&f[2])).unwrap();
        let file = std::fs::File::options().write(true).open(&p).unwrap();
        file.set_modified(epoch + Duration::from_secs(f[1].as_u64().unwrap())).unwrap();
    }
    let d = dir.to_str().unwrap();
    for c in &cases {
        let got = match c["fn"].as_str().unwrap() {
            "files" => continue,
            "search" => {
                let q = String::from_utf8(common::bytes(&c["in"][0])).unwrap();
                hits_out(&search(&q, d, c["in"][1].as_i64().unwrap()))
            }
            "missing" => hits_out(&search("pytest", &format!("{d}/missing"), 3)),
            "sparse" => {
                let sp = dir.join("sparse");
                std::fs::create_dir_all(&sp).unwrap();
                std::fs::write(
                    sp.join("s.json"),
                    r#"{"messages":[{"role":"user","content":"!!!"},{"role":"user","content":"??"},{"role":"user","content":"..."},{"role":"user","content":"x"},{"role":"assistant","content":"x y"}]}"#,
                )
                .unwrap();
                hits_out(&search(c["in"].as_str().unwrap(), sp.to_str().unwrap(), 5))
            }
            "log" => {
                let out: Vec<String> = c["out"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        let x = u64::from_str_radix(p.as_str().unwrap().split(':').next().unwrap(), 16).unwrap();
                        format!("{x:x}:{:x}", gomath::log(f64::from_bits(x)).to_bits())
                    })
                    .collect();
                json!(out)
            }
            other => panic!("unknown fn {other}"),
        };
        check.eq(format!("{} {}", c["fn"], c["in"]), &got, &c["out"]);
    }
    let _ = std::fs::remove_dir_all(&dir);
    check.done("sessionsearch");
}
