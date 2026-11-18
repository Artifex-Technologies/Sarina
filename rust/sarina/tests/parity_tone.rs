//! testdata/parity/tone.json: sarina-tone's parts (go/tone/tone_parity_test.go): ReadWAV, Analyze on made-up speech,
//! the baseline (Load, Mood with Z, Update, Save's file), the mood history (ReadHistory, AppendHistory's file) and Pattern
//! in made-up time zones. The whole program is compared by rust/parity/tone-differential.py.

mod common;
use common::{bytes, cases, text, Check};
use sarina::tone::{self, Baseline, Entry, Features};
use serde_json::{json, Value};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|c| format!("{c:02x}")).collect()
}

fn bits(f: f64) -> String {
    format!("{:016x}", f.to_bits())
}

fn feature_bits(f: &Features) -> Value {
    json!([bits(f.loud_db), bits(f.f0_median), bits(f.f0_range_st), bits(f.rate), bits(f.pause_ratio), bits(f.voiced_sec), bits(f.bright)])
}

fn from_bits(v: &Value) -> Features {
    let x: Vec<f64> = v.as_array().unwrap().iter().map(|s| f64::from_bits(u64::from_str_radix(s.as_str().unwrap(), 16).unwrap())).collect();
    Features { loud_db: x[0], f0_median: x[1], f0_range_st: x[2], rate: x[3], pause_ratio: x[4], voiced_sec: x[5], bright: x[6] }
}

