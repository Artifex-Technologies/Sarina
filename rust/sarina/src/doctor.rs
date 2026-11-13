//! Go: `go/doctor`: the read-only check-up of a Sarina installation and of the ecosystem pieces it talks to (the model
//! backend, sessions, compute tiers, the operator's extension files, Limbo, Heimdall, the ports). One report says WHY
//! something is broken.
//!
//! Every check: nothing is modified, every network call has a short timeout, and no token, key or environment value is
//! put in a finding (only "set"/"unset" and lengths; URLs reduced to scheme and host). Proved against the Go build by the
//! CLI differential's doctor cases (each in a network namespace of its own).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gohttp::{self, Dial, Opts};
use crate::gojson;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;

/// Go's `ShortTimeout`: the bound of every network call the check-up makes.
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(3);

const MIN_USABLE_WINDOW: i64 = 8192;

const OLLAMA_HINT: &str = "raise it: `PARAMETER num_ctx 16384` in the Modelfile or OLLAMA_CONTEXT_LENGTH=16384, or set SARINA_CONTEXT_WINDOW if the server's real window is larger";

const LIMBO_DOCS: &str = "Limbo docs/INTEGRATION-FAILURES.md";

/// One row of the report (Go's `Check`); the remedy is empty for PASS and SKIP.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub id: String,
    pub status: &'static str,
    pub finding: String,
    pub remedy: String,
}

pub const PASS: &str = "PASS";
pub const WARN: &str = "WARN";
pub const FAIL: &str = "FAIL";
pub const SKIP: &str = "SKIP";

/// What to inspect (Go's `Options`); empty values mean the real environment.
#[derive(Default)]
pub struct Options {
    /// the workspace ("" = ".")
    pub cwd: String,
    /// "" = <cwd>/.port_sessions/agent
    pub session_dir: String,
    /// "" = the user's home
    pub home: String,
    /// None: this process's environment
    pub getenv: Option<crate::cli::Getenv>,
    /// "" = the system's skills folder
    pub system_skills_dir: String,
    /// the ports the informational check looks at; None: the service's and the console's
    pub ports: Option<Vec<(String, u16)>>,
}

struct Runner {
    o: Options,
    home: String,
    env: crate::cli::Getenv,
    out: Vec<Check>,
}

impl Runner {
    fn add(&mut self, id: &str, st: &'static str, finding: String, remedy: &str) {
        let remedy = if st == PASS || st == SKIP { String::new() } else { remedy.to_owned() };
        self.out.push(Check { id: id.to_owned(), status: st, finding, remedy });
    }
}

fn process_env() -> crate::cli::Getenv {
    Arc::new(|k: &str| {
        std::env::var_os(k).map(|v| gotext::lossy(std::os::unix::ffi::OsStrExt::as_bytes(v.as_os_str()))).unwrap_or_default()
    })
}

/// Go's `Run`: every check, in a fixed order.
pub fn run(ctx: &Ctx, mut o: Options) -> Vec<Check> {
    if o.cwd.is_empty() {
        o.cwd = ".".into();
    }
    let env = o.getenv.clone().unwrap_or_else(process_env);
    let home = if o.home.is_empty() { std::env::var("HOME").unwrap_or_default() } else { o.home.clone() };
    let mut r = Runner { o, home, env, out: Vec::new() };
    r.backend(ctx);
    r.sessions();
    r.tiers(ctx);
    r.extensions();
    r.limbo();
    r.heimdall(ctx);
    r.ports();
    r.out
}

/// Go's `HasFailure`.
pub fn has_failure(cs: &[Check]) -> bool {
    cs.iter().any(|c| c.status == FAIL)
}

/// Go's `JSON`: `json.MarshalIndent(cs, "", "  ")`.
pub fn json(cs: &[Check]) -> String {
    let mut b = String::from("[");
    for (i, c) in cs.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        for (j, (k, v)) in [("id", c.id.as_str()), ("status", c.status), ("finding", &c.finding), ("remedy", &c.remedy)].iter().enumerate()
        {
            b.push(if j == 0 { '{' } else { ',' });
            b.push_str(&format!("\"{k}\":"));
            gojson::write_str(&mut b, v, true);
        }
        b.push('}');
    }
    b.push(']');
    gojson::indent(&b, "  ")
}

