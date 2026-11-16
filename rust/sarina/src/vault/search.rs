//! Go: `vault/search.go`: recall over the notes without a model, by reciprocal rank of three lists (BM25 keywords with
//! the title and tags weighed up, the link graph around the best keyword hits, recency); backlinks; a likely meant note
//! for a broken link.

use super::{resolve, Note};
use crate::gomath;
use crate::gotext;
use std::collections::{HashMap, HashSet};

/// A note that has more links than this (or a fifth of the vault) is an index: its links say nothing about relatedness.
const HUB_LINKS: usize = 8;

/// Go's `Hit`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub score: f64,
    pub snippet: String,
    /// which lists found it: keywords, linked, recent
    pub why: Vec<&'static str>,
}

/// Go's `tokens`: lower-cased runs of letters and digits.
fn tokens(s: &str) -> Vec<String> {
    gotext::to_lower(s)
        .split(|c: char| !gotext::is_letter(c) && !gotext::is_digit(c))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

const STOP: [&str; 21] = [
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "is", "it", "for", "with", "that", "this", "was", "are", "be", "as", "at", "by",
];

/// Go's `terms`: tokens of two bytes or more that are not stop words.
fn terms(s: &str) -> Vec<String> {
    tokens(s).into_iter().filter(|t| t.len() > 1 && !STOP.contains(&t.as_str())).collect()
}

/// Go's `Search`.
pub fn search(notes: &[Note], query: &str, limit: usize) -> Vec<Hit> {
    let q = terms(query);
    if q.is_empty() || notes.is_empty() {
        return Vec::new();
    }
    let limit = if limit == 0 { 8 } else { limit };
    let mut docs: Vec<(HashMap<String, f64>, f64)> = Vec::with_capacity(notes.len());
    let mut df: HashMap<String, usize> = HashMap::new();
    let mut total = 0.0;
    for n in notes {
        let (mut tf, mut len) = (HashMap::new(), 0.0);
        for t in terms(&n.title) {
            *tf.entry(t).or_insert(0.0) += 3.0;
            len += 3.0;
        }
        for tg in &n.tags {
            for t in terms(tg) {
                *tf.entry(t).or_insert(0.0) += 2.0;
                len += 2.0;
            }
        }
        for t in terms(&n.body) {
            *tf.entry(t).or_insert(0.0) += 1.0;
            len += 1.0;
        }
        for t in tf.keys() {
            *df.entry(t.clone()).or_insert(0) += 1;
        }
        total += len;
        docs.push((tf, len));
    }
    let count = notes.len() as f64;
    let avg = total / count;
    let (k1, b) = (1.2, 0.75);
    let mut bm = vec![0.0f64; notes.len()];
    for (i, (tf, len)) in docs.iter().enumerate() {
        for t in &q {
            let f = tf.get(t).copied().unwrap_or(0.0);
            if f == 0.0 {
                continue;
            }
            let d = df[t] as f64;
            let idf = gomath::log(1.0 + (count - d + 0.5) / (d + 0.5));
            bm[i] += idf * f * 2.2 / (f + k1 * (0.25 + b * len / avg.max(1.0)));
        }
    }
    for (i, n) in notes.iter().enumerate() {
        if bm[i] == 0.0 {
            continue;
        }
        let title: HashSet<String> = terms(&n.title).into_iter().collect();
        let hit = q.iter().filter(|t| title.contains(*t)).count();
        bm[i] *= 1.0 + 2.0 * hit as f64 / q.len() as f64;
    }
    let kw = rank(&bm);
    let index: HashMap<&str, usize> = notes.iter().enumerate().map(|(i, n)| (n.path.as_str(), i)).collect();
    let top: HashSet<usize> = kw.iter().take(5).copied().collect();
    let mut graph = vec![0.0f64; notes.len()];
    let hub = HUB_LINKS.max(notes.len() / 5);
    for (i, n) in notes.iter().enumerate() {
        if n.links.len() > hub {
            continue;
        }
        for l in &n.links {
            let Some(&j) = index.get(resolve(notes, l).as_str()) else { continue };
            if notes[j].links.len() > hub {
                continue;
            }
            if top.contains(&j) && !top.contains(&i) && i != j {
                graph[i] += 1.0;
            }
            if top.contains(&i) && !top.contains(&j) && i != j {
                graph[j] += 1.0;
            }
        }
    }
    let gr = rank(&graph);
    let matched: Vec<usize> = (0..notes.len()).filter(|&i| bm[i] > 0.0 || graph[i] > 0.0).collect();
    let mut score: HashMap<usize, f64> = HashMap::new();
    let mut why: HashMap<usize, HashSet<&'static str>> = HashMap::new();
    let mut note = |i: usize, s: f64, w: &'static str| {
        *score.entry(i).or_insert(0.0) += s;
        why.entry(i).or_default().insert(w);
    };
    for (r, &i) in kw.iter().enumerate() {
        note(i, 1.0 / (60 + r) as f64, "keywords");
    }
    for (r, &i) in gr.iter().enumerate() {
        note(i, 0.5 / (60 + r) as f64, "linked");
    }
    for (r, &i) in matched.iter().enumerate() {
        note(i, 0.05 / (60 + r) as f64, "recent");
    }
    let mut order: Vec<usize> = score.keys().copied().collect();
    order.sort_by(|&a, &c| score[&c].total_cmp(&score[&a]).then_with(|| notes[a].path.cmp(&notes[c].path)));
    order.truncate(limit);
    order
        .into_iter()
        .map(|i| Hit {
            path: notes[i].path.clone(),
            title: notes[i].title.clone(),
            score: score[&i],
            snippet: snippet(&notes[i].body, &q),
            why: ["keywords", "linked", "recent"].into_iter().filter(|w| why[&i].contains(w)).collect(),
        })
        .collect()
}

