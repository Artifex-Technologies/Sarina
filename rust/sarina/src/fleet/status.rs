//! Go: `chymaera/fleet/status.go`, `statusjson.go` and `fleet-status`: the other agents' records in Omarchy's folder,
//! READ-ONLY and untrusted: strings lose control and direction characters and are cut, ids must match a strict pattern,
//! numbers are clamped, big files are skipped. `--json` is Yggdrasil's contract (its Agents item): keep its keys.

use super::{ascii, jstr, names, stat};
use crate::goany::Any;
use crate::gotime::Time;
use std::sync::LazyLock;

/// a bigger record is not one of Omarchy's collectors' output
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const MAX_TEXT_LENGTH: usize = 80;
const MAX_LIMITS: usize = 16;
/// Omarchy refreshes every 15 minutes; an hour is four misses
const STALE_AFTER_NANOS: i64 = 3_600_000_000_000;
/// argv[0]'s last part of Sarina's service (packaging/arch/sarina-service.service)
const SERVICE_NAME: &[u8] = b"sarina-service";

static AGENT_ID_PATTERN: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{0,31}$").expect("agentIDPattern"));
static CONTROL_CHARS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new("[\\x00-\\x1f\\x{7f}-\\x{9f}\\x{200b}-\\x{200f}\\x{202a}-\\x{202e}\\x{2066}-\\x{2069}]").expect("controlChars")
});

/// One rate-limit window of another agent (Go's `LimitReading`).
#[derive(Clone)]
pub struct Limit {
    pub label: String,
    pub percent_used: Option<f64>,
    pub resets_at: String,
}

/// The sanitised view of one record (Go's `AgentStatus`).
#[derive(Clone)]
pub struct AgentStatus {
    pub id: String,
    pub name: String,
    pub ready: bool,
    pub stale: bool,
    pub updated_at: String,
    pub today_tokens: i64,
    pub week_tokens: i64,
    pub total_sessions: i64,
    pub limits: Vec<Limit>,
    pub status_text: String,
}

impl AgentStatus {
    /// Go's `HeadroomPercent`: the room left in the tightest window; None when no limit is known ("unknown", never
    /// "unlimited").
    pub fn headroom(&self) -> Option<f64> {
        let mut max_used: Option<f64> = None;
        for l in &self.limits {
            if let Some(p) = l.percent_used {
                if max_used.is_none_or(|m| p > m) {
                    max_used = Some(p);
                }
            }
        }
        max_used.map(|m| (100.0 - m).max(0.0))
    }
}

fn clean(v: Option<&Any>) -> String {
    let Some(Any::String(s)) = v else { return String::new() };
    let s = CONTROL_CHARS.replace_all(s, "");
    let s = crate::gotext::trim_space(&s);
    match s.char_indices().nth(MAX_TEXT_LENGTH) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}

fn number(v: Option<&Any>) -> Option<f64> {
    match v {
        Some(Any::Number(f)) => Some(*f),
        Some(Any::Int(n)) => Some(*n as f64),
        _ => None,
    }
}

fn count(v: Option<&Any>) -> i64 {
    match number(v) {
        Some(f) if !f.is_nan() && !f.is_infinite() => {
            if f >= 9.2e18 {
                i64::MAX
            } else {
                (f as i64).max(0)
            }
        }
        _ => 0,
    }
}

fn percent(v: Option<&Any>) -> Option<f64> {
    match number(v) {
        // Go's math.Max(f, 0) is +0 for -0 too
        Some(f) if !f.is_nan() && !f.is_infinite() => Some(if f <= 0.0 { 0.0 } else { f.min(100.0) }),
        _ => None,
    }
}

// ---- Go's time.Parse for the layouts records are written in ----

#[derive(Clone, Copy, PartialEq)]
enum El {
    Year,
    Lit(u8),
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Frac,
    /// "Z07:00"
    ZoneColon,
    /// "Z0700"
    Zone,
}

