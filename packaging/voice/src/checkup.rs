//! labyrinth-sarina-checkup [--held]: once a day, a quiet look at how the computer is doing (read-only), and Sarina's review of
//! what Heimdall reports; with --held, ask Heimdall to release a held file a program that was there before needs.
//!
//! A port of the Python script of the same name in Labyrinth_OS: the same programs run, the same notes in her vault, the same
//! state file (~/.local/state/sarina/checkup.json), the same notifications, output and exit status (75: the graphics card is
//! busy, try later).

use crate::ask::read_env_file;
use crate::json::{self, Value};
use crate::proc;
use crate::py::{self, Crash, Env, LocalTime, PyResult};
use crate::rx;

pub struct Checkup {
    pub environ: Env,
    pub state: String,
    pub requests: String,
}

impl Checkup {
    pub fn new(environ: Env) -> Checkup {
        let base = match environ.truthy("XDG_STATE_HOME") {
            Some(d) => d.to_string(),
            None => py::expanduser("~/.local/state", &environ),
        };
        let state = py::join_all(&[&base, "sarina", "checkup.json"]);
        let requests = environ.get_or("LABYRINTH_HEIMDALL_REQUESTS", "/var/lib/heimdall-requests").to_string();
        Checkup { environ, state, requests }
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

/// run(argv, timeout): stdout as text, "" when the program cannot be started or takes too long.
pub fn run(cmd: &[String], timeout: f64) -> PyResult<String> {
    match proc::run_capture(cmd, None, None, Some(timeout)) {
        Ok(o) => {
            let out = py::universal_newlines(py::decode_strict(o.stdout)?);
            py::decode_strict(o.stderr)?;
            Ok(out)
        }
        Err(_) => Ok(String::new()),
    }
}

pub fn gpu_busy() -> PyResult<bool> {
    let out = run(&argv(&["nvidia-smi", "--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"]), 10.0)?;
    let lines = py::splitlines(py::strip(&out));
    Ok(match lines.first().and_then(|l| py::parse_int(l)) {
        Some(n) => n >= 40,
        None => false,
    })
}

rx!(R_DF, concat!(r"^", crate::S!(), r"*(\d+)%", crate::S!(), r"+([^\s\x1c-\x1f]+)"));

pub fn findings(last: &Value) -> PyResult<Vec<String>> {
    let mut found = Vec::new();
    let since = match last {
        v if v.truthy() => match v {
            Value::Str(s) => s.clone(),
            _ => return Err(Crash::new("TypeError: can only concatenate str")),
        },
        _ => LocalTime::now().minus_seconds(86400).ymd_hms(),
    };
    let out = run(&argv(&["coredumpctl", "list", "--no-pager", "--no-legend", &format!("--since={}", since)]), 30.0)?;
    let uid = py::getuid().to_string();
    let mut progs: Vec<(String, u64)> = Vec::new();
    for line in py::splitlines(&out) {
        let f = py::split_ws(line);
        if f.len() >= 10 && f[5] == uid {
            let b = py::basename(f[9]).to_string();
            match progs.iter_mut().find(|(p, _)| *p == b) {
                Some(e) => e.1 += 1,
                None => progs.push((b, 1)),
            }
        }
    }
    progs.sort();
    for (p, n) in &progs {
        found.push(format!("{} crashed {} time{}", p, n, if *n == 1 { "" } else { "s" }));
    }
    for user in [false, true] {
        let mut cmd = argv(&["systemctl"]);
        if user {
            cmd.push("--user".into());
        }
        cmd.extend(argv(&["--failed", "--no-legend", "--plain", "--no-pager"]));
        let out = run(&cmd, 30.0)?;
        for line in py::splitlines(&out) {
            let unit = py::split_ws(line).first().copied().unwrap_or("");
            if !unit.is_empty() && !unit.starts_with("run-") {
                found.push(format!("service {} failed{}", unit, if user { " (your session)" } else { "" }));
            }
        }
    }
    let out = run(&argv(&["df", "--output=pcent,target", "-x", "tmpfs", "-x", "devtmpfs", "-x", "efivarfs"]), 30.0)?;
    for line in py::splitlines(&out).into_iter().skip(1) {
        if let Some(c) = R_DF.captures(line) {
            let pct = c.get(1).unwrap().as_str();
            if py::parse_int(pct).map(|n| n >= 90).unwrap_or(false) {
                found.push(format!("{} is {}% full", c.get(2).unwrap().as_str(), pct));
            }
        }
    }
    // stale_overrides(): Labyrinth's override copies of packaged units. The table is empty (as in the Python script: Ollama's
    // settings are a drop-in now), so there is nothing to compare; a new entry needs the unified diff ported too.
    let out = run(&argv(&["journalctl", "-k", "-p", "err", &format!("--since={}", since), "--no-pager", "-q", "-o", "cat"]), 30.0)?;
    let n = py::splitlines(&out).into_iter().filter(|l| !py::strip(l).is_empty()).count();
    if n >= 5 {
        found.push(format!("{} kernel errors", n));
    }
    Ok(found)
}

pub fn heimdall_audit() -> Option<Value> {
    let b = crate::http::request("http://127.0.0.1:8930/v1/audit", None, 15.0, 2 << 20).ok()?;
    json::loads_bytes(&b).ok()
}

/// The files Heimdall holds in quarantine (entries with status "held"); [] when it cannot be asked.
pub fn heimdall_held() -> PyResult<Vec<Value>> {
    let Ok(b) = crate::http::request("http://127.0.0.1:8930/v1/quarantine", None, 15.0, 4 << 20) else { return Ok(vec![]) };
    let Ok(v) = json::loads_bytes(&b) else { return Ok(vec![]) };
    held_of(&v)
}

/// [e for e in data if e.get("status") == "held"] (a non-dict entry: AttributeError, which the script catches: [])
pub fn held_of(v: &Value) -> PyResult<Vec<Value>> {
    let items: Vec<Value> = match v {
        Value::Arr(a) => a.clone(),
        Value::Obj(m) if m.is_empty() => vec![],
        Value::Obj(_) => return Ok(vec![]),
        Value::Str(s) if s.is_empty() => vec![],
        Value::Str(_) => return Ok(vec![]),
        _ => return Err(Crash::new("TypeError: object is not iterable")),
    };
    let mut out = Vec::new();
    for e in items {
        if !matches!(e, Value::Obj(_)) {
            return Ok(vec![]);
        }
        if e.get("status") == Some(&json::s("held")) {
            out.push(e);
        }
    }
    Ok(out)
}

/// What stopped working: (the program or command line, a description) for failed services and recent crashes.
pub fn broken_programs() -> PyResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    for user in [false, true] {
        let scope: Vec<String> = if user { argv(&["--user"]) } else { vec![] };
        let mut cmd = argv(&["systemctl"]);
        cmd.extend(scope.clone());
        cmd.extend(argv(&["--failed", "--no-legend", "--plain", "--no-pager"]));
        for line in py::splitlines(&run(&cmd, 30.0)?) {
            let unit = py::split_ws(line).first().copied().unwrap_or("").to_string();
            if unit.is_empty() {
                continue;
            }
            let mut show = argv(&["systemctl"]);
            show.extend(scope.clone());
            show.extend(argv(&["show", "-p", "ExecStart", "-p", "ExecStartPre", "--value", &unit]));
            let execs = run(&show, 30.0)?;
            out.push((execs, format!("the service {} failed{}", unit, if user { " (your session)" } else { "" })));
        }
    }
    let since = LocalTime::now().minus_seconds(2 * 3600).ymd_hms();
    let uid = py::getuid().to_string();
    for line in py::splitlines(&run(&argv(&["coredumpctl", "list", "--no-pager", "--no-legend", &format!("--since={}", since)]), 30.0)?) {
        let f = py::split_ws(line);
        if f.len() >= 10 && f[5] == uid {
            out.push((f[9].to_string(), format!("{} crashed", py::basename(f[9]))));
        }
    }
    Ok(out)
}

/// list(x or []): a list's items, a str's characters, a dict's keys; a truthy number is a TypeError.
fn list_or_empty(v: Option<&Value>) -> PyResult<Vec<Value>> {
    match v {
        None => Ok(vec![]),
        Some(v) if !v.truthy() => Ok(vec![]),
        Some(Value::Arr(a)) => Ok(a.clone()),
        Some(Value::Str(s)) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        Some(Value::Obj(m)) => Ok(m.iter().map(|(k, _)| Value::Str(k.clone())).collect()),
        Some(_) => Err(Crash::new("TypeError: object is not iterable")),
    }
}

/// e.get(k, default) on a held entry (a dict here: held_of keeps only dicts)
fn get_or(e: &Value, k: &str, default: Value) -> Value {
    e.get(k).cloned().unwrap_or(default)
}

/// Ask Heimdall to release held files that a program which was there before needs. Returns what was asked.
pub fn held_check(c: &Checkup, held: Option<Vec<Value>>, broken: Option<Vec<(String, String)>>) -> PyResult<Vec<String>> {
    let held = match held {
        Some(h) => h,
        None => heimdall_held()?,
    };
    if held.is_empty() {
        return Ok(vec![]);
    }
    let broken = match broken {
        Some(b) => b,
        None => broken_programs()?,
    };
    let mut asked = Vec::new();
    for e in &held {
        let mut paths = vec![get_or(e, "original", json::s(""))];
        paths.extend(list_or_empty(e.get("copies"))?);
        for (cmdline, what) in &broken {
            let mut hit: Option<String> = None;
            for p in &paths {
                if !p.truthy() {
                    continue;
                }
                let words = py::split_ws(cmdline);
                let Value::Str(ps) = p else {
                    // p in cmdline.split() is False for a non-str; then "path=" + p is a TypeError
                    return Err(Crash::new("TypeError: can only concatenate str"));
                };
                if words.contains(&ps.as_str()) || cmdline.contains(&format!("path={} ", ps)) || cmdline == ps {
                    hit = Some(ps.clone());
                    break;
                }
            }
            let Some(hit) = hit else { continue };
            let sha = e.get("sha256").ok_or_else(|| Crash::new("KeyError: 'sha256'"))?;
            let req = json::obj(vec![("release_if_proven", sha.clone()), ("why", json::s(&format!("{}, and it runs {}, which Heimdall holds", what, hit)))]);
            let short = match sha {
                Value::Str(s) => py::take(s, 12).to_string(),
                Value::Arr(a) => Value::Arr(a.iter().take(12).cloned().collect()).py_str(),
                _ => return Err(Crash::new("TypeError: object is not subscriptable")),
            };
            let name = py::join(&c.requests, &format!("sarina-{}-{}.json", short, py::getpid()));
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o666).open(&name) {
                if f.write_all(json::dumps(&req).as_bytes()).is_ok() {
                    asked.push(format!("{}: {}", hit, what));
                }
            }
            break;
        }
    }
    Ok(asked)
}