/// Go's `rank`: the indexes with a score above 0, highest first (equal ones in order).
fn rank(scores: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scores.len()).filter(|&i| scores[i] > 0.0).collect();
    crate::gosort::slice_stable(&mut idx, |&a, &b| scores[a] > scores[b]);
    idx
}

/// Go's `snippet`: the first line holding a query term (else the first line), not a heading, cut to 200 characters.
fn snippet(body: &str, q: &[String]) -> String {
    let mut pick = "";
    for l in body.split('\n') {
        let l = gotext::trim_space(l);
        if l.is_empty() || l.starts_with("# ") {
            continue;
        }
        if pick.is_empty() {
            pick = l;
        }
        let low = gotext::to_lower(l);
        if q.iter().any(|t| low.contains(t.as_str())) {
            return cut(l, 200);
        }
    }
    cut(pick, 200)
}

/// Go's `cut`: the first n characters and "…".
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n).collect::<String>() + "…"
}

/// Go's `Backlinks`: the notes that link to path, by path.
pub fn backlinks(notes: &[Note], path: &str) -> Vec<String> {
    let mut out: Vec<String> =
        notes.iter().filter(|n| n.path != path && n.links.iter().any(|l| resolve(notes, l) == path)).map(|n| n.path.clone()).collect();
    out.sort();
    out
}

/// Go's `stems`: each term's first four bytes.
fn stems(s: &str) -> HashSet<Vec<u8>> {
    terms(s).into_iter().map(|t| t.as_bytes()[..t.len().min(4)].to_vec()).collect()
}

/// Go's `Suggest`: the note whose title or file name shares at least half of a broken link's word stems (without
/// ".md"), or "".
pub fn suggest(notes: &[Note], link: &str) -> String {
    let want = stems(link);
    if want.is_empty() {
        return String::new();
    }
    let (mut best, mut best_score) = ("", 0.0);
    for n in notes {
        let file = &n.path[n.path.rfind('/').map_or(0, |i| i + 1)..];
        let have = stems(&format!("{} {}", n.title, file.strip_suffix(".md").unwrap_or(file)));
        let shared = want.iter().filter(|w| have.contains(*w)).count();
        let score = shared as f64 / want.len() as f64;
        if shared >= 1 && score > best_score {
            (best, best_score) = (&n.path, score);
        }
    }
    if best_score < 0.5 {
        return String::new();
    }
    best.strip_suffix(".md").unwrap_or(best).to_string()
}