/// Go's `Text`: an aligned table, a remedy on its own line after its row, the counts at the end.
pub fn text(cs: &[Check]) -> String {
    let w = cs.iter().map(|c| c.id.chars().count()).max().unwrap_or(0);
    let pad = |s: &str, n: usize| format!("{s}{}", " ".repeat(n.saturating_sub(s.chars().count())));
    let mut b = String::new();
    for c in cs {
        b.push_str(&format!("{}  {}  {}\n", pad(c.status, 4), pad(&c.id, w), one_line(&c.finding)));
        if !c.remedy.is_empty() {
            b.push_str(&format!("      {}  -> {}\n", pad("", w), one_line(&c.remedy)));
        }
    }
    let n = |st: &str| cs.iter().filter(|c| c.status == st).count();
    b.push_str(&format!("\n{} pass, {} warn, {} fail, {} skip\n", n(PASS), n(WARN), n(FAIL), n(SKIP)));
    b
}

fn one_line(s: &str) -> String {
    gotext::fields(s).join(" ")
}

/// Go's `hostOf`: scheme://host[:port] only, so a token in the URL never reaches a report.
fn host_of(raw: &str) -> String {
    match crate::gourl::parse(gotext::trim_space(raw)) {
        Some(u) if !u.host.is_empty() => format!("{}://{}", u.scheme, gotext::lossy(&u.host)),
        _ => "(unparseable URL)".into(),
    }
}

/// Go's `secretState`.
fn secret_state(v: &str) -> String {
    if v.is_empty() {
        "unset".into()
    } else {
        format!("set, {} chars", v.len())
    }
}

fn truncate(s: &str, n: usize) -> String {
    let s = one_line(s);
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s,
    }
}

/// Go's `nameList`: sorted, at most five shown.
fn name_list(mut names: Vec<String>) -> String {
    names.sort();
    if names.len() <= 5 {
        return names.join(", ");
    }
    format!("{} and {} more", names[..5].join(", "), names.len() - 5)
}

