//! Go: `chymaera/fleet/usage.go` and `fleet-usage`: Sarina's usage record for Omarchy's agents panel, in the panel's
//! own shape (its Codex collector's keys). A session file holds cumulative totals, so all of a session's tokens go to the
//! local day the file was last written. Sums wrap as Go's `int` does.

use super::{ascii, jkey, jstr, names, stat};
use crate::goany::Any;
use crate::gotime::Time;
use std::collections::{BTreeMap, BTreeSet};

pub const AGENT_ID: &str = "sarina";
const RECENT_DAY_COUNT: i64 = 7;
/// a session file larger than this is not one Sarina wrote
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;
pub const SERVICE_SESSION_DIR: &str = "/var/lib/sarina/sessions";
const SESSION_DIRS_ENV: &str = "SARINA_SESSION_DIRS";

/// What one saved session gives the record (Go's `SessionUsage`).
#[derive(Clone)]
struct SessionUsage {
    id: String,
    day: String,
    model: String,
    prompts: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
}

impl SessionUsage {
    fn total(&self) -> i64 {
        self.input.wrapping_add(self.output).wrapping_add(self.cache_read).wrapping_add(self.cache_write)
    }
}

/// Go's `nonNegInt`: a non-negative int from untrusted JSON; anything else is zero.
fn non_neg_int(v: Option<&Any>) -> i64 {
    match v {
        Some(Any::Number(n)) => {
            if n.is_nan() || n.is_infinite() {
                0
            } else if *n >= 9.2e18 {
                i64::MAX
            } else {
                (*n as i64).max(0)
            }
        }
        Some(Any::Int(n)) => (*n).max(0),
        _ => 0,
    }
}

/// Go's `UsageDirectory`: where Omarchy's panel looks for usage records.
pub fn usage_directory(getenv: &dyn Fn(&str) -> String, home: &str) -> Vec<u8> {
    let s = getenv("XDG_STATE_HOME");
    if !s.is_empty() {
        return super::join(&[s.as_bytes(), b"omarchy", b"agents", b"usage"]);
    }
    super::join(&[home.as_bytes(), b".local", b"state", b"omarchy", b"agents", b"usage"])
}

fn expand_user(p: &str, home: &str) -> Vec<u8> {
    if p == "~" {
        return home.as_bytes().to_vec();
    }
    if p.starts_with("~/") || p.starts_with("~\\") {
        return super::join(&[home.as_bytes(), &p.as_bytes()[2..]]);
    }
    p.as_bytes().to_vec()
}

