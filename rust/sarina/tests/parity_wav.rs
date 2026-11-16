//! testdata/parity/wav.json: stego's WAV reader, its lowest-bit extraction and judgement (go/stego/wav_parity_test.go).

mod common;
use common::{bytes, cases, Check};
use sarina::stego::wav;
use serde_json::{json, Value};

fn txt(b: &[u8]) -> Value {
    match std::str::from_utf8(b) {
        Ok(s) => json!(s),
        Err(_) => json!({ "hex": b.iter().map(|c| format!("{c:02x}")).collect::<String>() }),
    }
}

#[test]
fn wav_matches_go() {
    let mut d = Check::new();
    for c in cases("wav") {
        if c["fn"] == "judge" {
            for r in c["out"].as_array().unwrap() {
                let data = bytes(&r[0]);
                let got = match wav::judge(&data) {
                    Some(f) => json!([r[0], true, f.length, f.why, txt(&f.data)]),
                    None => json!([r[0], false, 0, "", ""]),
                };
                d.eq(format!("judge {:?}", String::from_utf8_lossy(&data)), &got, r);
            }
            continue;
        }
        for r in c["out"].as_array().unwrap() {
            let f = bytes(&r[0]);
            let got = match wav::parse_wav(&f) {
                Err(e) => json!([r[0], e]),
                Ok(w) => {
                    let (rate, n) = w.lsb_stats();
                    let fs: Vec<Value> = wav::search_wav(&w, 4096)
                        .iter()
                        .map(|f| json!([f.order, f.bits, f.msb_first, f.length, f.why, txt(&f.data)]))
                        .collect();
                    let samples = format!("[{}]", w.samples.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" "));
                    json!([
                        r[0],
                        "",
                        w.channels,
                        w.rate,
                        w.bits,
                        samples,
                        format!("{:016x}", rate.to_bits()),
                        n,
                        txt(&w.lsb_bytes(true, 5)),
                        txt(&w.lsb_bytes(false, 1000)),
                        if fs.is_empty() { Value::Null } else { json!(fs) }
                    ])
                }
            };
            d.eq(format!("wav {:.30?}", String::from_utf8_lossy(&f)), &got, r);
        }
    }
    d.done("wav");
}