/// Go's `runner.get`: a GET with the check-up's bound; the status and at most 4 MiB of the body (a body that ends
/// badly is what was read).
fn get(ctx: &Ctx, target: &str, headers: &[(&str, &str)]) -> Option<(u16, Vec<u8>)> {
    let o = Opts { dial: Dial::Tcp, method: "GET", url: target, headers, body: None, tls: None };
    let deadline: gohttp::Deadline = Rc::new(Cell::new(Instant::now() + SHORT_TIMEOUT));
    let mut resp = gohttp::open(&o, ctx, &deadline).ok()?;
    let mut body = Vec::new();
    let mut buf = [0u8; 16384];
    while body.len() < 4 << 20 {
        let want = ((4 << 20) - body.len()).min(buf.len());
        match resp.body.read(&mut buf[..want]) {
            Ok(n) if n > 0 => body.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    Some((resp.status, body))
}

/// Go's `modelListed`: an exact id, or Ollama's "name:tag" form either way.
fn model_listed(ids: &[String], model: &str) -> bool {
    ids.iter().any(|id| id == model || id.starts_with(&format!("{model}:")) || model.starts_with(&format!("{id}:")))
}

static MODELS_DOC: Ty = Ty::Struct("", "", &[Field("data", &MODELS_DATA)]);
static MODELS_DATA: Ty = Ty::Slice(&MODELS_ITEM);
static MODELS_ITEM: Ty = Ty::Struct("", "", &[Field("id", &Ty::Str)]);

fn listed_ids(body: &[u8]) -> Vec<String> {
    let (v, _) = gostruct::unmarshal(&MODELS_DOC, body);
    let Val::Struct(top) = v else { return Vec::new() };
    let Some(Val::Slice(Some(items))) = top.into_iter().next() else { return Vec::new() };
    items
        .into_iter()
        .map(|it| match it {
            Val::Struct(f) => match f.into_iter().next() {
                Some(Val::Str(s)) => s,
                _ => String::new(),
            },
            _ => String::new(),
        })
        .collect()
}

impl Runner {
    fn backend(&mut self, ctx: &Ctx) {
        let cfg = crate::config::model_from_env(&*self.env);
        let wh = host_of(&cfg.base_url);
        let key = format!("API key {}", secret_state(&(self.env)("OPENAI_API_KEY")));
        let auth = format!("Bearer {}", cfg.api_key);
        let headers: Vec<(&str, &str)> = if cfg.api_key.is_empty() { Vec::new() } else { vec![("Authorization", auth.as_str())] };
        let got = get(ctx, &format!("{}/models", cfg.base_url.trim_end_matches('/')), &headers);
        let status = match got {
            None => {
                self.add(
                    "backend",
                    FAIL,
                    format!("model server at {wh} is not reachable ({key})"),
                    "start the model server, or point OPENAI_BASE_URL at it (Ollama: http://127.0.0.1:11434/v1)",
                );
                self.add("context_window", SKIP, "not checked: the backend is unreachable".into(), "");
                return;
            }
            Some((status, body)) => {
                if status == 401 || status == 403 {
                    self.add(
                        "backend",
                        FAIL,
                        format!("model server at {wh} refused the credentials (HTTP {status}; {key})"),
                        "set OPENAI_API_KEY to a key the server accepts (Ollama accepts any non-empty value)",
                    );
                } else if !(200..=299).contains(&status) {
                    self.add(
                        "backend",
                        FAIL,
                        format!("model server at {wh} answered HTTP {status} on /models"),
                        "check the server log; OPENAI_BASE_URL should end in /v1",
                    );
                } else {
                    let ids = listed_ids(&body);
                    let model = gotext::quote(&cfg.model);
                    if ids.is_empty() {
                        self.add(
                            "backend",
                            PASS,
                            format!("{wh} reachable; it did not list models, so {model} was not confirmed ({key})"),
                            "",
                        );
                    } else if model_listed(&ids, &cfg.model) {
                        self.add("backend", PASS, format!("{wh} reachable and lists model {model} ({key})"), "");
                    } else {
                        self.add(
                            "backend",
                            WARN,
                            format!("{wh} reachable but does not list model {model} ({} other models)", ids.len()),
                            "set OPENAI_MODEL to a listed name, or pull/load the model on the server",
                        );
                    }
                }
                status
            }
        };
        if !(200..=299).contains(&status) {
            self.add("context_window", SKIP, "not checked: the backend did not answer /models normally".into(), "");
            return;
        }
        let (mut n, mut source) = (crate::config::context_window_from_env(&*self.env), "SARINA_CONTEXT_WINDOW".to_owned());
        if n == 0 {
            let mut d = crate::llm::discover::Discoverer::default();
            d.timeout = SHORT_TIMEOUT;
            (n, source) = d.context_window(ctx, &cfg);
        }
        if n == 0 {
            self.add(
                "context_window",
                PASS,
                "the backend does not report its context window; Sarina infers it from the model name".into(),
                "",
            );
        } else if n < MIN_USABLE_WINDOW {
            self.add(
                "context_window",
                WARN,
                format!("context window is {n} tokens (from {source}); Sarina needs at least {MIN_USABLE_WINDOW}"),
                OLLAMA_HINT,
            );
        } else {
            self.add("context_window", PASS, format!("context window is {n} tokens (from {source})"), "");
        }
    }
}

// ---- sessions ----

#[derive(Default)]
struct SessionScan {
    dir: String,
    /// the error's text and whether it is "no such file"
    stat_err: Option<(String, bool)>,
    total: usize,
    parsed: usize,
    unreadable: Vec<String>,
    retired: Vec<String>,
    bad_shape: Vec<String>,
    leaks: Vec<String>,
}

fn scan_sessions(dir: &str) -> SessionScan {
    let mut s = SessionScan { dir: dir.to_owned(), ..SessionScan::default() };
    match std::fs::metadata(dir) {
        Err(e) => {
            s.stat_err = Some((crate::goerr::path("stat", dir.as_bytes(), &e), e.raw_os_error() == Some(libc::ENOENT)));
            return s;
        }
        Ok(m) if !m.is_dir() => {
            s.stat_err = Some(("not a directory".into(), false));
            return s;
        }
        Ok(_) => {}
    }
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            s.stat_err = Some((crate::goerr::path("open", dir.as_bytes(), &e), e.raw_os_error() == Some(libc::ENOENT)));
            return s;
        }
    };
    // Go's os.ReadDir: sorted by name, the type from the folder entry (a link is not a folder)
    let mut entries: Vec<(Vec<u8>, bool)> = rd
        .filter_map(Result::ok)
        .map(|e| {
            let name = std::os::unix::ffi::OsStrExt::as_bytes(e.file_name().as_os_str()).to_vec();
            (name, e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        })
        .collect();
    entries.sort();
    for (name, is_dir) in entries {
        if is_dir || !name.ends_with(b".json") {
            continue;
        }
        let shown = gotext::lossy(&name);
        s.total += 1;
        let path = crate::gopath::join(&[dir.as_bytes(), &name]);
        // a bounded regular file: a named pipe planted under the name is refused at once, not waited on
        let Ok(raw) = crate::session::readfile::read_file(&path) else {
            s.unreadable.push(shown);
            continue;
        };
        let (mut doc, err) = gojson::unmarshal_any(&raw);
        let generic = match (&mut doc, err) {
            (Any::Object(m), None) => std::mem::take(m),
            _ => {
                s.unreadable.push(shown);
                continue;
            }
        };
        if leaks_key_map(&generic) {
            s.leaks.push(shown.clone());
        }
        if !heimdall_shape(&generic) {
            s.bad_shape.push(shown.clone());
        }
        if crate::session::parse(&raw).is_err() {
            s.unreadable.push(shown);
            continue;
        }
        s.parsed += 1;
        if has_retired_tool(&generic) {
            s.retired.push(shown);
        }
    }
    s
}

/// Go's `heimdallShape`: messages an array of objects, each with a non-empty string role.
fn heimdall_shape(m: &BTreeMap<String, Any>) -> bool {
    let Some(Any::Array(msgs)) = m.get("messages") else { return false };
    msgs.iter().all(|x| match x {
        Any::Object(o) => matches!(o.get("role"), Some(Any::String(r)) if !r.is_empty()),
        _ => false,
    })
}

/// Go's `hasRetiredTool`: a retired tool name in a result message or a call.
fn has_retired_tool(m: &BTreeMap<String, Any>) -> bool {
    let Some(Any::Array(msgs)) = m.get("messages") else { return false };
    for x in msgs {
        let Any::Object(o) = x else { continue };
        if let Some(Any::String(n)) = o.get("name") {
            if crate::session::migrate::is_retired_tool(n) {
                return true;
            }
        }
        let Some(Any::Array(calls)) = o.get("tool_calls") else { continue };
        for c in calls {
            let Any::Object(c) = c else { continue };
            let Some(Any::Object(f)) = c.get("function") else { continue };
            if let Some(Any::String(n)) = f.get("name") {
                if crate::session::migrate::is_retired_tool(n) {
                    return true;
                }
            }
        }
    }
    false
}

fn leaks_key_map(m: &BTreeMap<String, Any>) -> bool {
    m.iter().any(|(k, v)| {
        let n: String = gotext::to_lower(k).chars().filter(|&c| c != '_' && c != '-').collect();
        if let Any::String(s) = v {
            if n == "apikey" && !gotext::trim_space(s).is_empty() {
                return true;
            }
        }
        leaks_key(v)
    })
}

/// Go's `leaksKey`: a non-empty value under a key named api_key, apiKey or api-key, at any depth.
fn leaks_key(v: &Any) -> bool {
    match v {
        Any::Object(m) => leaks_key_map(m),
        Any::Array(a) => a.iter().any(leaks_key),
        _ => false,
    }
}

/// Go's `writableDir`: access(2), creating nothing.
fn writable_dir(dir: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(dir) else { return false };
    // SAFETY: access(2) on a NUL-terminated path
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

impl Runner {
    fn session_dir(&self) -> String {
        if !self.o.session_dir.is_empty() {
            return self.o.session_dir.clone();
        }
        crate::config::default_runtime(&self.o.cwd).session_directory
    }

    fn sessions(&mut self) {
        let s = scan_sessions(&self.session_dir());
        if let Some((err, missing)) = &s.stat_err {
            if *missing {
                self.add(
                    "sessions",
                    WARN,
                    format!("session directory {} does not exist yet", s.dir),
                    "it is created by the first chat; use --session-dir or --cwd if sessions are kept elsewhere",
                );
            } else {
                self.add(
                    "sessions",
                    FAIL,
                    format!("session directory {} cannot be read ({err})", s.dir),
                    "fix the directory's permissions or pass --session-dir",
                );
            }
            self.add("session_retired_tools", SKIP, "no session directory to scan".into(), "");
            self.add("session_contract", SKIP, "no session directory to scan".into(), "");
            return;
        }
        let (mut write, mut st, mut remedy) = ("writable", PASS, String::new());
        if !writable_dir(&s.dir) {
            (write, st, remedy) =
                ("NOT writable", FAIL, format!("give this user write access to {}; sessions cannot be saved without it", s.dir));
        }
        let mut finding = format!("{} is {write}; {} session files, {} parse", s.dir, s.total, s.parsed);
        if !s.unreadable.is_empty() {
            finding.push_str(&format!(", {} unreadable: {}", s.unreadable.len(), name_list(s.unreadable.clone())));
            if st == PASS {
                st = WARN;
                remedy = "inspect or delete the named files (a damaged file is skipped; Sarina rewrites a session on its next turn)".into();
            }
        }
        self.add("sessions", st, finding, &remedy);
        if s.retired.is_empty() {
            self.add("session_retired_tools", PASS, "no retired tool names in saved sessions".into(), "");
        } else {
            self.add(
                "session_retired_tools",
                WARN,
                format!("{} session file(s) still contain retired tool names: {}", s.retired.len(), name_list(s.retired.clone())),
                "run `sarina session-migrate` (use --dry-run first to preview)",
            );
        }
        if !s.leaks.is_empty() {
            self.add(
                "session_contract",
                FAIL,
                format!("an api_key value is stored in {} session file(s): {} (value not shown)", s.leaks.len(), name_list(s.leaks.clone())),
                "SECRET LEAK: rotate that key, delete or redact the named files, and report it; Sarina never writes the key, so something else did",
            );
        } else if !s.bad_shape.is_empty() {
            self.add(
                "session_contract",
                WARN,
                format!(
                    "{} session file(s) do not have the shape Heimdall reads (messages as objects with a role): {}",
                    s.bad_shape.len(),
                    name_list(s.bad_shape.clone())
                ),
                "Heimdall reports these as agent_session_unknown_shape (Heimdall docs/INTEGRATION-FAILURES.md); compare with docs/spec/01-agent-core.md section 4.2",
            );
        } else {
            self.add("session_contract", PASS, format!("{} session file(s) match what Heimdall reads and hold no api_key", s.parsed), "");
        }
    }
}

// ---- tiers ----

fn tier_id(name: &str) -> String {
    let mut out = String::from("tier_");
    let mut run = false;
    for c in gotext::to_lower(name).chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            out.push(c);
            run = false;
        } else if !run {
            out.push('_');
            run = true;
        }
    }
    out
}