fn field(p: &Value, k: &str) -> PyResult<Value> {
    match p {
        Value::Obj(_) => Ok(p.get(k).cloned().unwrap_or(Value::Null)),
        _ => Err(Crash::new("AttributeError: object has no attribute 'get'")),
    }
}

/// What Heimdall finds open, one short key per finding (a kernel's key changes when its count of holes does).
pub fn security_keys(audit: &Value) -> PyResult<Vec<(String, Value)>> {
    let mut keys: Vec<(String, Value)> = Vec::new();
    let mut put = |k: String, v: Value| {
        if let Some(e) = keys.iter_mut().find(|(kk, _)| *kk == k) {
            e.1 = v;
        } else {
            keys.push((k, v));
        }
    };
    for p in list_or_empty(audit.get("packages"))? {
        let (pkg, inst, adv) = (field(&p, "package")?, field(&p, "installed")?, field(&p, "advisory")?);
        let sev = field(&p, "severity")?;
        let sev = if sev.truthy() {
            match sev {
                Value::Str(s) => s.to_lowercase(),
                _ => return Err(Crash::new("AttributeError: object has no attribute 'lower'")),
            }
        } else {
            String::new()
        };
        put(
            format!("pkg|{}|{}|{}", pkg.py_str(), inst.py_str(), adv.py_str()),
            json::s(&format!("{} {}: {} vulnerability", pkg.py_str(), inst.py_str(), sev)),
        );
    }
    for k in list_or_empty(audit.get("kernels"))? {
        let count = field(&k, "count")?;
        if count.truthy() {
            let kernel = field(&k, "kernel")?;
            let running = field(&k, "running")?.truthy();
            put(
                format!("kernel|{}|{}", kernel.py_str(), count.py_str()),
                json::s(&format!("kernel {}{}: {} known holes", kernel.py_str(), if running { " (running)" } else { "" }, count.py_str())),
            );
        }
    }
    for g in list_or_empty(audit.get("hardening"))? {
        let id = field(&g, "id")?;
        let now = field(&g, "now")?;
        let val = match &g {
            Value::Obj(_) => g.get("now").cloned().unwrap_or(json::s("")),
            _ => Value::Null,
        };
        put(format!("hard|{}|{}", id.py_str(), now.py_str()), val);
    }
    for e in list_or_empty(audit.get("held"))? {
        let sha = field(&e, "sha256")?;
        put(
            format!("held|{}", sha.py_str()),
            json::s(&format!("Heimdall holds {} in quarantine ({})", field(&e, "original")?.py_str(), field(&e, "reason")?.py_str())),
        );
    }
    Ok(keys)
}