/// go/tone/tone_parity_test.go's `sig.wav`: the same integer arithmetic, so the same samples.
fn sig_wav(s: &Value) -> Vec<u8> {
    let g = |k: &str| s[k].as_i64().unwrap();
    let (rate, chans, n, lead, per0, glide, syl, gap, every, pause, amp, noise) =
        (g("Rate"), g("Chans"), g("N"), g("Lead"), g("Per"), g("Glide"), g("Syl"), g("Gap"), g("Every"), g("Pause"), g("Amp"), g("Noise"));
    let mut st = s["Seed"].as_u64().unwrap().wrapping_mul(2654435761).wrapping_add(1);
    let mut rnd = || {
        st ^= st << 13;
        st ^= st >> 7;
        st ^= st << 17;
        st
    };
    let mut env = vec![0i64; n as usize];
    let (mut pos, mut count) = (lead, 0i64);
    while syl > 0 && pos + syl <= n - lead {
        for p in 0..syl {
            env[(pos + p) as usize] = p.min(syl - p) * 2048 / syl;
        }
        pos += syl + gap;
        count += 1;
        if every > 0 && count % every == 0 {
            pos += pause;
        }
    }
    let mut data = Vec::new();
    let mut ph = 0i64;
    for j in 0..n {
        let per = per0 + glide * ((j / (rate / 4)) % 5 - 2);
        ph += 1;
        if ph >= per {
            ph = 0;
        }
        let mut v = amp * (ph * 2000 / per - 1000) * env[j as usize] / (1000 * 1024);
        if noise > 0 {
            v += (rnd() % (2 * noise as u64 + 1)) as i64 - noise;
        }
        let v = v.clamp(-32768, 32767);
        for c in 0..chans {
            data.extend_from_slice(&((v / (c + 1)) as i16).to_le_bytes());
        }
    }
    let mut b = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(chans as u16).to_le_bytes());
    b.extend_from_slice(&(rate as u32).to_le_bytes());
    b.extend_from_slice(&((rate as u32) * (chans as u32) * 2).to_le_bytes());
    b.extend_from_slice(&((chans as u16) * 2).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

/// The histories the Go test leaves out of the fixture for their size, made again.
fn history_data(name: &str) -> String {
    let long = |n: usize| format!("{{\"t\":5,\"mood\":\"{}\"}}", "a".repeat(n - r#"{"t":5,"mood":""}"#.len()));
    match name {
        "605 entries" => (0..605).map(|i| format!("{{\"t\":{},\"mood\":\"usual\"}}\n", 1000 + i)).collect(),
        "a line of 65535" => long(65535) + "\n{\"t\":6,\"mood\":\"low\"}\n",
        "a line of 65536" => "{\"t\":4,\"mood\":\"x\"}\n".to_string() + &long(65536) + "\n{\"t\":6,\"mood\":\"low\"}\n",
        "a last line of 65535" => "{\"t\":4,\"mood\":\"x\"}\n".to_string() + &long(65535),
        "a last line of 65536" => "{\"t\":4,\"mood\":\"x\"}\n".to_string() + &long(65536),
        "a CR line of 65536" => long(65535) + "\r\n{\"t\":6,\"mood\":\"low\"}\n",
        _ => panic!("no history named {name}"),
    }
}

fn short(b: &[u8]) -> String {
    use sha2::Digest;
    if b.len() > 4000 {
        return format!("sha256:{}", hexs(&sha2::Sha256::digest(b)));
    }
    hexs(b)
}

fn err(r: Result<(), Vec<u8>>) -> Value {
    match r {
        Ok(()) => json!(""),
        Err(e) => text(&e),
    }
}

#[test]
fn tone_matches_go() {
    let dir = std::env::temp_dir().join(format!("parity-tone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = |rel: &str| dir.join(rel).into_os_string().into_encoded_bytes();
    let mut d = Check::new();
    for c in cases("tone") {
        let (input, want) = (&c["in"], &c["out"]);
        match c["fn"].as_str().unwrap() {
            "wav" => {
                std::fs::write(dir.join("w.wav"), hex(input["data"].as_str().unwrap())).unwrap();
                let got = match tone::read_wav(&path("w.wav")) {
                    Ok((x, rate)) => json!({"err": "", "rate": rate,
                        "samples": if x.is_empty() { Value::Null } else { json!(x.iter().map(|v| bits(*v)).collect::<Vec<_>>()) }}),
                    Err(e) => json!({"err": text(&e), "rate": 0, "samples": null}),
                };
                d.eq(format!("wav {}", input["name"]), &got, want);
            }
            "analyze" => {
                std::fs::write(dir.join("s.wav"), sig_wav(input)).unwrap();
                let (x, rate) = tone::read_wav(&path("s.wav")).unwrap();
                let got = match tone::analyze(&x, rate) {
                    Some(f) => json!({"ok": true, "features": feature_bits(&f)}),
                    None => json!({"ok": false}),
                };
                d.eq(format!("analyze {input}"), &got, want);
            }
            "baseline" => {
                let name = input["name"].as_str().unwrap();
                let base = dir.join("base");
                let _ = std::fs::remove_dir_all(&base);
                let p = path(&format!("base/{name}.json"));
                let file = bytes(&input["file"]);
                if !file.is_empty() {
                    std::fs::create_dir_all(&base).unwrap();
                    std::fs::write(sarina::gopath::os_path(&p), &file).unwrap();
                }
                let mut b: Baseline = tone::load(&p);
                let mut steps = Vec::new();
                for f in input["features"].as_array().unwrap() {
                    let f = from_bits(f);
                    let mood = b.mood(&f);
                    let z = b.z(&f);
                    let zs: Vec<String> = ["loud", "f0", "range", "rate", "pause", "bright"].iter().map(|k| bits(z[k])).collect();
                    if input["update"].as_bool().unwrap() {
                        b.update(&f);
                    }
                    steps.push(json!({"mood": mood, "hint": tone::hint_for(mood), "z": zs, "n": b.n}));
                }
                let e = err(b.save(&p));
                let saved = std::fs::read(sarina::gopath::os_path(&p)).unwrap_or_default();
                let got = json!({"steps": if steps.is_empty() { Value::Null } else { json!(steps) }, "err": e, "saved": text(&saved)});
                d.eq(format!("baseline {name}"), &got, want);
            }
            "history" => {
                let name = input["name"].as_str().unwrap();
                let data = if input["data"].is_null() { history_data(name).into_bytes() } else { bytes(&input["data"]) };
                let hdir = dir.join("hist");
                let _ = std::fs::remove_dir_all(&hdir);
                let p = path("hist/m.jsonl");
                if name != "none" {
                    std::fs::create_dir_all(&hdir).unwrap();
                    std::fs::write(hdir.join("m.jsonl"), &data).unwrap();
                }
                let read: Vec<Value> = tone::read_history(&p).iter().map(|e| json!([e.t, text(e.mood.as_bytes())])).collect();
                let e = err(tone::append_history(&p, Entry { t: 1763400000, mood: tone::LOW.into() }));
                let after = std::fs::read(hdir.join("m.jsonl")).unwrap_or_default();
                let got = json!({"read": if read.is_empty() { Value::Null } else { json!(read) }, "err": e, "after": short(&after)});
                d.eq(format!("history {name}"), &got, want);
            }
            "pattern" => {
                let loc = sarina::tz::from_tzdata(&hex(input["zone"].as_str().unwrap())).unwrap();
                let h: Vec<Entry> = input["history"]
                    .as_array()
                    .map(|a| a.iter().map(|e| Entry { t: e[0].as_i64().unwrap(), mood: e[1].as_str().unwrap().into() }).collect())
                    .unwrap_or_default();
                let got = tone::pattern(&h, input["now"].as_i64().unwrap(), input["nanos"].as_u64().unwrap() as u32, &loc);
                d.eq(format!("pattern {} at {}", input["zone"], input["now"]), &json!(got), want);
            }
            other => panic!("unknown case {other}"),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    d.done("tone");
}
