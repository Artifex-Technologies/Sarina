//! Go: `sessionsearch`: lexical search over saved sessions. The query is matched as one phrase of whole words (without
//! regard to case) and ranked with a small BM25; when no message holds the phrase, the query's words are counted instead.
//! System messages and `<system-reminder>` envelopes are not searched. The ranking uses Go's amd64 `math.Log` (`gomath`)
//! and the words Go's letter and digit tables (`gotext`), so the scores are Go's to the bit.

use crate::gomath;
use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;
use std::time::UNIX_EPOCH;

const SNIPPET_RADIUS: usize = 160;
use crate::session::message::REMINDER_OPEN;

/// One matching message (Go's `Hit`).
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub session_id: String,
    pub role: String,
    pub turn_index: usize,
    pub snippet: String,
    /// lower is better (the BM25 value, or the negated match count)
    pub score: f64,
}

struct Row {
    session: String,
    turn: usize,
    role: String,
    content: String,
}

static MESSAGE: Ty = Ty::Struct(
    r#"struct { Role string "json:\"role\""; Content interface {} "json:\"content\"" }"#,
    "",
    &[Field("role", &Ty::Str), Field("content", &Ty::Any)],
);
static FILE: Ty = Ty::Struct(
    "struct { Messages []struct { Role string \"json:\\\"role\\\"\"; Content interface {} \"json:\\\"content\\\"\" } \"json:\\\"messages\\\"\" }",
    "",
    &[Field("messages", &Ty::Slice(&MESSAGE))],
);

/// Go's `Search`: up to `top_k` hits (10 when not positive) for `query` over every `*.json` in `dir`.
pub fn search(query: &str, dir: &str, top_k: i64) -> Vec<Hit> {
    let query = gotext::trim_space(query);
    if query.is_empty() {
        return Vec::new();
    }
    let top_k = if top_k <= 0 { 10 } else { top_k as usize };
    let rows = load(dir);
    if rows.is_empty() {
        return Vec::new();
    }
    let hits = phrase(query, &rows, top_k);
    if !hits.is_empty() {
        return hits;
    }
    terms(query, &rows, top_k)
}

fn load(dir: &str) -> Vec<Row> {
    let Ok(rd) = std::fs::read_dir(gopath::os_path(dir.as_bytes())) else { return Vec::new() };
    let mut names: Vec<(Vec<u8>, bool)> =
        rd.flatten().map(|e| (e.file_name().as_encoded_bytes().to_vec(), e.file_type().is_ok_and(|t| t.is_dir()))).collect();
    names.sort();
    let mut files: Vec<(Vec<u8>, i128)> = Vec::new();
    for (name, is_dir) in names {
        if is_dir || !name.ends_with(b".json") {
            continue;
        }
        // Go's DirEntry.Info: the entry itself (a link is not followed)
        let Ok(md) = std::fs::symlink_metadata(gopath::os_path(&gopath::join(&[dir.as_bytes(), &name]))) else { continue };
        let mod_ns = md.modified().ok().map_or(0, |t| match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i128,
            Err(e) => -(e.duration().as_nanos() as i128),
        });
        files.push((name, mod_ns));
    }
    // newest first; Go's stable sort keeps the name order for equal times
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    let mut rows = Vec::new();
    for (name, _) in files {
        let Ok(raw) = crate::session::readfile::read_file(&gopath::join(&[dir.as_bytes(), &name])) else { continue };
        let (v, err) = gostruct::unmarshal(&FILE, &raw);
        if err.is_some() {
            continue;
        }
        let id = gotext::lossy(name.strip_suffix(b".json").unwrap_or(&name));
        let Some(Val::Slice(Some(msgs))) = gostruct::field(&FILE, &v, "messages") else { continue };
        for (i, m) in msgs.iter().enumerate() {
            let role = gostruct::str_field(&MESSAGE, m, "role");
            let content = match gostruct::field(&MESSAGE, m, "content") {
                Some(Val::Any(crate::goany::Any::String(s))) => s.as_str(),
                _ => "",
            };
            if role.is_empty() || role == "system" || content.starts_with(REMINDER_OPEN) || gotext::trim_space(content).is_empty() {
                continue;
            }
            rows.push(Row { session: id.clone(), turn: i, role: role.to_string(), content: content.to_string() });
        }
    }
    rows
}