fn layouts() -> Vec<Vec<El>> {
    use El::*;
    let date = vec![Year, Lit(b'-'), Month, Lit(b'-'), Day];
    let clock = |sep: u8| {
        let mut v = date.clone();
        v.extend([Lit(sep), Hour, Lit(b':'), Minute, Lit(b':'), Second, Frac]);
        v
    };
    let with = |mut v: Vec<El>, z: El| {
        v.push(z);
        v
    };
    vec![
        with(clock(b'T'), ZoneColon), // RFC3339Nano
        with(clock(b'T'), Zone),
        with(clock(b' '), ZoneColon),
        with(clock(b' '), Zone),
        clock(b'T'), // naive: UTC
        clock(b' '),
        date,
    ]
}

/// Go's `getnum`: one or two digits (exactly two when `fixed`).
fn getnum(v: &[u8], fixed: bool) -> Option<(i64, usize)> {
    let digit = |i: usize| v.get(i).is_some_and(u8::is_ascii_digit);
    if !digit(0) {
        return None;
    }
    if !digit(1) {
        return if fixed { None } else { Some(((v[0] - b'0') as i64, 1)) };
    }
    Some((((v[0] - b'0') * 10 + (v[1] - b'0')) as i64, 2))
}

/// Go's `time.Parse(layout, s)` for one of [`layouts`]: the instant, or None where Go answers an error.
fn parse_layout(layout: &[El], s: &str) -> Option<Time> {
    let mut v = s.as_bytes();
    let (mut year, mut month, mut day, mut hour, mut min, mut sec, mut nanos) = (0i64, 1i64, 1i64, 0i64, 0i64, 0i64, 0i64);
    let mut offset: Option<i64> = None;
    for (i, el) in layout.iter().enumerate() {
        match *el {
            El::Lit(b' ') => {
                // a space in the layout takes one or more in the value
                if !v.is_empty() && v[0] != b' ' {
                    return None;
                }
                while v.first() == Some(&b' ') {
                    v = &v[1..];
                }
            }
            El::Lit(c) => {
                if v.first() != Some(&c) {
                    return None;
                }
                v = &v[1..];
            }
            El::Year => {
                if v.len() < 4 || !v[..4].iter().all(u8::is_ascii_digit) {
                    return None;
                }
                year = v[..4].iter().fold(0, |n, c| n * 10 + (c - b'0') as i64);
                v = &v[4..];
            }
            El::Month => {
                let (n, w) = getnum(v, true)?;
                if !(1..=12).contains(&n) {
                    return None;
                }
                month = n;
                v = &v[w..];
            }
            El::Day => {
                let (n, w) = getnum(v, true)?;
                day = n;
                v = &v[w..];
            }
            El::Hour => {
                let (n, w) = getnum(v, false)?;
                if n >= 24 {
                    return None;
                }
                hour = n;
                v = &v[w..];
            }
            El::Minute => {
                let (n, w) = getnum(v, true)?;
                if n >= 60 {
                    return None;
                }
                min = n;
                v = &v[w..];
            }
            El::Second => {
                let (n, w) = getnum(v, true)?;
                if n >= 60 {
                    return None;
                }
                sec = n;
                v = &v[w..];
                // a fraction in the value but none in the layout is read here (every layout here has one next)
                if v.len() >= 2 && (v[0] == b'.' || v[0] == b',') && v[1].is_ascii_digit() && layout.get(i + 1) != Some(&El::Frac) {
                    let mut n = 2;
                    while v.get(n).is_some_and(u8::is_ascii_digit) {
                        n += 1;
                    }
                    nanos = frac_nanos(&v[..n]);
                    v = &v[n..];
                }
            }
            El::Frac => {
                if v.len() < 2 || !(v[0] == b'.' || v[0] == b',') || !v[1].is_ascii_digit() {
                    continue; // the fraction left out
                }
                let mut n = 1;
                while v.get(n).is_some_and(u8::is_ascii_digit) {
                    n += 1;
                }
                nanos = frac_nanos(&v[..n]);
                v = &v[n..];
            }
            El::ZoneColon | El::Zone => {
                if v.first() == Some(&b'Z') {
                    v = &v[1..];
                    offset = Some(0);
                    continue;
                }
                let (sign, hh, mm, rest) = if *el == El::ZoneColon {
                    if v.len() < 6 || v[3] != b':' {
                        return None;
                    }
                    (v[0], &v[1..3], &v[4..6], &v[6..])
                } else {
                    if v.len() < 5 {
                        return None;
                    }
                    (v[0], &v[1..3], &v[3..5], &v[5..])
                };
                let (h, _) = getnum(hh, true)?;
                let (m, _) = getnum(mm, true)?;
                if h > 24 || m > 60 {
                    return None;
                }
                let o = (h * 60 + m) * 60;
                offset = Some(match sign {
                    b'+' => o,
                    b'-' => -o,
                    _ => return None,
                });
                v = rest;
            }
        }
    }
    if !v.is_empty() {
        return None;
    }
    if day < 1 || day > crate::gotime::days_in_month(year, month as u32) as i64 {
        return None;
    }
    let days = crate::gotime::days_from_civil(year, month as u32, day as u32);
    let local = days * 86_400 + hour * 3600 + min * 60 + sec;
    Some(Time::from_unix(local - offset.unwrap_or(0), nanos as u32))
}