/// Go's `DefaultSessionDirectories`: every place Sarina saves sessions; SARINA_SESSION_DIRS (a path list) replaces them.
pub fn default_session_directories(getenv: &dyn Fn(&str) -> String, cwd: &[u8], home: &str) -> Vec<Vec<u8>> {
    let o = getenv(SESSION_DIRS_ENV);
    if !o.is_empty() {
        return o.split(':').filter(|p| !p.is_empty()).map(|p| expand_user(p, home)).collect();
    }
    let mut out: Vec<Vec<u8>> = Vec::new();
    for c in [
        super::join(&[cwd, b".port_sessions", b"agent"]),
        super::join(&[home.as_bytes(), b".port_sessions", b"agent"]),
        SERVICE_SESSION_DIR.as_bytes().to_vec(),
    ] {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

fn truthy(v: Option<&Any>) -> bool {
    match v {
        None | Some(Any::Null) => false,
        Some(Any::String(s)) => !s.is_empty(),
        Some(Any::Bool(b)) => *b,
        Some(Any::Number(f)) => *f != 0.0,
        Some(Any::Array(a)) => !a.is_empty(),
        Some(Any::Object(m)) => !m.is_empty(),
        Some(_) => true,
    }
}

fn text(v: &Any) -> String {
    match v {
        Any::String(s) => s.clone(),
        other => super::sprint(other),
    }
}

/// The local date of a time (Go's `t.Local().Format("2006-01-02")`).
fn local_day(secs: i64, nanos: u32) -> String {
    let (y, m, d, ..) = Time::from_unix(secs, nanos).local().civil();
    format!("{y:04}-{m:02}-{d:02}")
}

fn read_session(path: &[u8], name: &[u8], st: &super::Stat) -> Option<SessionUsage> {
    if st.size > MAX_SESSION_BYTES {
        return None;
    }
    // a regular file only, bounded: a named pipe planted under a session's name would hold the reader for ever
    let raw = crate::session::readfile::read_regular(path, MAX_SESSION_BYTES).ok()?;
    let (data, err) = crate::gojson::unmarshal_any(&raw);
    if err.is_some() {
        return None;
    }
    let Any::Object(obj) = &data else { return None };
    let Some(Any::Object(usage)) = obj.get("usage") else { return None };
    let mut model = String::new();
    if let Some(Any::Object(mc)) = obj.get("model_config") {
        if truthy(mc.get("model")) {
            model = text(&mc["model"]);
        }
    }
    if model.is_empty() {
        model = "unknown".into();
    }
    // the file's name without its extension (Go's filepath.Ext: from the last dot)
    let base = crate::gopath::base(name);
    let stem = match base.iter().rposition(|&c| c == b'.') {
        Some(i) => &base[..i],
        None => &base[..],
    };
    let mut id = crate::gotext::lossy(stem);
    if truthy(obj.get("session_id")) {
        id = text(&obj["session_id"]);
    }
    Some(SessionUsage {
        id,
        day: local_day(st.mtime.0, st.mtime.1),
        model,
        prompts: non_neg_int(obj.get("turns")),
        input: non_neg_int(usage.get("input_tokens")),
        output: non_neg_int(usage.get("output_tokens")),
        cache_read: non_neg_int(usage.get("cache_read_input_tokens")),
        cache_write: non_neg_int(usage.get("cache_creation_input_tokens")),
    })
}

/// Go's `ReadSessions`: one per session id, the most recently written copy winning, in the order first seen.
fn read_sessions(dirs: &[Vec<u8>]) -> Vec<SessionUsage> {
    let mut newest: BTreeMap<String, ((i64, u32), SessionUsage)> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for dir in dirs {
        let Some(all) = names(dir) else { continue };
        for name in all.into_iter().filter(|n| n.ends_with(b".json")) {
            let path = super::join(&[dir, &name]);
            let Some(st) = stat(&path) else { continue };
            if st.is_dir {
                continue;
            }
            let Some(s) = read_session(&path, &name, &st) else { continue };
            match newest.get(&s.id) {
                None => {
                    order.push(s.id.clone());
                    newest.insert(s.id.clone(), (st.mtime, s));
                }
                Some((t, _)) if st.mtime > *t => {
                    newest.insert(s.id.clone(), (st.mtime, s));
                }
                Some(_) => {}
            }
        }
    }
    order.iter().map(|id| newest[id].1.clone()).collect()
}

/// Go's `isoNow`: the clock in the local zone, to the microsecond, trailing zeros of the fraction dropped.
fn iso_now() -> String {
    let t = Time::now_local();
    let (y, m, d, hh, mm, ss, _) = t.civil();
    let mut frac = format!("{:06}", t.nanos() / 1000);
    while frac.ends_with('0') {
        frac.pop();
    }
    let frac = if frac.is_empty() { String::new() } else { format!(".{frac}") };
    let off = t.offset();
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.unsigned_abs();
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{frac}{sign}{:02}:{:02}", off / 3600, off % 3600 / 60)
}

/// The civil date `days` days after 1970-01-01.
fn date_of(days: i64) -> String {
    let (y, m, d, ..) = Time::from_unix(days * 86_400, 0).civil();
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Default, Clone, Copy)]
struct ModelTokens {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
}

/// Go's `BuildRecord` with the clock's now, written as Go's `Record.JSON` writes it.
fn build_record(sessions: &[SessionUsage]) -> String {
    let (y, m, d, ..) = Time::now_local().civil();
    let today_days = crate::gotime::days_from_civil(y, m as u32, d as u32);
    let today = date_of(today_days);
    let recent_dates: Vec<String> = (0..RECENT_DAY_COUNT).rev().map(|off| date_of(today_days - off)).collect();
    let mut recent: BTreeMap<String, i64> = recent_dates.iter().map(|d| (d.clone(), 0)).collect();
    let mut models: Vec<(String, ModelTokens)> = Vec::new();
    let mut today_by_model: Vec<(String, i64)> = Vec::new();
    let mut today_sessions = BTreeSet::new();
    let mut total_sessions = BTreeSet::new();
    let mut active_days = BTreeSet::new();
    let (mut today_prompts, mut today_total, mut total_prompts) = (0i64, 0i64, 0i64);
    for s in sessions {
        let total = s.total();
        total_prompts = total_prompts.wrapping_add(s.prompts);
        total_sessions.insert(s.id.clone());
        active_days.insert(s.day.clone());
        let i = match models.iter().position(|(k, _)| *k == s.model) {
            Some(i) => i,
            None => {
                models.push((s.model.clone(), ModelTokens::default()));
                models.len() - 1
            }
        };
        let b = &mut models[i].1;
        b.input = b.input.wrapping_add(s.input);
        b.output = b.output.wrapping_add(s.output);
        b.cache_read = b.cache_read.wrapping_add(s.cache_read);
        b.cache_write = b.cache_write.wrapping_add(s.cache_write);
        if let Some(n) = recent.get_mut(&s.day) {
            *n = n.wrapping_add(total);
        }
        if s.day == today {
            today_prompts = today_prompts.wrapping_add(s.prompts);
            today_sessions.insert(s.id.clone());
            today_total = today_total.wrapping_add(total);
            match today_by_model.iter_mut().find(|(k, _)| *k == s.model) {
                Some((_, n)) => *n = n.wrapping_add(total),
                None => today_by_model.push((s.model.clone(), total)),
            }
        }
    }
    let by_model: Vec<String> = today_by_model.iter().map(|(k, n)| format!("{}:{n}", jkey(k))).collect();
    let days: Vec<String> = recent_dates.iter().map(|d| format!("{{\"date\":{},\"messageCount\":{}}}", jstr(d), recent[d])).collect();
    let dates: Vec<String> = active_days.iter().map(|d| jstr(d)).collect();
    let usage: Vec<String> = models
        .iter()
        .map(|(k, b)| {
            format!(
                "{}:{{\"inputTokens\":{},\"outputTokens\":{},\"cacheReadInputTokens\":{},\"cacheCreationInputTokens\":{}}}",
                jkey(k),
                b.input,
                b.output,
                b.cache_read,
                b.cache_write
            )
        })
        .collect();
    let json = format!(
        "{{\"schemaVersion\":1,\"id\":{},\"name\":\"Sarina\",\"updatedAt\":{},\"ready\":true,\"hasLocalStats\":true,\
         \"todayPrompts\":{today_prompts},\"todaySessions\":{},\"todayTotalTokens\":{today_total},\"todayTokensByModel\":{{{}}},\
         \"recentDays\":[{}],\"totalPrompts\":{total_prompts},\"totalSessions\":{},\"activeDays\":{},\"activeDates\":[{}],\
         \"modelUsage\":{{{}}}}}",
        jstr(AGENT_ID),
        jstr(&iso_now()),
        today_sessions.len(),
        by_model.join(","),
        days.join(","),
        total_sessions.len(),
        active_days.len(),
        dates.join(","),
        usage.join(",")
    );
    ascii(&json)
}

/// Go's `WriteRecord`: sarina.json written whole (a temporary file renamed over it), so the panel never reads a torn one.
fn write_record(text: &str, dir: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    crate::goos::mkdir_all(dir)?;
    let path = super::join(&[dir, b"sarina.json"]);
    let (mut f, name) = crate::goos::create_temp(dir, ".sarina.")?;
    let cleanup = |name: &[u8]| {
        let _ = std::fs::remove_file(crate::gopath::os_path(name));
    };
    if let Err(e) = f.write_all(format!("{text}\n").as_bytes()) {
        drop(f);
        cleanup(&name);
        return Err(crate::goerr::path("write", &name, &e));
    }
    drop(f);
    // CreateTemp opens 0600; nothing in this record is sensitive
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(crate::gopath::os_path(&name), std::fs::Permissions::from_mode(0o644));
    }
    // Go's os.Rename refuses a folder at the new name itself ("file exists"), whatever the kernel would say
    let onto_dir = std::fs::symlink_metadata(crate::gopath::os_path(&path)).is_ok_and(|m| m.is_dir());
    let r = if onto_dir {
        Err(std::io::Error::from_raw_os_error(libc::EEXIST))
    } else {
        std::fs::rename(crate::gopath::os_path(&name), crate::gopath::os_path(&path))
    };
    if let Err(e) = r {
        cleanup(&name);
        return Err(format!("rename {} {}: {}", crate::gotext::lossy(&name), crate::gotext::lossy(&path), crate::goerr::text(&e)));
    }
    Ok(path)
}

