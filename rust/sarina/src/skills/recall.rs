//! Go: `go/skills/recall.go`: finding passages in long skills, by meaning (the local embedding model and the compressed
//! vector index, made the first time a skill is searched and kept under $XDG_STATE_HOME/sarina/skill-index) or, without
//! the model, by words; prefetched context; and `ExpandWith`, a skill run with a task (testdata/parity/skills.json, with
//! a made-up embedder).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use super::discover::Catalog;
use super::library::{cut_parts, Section};
use super::parse::Skill;
use crate::embed::Embedder;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext::{self, trim_space};
use crate::vecindex::Index;

const PASSAGE_BYTES: usize = 1200;
const MIN_PASSAGE_SCORE: f64 = 0.45;
const PASSAGE_BUDGET: usize = 6 << 10;
const PASSAGE_BAND: f64 = 0.08;
const CONTEXT_BUDGET: usize = 8 << 10;

/// One piece of a long skill found.
#[derive(Clone, Debug, PartialEq)]
pub struct Passage {
    pub skill: String,
    /// 1 first
    pub section: usize,
    pub title: String,
    pub text: String,
    pub score: f64,
}

struct SkillIndex {
    ix: Index,
    refs: HashMap<u64, (usize, String)>,
}

#[derive(Default)]
struct LibState {
    indexes: HashMap<String, Arc<SkillIndex>>,
    no_model: bool,
    last_note: String,
    started: HashSet<String>,
}

/// Searches the sections of a catalog's long skills (Go's `Library`).
pub struct Library {
    cat: Arc<Catalog>,
    emb: Option<Arc<dyn Embedder + Send + Sync>>,
    dir: String,
    st: Mutex<LibState>,
}

/// Go's `DefaultIndexDir`: $XDG_STATE_HOME/sarina/skill-index (~/.local/state/... without it).
pub fn default_index_dir(getenv: &dyn Fn(&str) -> String, home: &str) -> String {
    let mut base = getenv("XDG_STATE_HOME");
    if base.is_empty() {
        base = format!("{home}/.local/state");
    }
    format!("{base}/sarina/skill-index")
}