/// Go's `parseNanoseconds` on ".ddd" (at most nine digits read).
fn frac_nanos(v: &[u8]) -> i64 {
    let digits = &v[1..v.len().min(10)];
    let mut ns: i64 = digits.iter().fold(0, |x, c| x * 10 + (c - b'0') as i64);
    for _ in digits.len()..9 {
        ns *= 10;
    }
    ns
}

/// Go's `parseTime`: the first layout that reads the value.
fn parse_time(v: Option<&Any>) -> Option<Time> {
    let Some(Any::String(s)) = v else { return None };
    layouts().iter().find_map(|l| parse_layout(l, s))
}

/// Go's `ParseRecord` against the clock: the sanitised status of one decoded record, or None.
fn parse_record(data: &Any, now: &Time) -> Option<AgentStatus> {
    let Any::Object(obj) = data else { return None };
    let Some(Any::String(id)) = obj.get("id") else { return None };
    if !AGENT_ID_PATTERN.is_match(id) {
        return None;
    }
    let stale = match parse_time(obj.get("updatedAt")) {
        Some(t) => now.sub_nanos(&t) > STALE_AFTER_NANOS,
        None => true,
    };
    let mut week = 0i64;
    if let Some(Any::Array(days)) = obj.get("recentDays") {
        for d in days {
            if let Any::Object(dm) = d {
                // Omarchy's panel names this messageCount but it carries tokens
                week = week.wrapping_add(count(dm.get("messageCount")));
            }
        }
    }
    let mut limits = Vec::new();
    if let Some(Any::Array(raw)) = obj.get("limits") {
        for e in raw.iter().take(MAX_LIMITS) {
            if let Any::Object(em) = e {
                let mut label = clean(em.get("label"));
                if label.is_empty() {
                    label = "limit".into();
                }
                limits.push(Limit { label, percent_used: percent(em.get("percent")), resets_at: clean(em.get("resetsAt")) });
            }
        }
    }
    let mut name = clean(obj.get("name"));
    if name.is_empty() {
        name = id.clone();
    }
    Some(AgentStatus {
        id: id.clone(),
        name,
        ready: matches!(obj.get("ready"), Some(Any::Bool(true))),
        stale,
        updated_at: clean(obj.get("updatedAt")),
        today_tokens: count(obj.get("todayTotalTokens")),
        week_tokens: week,
        total_sessions: count(obj.get("totalSessions")),
        limits,
        status_text: clean(obj.get("usageStatusText")),
    })
}