/// Go's `usageCommand`: `fleet-usage [--session-dir D ...] [--write] [--usage-dir D] [--force] [--limits-only]`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("fleet-usage");
    fs.list("session-dir", "directory of saved sessions (repeatable; replaces the defaults)");
    fs.bool("write", false, "write sarina.json into the panel's usage directory instead of printing");
    fs.string("usage-dir", "", "panel usage directory (default ~/.local/state/omarchy/agents/usage)");
    fs.bool("force", false, "accepted and ignored (collector compatibility)");
    fs.bool("limits-only", false, "accepted and ignored (collector compatibility)");
    if let Err(code) = super::parse_flags(&mut fs, args, env) {
        return code;
    }
    let home = super::home_of(env);
    let mut dirs: Vec<Vec<u8>> = fs.values("session-dir").into_iter().map(String::into_bytes).collect();
    if dirs.is_empty() {
        let cwd = crate::gopath::getwd().unwrap_or_default();
        dirs = default_session_directories(&env.getenv_fn(), &cwd, &home);
    }
    let record = build_record(&read_sessions(&dirs));
    if !fs.is_true("write") {
        env.println(&record);
        return 0;
    }
    let mut target = fs.str("usage-dir").into_bytes();
    if target.is_empty() {
        target = usage_directory(&env.getenv_fn(), &home);
    }
    match write_record(&record, &target) {
        Ok(path) => {
            env.stdout.extend_from_slice(&path);
            env.stdout.push(b'\n');
            env.flush();
            0
        }
        Err(e) => {
            env.println(&format!("fleet-usage: {e}"));
            1
        }
    }
}
