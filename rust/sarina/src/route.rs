//! Go: `route`: a question sorted into what to look up before the model starts, by meaning, with the small embedding
//! model: each topic a few example questions, embedded once and kept in a cache file
//! (`$XDG_STATE_HOME/sarina/route-MODEL.json`, byte for byte Go's), a question going to the topics it is closest to.
//! Used by the skills' "status" prefetch (`sysinfo`). The cache is read as a bounded regular file and replaced by a
//! rename, as Go does since `route: the cache read as a bounded regular file`.

use crate::embed::Embedder;
use crate::gostruct::{self, Field, Ty, Val};
use std::collections::BTreeMap;

/// Go's `Topics`: system_status topics, plus "guides" (her notes about this desktop) and "chat" (nothing).
pub const TOPICS: &[(&str, &[&str])] = &[
    ("audio", &["there is no sound", "my microphone doesn't work", "the speakers are silent", "audio crackles"]),
    ("boot", &["why does it take so long to start", "booting is slow", "what slows down startup"]),
    (
        "chat",
        &["tell me a joke", "how are you today", "what is the capital of france", "thank you", "good morning", "write me a short poem"],
    ),
    (
        "crashes",
        &[
            "did anything crash",
            "an app keeps closing by itself",
            "a program crashed when I opened it",
            "why did my game quit suddenly",
            "something froze and closed",
        ],
    ),
    ("disk", &["is my disk full", "how much storage space do I have left", "I am running out of space", "which drive is almost full"]),
    (
        "gaming",
        &[
            "my game keeps stuttering",
            "steam won't start my game",
            "which proton version should I use",
            "is this computer ready for gaming",
            "how do I show my frame rate in a game",
            "where do I put mods for a steam game",
            "does your model slow down my games",
            "my controller is not working in the game",
            "the game crashes on launch with proton",
        ],
    ),
    (
        "guides",
        &[
            "how do I change the default app for pdf files",
            "where is the setting for the wallpaper",
            "how do I sign in to proton drive",
            "what is the shortcut to talk to you",
            "how do I lock the screen",
            "where do I change the power mode",
            "how do I install an app",
        ],
    ),
    (
        "network",
        &["is my internet working", "the wifi keeps disconnecting", "I can't reach any websites", "my connection is slow", "am I online"],
    ),
    (
        "performance",
        &[
            "why is my computer so slow",
            "the system feels laggy and sluggish",
            "my games stutter and drop frames",
            "something is using all the cpu",
            "the fans are loud and it is hot",
            "how much memory is free",
        ],
    ),
    (
        "services",
        &[
            "are there any errors on my system",
            "something is broken since the update",
            "is everything working properly",
            "any problems I should know about",
        ],
    ),
];

const FLOOR: f64 = 0.62;
const BAND: f64 = 0.05;
/// Go's `maxCacheBytes`.
const MAX_CACHE_BYTES: u64 = 64 << 20;

/// Go's `cacheFile`: never relative (no home: the temporary folder).
pub fn cache_file(model: &str) -> String {
    let mut base = std::env::var("XDG_STATE_HOME").unwrap_or_default();
    if base.is_empty() {
        let h = std::env::var("HOME").unwrap_or_default();
        if h.is_empty() {
            let tmp = std::env::var("TMPDIR").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| "/tmp".into());
            return join(&[&tmp, "sarina-route", &format!("route-{model}.json")]);
        }
        base = join(&[&h, ".local", "state"]);
    }
    join(&[&base, "sarina", &format!("route-{model}.json")])
}

fn join(parts: &[&str]) -> String {
    let b: Vec<&[u8]> = parts.iter().map(|p| p.as_bytes()).collect();
    crate::gotext::lossy(&crate::gopath::join(&b))
}