impl Runner {
    fn tiers(&mut self, ctx: &Ctx) {
        use crate::tiers::config::KIND_CLAUDE_CLI;
        let env = self.env.clone();
        let getenv = |k: &str| env(k);
        let path = crate::tiers::config::path(&self.home, Some(&getenv));
        let (cfg, err) = crate::tiers::config::load(&self.home, Some(&getenv));
        if let Some(e) = err {
            self.add(
                "tiers_config",
                WARN,
                format!("tiers config is invalid, so Sarina stays on the local backend only: {e}"),
                "fix the named file (tiers.json): JSON syntax, tier kinds, api_key_env must be a variable NAME",
            );
            return;
        }
        if path.is_empty() || std::fs::metadata(&path).is_err() {
            self.add("tiers_config", PASS, "tiers.json absent: a single local tier (the OPENAI_* backend)".into(), "");
            return;
        }
        let names: Vec<&str> = cfg.tiers.iter().map(|t| t.name.as_str()).collect();
        let finding = format!("tiers.json present and valid: {} tier(s): {}", cfg.tiers.len(), names.join(", "));
        let warnings = cfg.warnings.clone().unwrap_or_default();
        if warnings.is_empty() {
            self.add("tiers_config", PASS, finding, "");
        } else {
            self.add("tiers_config", WARN, format!("{finding}; {}", warnings.join("; ")), "review the warnings in tiers.json");
        }
        let env2 = self.env.clone();
        let opts = crate::tiers::router::Options {
            config: cfg,
            base: crate::config::model_from_env(&*self.env),
            getenv: Box::new(move |k: &str| env2(k)),
            health_timeout: SHORT_TIMEOUT,
            ..Default::default()
        };
        let Ok(router) = crate::tiers::router::Router::new(opts) else { return };
        for s in router.status(ctx) {
            let id = tier_id(&s.name);
            let mut wh = s.kind.clone();
            if !s.base_url.is_empty() {
                wh.push_str(&format!(" at {}", host_of(&s.base_url)));
            }
            if !s.api_key_env.is_empty() {
                wh.push_str(&format!(", key variable {} {}", s.api_key_env, if s.key_set { "set" } else { "unset" }));
            }
            if !s.enabled {
                self.add(&id, SKIP, format!("{wh}: disabled in tiers.json"), "");
            } else if s.healthy && s.kind == KIND_CLAUDE_CLI {
                self.add(&id, PASS, format!("{wh}: claude executable {} found", gotext::quote(&s.cli_path)), "");
            } else if s.healthy {
                self.add(&id, PASS, format!("{wh}: healthy"), "");
            } else if s.kind == KIND_CLAUDE_CLI {
                self.add(
                    &id,
                    WARN,
                    format!("{wh}: claude executable {} was not found on PATH", gotext::quote(&s.cli_path)),
                    "install the claude CLI or set cli_path in tiers.json; until then this tier is skipped",
                );
            } else {
                self.add(
                    &id,
                    WARN,
                    format!("{wh}: {}", s.why),
                    "start that machine or server, or disable the tier; Sarina falls through to the next tier meanwhile",
                );
            }
        }
    }
}

