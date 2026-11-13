//! Go: `service`: the integration service (`sarina service`, port 8899): a small bearer-authenticated JSON API that
//! Heimdall and scripts use to run agent turns in pooled sessions. The wire contract is docs/spec/05-gui-service.md
//! section 2: `tool_allowlist: []` means no tools and the create answer reports the real count; the pool is a bounded
//! LRU and an evicted id is a 404 (the caller resumes from disk with `resume_session_id`); a caller can never be
//! granted more than the operator's ceiling.
//!
//! Each pooled session's agent lives on a thread of its own (the agent and its transport stay on one thread), which also
//! serialises the session's turns as Go's per-entry mutex does. The HTTP side is `gohttpd` (Go's net/http server).

use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::agent::run::RunResult;
use crate::cli::Getenv;
use crate::config::{default_runtime, ModelConfig, DEFAULT_TIMEOUT_SECONDS};
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gohttpd::{self, BodyErr, Exchange, Response, Route};
use crate::gopath;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;
use crate::runkit;
use crate::tiers::config::EscalationOptions;

/// Go's `maxBody`.
const MAX_BODY: usize = 8 << 20;
/// Go's `bodyReadTimeout`: a request's body must arrive within this (a read deadline set when a request with a body
/// starts, cleared once the body is read).
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Go's `Config`: the operator's configuration. Zero values mean "nothing granted".
#[derive(Clone)]
pub struct Config {
    /// bearer token; empty = no authentication
    pub token: String,
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub session_dir: String,
    pub scratchpad_root: String,
    /// pool bound; <= 0 means 20
    pub max_sessions: i64,
    pub permit_shell: bool,
    pub permit_write: bool,
    pub permit_backend_override: bool,
    pub cwd_roots: Vec<String>,
    /// "" = the user's real home
    pub home: String,
    pub getenv: Getenv,
    pub discover: bool,
    pub escalation: Option<EscalationOptions>,
}

/// A turn handed to a session's thread.
struct Job {
    prompt: String,
    resume: Option<String>,
    ctx: Ctx,
    reply: Sender<Response>,
}

struct Entry {
    tx: Sender<Job>,
    used: u64,
}

/// Go's `Server`: the session pool.
pub struct Server {
    cfg: Config,
    pool: Mutex<(BTreeMap<String, Entry>, u64)>,
}