/// Sarina's proposal for Heimdall's new findings: saved, and one notification.
pub fn security_review(c: &Checkup, env: &Env, vault: &str, new: &[String]) -> PyResult<String> {
    let mut env = env.clone();
    env.set("SARINA_SYSTEM", "on");
    let home = py::expanduser("~", &c.environ);
    let joined = new.join("; ");
    let cmd = argv(&["sarina", "skill", "run", "security-advisor", &format!("new since the last check-up: {}", joined)]);
    let answer = match proc::run_capture(&cmd, Some(&env), Some(&home), Some(600.0)) {
        Ok(o) => {
            let out = py::universal_newlines(py::decode_strict(o.stdout)?);
            py::decode_strict(o.stderr)?;
            py::strip(&out).to_string()
        }
        Err(_) => String::new(),
    };
    if answer.is_empty() {
        return Ok(format!("Heimdall found {} new thing(s) ({}); Sarina could not review them", new.len(), joined));
    }
    let today = LocalTime::now().date_iso();
    let (path, front) = if !vault.is_empty() && py::isdir(vault) {
        let d = py::join(vault, "Security");
        py::makedirs(&d, 0o777).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
        (
            py::join(&d, &format!("{}.md", today)),
            format!("---\ncreated: {}\nsource: labyrinth-sarina-checkup (security-advisor)\ntags: [security]\n---\n\n", today),
        )
    } else {
        let base = match c.environ.truthy("XDG_DATA_HOME") {
            Some(d) => d.to_string(),
            None => py::expanduser("~/.local/share", &c.environ),
        };
        let d = py::join_all(&[&base, "sarina", "security-reviews"]);
        py::makedirs(&d, 0o777).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
        (py::join(&d, &format!("{}.md", today)), String::new())
    };
    let size = {
        // open(path, "a") creates it first, then its size is asked
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().append(true).create(true).mode(0o666).open(&path).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    };
    let text = format!(
        "{}# Security review {}\n\nHeimdall's new findings: {}\n\n{}\n",
        if size == 0 { front } else { "\n\n".to_string() },
        LocalTime::now().ymd_hm(),
        joined,
        answer
    );
    py::append_text(&path, &text).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
    let first = answer.split('\n').next().unwrap_or("");
    let summary = py::strip(first.strip_prefix("Summary:").unwrap_or(first)).to_string();
    let msg = format!("{}\n\nHer proposal: {}", summary, path);
    proc::run_inherit(&argv(&["notify-send", "-a", "Sarina", "-i", "security-high", "Security review", py::take(&msg, 800)]), None)
        .map_err(|_| Crash::new("FileNotFoundError: notify-send"))?;
    Ok(format!("security review of {} new finding(s) saved in {}", new.len(), path))
}