// ---- the operator's extension files ----

fn presence(path: &str) -> &'static str {
    match std::fs::metadata(path) {
        Ok(m) if !m.is_dir() => "present",
        _ => "absent",
    }
}

impl Runner {
    fn extensions(&mut self) {
        let checker =
            Arc::new(crate::trust::Checker::new(crate::trust::default_dir(&|k: &str| std::env::var(k).unwrap_or_default()).as_bytes()));
        let trust: Arc<dyn Fn(&str) -> String + Send + Sync> = Arc::new(move |p: &str| checker.refuse(p.as_bytes()));
        let home = self.home.clone();
        let sarina = |name: &str| gotext::lossy(&crate::gopath::join(&[home.as_bytes(), b".sarina", name.as_bytes()]));

        let opt = crate::skills::discover::Options {
            home: self.home.clone(),
            getenv: Some(self.env.clone()),
            workspace: self.o.cwd.clone(),
            system_dir: self.o.system_skills_dir.clone(),
            trust: Some(trust.clone()),
            ..Default::default()
        };
        let cat = crate::skills::discover::discover(&opt);
        let n =
            crate::skills::discover::roots(&opt).iter().filter(|r| std::fs::metadata(&r.dir).map(|m| m.is_dir()).unwrap_or(false)).count();
        let finding = format!("{} skill(s) loaded from {n} existing director(ies)", cat.skills.len());
        if cat.problems.is_empty() {
            self.add("ext_skills", PASS, finding, "");
        } else {
            let ps: Vec<String> =
                cat.problems.iter().map(|p| format!("{}: {}", gotext::lossy(&crate::gopath::base(p.path.as_bytes())), p.message)).collect();
            self.add(
                "ext_skills",
                WARN,
                format!("{finding}; {} unusable: {}", ps.len(), name_list(ps)),
                "fix or remove the named skill files (`sarina skill list` shows the same problems)",
            );
        }

        let mc = crate::mcp::config::load(&crate::mcp::config::LoadOptions {
            home: self.home.clone(),
            getenv: Some(self.env.clone()),
            workspace: self.o.cwd.clone(),
            trust_workspace: false,
            trust: Some(trust.clone()),
        });
        let mpath = sarina("mcp.json");
        if mc.problems.is_empty() {
            self.add("ext_mcp", PASS, format!("mcp.json {}; {} server(s) defined", presence(&mpath), mc.servers.len()), "");
        } else {
            self.add(
                "ext_mcp",
                WARN,
                format!("mcp.json {}; problems: {}", presence(&mpath), mc.problems.join("; ")),
                "fix the JSON in the named file; a broken MCP file only disables MCP servers",
            );
        }

        let env = self.env.clone();
        let pol = crate::policy::load(&crate::policy::Options {
            home: self.home.clone(),
            getenv: &|k: &str| env(k),
            workspace: self.o.cwd.clone(),
            trust: Some(&*trust),
        });
        let ppath = sarina("policy.json");
        if !pol.fatal.is_empty() {
            self.add(
                "ext_policy",
                FAIL,
                format!("operator policy would stop startup: {}", pol.fatal.join("; ")),
                "repair the JSON in the named policy file; do not delete denials just to get started",
            );
        } else if !pol.notices.is_empty() {
            self.add(
                "ext_policy",
                WARN,
                format!("policy.json {}; {} notice(s): {}", presence(&ppath), pol.notices.len(), pol.notices.join("; ")),
                "read the notices; a workspace policy file can only tighten the operator policy",
            );
        } else {
            self.add(
                "ext_policy",
                PASS,
                format!("policy.json {}; {} denial rule(s), trusted={}", presence(&ppath), pol.deny_rules(), pol.trusted),
                "",
            );
        }
    }
}