impl Server {
    /// Go's `New`.
    pub fn new(mut cfg: Config) -> Server {
        if cfg.max_sessions <= 0 {
            cfg.max_sessions = 20;
        }
        Server { cfg, pool: Mutex::new((BTreeMap::new(), 0)) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (BTreeMap<String, Entry>, u64)> {
        self.pool.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Go's `Active`.
    pub fn active(&self) -> usize {
        self.lock().0.len()
    }

    /// Go's `add`: the least recently used entries evicted while the pool is full.
    fn add(&self, id: String, tx: Sender<Job>) {
        let mut g = self.lock();
        while g.0.len() as i64 >= self.cfg.max_sessions {
            let Some(oldest) = g.0.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            g.0.remove(&oldest);
        }
        g.1 += 1;
        let used = g.1;
        g.0.insert(id, Entry { tx, used });
    }

    /// Go's `get`: the entry's channel, marked used.
    fn get(&self, id: &str) -> Option<Sender<Job>> {
        let mut g = self.lock();
        g.1 += 1;
        let tick = g.1;
        let e = g.0.get_mut(id)?;
        e.used = tick;
        Some(e.tx.clone())
    }

    /// The HTTP surface (Go's `Handler`: the browser guard around the mux).
    pub fn handle(&self, ex: &mut Exchange) -> Response {
        if ex.req.content_length != 0 {
            ex.set_read_deadline(Some(Instant::now() + BODY_READ_TIMEOUT));
        }
        if let Some(r) = browser_guard(&ex.req) {
            return r;
        }
        match gohttpd::route(&ex.req) {
            Route::Star => {
                let mut r = Response::new(400);
                if ex.req.proto_at_least(1, 1) {
                    r.set("Connection", "close");
                }
                r
            }
            Route::Redirect(to) => gohttpd::redirect(&ex.req, &to),
            Route::NotFound => gohttpd::not_found_text(),
            Route::Healthz => json(200, &obj(vec![("status", s("ok")), ("active_sessions", n(self.active() as f64))])),
            route => {
                if let Some(r) = self.auth(&ex.req) {
                    return r;
                }
                match route {
                    Route::Create => self.create(ex),
                    Route::Chat(id) => self.chat(ex, &id),
                    Route::Evict(id) => self.evict(&id),
                    _ => detail(404, "not found"),
                }
            }
        }
    }

    /// Go's `auth`: the bearer token on everything but /healthz.
    fn auth(&self, req: &gohttpd::Request) -> Option<Response> {
        if self.cfg.token.is_empty() {
            return None;
        }
        let h = req.header("Authorization");
        let ok = h.starts_with(b"Bearer ");
        let got = h.strip_prefix(b"Bearer ").unwrap_or(h);
        use sha2::Digest;
        let want = sha2::Sha256::digest(self.cfg.token.as_bytes());
        let have = sha2::Sha256::digest(got);
        let same = want.iter().zip(have.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        if !ok || !same {
            let mut r = detail(401, "missing or invalid bearer token");
            r.set("Www-Authenticate", "Bearer");
            return Some(r);
        }
        None
    }

    fn home_dir(&self) -> String {
        if !self.cfg.home.is_empty() {
            return self.cfg.home.clone();
        }
        std::env::var("HOME").unwrap_or_default()
    }

    /// Go's `expand`: a leading `~` is the home folder.
    fn expand(&self, p: &str) -> String {
        if p == "~" || p.starts_with("~/") || p.starts_with("~\\") {
            return lossy(&gopath::join(&[self.home_dir().as_bytes(), &p.as_bytes()[1..]]));
        }
        p.to_owned()
    }

    fn create(&self, ex: &mut Exchange) -> Response {
        let v = match read_body(ex, &CREATE) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let f = |name: &str| gostruct::field(&CREATE, &v, name);
        let Some(cwd) = opt_str(f("cwd")) else { return invalid("cwd", "field required", "value_error.missing") };
        let cwd = self.expand(gotext::trim_space(&cwd));
        if cwd.is_empty() {
            return detail(400, "cwd must be a non-empty directory path");
        }
        if !std::fs::metadata(&cwd).map(|m| m.is_dir()).unwrap_or(false) {
            return detail(400, "cwd does not exist or is not a directory");
        }
        let allow_shell = opt_bool(f("allow_shell")) == Some(true);
        let allow_write = opt_bool(f("allow_write")) == Some(true);
        if allow_shell && !self.cfg.permit_shell {
            return detail(403, "allow_shell is not permitted by this service's configuration");
        }
        if allow_write && !self.cfg.permit_write {
            return detail(403, "allow_write is not permitted by this service's configuration");
        }
        let (mut model, mut base_url, mut api_key) = (self.cfg.model.clone(), self.cfg.base_url.clone(), self.cfg.api_key.clone());
        if let Some(m) = opt_str(f("model")).filter(|m| !m.is_empty()) {
            model = m;
        }
        if let Some(b) = opt_str(f("base_url")).filter(|b| !b.is_empty() && *b != self.cfg.base_url) {
            if !self.cfg.permit_backend_override {
                return detail(403, "base_url is not permitted by this service's configuration");
            }
            base_url = b;
        }
        if let Some(k) = opt_str(f("api_key")).filter(|k| !k.is_empty() && *k != self.cfg.api_key) {
            if !self.cfg.permit_backend_override {
                return detail(403, "api_key is not permitted by this service's configuration");
            }
            api_key = k;
        }
        if !self.cfg.cwd_roots.is_empty() && !self.cfg.cwd_roots.iter().any(|root| inside(root, &cwd)) {
            return detail(403, "cwd is outside the directories this service may work in");
        }
        let (mut session_dir, mut scratch) = (self.cfg.session_dir.clone(), self.cfg.scratchpad_root.clone());
        if let Some(d) = opt_str(f("session_directory")).filter(|d| !d.is_empty()) {
            let d = self.expand(&d);
            if !inside(&self.cfg.session_dir, &d) {
                return detail(403, "session_directory must be inside the service's session directory");
            }
            session_dir = d;
        }
        if let Some(d) = opt_str(f("scratchpad_root")).filter(|d| !d.is_empty()) {
            let d = self.expand(&d);
            if !inside(&self.cfg.scratchpad_root, &d) {
                return detail(403, "scratchpad_root must be inside the service's scratchpad root");
            }
            scratch = d;
        }
        let mut m =
            ModelConfig { model, base_url, api_key: api_key.clone(), timeout_seconds: DEFAULT_TIMEOUT_SECONDS, ..Default::default() };
        if let Some(t) = opt_f64(f("temperature")) {
            m.temperature = t;
        }
        if let Some(t) = opt_f64(f("timeout_seconds")) {
            m.timeout_seconds = t;
        }
        let mut rt = default_runtime(&cwd);
        rt.session_directory = session_dir.clone();
        rt.scratchpad_root = scratch;
        if let Some(t) = opt_i64(f("max_turns")) {
            rt.max_turns = t;
        }
        rt.permissions.allow_shell_commands = allow_shell;
        rt.permissions.allow_file_write = allow_write;
        let b = &mut rt.budget_config;
        b.max_total_tokens = opt_i64(f("max_total_tokens"));
        b.max_input_tokens = opt_i64(f("max_input_tokens"));
        b.max_output_tokens = opt_i64(f("max_output_tokens"));
        b.max_reasoning_tokens = opt_i64(f("max_reasoning_tokens"));
        b.max_tool_calls = opt_i64(f("max_tool_calls"));
        b.max_delegated_tasks = opt_i64(f("max_delegated_tasks"));
        b.max_model_calls = opt_i64(f("max_model_calls"));
        b.max_session_turns = opt_i64(f("max_session_turns"));
        b.max_total_cost_usd = opt_f64(f("max_total_cost_usd"));
        let allowlist = match f("tool_allowlist") {
            Some(Val::Slice(Some(items))) => {
                Some(items.iter().map(|x| if let Val::Str(s) = x { s.clone() } else { String::new() }).collect())
            }
            _ => None,
        };
        let spec = WorkerSpec {
            model: m,
            runtime: rt,
            allowlist,
            home: self.cfg.home.clone(),
            getenv: self.cfg.getenv.clone(),
            discover: self.cfg.discover,
            escalation: self.cfg.escalation.clone(),
            session_dir,
            build_scrub: runkit::scrubber(&[&api_key, &self.cfg.token]),
            scrub: runkit::scrubber(&[&api_key, &self.cfg.api_key, &self.cfg.token]),
        };
        let (tools, tx) = match start_worker(spec) {
            Ok(x) => x,
            Err(e) => return detail(400, &e),
        };
        let id = crate::agent::run::new_session_id();
        self.add(id.clone(), tx);
        json(200, &obj(vec![("session_id", s(&id)), ("tools", n(tools as f64))]))
    }

    fn chat(&self, ex: &mut Exchange, id: &str) -> Response {
        let Some(tx) = self.get(id) else {
            return detail(
                404,
                "session not found (it may have been evicted); create a new one and pass the agent session_id as resume_session_id to continue",
            );
        };
        let v = match read_body(ex, &CHAT) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let Some(prompt) = opt_str(gostruct::field(&CHAT, &v, "prompt")) else {
            return invalid("prompt", "field required", "value_error.missing");
        };
        if prompt.is_empty() {
            return invalid("prompt", "ensure this value has at least 1 characters", "value_error.any_str.min_length");
        }
        let prompt = gotext::trim_space(&prompt).to_owned();
        if prompt.is_empty() {
            return detail(400, "prompt must not be blank");
        }
        let resume = opt_str(gostruct::field(&CHAT, &v, "resume_session_id"));
        let ctx = Ctx::background();
        let (rtx, rrx) = channel();
        if tx.send(Job { prompt, resume, ctx: ctx.clone(), reply: rtx }).is_err() {
            return run_failed("the run produced no result");
        }
        // Go's background read: a client that hangs up while the turn runs (or waits for the session) cancels it
        let mut watching = true;
        loop {
            match rrx.recv_timeout(Duration::from_millis(50)) {
                Ok(r) => return r,
                Err(RecvTimeoutError::Timeout) => {
                    if watching {
                        match ex.peer_gone() {
                            Some(true) => {
                                ctx.cancel();
                                watching = false;
                            }
                            Some(false) => watching = false,
                            None => {}
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return run_failed("the run produced no result"),
            }
        }
    }

    fn evict(&self, id: &str) -> Response {
        if self.lock().0.remove(id).is_none() {
            return detail(404, "session not found");
        }
        json(200, &obj(vec![("evicted", Any::Bool(true))]))
    }
}

// ---- a session's thread -------------------------------------------------------------------------------------------------

struct WorkerSpec {
    model: ModelConfig,
    runtime: crate::config::RuntimeConfig,
    allowlist: Option<Vec<String>>,
    home: String,
    getenv: Getenv,
    discover: bool,
    escalation: Option<EscalationOptions>,
    session_dir: String,
    build_scrub: runkit::Scrub,
    scrub: runkit::Scrub,
}

/// Builds the session's agent on a thread of its own, which then runs the session's turns one at a time; the tool count
/// and the channel for turns, or the build's error (scrubbed).
fn start_worker(spec: WorkerSpec) -> Result<(usize, Sender<Job>), String> {
    let (ready_tx, ready_rx) = channel::<Result<usize, String>>();
    let (tx, rx) = channel::<Job>();
    let spawned = std::thread::Builder::new().name("session".into()).stack_size(8 << 20).spawn(move || {
        let built = runkit::build(runkit::Spec {
            model: spec.model,
            runtime: spec.runtime,
            allowlist: spec.allowlist,
            prompt: Default::default(),
            home: spec.home,
            getenv: Some(spec.getenv),
            client: None,
            approver: None,
            discover: spec.discover,
            use_tiers: false,
            escalation: spec.escalation,
            subagents: false,
            builtins: None,
            trust: None,
        });
        let mut agent = match built {
            Ok(b) => b.agent,
            Err(e) => {
                let _ = ready_tx.send(Err((spec.build_scrub)(&e)));
                return;
            }
        };
        let _ = ready_tx.send(Ok(agent.tool_count()));
        let mut last = String::new();
        turns(&mut agent, rx, &spec.session_dir, &spec.scrub, &mut last);
    });
    if let Err(e) = spawned {
        return Err(format!("cannot start the session: {e}"));
    }
    match ready_rx.recv() {
        Ok(Ok(n)) => Ok((n, tx)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("cannot start the session".into()),
    }
}

/// Go's `chat` after the session's lock: the session to resume, a local slash command, the run.
fn turns(agent: &mut crate::agent::run::Agent, rx: Receiver<Job>, session_dir: &str, scrub: &runkit::Scrub, last: &mut String) {
    for job in rx {
        let mut resume_id = last.clone();
        if let Some(r) = job.resume.as_deref().filter(|r| !r.is_empty()) {
            resume_id = r.to_owned();
        }
        let mut stored = None;
        if !resume_id.is_empty() {
            if !valid_id(&resume_id) {
                let _ = job.reply.send(detail(404, "session to resume not found"));
                continue;
            }
            match crate::session::readfile::load_by_id(session_dir, &resume_id) {
                Ok(s) => stored = Some(s),
                Err(_) => {
                    let _ = job.reply.send(detail(404, "session to resume not found in this session's directory"));
                    continue;
                }
            }
        }
        if let Some(text) = crate::cli::chat::local_slash(&job.prompt) {
            let usage = obj(vec![
                ("input_tokens", n(0.0)),
                ("output_tokens", n(0.0)),
                ("cache_creation_input_tokens", n(0.0)),
                ("cache_read_input_tokens", n(0.0)),
                ("reasoning_tokens", n(0.0)),
                ("total_tokens", n(0.0)),
            ]);
            let transcript = Any::Array(vec![
                obj(vec![("role", s("user")), ("content", s(&job.prompt))]),
                obj(vec![("role", s("assistant")), ("content", s(&text))]),
            ]);
            let res = obj(vec![
                ("final_output", s(&text)),
                ("turns", n(0.0)),
                ("tool_calls", n(0.0)),
                ("session_id", s(&resume_id)),
                ("usage", usage),
                ("total_cost_usd", n(0.0)),
                ("stop_reason", s("slash_command")),
                ("transcript", transcript),
            ]);
            let _ = job.reply.send(json(200, &res));
            continue;
        }
        let (res, err) = match stored {
            Some(st) => agent.resume(&job.ctx, &job.prompt, st),
            None => agent.run(&job.ctx, &job.prompt),
        };
        let resp = match (res, err) {
            (Some(res), None) => {
                *last = res.session_id.clone();
                result_response(&res, scrub)
            }
            (_, Some(e)) => {
                let typ = if e.starts_with("saving session: ") || e.starts_with("cannot create scratchpad: ") {
                    "wrapError"
                } else {
                    "errorString"
                };
                json(500, &obj(vec![("error", s(&scrub(&e))), ("error_type", s(typ))]))
            }
            (None, None) => run_failed("the run produced no result"),
        };
        let _ = job.reply.send(resp);
    }
}

fn result_response(res: &RunResult, scrub: &runkit::Scrub) -> Response {
    let sc = |x: &str| scrub(x);
    match runkit::result_json(res, Some(&sc)) {
        Ok(body) => {
            let mut r = Response::new(200);
            r.set("Content-Type", "application/json");
            r.body = format!("{body}\n").into_bytes();
            r
        }
        Err(e) => run_failed(&e.to_string()),
    }
}

fn run_failed(msg: &str) -> Response {
    json(500, &obj(vec![("error", s(msg)), ("error_type", s("errorString"))]))
}

/// Go's `idRe`: `^[A-Za-z0-9_-]{1,64}$`.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// ---- the guard and the bodies --------------------------------------------------------------------------------------------

/// Go's `browserGuard`: what only a web page could send is refused (a Host that is not a loopback name, a cross-site
/// request); programs on this computer send a loopback Host and no Origin.
fn browser_guard(req: &gohttpd::Request) -> Option<Response> {
    let raw = String::from_utf8_lossy(&req.host).into_owned();
    let mut host = raw.as_str();
    if let Some((h, _)) = split_host_port(host) {
        host = h;
    }
    let host = host.trim_matches(|c| c == '[' || c == ']');
    let ip = parse_ip(host);
    if !(gotext::equal_fold(host, "localhost") || ip.is_some_and(|ip| ip.is_loopback())) {
        return Some(detail(403, "unexpected Host header: this service only answers on a loopback address"));
    }
    let site = req.header("Sec-Fetch-Site");
    if !site.is_empty() && site != b"same-origin" && site != b"none" {
        return Some(detail(403, "cross-site request refused"));
    }
    let origin = req.header("Origin");
    if !origin.is_empty() {
        match crate::gourl::parse_bytes(origin) {
            Some(u) if u.host == req.host => {}
            _ => return Some(detail(403, "cross-origin request refused")),
        }
    }
    None
}

/// Go's `net.SplitHostPort`.
pub fn split_host_port(hp: &str) -> Option<(&str, &str)> {
    if let Some(rest) = hp.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let port = rest[end + 1..].strip_prefix(':')?;
        if port.contains(':') || port.contains('[') || port.contains(']') || host.contains('[') {
            return None;
        }
        return Some((host, port));
    }
    let i = hp.rfind(':')?;
    let (host, port) = (&hp[..i], &hp[i + 1..]);
    if host.contains(':') || host.contains('[') || host.contains(']') || port.contains('[') || port.contains(']') {
        return None;
    }
    Some((host, port))
}

/// Go's `net.ParseIP` (no zone; an IPv4-mapped address is its IPv4 address).
pub fn parse_ip(s: &str) -> Option<std::net::IpAddr> {
    if s.contains('%') {
        return None;
    }
    match s.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V6(v6) => Some(v6.to_ipv4_mapped().map_or(std::net::IpAddr::V6(v6), std::net::IpAddr::V4)),
        v4 => Some(v4),
    }
}

/// Go's `IsLoopbackHost`.
pub fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_matches(|c| c == '[' || c == ']');
    gotext::equal_fold(h, "localhost") || parse_ip(h).is_some_and(|ip| ip.is_loopback())
}

static STR: Ty = Ty::Ptr(&Ty::Str);
static INT: Ty = Ty::Ptr(&Ty::Int);
static FLOAT: Ty = Ty::Ptr(&Ty::Float);
static BOOL: Ty = Ty::Ptr(&Ty::Bool);

/// Go's `createReq`.
static CREATE: Ty = Ty::Struct(
    "service.createReq",
    "createReq",
    &[
        Field("cwd", &STR),
        Field("model", &STR),
        Field("base_url", &STR),
        Field("api_key", &STR),
        Field("allow_shell", &BOOL),
        Field("allow_write", &BOOL),
        Field("temperature", &FLOAT),
        Field("timeout_seconds", &FLOAT),
        Field("max_turns", &INT),
        Field("max_total_tokens", &INT),
        Field("max_input_tokens", &INT),
        Field("max_output_tokens", &INT),
        Field("max_reasoning_tokens", &INT),
        Field("max_tool_calls", &INT),
        Field("max_delegated_tasks", &INT),
        Field("max_model_calls", &INT),
        Field("max_session_turns", &INT),
        Field("max_total_cost_usd", &FLOAT),
        Field("session_directory", &STR),
        Field("scratchpad_root", &STR),
        Field("tool_allowlist", &Ty::Slice(&Ty::Str)),
    ],
);

/// Go's `chatReq`.
static CHAT: Ty = Ty::Struct("service.chatReq", "chatReq", &[Field("prompt", &STR), Field("resume_session_id", &STR)]);

/// Go's `readBody`: the body (8 MiB at most, within its deadline) read as JSON into the request's struct.
fn read_body(ex: &mut Exchange, t: &Ty) -> Result<Val, Response> {
    let body = match ex.read_body(MAX_BODY) {
        Ok(b) => b,
        Err(BodyErr::TooLarge) => return Err(detail(413, "request body too large")),
        Err(BodyErr::Broken) => {
            let mut r = detail(400, "request body could not be read");
            r.set("Connection", "close");
            return Err(r);
        }
    };
    ex.set_read_deadline(None);
    let (v, err) = gostruct::unmarshal(t, &body);
    let Some(e) = err else { return Ok(v) };
    Err(match type_error(&e.0) {
        Some((field, typ)) => invalid(&field, &format!("wrong type: expected {typ}"), "type_error"),
        None => invalid("", "request body is not valid JSON", "json_invalid"),
    })
}

/// The field and type of Go's `*json.UnmarshalTypeError` from its text (a number past a field's range is one too); None
/// for any other error.
fn type_error(msg: &str) -> Option<(String, String)> {
    let rest = msg.strip_prefix("json: cannot unmarshal ")?;
    let (_, rest) = rest.split_once(" into ")?;
    let (target, typ) = rest.rsplit_once(" of type ")?;
    let field = if target.starts_with("Go value") {
        String::new()
    } else {
        let t = target.strip_prefix("Go struct field ").unwrap_or(target);
        let t = t.strip_prefix("createReq.").or_else(|| t.strip_prefix("chatReq.")).unwrap_or(t);
        t.to_owned()
    };
    Some((field, typ.to_owned()))
}

fn opt_str(v: Option<&Val>) -> Option<String> {
    match v {
        Some(Val::Ptr(Some(b))) => match &**b {
            Val::Str(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn opt_bool(v: Option<&Val>) -> Option<bool> {
    match v {
        Some(Val::Ptr(Some(b))) => match **b {
            Val::Bool(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

fn opt_i64(v: Option<&Val>) -> Option<i64> {
    match v {
        Some(Val::Ptr(Some(b))) => match **b {
            Val::Int(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

fn opt_f64(v: Option<&Val>) -> Option<f64> {
    match v {
        Some(Val::Ptr(Some(b))) => match **b {
            Val::Float(x) => Some(x),
            _ => None,
        },
        _ => None,
    }
}

// ---- paths -----------------------------------------------------------------------------------------------------------------

fn lossy(b: &[u8]) -> String {
    gotext::lossy(b)
}

/// Go's `resolve`: absolute, with links followed through the deepest part that exists.
fn resolve(p: &str) -> Vec<u8> {
    let abs = match gopath::abs(p.as_bytes()) {
        Ok(a) => a,
        Err(_) => return gopath::clean(p.as_bytes()),
    };
    let mut rest: Vec<u8> = Vec::new();
    let mut cur = abs.clone();
    loop {
        if let Ok(real) = gopath::eval_symlinks(&cur) {
            return gopath::join(&[&real, &rest]);
        }
        let parent = gopath::dir(&cur);
        if parent == cur {
            return abs;
        }
        rest = gopath::join(&[&gopath::base(&cur), &rest]);
        cur = parent;
    }
}

/// Go's `inside`: `p` is `root` or below it, links followed.
fn inside(root: &str, p: &str) -> bool {
    let Ok(rel) = gopath::rel(&resolve(root), &resolve(p)) else { return false };
    rel == b"." || (rel != b".." && !rel.starts_with(b"../") && !gopath::is_abs(&rel))
}

// ---- answers ----------------------------------------------------------------------------------------------------------------

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn n(v: f64) -> Any {
    Any::Number(v)
}

fn obj(kv: Vec<(&str, Any)>) -> Any {
    Any::Object(kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// Go's `writeJSON`: `json.NewEncoder(w).Encode(v)` (HTML escaped, a newline after).
fn json(code: u16, v: &Any) -> Response {
    let mut out = String::new();
    let _ = crate::gojson::write_any(&mut out, v, true);
    out.push('\n');
    let mut r = Response::new(code);
    r.set("Content-Type", "application/json");
    r.body = out.into_bytes();
    r
}

fn detail(code: u16, msg: &str) -> Response {
    json(code, &obj(vec![("detail", s(msg))]))
}

/// Go's `invalid`: the 422 body.
fn invalid(field: &str, msg: &str, typ: &str) -> Response {
    let d = obj(vec![("loc", Any::Array(vec![s("body"), s(field)])), ("msg", s(msg)), ("type", s(typ))]);
    json(422, &obj(vec![("detail", Any::Array(vec![d]))]))
}

/// Go's `Serve`: the server on `listener` until `stop` fires, then Go's `Shutdown` with 5 s of grace: no new
/// connections, idle ones closed, the busy ones waited for.
pub fn serve(server: Arc<Server>, listener: std::net::TcpListener, stop: Receiver<()>) {
    let settings = Arc::new(gohttpd::Settings { read_header_timeout: Duration::from_secs(15), idle_timeout: Duration::from_secs(120) });
    let tracker = Arc::new(gohttpd::Tracker::default());
    let fd = std::os::fd::AsRawFd::as_raw_fd(&listener);
    let srv = server.clone();
    let handler: Arc<gohttpd::Handler> = Arc::new(move |ex: &mut Exchange| srv.handle(ex));
    let (t, s2) = (tracker.clone(), settings.clone());
    let accept = std::thread::Builder::new().name("accept".into()).spawn(move || gohttpd::accept_loop(listener, s2, handler, t));
    let _ = stop.recv();
    tracker.shutting_down.store(true, std::sync::atomic::Ordering::SeqCst);
    // SAFETY: shutting the listening socket's reading side wakes a blocked accept; the descriptor stays open (the
    // accept thread owns it).
    unsafe {
        libc::shutdown(fd, libc::SHUT_RDWR);
    }
    let until = Instant::now() + Duration::from_secs(5);
    let mut wait = Duration::from_millis(1);
    while Instant::now() < until {
        if tracker.close_idle() {
            break;
        }
        std::thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_millis(500));
    }
    drop(accept);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_errors() {
        assert_eq!(
            type_error("json: cannot unmarshal number into Go struct field createReq.cwd of type string"),
            Some(("cwd".into(), "string".into()))
        );
        assert_eq!(
            type_error("json: cannot unmarshal number into createReq.tool_allowlist.0 of type string"),
            Some(("tool_allowlist.0".into(), "string".into()))
        );
        assert_eq!(
            type_error("json: cannot unmarshal array into Go value of type service.createReq"),
            Some(("".into(), "service.createReq".into()))
        );
        assert_eq!(
            type_error("json: cannot unmarshal number -1e999 into Go struct field createReq.max_total_cost_usd of type float64"),
            Some(("max_total_cost_usd".into(), "float64".into()))
        );
        assert!(valid_id("abc_-1") && !valid_id("../x") && !valid_id(&"x".repeat(65)));
        assert!(is_loopback_host("[::1]") && is_loopback_host("LOCALHOST") && !is_loopback_host("0.0.0.0"));
    }
}