/// One check-up line in the day's note, above the status sections `sarina daily-report` keeps there.
pub fn note(vault: &str, text: &str) -> PyResult<()> {
    if vault.is_empty() || !py::isdir(vault) {
        return Ok(());
    }
    let d = py::join(vault, "Daily");
    py::makedirs(&d, 0o777).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
    let now = LocalTime::now();
    let today = now.date_iso();
    let p = py::join(&d, &format!("{}.md", today));
    let line = format!("- {} check-up: {}\n", now.hm(), text);
    let old = match py::read_text(&p) {
        Ok(t) => t,
        Err(py::ReadErr::Os) => format!("---\ncreated: {}\nsource: labyrinth-sarina-checkup\ntags: [daily]\n---\n\n# {}\n\n", today, today),
        Err(py::ReadErr::Crash(c)) => return Err(c),
    };
    let mut cut = old.len();
    for marker in ["\n## Status (", "\n## Numbers"] {
        if let Some(i) = old.find(marker) {
            cut = cut.min(i + 1);
        }
    }
    let mut head = old[..cut].to_string();
    if !head.ends_with('\n') {
        head.push('\n');
    }
    py::write_text(&p, &format!("{}{}{}", head, line, &old[cut..])).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
    Ok(())
}

/// The day's status section and numbers line, kept by Sarina herself (`sarina daily-report`).
pub fn refresh_status(vault: &str) -> PyResult<()> {
    if vault.is_empty() || !py::isdir(vault) {
        return Ok(());
    }
    run(&argv(&["sarina", "daily-report", "--path", vault]), 60.0)?;
    Ok(())
}

pub fn main(args: &[String]) -> i32 {
    py::coerce_c_locale();
    let c = Checkup::new(Env::from_process());
    match run_main(&c, args) {
        Ok(code) => code,
        Err(Crash(what)) => {
            eprintln!("Traceback: {}", what);
            1
        }
    }
}