// ---- Limbo, Heimdall, the ports ----

/// The first checks Limbo's runbook gives for each code.
fn limbo_remedy(code: &str) -> Option<&'static str> {
    Some(match code {
        "virt_unavailable" => "on the host run `sudo limbo doctor` and `systemctl status libvirtd virtqemud`",
        "profile_invalid" => {
            "the operator fixes the profile file under /etc/limbo/validation/ (an invalid profile is silently absent from the list)"
        }
        "profile_unknown" => "pick a name from limbo_profiles; the operator adds missing profiles",
        "image_missing" => "restore the base image at the path named in the profile",
        "image_mismatch" => "re-verify the image; update the profile sha256 only if the change is intended",
        "boot_failed" => "retry once; then `journalctl -u limbo` and `virsh -c qemu:///system list --all`",
        "boot_timeout" => "retry; check host load and that /dev/kvm exists",
        "host_resources" => "free disk or memory (`df -h /var/lib/limbo`, `free -m`) and retry",
        "busy" => "wait a minute and retry (two runs at a time)",
        "bad_request" => "the request was malformed; this is a client bug, report it",
        "run_unknown" => "start the run again",
        _ => return None,
    })
}

/// Go's `codeRE`, `^([a-z][a-z_]+):`.
fn error_code(msg: &str) -> Option<&str> {
    let b = msg.as_bytes();
    if !b.first().is_some_and(u8::is_ascii_lowercase) {
        return None;
    }
    let end = b.iter().position(|c| !(c.is_ascii_lowercase() || *c == b'_')).unwrap_or(b.len());
    (end >= 2 && b.get(end) == Some(&b':')).then(|| &msg[..end])
}