/// Go's `ReadAgentStatus`: every readable record in the folder, sorted by agent id (stable).
pub fn read_agent_status(dir: &[u8]) -> Vec<AgentStatus> {
    let Some(all) = names(dir) else { return Vec::new() };
    let now = Time::now();
    let mut out = Vec::new();
    for name in all.into_iter().filter(|n| n.ends_with(b".json")) {
        let path = super::join(&[dir, &name]);
        let Some(st) = stat(&path) else { continue };
        if st.is_dir || st.size > MAX_RECORD_BYTES {
            continue;
        }
        // a regular file only, bounded: these records are other programs' (untrusted)
        let Ok(raw) = crate::session::readfile::read_regular(&path, MAX_RECORD_BYTES) else { continue };
        let (data, err) = crate::gojson::unmarshal_any(&raw);
        if err.is_some() {
            continue;
        }
        if let Some(s) = parse_record(&data, &now) {
            out.push(s);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Go's `SuggestAgent` with Sarina excluded: the most measured headroom among current, ready agents; ties to the one that
/// used fewer tokens today.
pub fn suggest(statuses: &[AgentStatus]) -> Option<&AgentStatus> {
    let mut best: Option<(&AgentStatus, f64)> = None;
    for s in statuses {
        let h = s.headroom();
        if s.id == super::usage::AGENT_ID || !s.ready || s.stale {
            continue;
        }
        let Some(h) = h else { continue };
        match best {
            None => best = Some((s, h)),
            Some((b, bh)) if h > bh || (h == bh && s.today_tokens < b.today_tokens) => best = Some((s, h)),
            _ => {}
        }
    }
    best.map(|(s, _)| s)
}

fn tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Go's `RenderStatus`: the view for people.
fn render(statuses: &[AgentStatus]) -> String {
    if statuses.is_empty() {
        return "No agent usage records found.".into();
    }
    let mut lines = vec!["Agents".to_string(), String::new()];
    for s in statuses {
        let mut flags = Vec::new();
        if s.stale {
            flags.push("stale");
        }
        if !s.ready {
            flags.push("not ready");
        }
        let suffix = if flags.is_empty() { String::new() } else { format!("  [{}]", flags.join(", ")) };
        lines.push(format!(
            "{} ({}): {} today, {} this week, {} sessions{suffix}",
            s.name,
            s.id,
            tokens(s.today_tokens),
            tokens(s.week_tokens),
            s.total_sessions
        ));
        for l in &s.limits {
            let used = match l.percent_used {
                Some(p) => format!("{p:.0}% used"),
                None => "unknown".into(),
            };
            let reset = if l.resets_at.is_empty() { String::new() } else { format!(", resets {}", l.resets_at) };
            lines.push(format!("    {}: {used}{reset}", l.label));
        }
        if s.limits.is_empty() {
            let text = if s.status_text.is_empty() { "no limit data" } else { &s.status_text };
            lines.push(format!("    {text}"));
        }
    }
    lines.join("\n")
}

/// Go's `ReadSelf`: Sarina's last activity (the newest session folder's own time) and whether her service runs.
fn read_self(session_dirs: &[Vec<u8>], proc_root: &[u8]) -> (bool, String) {
    let mut last: Option<(i64, u32)> = None;
    for d in session_dirs {
        if let Some(st) = stat(d) {
            if st.is_dir && last.is_none_or(|l| st.mtime > l) {
                last = Some(st.mtime);
            }
        }
    }
    let last_active = match last {
        // Go's zero time is never a folder's
        Some((s, n)) => Time::from_unix(s, n).format_rfc3339_utc(),
        None => String::new(),
    };
    (service_running(proc_root), last_active)
}

/// Go's `serviceRunning`: a process whose first argument ends in the service's name; only regular files are read
/// (procfs's are), and only their first 4 KiB.
fn service_running(proc_root: &[u8]) -> bool {
    use std::io::Read;
    let Some(all) = names(proc_root) else { return false };
    for n in all {
        if crate::gotext::atoi(&n).is_none() {
            continue;
        }
        let p = super::join(&[proc_root, &n, b"cmdline"]);
        match std::fs::symlink_metadata(crate::gopath::os_path(&p)) {
            Ok(m) if m.file_type().is_file() => {}
            _ => continue,
        }
        let Ok(f) = std::fs::File::open(crate::gopath::os_path(&p)) else { continue };
        let mut raw = Vec::new();
        let _ = f.take(4096).read_to_end(&mut raw);
        let argv0 = raw.split(|&c| c == 0).next().unwrap_or_default();
        if !argv0.is_empty() && crate::gopath::base(argv0) == SERVICE_NAME {
            return true;
        }
    }
    false
}

/// Go's `StatusJSON`: `{"agents": [...], "self": {...}}` and, with suggest, `"mostHeadroom"` (an id, or "").
fn status_json(statuses: &[AgentStatus], running: bool, last_active: &str, with_suggest: bool) -> Result<String, String> {
    let mut agents = Vec::new();
    for s in statuses {
        let mut limits = Vec::new();
        for l in &s.limits {
            let o =
                crate::gojson::Obj::new(false).str("label", &l.label).f64_ptr("percentUsed", l.percent_used).str("resetsAt", &l.resets_at);
            limits.push(o.end().map_err(|e| e.0)?);
        }
        let o = crate::gojson::Obj::new(false)
            .str("id", &s.id)
            .str("name", &s.name)
            .bool("ready", s.ready)
            .bool("stale", s.stale)
            .str("updatedAt", &s.updated_at)
            .int("todayTokens", s.today_tokens)
            .int("weekTokens", s.week_tokens)
            .int("totalSessions", s.total_sessions)
            .raw("limits", &format!("[{}]", limits.join(",")))
            .str("statusText", &s.status_text);
        agents.push(o.end().map_err(|e| e.0)?);
    }
    let me = format!(
        "{{\"id\":{},\"name\":\"Sarina\",\"running\":{running},\"lastActive\":{}}}",
        jstr(super::usage::AGENT_ID),
        jstr(last_active)
    );
    let mut doc = format!("{{\"agents\":[{}],\"self\":{me}", agents.join(","));
    if with_suggest {
        doc.push_str(&format!(",\"mostHeadroom\":{}", jstr(suggest(statuses).map_or("", |s| &s.id))));
    }
    doc.push('}');
    Ok(ascii(&doc))
}

/// Go's `statusCommand`: `fleet-status [--usage-dir D] [--suggest] [--json]`.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("fleet-status");
    fs.string("usage-dir", "", "panel usage directory (default ~/.local/state/omarchy/agents/usage)");
    fs.bool("suggest", false, "also name the agent with the most headroom");
    fs.bool("json", false, "print one JSON object (the records, and Sarina's own state) instead of text");
    if let Err(code) = super::parse_flags(&mut fs, args, env) {
        return code;
    }
    let home = super::home_of(env);
    let mut dir = fs.str("usage-dir").into_bytes();
    if dir.is_empty() {
        dir = super::usage::usage_directory(&env.getenv_fn(), &home);
    }
    let statuses = read_agent_status(&dir);
    let with_suggest = fs.is_true("suggest");
    if fs.is_true("json") {
        let cwd = crate::gopath::getwd().unwrap_or_default();
        let dirs = super::usage::default_session_directories(&env.getenv_fn(), &cwd, &home);
        let (running, last) = read_self(&dirs, b"/proc");
        return match status_json(&statuses, running, &last, with_suggest) {
            Ok(text) => {
                env.println(&text);
                0
            }
            Err(e) => {
                env.println(&format!("fleet-status: {e}"));
                1
            }
        };
    }
    env.println(&render(&statuses));
    if with_suggest {
        env.println("");
        match suggest(&statuses) {
            Some(pick) => {
                env.println(&format!(
                    "Most headroom: {} ({:.0}% left in its tightest window)",
                    pick.name,
                    pick.headroom().unwrap_or_default()
                ));
            }
            None => env.println("No agent has a current, measured limit to compare."),
        }
    }
    0
}