fn run_main(c: &Checkup, args: &[String]) -> PyResult<i32> {
    if args.iter().any(|a| a == "--held") {
        for a in held_check(c, None, None)? {
            py::print_flush(&format!("asked Heimdall to release (if it can prove it was there before): {}", a));
        }
        return Ok(0);
    }
    if gpu_busy()? {
        py::print_flush("the graphics card is busy; trying again later");
        return Ok(75);
    }
    let mut st = match py::read_text(&c.state) {
        Ok(t) => json::loads(&t).unwrap_or(Value::Obj(vec![])),
        Err(_) => Value::Obj(vec![]),
    };
    if !matches!(st, Value::Obj(_)) {
        return Err(Crash::new("AttributeError: object has no attribute 'get'"));
    }
    let mut env = c.environ.clone();
    for (k, v) in &read_env_file(&c.environ)?.vars {
        env.setdefault(k, v);
    }
    let vault = env.get_or("SARINA_VAULT", "").to_string();
    let found = findings(&st.get("last").cloned().unwrap_or(json::s("")))?;
    if found.is_empty() {
        note(&vault, "all fine")?;
    } else {
        let joined = found.join("; ");
        let q = format!(
            "Daily check-up. The computer reported: {}. Look into these with your tools (crash_detail for a crash, system_status for the rest) and tell me in at most three plain sentences what matters and what I could do; do not change anything.",
            joined
        );
        if let Some(r) = py::read_text_opt("/usr/share/labyrinth/sarina-voice-rules.txt")? {
            env.set("LABYRINTH_VOICE_RULES", py::strip(&r));
        }
        let home = py::expanduser("~", &c.environ);
        let cmd = argv(&["labyrinth-ask-sarina", "--voice", "--cwd", &home, &q]);
        let answer = match proc::run_capture(&cmd, Some(&env), None, Some(300.0)) {
            Ok(o) => {
                let out = py::universal_newlines(py::decode_strict(o.stdout)?);
                py::decode_strict(o.stderr)?;
                let a = py::strip(&out).to_string();
                if a.is_empty() {
                    "(no answer)".to_string()
                } else {
                    a
                }
            }
            Err(_) => "(Sarina could not look at it)".to_string(),
        };
        note(&vault, &format!("{}. {}", joined, answer.replace('\n', " ")))?;
        let msg = format!("{}\n\n{}", joined, answer);
        proc::run_inherit(&argv(&["notify-send", "-a", "Sarina", "-i", "dialog-information", "Daily check-up", py::take(&msg, 800)]), None)
            .map_err(|_| Crash::new("FileNotFoundError: notify-send"))?;
    }
    if let Some(mut audit) = heimdall_audit() {
        if !matches!(audit, Value::Obj(_)) {
            return Err(Crash::new("TypeError: object does not support item assignment"));
        }
        audit.set("held", Value::Arr(heimdall_held()?));
        let keys = security_keys(&audit)?;
        let seen: Vec<String> = match st.get("security_seen") {
            Some(v) if v.truthy() => {
                let items = list_or_empty(Some(v))?;
                let mut out = Vec::new();
                for x in items {
                    match x {
                        Value::Str(s) => out.push(s),
                        Value::Arr(_) | Value::Obj(_) => return Err(Crash::new("TypeError: unhashable type")),
                        _ => {}
                    }
                }
                out
            }
            _ => vec![],
        };
        let mut sorted: Vec<&(String, Value)> = keys.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let new: Vec<&Value> = sorted.iter().filter(|(k, _)| !seen.contains(k)).map(|(_, v)| v).collect();
        if !new.is_empty() {
            let mut strs = Vec::new();
            for v in &new {
                match v {
                    Value::Str(s) => strs.push(s.clone()),
                    _ => return Err(Crash::new("TypeError: sequence item: expected str instance")),
                }
            }
            let r = security_review(c, &env, &vault, &strs)?;
            note(&vault, &r)?;
        }
        st.set("security_seen", Value::Arr(sorted.iter().map(|(k, _)| json::s(k)).collect()));
    }
    refresh_status(&vault)?;
    st.set("last", json::s(&LocalTime::now().ymd_hms()));
    py::makedirs(&py::dirname(&c.state), 0o777).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
    py::write_text(&c.state, &json::dumps(&st)).map_err(|e| Crash::new(format!("OSError: {}", e)))?;
    py::print_flush(if found.is_empty() { "all fine".to_string() } else { found.join("; ") }.as_str());
    Ok(0)
}