impl Runner {
    fn limbo(&mut self) {
        let var = crate::limbo::ENV_VAR;
        let sock = gotext::trim_space(&(self.env)(var)).to_owned();
        if sock.is_empty() {
            self.add("limbo", SKIP, format!("{var} is unset: Limbo validation tools are inactive"), "");
            return;
        }
        let base = &sock[sock.rfind(['/', '\\']).map_or(0, |i| i + 1)..];
        if base != "agent.sock" {
            self.add(
                "limbo",
                FAIL,
                format!("{var} names {}, not an agent.sock; Sarina refuses any other socket", gotext::quote(base)),
                &format!("set it to /run/limbo/agent.sock ({LIMBO_DOCS} section A)"),
            );
            return;
        }
        let st = match std::fs::metadata(&sock) {
            Err(e) => {
                let why = match e.raw_os_error() {
                    Some(libc::ENOENT) => "no such file".to_owned(),
                    Some(libc::EACCES | libc::EPERM) => "permission denied".to_owned(),
                    _ => truncate(&crate::goerr::path("stat", sock.as_bytes(), &e), 120),
                };
                self.add(
                    "limbo",
                    FAIL,
                    format!("Limbo socket {sock} cannot be used ({why})"),
                    &format!(
                        "start it: `sudo systemctl start limbo`, then `ls -l /run/limbo/` and `journalctl -u limbo` ({LIMBO_DOCS} section A)"
                    ),
                );
                return;
            }
            Ok(m) => m,
        };
        let perm = st.permissions().mode() & 0o777;
        if !st.file_type().is_socket() {
            self.add(
                "limbo",
                FAIL,
                format!("{sock} exists but is not a unix socket"),
                &format!("point {var} at the agent socket, normally /run/limbo/agent.sock ({LIMBO_DOCS} section A)"),
            );
            return;
        }
        if perm & 0o002 != 0 {
            self.add(
                "limbo",
                FAIL,
                format!("{sock} is world-writable (mode {perm:04o}): any local user could submit commands"),
                &format!("the packaged socket is root:limbo-agent mode 0660; restart `limbo serve` with the packaged unit ({LIMBO_DOCS} section A)"),
            );
            return;
        }
        let c = crate::limbo::Client { path: sock.clone(), timeout: SHORT_TIMEOUT, ..Default::default() };
        let listing = match c.describe_profiles_and_skipped(&*self.env) {
            Ok(l) => l,
            Err(e) => {
                let msg = e.0;
                if let Some(code) = error_code(&msg) {
                    match limbo_remedy(code) {
                        Some(rem) => self.add(
                            "limbo",
                            FAIL,
                            format!("Limbo answered with error code {code}: {}", truncate(&msg, 200)),
                            &format!("{rem} ({LIMBO_DOCS})"),
                        ),
                        None => self.add(
                            "limbo",
                            FAIL,
                            format!("Limbo answered with an error: {}", truncate(&msg, 200)),
                            &format!("look the code up in {LIMBO_DOCS}"),
                        ),
                    }
                    return;
                }
                let rem = if msg.contains("permission denied") {
                    format!("add Sarina's user to the limbo-agent group (`sudo usermod -aG limbo-agent <user>`) and start a new login or service process ({LIMBO_DOCS} section A)")
                } else if msg.contains("refused") {
                    format!("stale socket from a crashed process: `sudo systemctl restart limbo` ({LIMBO_DOCS} section A)")
                } else {
                    format!("check `systemctl status limbo` and `journalctl -u limbo -n 50` ({LIMBO_DOCS} section A)")
                };
                self.add("limbo", FAIL, format!("socket exists but GET /v1/profiles failed: {}", truncate(&msg, 200)), &rem);
                return;
            }
        };
        let extra = if perm & 0o007 != 0 {
            format!(" (note: mode {perm:04o} gives other users access; the packaged mode is 0660)")
        } else {
            String::new()
        };
        let profiles = listing.profiles.len();
        if !listing.skipped.is_empty() {
            let shown = &listing.skipped[..listing.skipped.len().min(5)];
            let items: Vec<String> =
                shown.iter().map(|s| format!("{} [{}]", if s.name.is_empty() { "(unnamed)" } else { &s.name }, s.code)).collect();
            let more = if listing.skipped.len() > shown.len() {
                format!(" and {} more", listing.skipped.len() - shown.len())
            } else {
                String::new()
            };
            let mut rem = truncate(&shown[0].remedy, 200);
            if rem.is_empty() {
                rem = "fix the profile files under /etc/limbo/validation/".into();
            }
            self.add(
                "limbo",
                WARN,
                format!(
                    "Limbo answers with {profiles} usable profile(s) but skipped {} invalid: {}{more}{extra}",
                    listing.skipped.len(),
                    items.join(", ")
                ),
                &format!("operator remedy: {rem} ({LIMBO_DOCS})"),
            );
            return;
        }
        if profiles == 0 {
            self.add(
                "limbo",
                WARN,
                format!("Limbo answers but lists 0 validation profiles{extra}"),
                &format!("a profile that is malformed or not disposable is silently absent: check /etc/limbo/validation/ and `journalctl -u limbo` ({LIMBO_DOCS} known gaps)"),
            );
            return;
        }
        self.add("limbo", PASS, format!("agent socket answers; {profiles} profile(s){extra}"), "");
    }

