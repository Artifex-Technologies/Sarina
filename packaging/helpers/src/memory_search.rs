//! `sarina memory-search` (Python's `memory_search.py` and its `__main__` part): semantic search over saved agent sessions.
//! Reads Sarina's session files (`<dir>/<id>.json`, `messages` an array of objects with string `role`/`content`) and
//! re-embeds an excerpt of each on every call: session folders are single-operator scale, so no index file is kept.
//!
//! Hostile folders, where the Python stopped or hung: a session file that is not a regular file (a named pipe, a device) is
//! skipped without being opened for reading (the Python blocked on a pipe for ever), as is one that cannot be stat'ed (a
//! dangling link stopped the Python with a traceback) or one over `MAX_SESSION_BYTES`.

use crate::argparse::{Opt, Outcome, Parser, Ty};
use crate::embed::{pathlib_str, rank, EmbeddingModel};
use crate::pyjson::{self, PStr, V};
use sarina_voice::py;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

pub const EXCERPT_CHAR_LIMIT: usize = 800;
/// A session file larger than this is skipped (the Python read any size).
pub const MAX_SESSION_BYTES: u64 = 256 << 20;

pub const PARSER: Parser = Parser {
    prog: "sarina memory-search",
    opts: &[Opt::store("--model-dir").required(), Opt::store("--session-dir"), Opt::store("--cwd"), Opt::store("--top-k").ty(Ty::Int)],
    positional: Some("query"),
};

#[derive(Clone, Debug, PartialEq)]
pub struct Excerpt {
    pub session_id: String,
    pub task: String,
    pub outcome: String,
}

impl Excerpt {
    pub fn text(&self) -> String {
        py::strip(&format!("{}\n{}", self.task, self.outcome)).to_string()
    }
}

/// `<system-reminder>.*?</system-reminder>` (DOTALL) removed.
pub fn strip_reminders(s: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(OPEN) {
        let Some(j) = rest[i + OPEN.len()..].find(CLOSE) else { break };
        out.push_str(&rest[..i]);
        rest = &rest[i + OPEN.len() + j + CLOSE.len()..];
    }
    out.push_str(rest);
    out
}

fn take(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The first user message left non-empty once reminders are gone (the task), the last non-empty assistant one (the outcome).
pub fn excerpt_from_messages(session_id: &str, messages: &[V]) -> Excerpt {
    let (mut task, mut outcome) = (String::new(), String::new());
    for m in messages {
        if !matches!(m, V::Obj(_)) {
            continue;
        }
        let role = m.get("role");
        let text = match m.get("content") {
            Some(V::Str(s)) => s.s.as_str(),
            _ => "",
        };
        let is = |r: &str| matches!(role, Some(V::Str(s)) if s.s == r);
        if is("user") && task.is_empty() {
            task = py::strip(&strip_reminders(text)).to_string();
        } else if is("assistant") && !text.is_empty() {
            outcome = text.to_string();
        }
    }
    Excerpt { session_id: session_id.to_string(), task: take(&task, EXCERPT_CHAR_LIMIT), outcome: take(&outcome, EXCERPT_CHAR_LIMIT) }
}

/// `Path(name).stem` (Python 3.14: leading dots are part of the stem, a trailing dot is a suffix)
fn stem(name: &str) -> &str {
    let lead = name.len() - name.trim_start_matches('.').len();
    match name[lead..].rfind('.') {
        Some(j) => &name[..lead + j],
        None => name,
    }
}

/// A regular file's bytes, opened without blocking (a planted pipe is never waited on).
fn read_regular(path: &std::path::Path) -> Option<Vec<u8>> {
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).ok()?;
    let md = f.metadata().ok()?;
    if !md.is_file() || md.len() > MAX_SESSION_BYTES {
        return None;
    }
    let mut b = Vec::new();
    f.take(MAX_SESSION_BYTES + 1).read_to_end(&mut b).ok()?;
    (b.len() as u64 <= MAX_SESSION_BYTES).then_some(b)
}

pub fn load_excerpt(path: &std::path::Path, name: &str) -> Option<Excerpt> {
    let bytes = read_regular(path)?;
    let text = std::str::from_utf8(&bytes).ok()?;
    let data = pyjson::loads(text).ok()?;
    let V::Obj(_) = data else { return None };
    let Some(V::Arr(messages)) = data.get("messages") else { return None };
    let sid = match data.get("session_id") {
        Some(V::Str(s)) if !s.s.is_empty() => s.s.clone(),
        _ => stem(name).to_string(),
    };
    Some(excerpt_from_messages(&sid, messages))
}

