//! testdata/parity/vecindex.json: Go's math/rand, math.Exp on this machine, the Lloyd-Max levels, a new index's rotation,
//! whole indexes (added, replaced, removed, searched, saved, reloaded) and damaged files.

mod common;

use sarina::gomath;
use sarina::gorand::Rand;
use sarina::vecindex::{lloyd_max, Hit, Index};
use serde_json::{json, Value};

fn fnv(b: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h ^= c as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}/{}", b.len())
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_bits().to_le_bytes()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn vecs(rng: &mut Rand, n: usize, dim: usize) -> Vec<Vec<f32>> {
    (0..n).map(|_| (0..dim).map(|_| (rng.int63() % 2001 - 1000) as f32 / 37.0).collect()).collect()
}

fn hits(r: Result<Vec<Hit>, String>) -> Value {
    match r {
        Err(e) => json!(e),
        Ok(hs) => json!(hs.iter().map(|h| format!("{}:{:08x}", h.id, h.score.to_bits())).collect::<Vec<_>>()),
    }
}

fn sprint(p: &[usize]) -> String {
    format!("[{}]", p.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" "))
}

fn err(r: Result<(), String>) -> Value {
    json!(r.err().unwrap_or_default())
}

#[test]
fn vecindex_matches_go() {
    let mut check = common::Check::new();
    let dir = std::env::temp_dir().join(format!("sarina-vecindex-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for c in common::cases("vecindex") {
        let input = &c["in"];
        let got = match c["fn"].as_str().unwrap() {
            "rand" => {
                let mut r = Rand::new(input.as_str().unwrap().parse().unwrap());
                let mut out: Vec<String> = (0..12).map(|_| r.int63().to_string()).collect();
                for _ in 0..12 {
                    out.push(r.intn(2).to_string());
                    out.push(r.intn(768).to_string());
                    out.push(r.int31n(1000).to_string());
                }
                for n in [1i64, 3, 1 << 40, (1 << 62) + 1, i64::MAX] {
                    out.push(r.int63n(n).to_string());
                }
                out.push(sprint(&r.perm(0)));
                out.push(sprint(&r.perm(1)));
                out.push(sprint(&r.perm(10)));
                out.push(r.int31().to_string());
                json!(out)
            }
            "exp" => {
                let out: Vec<String> = c["out"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        let x = f64::from_bits(u64::from_str_radix(p.as_str().unwrap().split(':').next().unwrap(), 16).unwrap());
                        format!("{:x}:{:x}", x.to_bits(), gomath::exp(x).to_bits())
                    })
                    .collect();
                json!(out)
            }
            "lloydMax" => {
                json!(lloyd_max(input.as_u64().unwrap() as usize).iter().map(|v| format!("{:x}", v.to_bits())).collect::<Vec<_>>())
            }
            "New" => {
                let a = input.as_array().unwrap();
                json!(Index::new(a[0].as_u64().unwrap() as usize, a[1].as_u64().unwrap() as usize, a[2].as_i64().unwrap())
                    .err()
                    .unwrap_or_default())
            }
            "index" => {
                let a = input.as_array().unwrap();
                let (dim, bits, seed, sc) =
                    (a[0].as_u64().unwrap() as usize, a[1].as_u64().unwrap() as usize, a[2].as_i64().unwrap(), a[3].as_i64().unwrap());
                let mut x = Index::new(dim, bits, seed).unwrap();
                let (blocks, per, cells, book, cuts, signs, perm) = x.rotation();
                // the Go test hashes the hex text of the signs and of the rotated vectors
                let signs: Vec<u8> = signs.iter().flat_map(|s| hex(&f32s(s)).into_bytes()).collect();
                let perms: Vec<u8> = perm.iter().flat_map(|p| p.iter().flat_map(|v| (*v as u32).to_le_bytes())).collect();
                let mut out = json!({"blocks": blocks, "per": per, "cells": cells, "book": hex(&f32s(book)), "cuts": hex(&f32s(cuts)),
                    "signs": fnv(&signs), "perm": fnv(&perms)});
                let mut drng = Rand::new(sc);
                let mut n = 1 + (drng.int63() % 30) as usize;
                if dim >= 768 {
                    n = 1 + (drng.int63() % 5) as usize;
                }
                let vs = vecs(&mut drng, n, dim);
                let mut rot = Vec::new();
                for (i, v) in vs.iter().enumerate() {
                    x.add((i * 7 + 1) as u64, v).unwrap();
                    if i < 2 {
                        rot.extend(hex(&f32s(&x.rotated(v))).into_bytes());
                    }
                }
                for id in 1000..1005u64 {
                    x.add(id, &vs[0]).unwrap();
                }
                out["rotated"] = json!(fnv(&rot));
                let mut ops = Vec::new();
                if n > 2 {
                    let v = vecs(&mut drng, 1, dim).remove(0);
                    ops.push(err(x.add(8, &v)));
                    ops.push(json!(x.remove(1)));
                    ops.push(json!(x.remove(1)));
                    ops.push(json!(x.remove((n * 7 + 50) as u64)));
                }
                ops.push(err(x.add(999, &vec![0.0; dim])));
                ops.push(err(x.add(5, &vec![0.0; dim + 1])));
                let mut bad = vec![0f32; dim];
                bad[dim - 1] = f32::NAN;
                ops.push(err(x.add(6, &bad)));
                bad[dim - 1] = f32::NEG_INFINITY;
                ops.push(err(x.add(6, &bad)));
                out["ops"] = json!(ops);
                out["len"] = json!([x.len(), x.bytes(), x.dim(), x.bits()]);
                let qs = vecs(&mut drng, 3, dim);
                let mut searches = Vec::new();
                for (qi, q) in qs.iter().enumerate() {
                    for k in [0, 1, 3, 100] {
                        searches.push(hits(x.search(q, k, None)));
                    }
                    let allowed = [8u64, 15, 999, qi as u64 * 7 + 22];
                    searches.push(hits(x.search(q, 2, Some(&|id| allowed.contains(&id)))));
                    searches.push(hits(x.search(q, 2, Some(&|_| false))));
                }
                searches.push(hits(x.search(&vec![0.0; dim + 2], 1, None)));
                out["search"] = json!(searches);
                let path = dir.join(format!("i{sc}.vec"));
                x.save(&path).unwrap();
                out["file"] = json!(fnv(&std::fs::read(&path).unwrap()));
                let back = Index::load(&path).unwrap();
                out["reloaded"] = hits(back.search(&qs[0], 3, None));
                out
            }
            "damaged" => {
                let data = unhex(input.as_str().unwrap());
                let mut out = Vec::new();
                for d in c["out"].as_array().unwrap() {
                    let file = match &d[0] {
                        Value::Number(n) => data[..n.as_u64().unwrap() as usize].to_vec(),
                        Value::String(h) => unhex(h),
                        _ => unreachable!(),
                    };
                    out.push(json!([d[0], Index::from_bytes(&file).err().unwrap_or_default()]));
                }
                json!(out)
            }
            other => panic!("unknown fn {other}"),
        };
        // an index is compared part by part, so a difference names the part
        match (&got, &c["out"]) {
            (Value::Object(g), Value::Object(w)) => {
                for (k, want) in w {
                    check.eq(format!("{} {input} {k}", c["fn"]), g.get(k).unwrap_or(&Value::Null), want);
                }
                check.eq(format!("{} {input} keys", c["fn"]), &json!(g.len()), &json!(w.len()));
            }
            _ => check.eq(format!("{} {input}", c["fn"]), &got, &c["out"]),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    check.done("vecindex");
}