    fn heimdall(&mut self, ctx: &Ctx) {
        const DOCS: &str = "Heimdall docs/INTEGRATION-FAILURES.md";
        let var = crate::heimdall::ENV_VAR;
        let raw = crate::heimdall::url(&*self.env);
        if raw.is_empty() {
            self.add(
                "heimdall",
                SKIP,
                format!("{var} is unset: no external governance review; memory writes rely on Sarina's own in-process scan (fail-open by design)"),
                "",
            );
            return;
        }
        let u = match crate::gourl::parse(&raw) {
            Some(u) if !u.host.is_empty() && (u.scheme == "http" || u.scheme == "https") => u,
            _ => {
                self.add(
                    "heimdall",
                    WARN,
                    format!("{var} is set ({}) but is not a usable http(s) URL; the review is skipped (fail-open)", secret_state(&raw)),
                    &format!("set it to Heimdall's review URL, e.g. http://127.0.0.1:8930/v1/review ({DOCS})"),
                );
                return;
            }
        };
        let wh = format!("{}://{}", u.scheme, gotext::lossy(&u.host));
        // Heimdall's cheap health endpoint, on the same address; no memory text is sent
        match get(ctx, &format!("{wh}/healthz"), &[]) {
            None => self.add(
                "heimdall",
                WARN,
                format!("Heimdall at {wh} is not reachable; reviews are skipped (fail-open: memory writes still pass Sarina's own scan)"),
                &format!("start heimdall or correct {var}; `journalctl -u heimdall -n 50` ({DOCS}, section 'Sarina consulting Heimdall')"),
            ),
            Some((status, _)) if !(200..=299).contains(&status) => self.add(
                "heimdall",
                WARN,
                format!("Heimdall at {wh} answered HTTP {status} on /healthz; the address may not be Heimdall's API"),
                &format!("check {var} matches Heimdall's api_addr, default 127.0.0.1:8930 ({DOCS})"),
            ),
            Some(_) if !gotext::lossy(&u.path).trim_end_matches('/').ends_with("/v1/review") => self.add(
                "heimdall",
                WARN,
                format!("Heimdall at {wh} is healthy, but the URL path is not /v1/review, so consultations will fail open"),
                &format!("set {var} to http://HOST:PORT/v1/review ({DOCS})"),
            ),
            Some(_) => self.add(
                "heimdall",
                PASS,
                format!("Heimdall at {wh} answers /healthz; governance review is active (a 'deny' verdict blocks the write)"),
                "",
            ),
        }
    }

    fn ports(&mut self) {
        let mut ports = self.o.ports.clone().unwrap_or_else(|| vec![("service".into(), 8899), ("gui".into(), 8765)]);
        ports.sort();
        let parts: Vec<String> = ports
            .iter()
            .map(|(n, p)| {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], *p));
                let state = if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() { "in use" } else { "free" };
                format!("{n} {p} {state}")
            })
            .collect();
        self.add("ports", PASS, format!("{} (informational: 'in use' is normal while that server runs)", parts.join("; ")), "");
    }
}