/// Go's `passages`: a section in pieces of about a paragraph (short paragraphs joined, long ones cut).
pub fn passages(sec: &Section) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let t = trim_space(cur);
        if !t.is_empty() {
            out.push(t.to_owned());
        }
        cur.clear();
    };
    for para in sec.text.split("\n\n") {
        let para = trim_space(para);
        if para.is_empty() {
            continue;
        }
        if !cur.is_empty() && cur.len() + para.len() > PASSAGE_BYTES {
            flush(&mut cur, &mut out);
        }
        for p in cut_parts(para, PASSAGE_BYTES) {
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            cur.push_str(&p);
            if cur.len() >= PASSAGE_BYTES {
                flush(&mut cur, &mut out);
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// FNV-1a 64 (Go's hash/fnv).
fn fnv64a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &c in b {
        h ^= u64::from(c);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn passage_id(skill: &str, section: usize, i: usize) -> u64 {
    fnv64a(format!("{skill}#{section}#{i}").as_bytes())
}

/// Go's `sum`: identifies a skill's text, so its index is made again when the skill changes.
fn sum(s: &Skill, model: &str) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(format!("{model}\0{}\0", s.name).as_bytes());
    for sec in &s.sections {
        h.update(format!("{}\0{}\0", sec.title, sec.text).as_bytes());
    }
    let hex: String = h.finalize().iter().map(|x| format!("{x:02x}")).collect();
    hex[..24].to_owned()
}

/// Go's `unit`.
fn unit(v: &[f32]) -> Vec<f32> {
    let t: f64 = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    if t == 0.0 {
        return v.to_vec();
    }
    let inv = (1.0 / t.sqrt()) as f32;
    v.iter().map(|&x| x * inv).collect()
}

static META_MODEL: Ty = Ty::Str;
static META_COUNT: Ty = Ty::Int;
static META_FIELDS: [Field; 2] = [Field("model", &META_MODEL), Field("count", &META_COUNT)];
static META_TY: Ty = Ty::Struct("struct { Model string; Count int }", "", &META_FIELDS);

impl Library {
    /// Go's `NewLibrary`: c's long skills; state_dir is where the indexes are kept ("" = not kept).
    pub fn new(cat: Arc<Catalog>, emb: Option<Arc<dyn Embedder + Send + Sync>>, state_dir: &str) -> Library {
        Library { cat, emb, dir: state_dir.to_owned(), st: Mutex::new(LibState::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LibState> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The operator ran this skill: its sections may be read in this session.
    pub fn start(&self, name: &str) {
        self.lock().started.insert(name.to_lowercase());
    }

    /// Whether the model may read this skill's sections (it may invoke it, or the operator ran it in this session).
    pub fn readable(&self, s: &Skill) -> bool {
        s.model_invocable || self.lock().started.contains(&s.name.to_lowercase())
    }

    /// Go's `index`: the skill's vector index, loaded or made.
    fn index(&self, st: &mut LibState, s: &Skill, emb: &(dyn Embedder + Send + Sync)) -> Result<Arc<SkillIndex>, String> {
        if let Some(si) = st.indexes.get(&s.name) {
            return Ok(si.clone());
        }
        let mut refs = HashMap::new();
        let mut pieces: Vec<(u64, String)> = Vec::new();
        for (si, sec) in s.sections.iter().enumerate() {
            for (i, p) in passages(sec).into_iter().enumerate() {
                let id = passage_id(&s.name, si, i);
                pieces.push((id, format!("{}: {p}", sec.title)));
                refs.insert(id, (si, p));
            }
        }
        let name = emb.name();
        let key = sum(s, &name);
        let mut dir = String::new();
        if !self.dir.is_empty() {
            dir = format!("{}/{key}", self.dir);
            if let Ok(b) = std::fs::read(format!("{dir}/meta.json")) {
                let (v, err) = gostruct::unmarshal(&META_TY, &b);
                if let (None, Val::Struct(f)) = (err, v) {
                    if f[0] == Val::Str(name.clone()) && f[1] == Val::Int(pieces.len() as i64) {
                        if let Ok(ix) = Index::load(std::path::Path::new(&format!("{dir}/passages.vec"))) {
                            let si = Arc::new(SkillIndex { ix, refs });
                            st.indexes.insert(s.name.clone(), si.clone());
                            return Ok(si);
                        }
                    }
                }
            }
        }
        let mut ix: Option<Index> = None;
        for batch in pieces.chunks(32) {
            let texts: Vec<String> = batch.iter().map(|(_, t)| format!("search_document: {t}")).collect();
            let vecs = emb.embed(&texts)?;
            for (k, (id, _)) in batch.iter().enumerate() {
                let v = vecs.get(k).ok_or("the embedder gave fewer vectors than texts")?;
                if ix.is_none() {
                    ix = Some(Index::new(v.len(), 4, 1)?);
                }
                if let Some(ix) = ix.as_mut() {
                    ix.add(*id, &unit(v))?;
                }
            }
        }
        let Some(ix) = ix else { return Err(format!("skill {} has nothing to index", s.name)) };
        if !dir.is_empty() {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            if std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).is_ok()
                && ix.save(std::path::Path::new(&format!("{dir}/passages.vec"))).is_ok()
            {
                let meta = format!(
                    "{{\"count\":{},\"model\":{},\"skill\":{}}}",
                    pieces.len(),
                    {
                        let mut q = String::new();
                        crate::gojson::write_str(&mut q, &name, true);
                        q
                    },
                    {
                        let mut q = String::new();
                        crate::gojson::write_str(&mut q, &s.name, true);
                        q
                    }
                );
                use std::io::Write;
                if let Ok(mut f) =
                    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(format!("{dir}/meta.json"))
                {
                    let _ = f.write_all(meta.as_bytes());
                }
            }
        }
        let si = Arc::new(SkillIndex { ix, refs });
        st.indexes.insert(s.name.clone(), si.clone());
        Ok(si)
    }

    /// Go's `Search`: up to limit passages for query, in one long skill (by name) or in all ("" ); the note says how.
    pub fn search(&self, skill: &str, query: &str, limit: usize) -> Result<(Vec<Passage>, String), String> {
        let limit = if limit == 0 { 5 } else { limit };
        let mut list: Vec<&Skill> = Vec::new();
        if !skill.is_empty() {
            let Some(s) = self.cat.get(skill) else { return Err(format!("no skill named {}", gotext::quote(skill))) };
            if !s.is_library() {
                return Err(format!("skill {} is short: all of it is in its prompt, there is nothing to search", gotext::quote(&s.name)));
            }
            if !self.readable(s) {
                return Err(format!("skill {} can only be used when the operator runs it", gotext::quote(&s.name)));
            }
            list.push(s);
        } else {
            list = self.cat.skills.iter().filter(|s| s.is_library() && self.readable(s)).collect();
        }
        if list.is_empty() || trim_space(query).is_empty() {
            return Ok((Vec::new(), String::new()));
        }
        let mut st = self.lock();
        if let (Some(emb), false) = (&self.emb, st.no_model) {
            match self.by_meaning(&mut st, emb.as_ref(), &list, query, limit) {
                Ok(found) => return Ok((found, "by meaning".into())),
                Err(e) => {
                    st.no_model = true;
                    st.last_note = format!("by words (the embedding model is not available: {e})");
                }
            }
        }
        let note = if st.last_note.is_empty() { "by words".to_owned() } else { st.last_note.clone() };
        Ok((by_words(&list, query, limit), note))
    }

    fn by_meaning(
        &self,
        st: &mut LibState,
        emb: &(dyn Embedder + Send + Sync),
        list: &[&Skill],
        query: &str,
        limit: usize,
    ) -> Result<Vec<Passage>, String> {
        let qv = emb.embed(&[format!("search_query: {query}")])?;
        let q = unit(qv.first().ok_or("the embedder gave no vector")?);
        let mut out = Vec::new();
        for s in list {
            let si = self.index(st, s, emb)?;
            for h in si.ix.search(&q, limit, None)? {
                let Some((sec, text)) = si.refs.get(&h.id) else { continue };
                if f64::from(h.score) < MIN_PASSAGE_SCORE {
                    continue;
                }
                out.push(Passage {
                    skill: s.name.clone(),
                    section: sec + 1,
                    title: s.sections[*sec].title.clone(),
                    text: text.clone(),
                    score: f64::from(h.score),
                });
            }
        }
        out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        out.truncate(limit);
        Ok(out)
    }
}

/// Go's `byWords`: passages scored by the question's words (three letters or more), rarer words counting more.
pub fn by_words(list: &[&Skill], query: &str, limit: usize) -> Vec<Passage> {
    let lower = gotext::to_lower(query);
    let words: Vec<&str> = lower
        .split(|r: char| !(r == '_' || r == '-' || r.is_ascii_digit() || r.is_ascii_lowercase() || r as u32 > 127))
        .filter(|w| w.len() >= 3)
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut all: Vec<(Passage, String)> = Vec::new();
    let mut df: HashMap<&str, usize> = HashMap::new();
    for s in list {
        for (si, sec) in s.sections.iter().enumerate() {
            for p in passages(sec) {
                let low = gotext::to_lower(&format!("{} {p}", sec.title));
                for w in &words {
                    if low.contains(w) {
                        *df.entry(w).or_default() += 1;
                    }
                }
                all.push((Passage { skill: s.name.clone(), section: si + 1, title: sec.title.clone(), text: p, score: 0.0 }, low));
            }
        }
    }
    let n_all = all.len() as f64;
    for (p, low) in all.iter_mut() {
        for w in &words {
            let n = low.matches(w).count();
            if n > 0 {
                let d = df.get(w).copied().unwrap_or(0) as f64;
                p.score += (1.0 + crate::gomath::log(n as f64)) * crate::gomath::log(1.0 + n_all / (1.0 + d));
            }
        }
    }
    all.sort_by(|a, b| b.0.score.partial_cmp(&a.0.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Vec::new();
    for (p, _) in all {
        if p.score <= 0.0 || out.len() == limit {
            break;
        }
        out.push(p);
    }
    out
}

/// A prefetch: the task in, the context out (bytes, as Go's string: what a command printed may not be UTF-8).
pub type ContextFn = Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync>;

/// Go's registered contexts: the ones its packages add at start (sysinfo's "status" and "security"), and any added later.
static CONTEXTS: LazyLock<RwLock<HashMap<String, ContextFn>>> = LazyLock::new(|| {
    let mut m: HashMap<String, ContextFn> = HashMap::new();
    m.insert("status".into(), Arc::new(crate::sysinfo::status_context));
    m.insert("security".into(), Arc::new(crate::sysinfo::security_context));
    RwLock::new(m)
});

/// Go's `RegisterContext`: a kind of context a skill can ask for with "prefetch".
pub fn register_context(name: &str, f: ContextFn) {
    CONTEXTS.write().unwrap_or_else(|p| p.into_inner()).insert(name.to_owned(), f);
}

/// The first piece Go's `cutParts(text, max)` makes of bytes longer than `max`: cut at the last paragraph or line break in
/// the second half, else at `max` (stepped back to a character's start), trimmed.
fn first_part(b: &[u8], max: usize) -> &[u8] {
    let last = |pat: &[u8]| b[..max].windows(pat.len()).rposition(|w| w == pat).map_or(-1, |i| i as i64);
    let half = (max / 2) as i64;
    let mut cut = last(b"\n\n");
    if cut < half {
        cut = last(b"\n");
    }
    let cut = if cut < half {
        let mut c = max;
        while c > 0 && b[c] & 0xC0 == 0x80 {
            c -= 1;
        }
        c
    } else {
        cut as usize
    };
    gotext::trim_space_bytes(&b[..cut])
}

impl Skill {
    fn prefetched(&self, task: &str) -> String {
        let Some(names) = &self.prefetch else { return String::new() };
        if names.is_empty() || trim_space(task).is_empty() {
            return String::new();
        }
        let mut b: Vec<u8> = Vec::new();
        for name in names {
            let f = CONTEXTS.read().unwrap_or_else(|p| p.into_inner()).get(name).cloned();
            let Some(f) = f else { continue };
            let t = f(task);
            let t = gotext::trim_space_bytes(&t);
            if !t.is_empty() {
                b.extend_from_slice(t);
                b.extend_from_slice(b"\n\n");
            }
        }
        if b.len() > CONTEXT_BUDGET {
            let mut cut = first_part(&b, CONTEXT_BUDGET).to_vec();
            cut.extend_from_slice("\n[…]\n\n".as_bytes());
            b = cut;
        }
        // Go's string goes into the prompt as it is; a byte that is not UTF-8 is one U+FFFD when it is sent
        gotext::lossy(&b)
    }

    /// Go's `ExpandWith`: Expand for a run with a task: the context the skill asks for first, and for a long skill the
    /// passages that fit the task best after it (within the budget, in reading order).
    pub fn expand_with(&self, args: &str, lib: Option<&Library>) -> String {
        if let Some(lib) = lib {
            lib.start(&self.name);
        }
        let text = self.prefetched(args) + &self.expand(args);
        let Some(lib) = lib else { return text };
        if !self.is_library() || trim_space(args).is_empty() {
            return text;
        }
        let found = match lib.search(&self.name, args, 8) {
            Ok((f, _)) if !f.is_empty() => f,
            _ => return text,
        };
        let mut keep: Vec<Passage> = Vec::new();
        let mut used = 0;
        for p in &found {
            if p.score < found[0].score - PASSAGE_BAND || used + p.text.len() > PASSAGE_BUDGET {
                break;
            }
            used += p.text.len();
            keep.push(p.clone());
        }
        let mut order: HashMap<(usize, String), usize> = HashMap::new();
        for (si, sec) in self.sections.iter().enumerate() {
            for (i, t) in passages(sec).into_iter().enumerate() {
                order.insert((si + 1, t), i);
            }
        }
        let pos = |p: &Passage| order.get(&(p.section, p.text.clone())).copied().unwrap_or(0);
        keep.sort_by(|a, b| a.section.cmp(&b.section).then_with(|| pos(a).cmp(&pos(b))));
        let mut b = text;
        b.push_str("\n\nThe parts of this skill that fit the task (open a whole section with skill_section when you need more):\n");
        let mut last = 0;
        for p in &keep {
            if p.section != last {
                b.push_str(&format!("\n--- section {}: {} ---\n", p.section, p.title));
                last = p.section;
            }
            b.push_str(&p.text);
            b.push('\n');
        }
        b
    }
}