/// Every session in `dir` (`*.json`, dot files too, as pathlib's glob), newest first; unreadable ones skipped.
pub fn index_sessions(dir: &str) -> Vec<Excerpt> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    if !std::path::Path::new(dir).is_dir() {
        return Vec::new();
    }
    let mut files: Vec<(f64, std::path::PathBuf, String)> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".json") {
            continue;
        }
        let path = e.path();
        let Ok(md) = std::fs::metadata(&path) else { continue };
        use std::os::unix::fs::MetadataExt;
        // os.stat_result.st_mtime: seconds + nanoseconds * 1e-9, as a double
        let mtime = md.mtime() as f64 + md.mtime_nsec() as f64 * 1e-9;
        files.push((mtime, path, name));
    }
    files.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    files.iter().filter_map(|(_, p, n)| load_excerpt(p, n)).collect()
}

/// The search: (excerpt, score), best first. Duplicate session ids show the last excerpt of that id, as the Python's dict did.
pub fn search_sessions(query: &str, model: &mut EmbeddingModel, dir: &str, top_k: i64) -> Result<Vec<(Excerpt, f64)>, String> {
    let excerpts = index_sessions(dir);
    if excerpts.is_empty() {
        return Ok(Vec::new());
    }
    let qv = model.embed_one(&PStr::new(query))?;
    let texts: Vec<PStr> = excerpts.iter().map(|e| PStr::new(if e.text().is_empty() { e.session_id.clone() } else { e.text() })).collect();
    let vectors = model.embed(&texts)?;
    let by_id = |sid: &str| excerpts.iter().rev().find(|e| e.session_id == sid).cloned();
    let cands: Vec<(String, Vec<f64>)> = excerpts.iter().map(|e| e.session_id.clone()).zip(vectors).collect();
    Ok(rank(&qv, &cands, Some(top_k)).into_iter().filter_map(|(sid, s)| by_id(&sid).map(|e| (e, s))).collect())
}

/// f'{score:.3f}'
pub fn fmt3(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    format!("{x:.3}")
}

pub fn format_results(results: &[(Excerpt, f64)]) -> String {
    let mut lines = Vec::new();
    for (e, score) in results {
        lines.push(format!("{}  {}", fmt3(*score), e.session_id));
        lines.push(format!("  task:    {}", e.task));
        lines.push(format!("  outcome: {}", e.outcome));
    }
    lines.join("\n")
}

/// The command: its exit status.
pub fn main(args: &[String], out: &mut impl Write, err: &mut impl Write) -> i32 {
    let p = match PARSER.parse(args) {
        Outcome::Ok(p) => p,
        Outcome::Help(h) => {
            let _ = out.write_all(h.as_bytes());
            return 0;
        }
        Outcome::Error(e) => {
            let _ = err.write_all(e.as_bytes());
            return 2;
        }
    };
    let dir = match p.get("--session-dir") {
        Some(d) if !d.is_empty() => pathlib_str(d),
        _ => crate::embed::path_join(&crate::embed::path_join(p.get("--cwd").unwrap_or("."), ".port_sessions"), "agent"),
    };
    let top_k = p.get("--top-k").and_then(crate::argparse::py_int).unwrap_or(10);
    let mut model = match EmbeddingModel::load(p.get("--model-dir").unwrap_or_default()) {
        Ok(m) => m,
        Err(e) => {
            // the Python printed its MLUnavailable to stdout; any other failure was a traceback on stderr
            if e.starts_with("no model.onnx") || e.starts_with("no tokenizer.json") || e.starts_with("missing onnxruntime") {
                let _ = writeln!(out, "{e}");
            } else {
                let _ = writeln!(err, "{e}");
            }
            return 1;
        }
    };
    if index_sessions(&dir).is_empty() {
        let _ = writeln!(out, "No saved sessions found.");
        return 0;
    }
    match search_sessions(&p.words.join(" "), &mut model, &dir, top_k) {
        Ok(r) => {
            let _ = writeln!(out, "{}", format_results(&r));
            0
        }
        Err(e) => {
            let _ = writeln!(err, "{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts() {
        let m = pyjson::loads(
            r#"[1,{"role":"user","content":"<system-reminder>x</system-reminder>  "},{"role":"user","content":"a<system-reminder>\ny</system-reminder>b <system-reminder>open"},{"role":"assistant","content":"done"},{"role":"assistant","content":""},{"role":"assistant","content":5}]"#,
        )
        .unwrap();
        let V::Arr(a) = m else { panic!() };
        let e = excerpt_from_messages("s", &a);
        assert_eq!(e.task, "ab <system-reminder>open");
        assert_eq!(e.outcome, "done");
        assert_eq!(stem("a.b.json"), "a.b");
        assert_eq!(stem(".json"), ".json");
        assert_eq!(stem("..json"), "..json");
        assert_eq!(stem(".a.json"), ".a");
    }

    #[test]
    fn three_places_as_python() {
        // f'{x:.3f}' in Python 3.14
        assert_eq!(fmt3(0.0005), "0.001");
        assert_eq!(fmt3(0.1235), "0.123");
        assert_eq!(fmt3(0.0625), "0.062");
        assert_eq!(fmt3(-0.0001), "-0.000");
        assert_eq!(fmt3(1.0), "1.000");
    }
}