/// Go's `words`: the lowered text split at every character that is neither a letter nor a digit.
fn words(s: &str) -> Vec<String> {
    gotext::to_lower(s).split(|c: char| !gotext::is_letter(c) && !gotext::is_digit(c)).filter(|w| !w.is_empty()).map(String::from).collect()
}

fn count_phrase(doc: &[String], q: &[String]) -> usize {
    if q.len() > doc.len() {
        return 0;
    }
    (0..=doc.len() - q.len()).filter(|&i| doc[i..i + q.len()] == *q).count()
}

fn phrase(query: &str, rows: &[Row], top_k: usize) -> Vec<Hit> {
    let q = words(query);
    if q.is_empty() {
        return Vec::new();
    }
    let mut counts = vec![0usize; rows.len()];
    let mut lens = vec![0usize; rows.len()];
    let (mut total, mut matching) = (0usize, 0usize);
    for (i, r) in rows.iter().enumerate() {
        let d = words(&r.content);
        lens[i] = d.len();
        total += d.len();
        counts[i] = count_phrase(&d, &q);
        if counts[i] > 0 {
            matching += 1;
        }
    }
    if matching == 0 {
        return Vec::new();
    }
    let mut avg = total as f64 / rows.len() as f64;
    if avg == 0.0 {
        avg = 1.0;
    }
    let idf = gomath::log((rows.len() as f64 - matching as f64 + 0.5) / (matching as f64 + 0.5) + 1.0);
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    let mut hits = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if counts[i] == 0 {
            continue;
        }
        let tf = counts[i] as f64;
        let score = -idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * lens[i] as f64 / avg));
        hits.push(Hit {
            session_id: r.session.clone(),
            role: r.role.clone(),
            turn_index: r.turn,
            snippet: snippet(&r.content, query),
            score,
        });
    }
    finish(hits, top_k)
}

fn finish(mut hits: Vec<Hit>, top_k: usize) -> Vec<Hit> {
    // Go's sort.SliceStable with "<": equal scores keep their order
    hits.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(top_k);
    hits
}

fn terms(query: &str, rows: &[Row], top_k: usize) -> Vec<Hit> {
    let lowered = gotext::to_lower(query);
    let ts = gotext::fields(&lowered);
    let mut hits = Vec::new();
    for r in rows {
        let low = gotext::to_lower(&r.content);
        let n: usize = ts.iter().map(|t| low.matches(t).count()).sum();
        if n > 0 {
            hits.push(Hit {
                session_id: r.session.clone(),
                role: r.role.clone(),
                turn_index: r.turn,
                snippet: snippet(&r.content, query),
                score: -(n as f64),
            });
        }
    }
    finish(hits, top_k)
}

fn index_runes(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// Go's `snippet`: 160 characters either side of the first query word found (words tried in order); with none, the first
/// 320 characters.
pub fn snippet(content: &str, query: &str) -> String {
    let c: Vec<char> = content.chars().collect();
    let low: Vec<char> = gotext::to_lower(content).chars().collect();
    let lowered_query = gotext::to_lower(query);
    let pos = gotext::fields(&lowered_query).iter().find_map(|t| {
        let needle: Vec<char> = gotext::to_lower(t).chars().collect();
        index_runes(&low, &needle)
    });
    let Some(pos) = pos else {
        let n = c.len().min(SNIPPET_RADIUS * 2);
        let head: String = c[..n].iter().collect();
        return if c.len() <= n { head } else { head + "..." };
    };
    let start = pos.saturating_sub(SNIPPET_RADIUS);
    let end = c.len().min(pos + SNIPPET_RADIUS);
    let part: String = c[start..end].iter().collect();
    let mut out = gotext::trim_space(&part).to_string();
    if start > 0 {
        out = format!("...{out}");
    }
    if end < c.len() {
        out.push_str("...");
    }
    out
}
