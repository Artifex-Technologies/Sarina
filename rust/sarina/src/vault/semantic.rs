//! Go: `vault/semantic.go`: recall by meaning (SARINA_SEMANTIC=on): the notes cut into paragraphs and list items,
//! embedded by the local model and kept in a compressed vector index under $XDG_STATE_HOME/sarina/vault-index (made
//! again only for notes that changed), searched with the question, merged with the keyword hits by reciprocal rank.
//! Where Go's own order is random (stale notes removed from the index, hits of equal rank in a merge), the port takes
//! the notes' order.

use super::{search, Hit, Note, Vault};
use crate::embed::Embedder;
use crate::gostruct::{self, Field, Ty, Val};
use crate::vecindex::Index;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Go's `SemanticEnabled`: SARINA_SEMANTIC=on.
pub fn semantic_enabled() -> bool {
    std::env::var("SARINA_SEMANTIC").unwrap_or_default() == "on"
}

/// Go's `indexDir`.
fn index_dir(v: &Vault) -> String {
    let mut base = std::env::var("XDG_STATE_HOME").unwrap_or_default();
    if base.is_empty() {
        let home = std::env::var("HOME").unwrap_or_default();
        base = crate::gotext::lossy(&crate::gopath::join(&[home.as_bytes(), b".local", b"state"]));
    }
    let sum = sha_hex(v.root.as_bytes());
    crate::gotext::lossy(&crate::gopath::join(&[base.as_bytes(), b"sarina", b"vault-index", &sum.as_bytes()[..16]]))
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Go's `chunkID`: FNV-1a 64 of "path#i".
fn chunk_id(path: &str, i: usize) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in format!("{path}#{i}").as_bytes() {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Go's `chunks`: a note's paragraphs and list items (at most 60, each cut to 600 characters), each after its title.
pub fn chunks(n: &Note) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<String>| {
        let t = crate::gotext::trim_space(&para.join(" ")).to_string();
        if !t.is_empty() {
            out.push(t);
        }
        para.clear();
    };
    for l in n.body.split('\n') {
        let t = crate::gotext::trim_space(l);
        if t.is_empty() || t.starts_with('#') {
            flush(&mut para, &mut out);
        } else if t.starts_with("- ") || t.starts_with("* ") {
            flush(&mut para, &mut out);
            para.push(crate::gotext::trim_space(&t[2..]));
            flush(&mut para, &mut out);
        } else {
            para.push(t);
        }
    }
    flush(&mut para, &mut out);
    if out.is_empty() {
        out.push(n.title.clone());
    }
    let mut out: Vec<String> = out.into_iter().map(|c| format!("{}: {}", n.title, c.chars().take(600).collect::<String>())).collect();
    out.truncate(60);
    out
}

/// Go's `noteSum`.
fn note_sum(n: &Note) -> String {
    sha_hex(format!("{}\n{}", n.title, n.body).as_bytes())[..16].to_string()
}

/// Go's `normalize`: to unit length (float32 times the float64 inverse made float32).
fn normalize(v: &[f32]) -> Vec<f32> {
    let s: f64 = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    if s == 0.0 {
        return v.to_vec();
    }
    let inv = (1.0 / s.sqrt()) as f32;
    v.iter().map(|&x| x * inv).collect()
}

static IDS: Ty = Ty::Slice(&UINT);
static UINT: Ty = Ty::Uint;
static STR: Ty = Ty::Str;
static INT: Ty = Ty::Int;
static ENTRY_FIELDS: [Field; 2] = [Field("sum", &STR), Field("ids", &IDS)];
static ENTRY: Ty = Ty::Struct("vault.metaEntry", "metaEntry", &ENTRY_FIELDS);
static NOTES: Ty = Ty::Map(&ENTRY);
static CHUNKS: Ty = Ty::Map(&STR);
static META_FIELDS: [Field; 4] = [Field("model", &STR), Field("dim", &INT), Field("notes", &NOTES), Field("chunks", &CHUNKS)];
static META: Ty = Ty::Struct("vault.semMeta", "semMeta", &META_FIELDS);

/// Go's `semMeta`.
#[derive(Default)]
struct Meta {
    model: String,
    dim: i64,
    notes: BTreeMap<String, (String, Vec<u64>)>,
    chunks: Option<BTreeMap<String, String>>,
}

fn read_meta(b: &[u8]) -> Option<Meta> {
    let (v, err) = gostruct::unmarshal(&META, b);
    if err.is_some() {
        return None;
    }
    let Val::Struct(f) = v else { return None };
    let mut m = Meta::default();
    if let Val::Str(s) = &f[0] {
        m.model = s.clone();
    }
    if let Val::Int(d) = f[1] {
        m.dim = d;
    }
    if let Val::Map(Some(notes)) = &f[2] {
        for (p, e) in notes {
            let Val::Struct(ef) = e else { continue };
            let sum = if let Val::Str(s) = &ef[0] { s.clone() } else { String::new() };
            let ids = match &ef[1] {
                Val::Slice(Some(ids)) => ids.iter().map(|x| if let Val::Uint(u) = x { *u } else { 0 }).collect(),
                _ => Vec::new(),
            };
            m.notes.insert(p.clone(), (sum, ids));
        }
    }
    if let Val::Map(Some(chunks)) = &f[3] {
        m.chunks = Some(chunks.iter().map(|(k, v)| (k.clone(), if let Val::Str(s) = v { s.clone() } else { String::new() })).collect());
    }
    Some(m)
}

/// Go's `json.Marshal(meta)`: the keys sorted, a note without vectors "ids":null.
fn write_meta(m: &Meta) -> Vec<u8> {
    let q = |s: &str| {
        let mut o = String::new();
        crate::gojson::write_str(&mut o, s, true);
        o
    };
    let notes: Vec<String> = m
        .notes
        .iter()
        .map(|(p, (sum, ids))| {
            let ids =
                if ids.is_empty() { "null".into() } else { format!("[{}]", ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")) };
            format!("{}:{{\"sum\":{},\"ids\":{ids}}}", q(p), q(sum))
        })
        .collect();
    let chunks: Vec<String> = m.chunks.as_ref().map(|c| c.iter().map(|(k, v)| format!("{}:{}", q(k), q(v))).collect()).unwrap_or_default();
    format!("{{\"model\":{},\"dim\":{},\"notes\":{{{}}},\"chunks\":{{{}}}}}", q(&m.model), m.dim, notes.join(","), chunks.join(","))
        .into_bytes()
}

/// Where each vector comes from: its note and the chunk's place in it.
type Refs = HashMap<u64, (String, usize)>;

/// Go's `refresh`: the index brought up to date with the notes (only changed notes embedded again) and saved; and where
/// each vector comes from.
fn refresh(v: &Vault, notes: &[Note], emb: &dyn Embedder) -> Result<(Option<Index>, Refs), String> {
    let dir = index_dir(v);
    let name = emb.name();
    let mut meta = Meta::default();
    let mut ix: Option<Index> = None;
    if let Some(m) = std::fs::read(format!("{dir}/meta.json")).ok().and_then(|b| read_meta(&b)) {
        if m.model == name && m.chunks.is_some() {
            if let Ok(loaded) = Index::load(std::path::Path::new(&format!("{dir}/notes.vec"))) {
                if loaded.dim() as i64 == m.dim {
                    ix = Some(loaded);
                    meta = m;
                }
            }
        }
    }
    if ix.is_none() {
        meta = Meta { model: name, chunks: Some(BTreeMap::new()), ..Meta::default() };
    }
    let chunks_map = meta.chunks.get_or_insert_with(BTreeMap::new);
    let mut todo: Vec<(&Note, Vec<String>)> = Vec::new();
    let live: HashSet<&str> = notes.iter().map(|n| n.path.as_str()).collect();
    let mut changed = false;
    for n in notes {
        if let Some((sum, ids)) = meta.notes.get(&n.path) {
            if *sum == note_sum(n) && ix.is_some() {
                continue;
            }
            if let Some(ix) = ix.as_mut() {
                for id in ids {
                    ix.remove(*id);
                    chunks_map.remove(&id.to_string());
                }
            }
        }
        meta.notes.remove(&n.path);
        todo.push((n, chunks(n)));
        changed = true;
    }
    let stale: Vec<String> = meta.notes.keys().filter(|p| !live.contains(p.as_str())).cloned().collect();
    for p in stale {
        if let Some((_, ids)) = meta.notes.remove(&p) {
            for id in ids {
                if let Some(ix) = ix.as_mut() {
                    ix.remove(id);
                }
                chunks_map.remove(&id.to_string());
            }
        }
        changed = true;
    }
    let mut pieces: Vec<(&Note, usize, &str)> = Vec::new();
    for (n, parts) in &todo {
        for (i, c) in parts.iter().enumerate() {
            pieces.push((n, i, c));
        }
        meta.notes.insert(n.path.clone(), (note_sum(n), Vec::new()));
    }
    for batch in pieces.chunks(32) {
        let texts: Vec<String> = batch.iter().map(|(_, _, t)| format!("search_document: {t}")).collect();
        let vecs = emb.embed(&texts)?;
        for (k, (n, i, _)) in batch.iter().enumerate() {
            let vec = vecs.get(k).ok_or("the embedder gave fewer vectors than texts")?;
            if ix.is_none() {
                ix = Some(Index::new(vec.len(), 4, 1)?);
                meta.dim = vec.len() as i64;
            }
            let id = chunk_id(&n.path, *i);
            if let Some(ix) = ix.as_mut() {
                ix.add(id, &normalize(vec))?;
            }
            if let Some(e) = meta.notes.get_mut(&n.path) {
                e.1.push(id);
            }
            chunks_map.insert(id.to_string(), n.path.clone());
        }
    }
    let mut refs = HashMap::new();
    for (p, (_, ids)) in &meta.notes {
        for (i, id) in ids.iter().enumerate() {
            refs.insert(*id, (p.clone(), i));
        }
    }
    let Some(ix) = ix else { return Ok((None, refs)) };
    if changed && crate::testguard::allow(dir.as_bytes()).is_ok() {
        crate::goos::mkdir_all_mode(dir.as_bytes(), 0o700)?;
        ix.save(std::path::Path::new(&format!("{dir}/notes.vec"))).map_err(|e| crate::goerr::text(&e))?;
        let tmp = format!("{dir}/.meta.tmp");
        crate::goos::write_file(tmp.as_bytes(), &write_meta(&meta), 0o600)?;
        std::fs::rename(&tmp, format!("{dir}/meta.json"))
            .map_err(|e| format!("rename {tmp} {dir}/meta.json: {}", crate::goerr::text(&e)))?;
    }
    Ok((Some(ix), refs))
}

/// Go's `SemanticHits`: the notes whose best paragraph is close in meaning to the query (cosine 0.55 or more, within
/// 0.08 of the best), one hit a note.
pub fn semantic_hits(v: &Vault, notes: &[Note], query: &str, limit: usize, emb: &dyn Embedder) -> Result<Vec<Hit>, String> {
    if crate::gotext::trim_space(query).is_empty() || notes.is_empty() {
        return Ok(Vec::new());
    }
    let (ix, refs) = refresh(v, notes, emb)?;
    let Some(ix) = ix else { return Ok(Vec::new()) };
    let qv = emb.embed(&[format!("search_query: {query}")])?;
    let q = qv.first().ok_or("the embedder gave no vector")?;
    let found = ix.search(&normalize(q), limit * 6, None)?;
    let by_path: HashMap<&str, &Note> = notes.iter().map(|n| (n.path.as_str(), n)).collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut top = 0.0;
    for f in found {
        let Some((path, index)) = refs.get(&f.id) else { continue };
        let Some(n) = by_path.get(path.as_str()) else { continue };
        if seen.contains(path) {
            continue;
        }
        let score = f64::from(f.score);
        if top == 0.0 {
            top = score;
        }
        if score < 0.55 || score < top - 0.08 {
            break;
        }
        seen.insert(path.clone());
        let mut snip = String::new();
        if let Some(c) = chunks(n).get(*index) {
            let prefix = format!("{}: ", n.title);
            snip = c.strip_prefix(&prefix).unwrap_or(c).to_string();
            if snip.chars().count() > 200 {
                snip = snip.chars().take(200).collect::<String>() + "…";
            }
        }
        out.push(Hit { path: n.path.clone(), title: n.title.clone(), score, snippet: snip, why: vec!["meaning"] });
        if out.len() == limit {
            break;
        }
    }
    Ok(out)
}

/// Go's `Merge`: keyword and meaning hits by reciprocal rank (meaning counts double).
pub fn merge(keyword: &[Hit], meaning: &[Hit], limit: usize) -> Vec<Hit> {
    let mut all: Vec<(Hit, f64)> = Vec::new();
    for (list, weight) in [(keyword, 1.0), (meaning, 2.0)] {
        for (rank, h) in list.iter().enumerate() {
            let i = match all.iter().position(|(a, _)| a.path == h.path) {
                Some(i) => {
                    let a = &mut all[i].0;
                    for w in &h.why {
                        if !a.why.contains(w) {
                            a.why.push(w);
                        }
                    }
                    if a.snippet.is_empty() {
                        a.snippet = h.snippet.clone();
                    }
                    i
                }
                None => {
                    all.push((h.clone(), 0.0));
                    all.len() - 1
                }
            };
            all[i].1 += weight / (60.0 + (rank + 1) as f64);
        }
    }
    crate::gosort::slice_stable(&mut all, |a, b| a.1 > b.1);
    all.into_iter()
        .take(limit)
        .map(|(mut h, rrf)| {
            h.score = rrf;
            h
        })
        .collect()
}

/// Go's `Recall`: the keyword hits, merged with the hits by meaning when SARINA_SEMANTIC=on (and the model answers).
pub fn recall(v: &Vault, notes: &[Note], query: &str, limit: usize) -> Vec<Hit> {
    let kw = search(notes, query, limit);
    if !semantic_enabled() {
        return kw;
    }
    let getenv = |k: &str| std::env::var(k).unwrap_or_default();
    match semantic_hits(v, notes, query, limit, &crate::embed::default_embedder(&getenv)) {
        Ok(sem) if !sem.is_empty() => merge(&kw, &sem, limit),
        _ => kw,
    }
}
