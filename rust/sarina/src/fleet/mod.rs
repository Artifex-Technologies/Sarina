//! Go: `chymaera/fleet`: Sarina's side of the agent fleet (Chymaera_OS contract item 7): `fleet-usage` writes Sarina's
//! record for Omarchy's agents panel, `fleet-status` reads the other agents' records (untrusted data, sanitised;
//! `--json` is Yggdrasil's contract) and `fleet-launch` proposes launching another agent and runs it only after a human
//! confirms. Proved by the CLI differential's fleet cases (rust/parity).
//!
//! HARD RULE, as in Go: nothing here may ever become a model-callable tool. These are commands in `cli::ported` and
//! nothing else; `tests/fleet_invariant.rs` checks that no other part of the crate names this module or the launcher.

pub mod launch;
pub mod status;
pub mod usage;

use crate::goany::Any;
use crate::gojson;

/// Go's `compactJSON` over a value already written by Go's encoder without HTML escaping: every character past ASCII
/// as a `\uXXXX` escape (Python's `json.dumps(v, separators=(',', ':'))`).
pub(crate) fn ascii(json: &str) -> String {
    gojson::ascii_only(json)
}

/// A JSON string as Go's encoder writes it without HTML escaping.
pub(crate) fn jstr(s: &str) -> String {
    let mut out = String::new();
    gojson::write_str(&mut out, s, false);
    out
}

/// A key written by `json.Marshal` (HTML escaping on), as the record's ordered maps write theirs.
pub(crate) fn jkey(s: &str) -> String {
    let mut out = String::new();
    gojson::write_str(&mut out, s, true);
    out
}

/// Go's `fmt.Sprint(v)` of a value decoded from JSON: `<nil>`, `true`, a float64 as `%v`, a string as it is, `[a b]`,
/// `map[k:v]` with the keys sorted.
pub(crate) fn sprint(v: &Any) -> String {
    match v {
        Any::Null => "<nil>".into(),
        Any::Bool(b) => b.to_string(),
        Any::Number(f) => crate::gotext::float_v(*f),
        Any::Int(n) => n.to_string(),
        Any::Num(s) | Any::String(s) => s.clone(),
        Any::Array(a) => format!("[{}]", a.iter().map(sprint).collect::<Vec<_>>().join(" ")),
        Any::Object(m) => format!("map[{}]", m.iter().map(|(k, v)| format!("{k}:{}", sprint(v))).collect::<Vec<_>>().join(" ")),
    }
}

/// Go's `os.UserHomeDir` as `homeOf` uses it: the command's home, else $HOME.
pub(crate) fn home_of(env: &crate::cli::Env) -> String {
    env.home_dir()
}

/// Go's `filepath.Join`.
pub(crate) fn join(parts: &[&[u8]]) -> Vec<u8> {
    crate::gopath::join(parts)
}

/// An `os.Stat`'s answer as fleet reads it: whether it is a folder, its size, its time (seconds and nanoseconds).
pub(crate) struct Stat {
    pub is_dir: bool,
    pub size: u64,
    pub mtime: (i64, u32),
}

pub(crate) fn stat(path: &[u8]) -> Option<Stat> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(crate::gopath::os_path(path)).ok()?;
    Some(Stat { is_dir: m.is_dir(), size: m.len(), mtime: (m.mtime(), m.mtime_nsec() as u32) })
}

/// The names in a folder (Go's `os.ReadDir`, sorted by their bytes); None when it cannot be read.
pub(crate) fn names(dir: &[u8]) -> Option<Vec<Vec<u8>>> {
    let entries = crate::gopath::read_dir(dir).ok()?;
    Some(entries.into_iter().map(|e| e.name).collect())
}

/// Go's `parseFlags` on a set already set up: the arguments left, or the exit status (0 for -h, 2 for a mistake).
pub(crate) fn parse_flags(fs: &mut crate::goflag::FlagSet, args: &[String], env: &mut crate::cli::Env) -> Result<Vec<String>, i32> {
    let r = fs.parse(args, &mut env.stderr);
    env.flush();
    match r {
        Ok(rest) => Ok(rest),
        Err(e) if e == crate::goflag::HELP => Err(0),
        Err(_) => Err(2),
    }
}