/// Go's `signature`: the first 16 hex digits of the sha256 of the topics and their examples.
pub fn signature() -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for (k, ex) in TOPICS {
        h.update(format!("{k}\0{}\x01", ex.join("\0")).as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string()
}

fn unit(v: &[f32]) -> Vec<f32> {
    let s: f64 = v.iter().map(|&x| x as f64 * x as f64).sum();
    if s == 0.0 {
        return v.to_vec();
    }
    let inv = (1.0 / s.sqrt()) as f32;
    v.iter().map(|&x| x * inv).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    let mut s = 0.0;
    for (i, &x) in a.iter().enumerate() {
        if let Some(&y) = b.get(i) {
            s += x as f64 * y as f64;
        }
    }
    s
}

static VECS: Ty = Ty::Map(&Ty::Slice(&Ty::Slice(&Ty::Float32)));
static CACHE: Ty = Ty::Struct("route.cache", "cache", &[Field("sig", &Ty::Str), Field("vecs", &VECS)]);

type Protos = BTreeMap<String, Vec<Vec<f32>>>;

fn read_cache(path: &str) -> Option<Protos> {
    let b = crate::session::readfile::read_regular(path.as_bytes(), MAX_CACHE_BYTES).ok()?;
    let (v, err) = gostruct::unmarshal(&CACHE, &b);
    if err.is_some() {
        return None;
    }
    let Val::Struct(f) = v else { return None };
    if !matches!(&f[0], Val::Str(s) if *s == signature()) {
        return None;
    }
    let mut out = Protos::new();
    if let Val::Map(Some(m)) = &f[1] {
        for (k, v) in m {
            let mut vs = Vec::new();
            if let Val::Slice(Some(items)) = v {
                for it in items {
                    vs.push(match it {
                        Val::Slice(Some(xs)) => xs.iter().map(|x| if let Val::Float32(y) = x { *y } else { 0.0 }).collect(),
                        _ => Vec::new(),
                    });
                }
            }
            out.insert(k.clone(), vs);
        }
    }
    Some(out)
}

/// Go's `json.Marshal(cache{...})`.
fn marshal(protos: &Protos) -> Option<String> {
    let mut o = String::from("{\"sig\":");
    crate::gojson::write_str(&mut o, &signature(), true);
    o.push_str(",\"vecs\":{");
    for (i, (k, vs)) in protos.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        crate::gojson::write_str(&mut o, k, true);
        o.push_str(":[");
        for (j, v) in vs.iter().enumerate() {
            if j > 0 {
                o.push(',');
            }
            o.push('[');
            for (n, &x) in v.iter().enumerate() {
                if n > 0 {
                    o.push(',');
                }
                crate::gojson::write_f32(&mut o, x).ok()?;
            }
            o.push(']');
        }
        o.push(']');
    }
    o.push_str("}}");
    Some(o)
}

/// Go's `writeCache`: a new file renamed over the old one.
fn write_cache(path: &str, b: &[u8]) {
    let dir = crate::gopath::dir(path.as_bytes());
    let Ok((mut f, tmp)) = crate::goos::create_temp(&dir, ".route-*") else { return };
    use std::io::Write;
    let ok = f.write_all(b).is_ok();
    drop(f);
    let tmp = crate::gopath::os_path(&tmp);
    if !ok || std::fs::rename(tmp, path).is_err() {
        let _ = std::fs::remove_file(tmp);
    }
}

/// Go's `prototypes`: the examples' vectors, from the cache or embedded now (and the cache written).
fn prototypes(emb: &dyn Embedder) -> Result<Protos, String> {
    let path = cache_file(&emb.name());
    if let Some(p) = read_cache(&path) {
        return Ok(p);
    }
    let mut c = Protos::new();
    for (topic, ex) in TOPICS {
        let texts: Vec<String> = ex.iter().map(|e| format!("search_query: {e}")).collect();
        let vecs = emb.embed(&texts)?;
        let e = c.entry(topic.to_string()).or_default();
        for v in vecs {
            e.push(unit(&v));
        }
    }
    if crate::goos::mkdir_all_mode(&crate::gopath::dir(path.as_bytes()), 0o700).is_ok() {
        if let Some(b) = marshal(&c) {
            write_cache(&path, b.as_bytes());
        }
    }
    Ok(c)
}

/// Go's `Scores`: every topic's best similarity to the question, best first.
pub fn scores(question: &str, emb: &dyn Embedder) -> Result<Vec<(String, f64)>, String> {
    let protos = prototypes(emb)?;
    let qv = emb.embed(&[format!("search_query: {question}")])?;
    let q = unit(qv.first().map(Vec::as_slice).unwrap_or_default());
    let mut out: Vec<(String, f64)> = protos
        .iter()
        .map(|(t, vs)| (t.clone(), vs.iter().map(|v| dot(&q, v)).fold(-1.0, |best: f64, d| if d > best { d } else { best })))
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

/// Go's `Route`: at most two topics to look up ("chat" and none both mean nothing).
pub fn route(question: &str, emb: &dyn Embedder) -> Result<Vec<String>, String> {
    Ok(pick(&scores(question, emb)?))
}

fn pick(scores: &[(String, f64)]) -> Vec<String> {
    if scores.is_empty() || scores[0].1 < FLOOR {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, (t, v)) in scores.iter().enumerate() {
        if i >= 2 || *v < FLOOR || *v < scores[0].1 - BAND {
            break;
        }
        if t == "chat" {
            if i == 0 {
                return Vec::new();
            }
            break;
        }
        out.push(t.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_as_go() {
        let s = |v: &[(&str, f64)]| v.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        assert_eq!(pick(&s(&[("performance", 0.80), ("crashes", 0.78), ("disk", 0.70)])), ["performance", "crashes"]);
        assert!(pick(&s(&[("chat", 0.85), ("performance", 0.6)])).is_empty());
        assert!(pick(&s(&[("network", 0.50)])).is_empty());
        assert_eq!(pick(&s(&[("guides", 0.75), ("chat", 0.74)])), ["guides"]);
    }

    #[test]
    fn signature_is_gos() {
        // the signature Go's route package writes into its cache (rust/parity/cli-differential.py ROUTE_SIG)
        assert_eq!(signature(), "40731a7421e6b47f");
    }
}
