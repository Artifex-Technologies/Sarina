//! Go: `tiers/router.go`: the model transport that picks the tier. Every call starts on the lowest tier (or the one the
//! operator asked for with /tier up) and moves up past tiers that are disabled, unhealthy, too small for the request,
//! refused by their allow_data, not confirmed by the operator, not approved by the owner in Heimdall's grants file (a
//! tier off this computer while the gate is on), or failing. What leaves the machine goes through `sanitize` first; every
//! move is shown to the operator and recorded in the session file. Moving up changes where the model runs, never the
//! tools or permissions.

use super::cli::CliClient;
use super::config::{Config, Thresholds, Tier, DEFAULT_CLI_PATH, KIND_CLAUDE, KIND_CLAUDE_CLI};
use super::config::{TRIGGER_BACKEND_ERROR, TRIGGER_RESOURCE_EXHAUSTED};
use super::guard::{describe_guard, sanitize};
use super::resource::{Sample, Sampler, SystemSampler};
use super::wake::WakeFunc;
use crate::config::ModelConfig;
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gohttp::{self, Dial, Opts};
use crate::gotext;
use crate::gotime::Time;
use crate::heimdall::{self, Grants};
use crate::llm::anthropic::AnthropicClient;
use crate::llm::client::{Client, Error};
use crate::llm::openai::{BackendError, Request, StreamEvent};
use crate::llm::types::Turn;
use crate::llm::Completer;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The plugin_state key the tier record is stored under.
pub const SESSION_KEY: &str = "compute_tiers";
const MAX_RECORDED_EVENTS: usize = 200;

/// An event shown to the operator and, for most, kept for the session file.
pub type Event = BTreeMap<String, Any>;

/// Asks the operator a yes-or-no question (Go's `Confirm`).
pub type ConfirmFunc = Box<dyn FnMut(&str) -> bool>;

/// How a Router is set up (Go's `Options`).
pub struct Options {
    pub config: Config,
    /// the CLI's model settings: the first tier's defaults
    pub base: ModelConfig,
    /// the first tier's transport when it names no base_url or model of its own
    pub base_client: Option<Box<dyn Completer>>,
    pub getenv: Box<dyn Fn(&str) -> String>,
    /// the authorities for https; None = the system's
    pub tls: Option<Arc<rustls::ClientConfig>>,
    /// None: the real machine
    pub sampler: Option<Box<dyn Sampler>>,
    /// claude_cli: the child's folder ("" = this process's)
    pub workdir: String,
    /// claude_cli: the environment to filter for the child (None = this process's)
    pub environ: Option<Vec<String>>,
    /// None: wake entries are reported and skipped
    pub wake: Option<WakeFunc>,
    /// asks the operator before first use of a tier whose auto is false; None: no one can be asked (refused)
    pub confirm: Option<ConfirmFunc>,
    pub now: Option<Box<dyn Fn() -> Time>>,
    /// zero: 2 s
    pub health_timeout: Duration,
    /// zero: 30 s
    pub health_ttl: Duration,
    /// zero: 2 s
    pub wake_poll: Duration,
    /// Heimdall's grants (None: the file /etc/heimdall/grants.json)
    pub grants: Option<Box<dyn Fn() -> Grants>>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            config: Config::default(),
            base: ModelConfig::default(),
            base_client: None,
            getenv: Box::new(|_| String::new()),
            tls: None,
            sampler: None,
            workdir: String::new(),
            environ: None,
            wake: None,
            confirm: None,
            now: None,
            health_timeout: Duration::ZERO,
            health_ttl: Duration::ZERO,
            wake_poll: Duration::ZERO,
            grants: None,
        }
    }
}

enum Tc {
    Base,
    OpenAI(Client),
    Anthropic(AnthropicClient),
    Cli(Box<CliClient>),
}

struct Health {
    ok: bool,
    why: String,
    when: Time,
}

#[derive(Default)]
struct State {
    cur: usize,
    pending: Option<usize>,
    emit: Option<Box<dyn FnMut(Event)>>,
    clients: HashMap<usize, Tc>,
    health: HashMap<usize, Health>,
    timeouts: HashMap<usize, i64>,
    wake_at: HashMap<usize, Time>,
    approved: HashSet<usize>,
    denied: HashSet<usize>,
    /// a grant request was filed with Heimdall for this tier this session
    asked: HashSet<usize>,
    /// escalations and refusals not yet recorded in a session
    events: Vec<Event>,
    /// redactions since the last record, by tier name
    redacted: BTreeMap<String, i64>,
    highest: usize,
    /// a request body reached a transport during the current call
    sent_any: bool,
    cli_runs: HashMap<usize, i64>,
    confirm: Option<ConfirmFunc>,
}

/// The tier router (Go's `Router`): the agent's model transport and its tier tracker.
pub struct Router {
    opt: Options,
    sampler: Box<dyn Sampler>,
    /// secret values this process holds, for exact-match redaction
    known: Vec<String>,
    st: Mutex<State>,
}

/// One row of `sarina tiers status` (Go's `TierStatus`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TierStatus {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    pub key_set: bool,
    pub allow_data: String,
    pub max_context: i64,
    pub wake: bool,
    pub enabled: bool,
    pub auto: bool,
    pub healthy: bool,
    pub why: String,
    pub current: bool,
    pub cli_path: String,
    pub cli_found: bool,
}

fn s(v: &str) -> Any {
    Any::String(v.to_string())
}

fn ev(pairs: &[(&str, Any)]) -> Event {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

/// Go's `exec.LookPath` as a yes or no.
fn found(p: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if p.contains('/') {
        return std::fs::metadata(p).is_ok_and(|m| !m.is_dir() && m.permissions().mode() & 0o111 != 0);
    }
    crate::tools::exec::look_path_go(p).is_ok()
}

/// Go's `estimateTokens`: chars/4 over everything the request carries.
pub fn estimate_tokens(req: &Request) -> i64 {
    let mut n = 0usize;
    for m in &req.messages {
        n += m.content.len() + 16;
        for tc in &m.tool_calls {
            n += tc.function.name.len() + tc.function.arguments.len() + 16;
        }
    }
    for t in &req.tools {
        if let Ok(b) = t.to_json() {
            n += b.len();
        }
    }
    (n / 4) as i64
}

/// Go's `offMachine`: the tier sends the conversation off this computer (the claude CLI and the Claude API always; any
/// other tier unless its base URL's host is localhost or a loopback address; anything that does not parse counts as off).
pub fn off_machine(t: &Tier) -> bool {
    if t.kind == KIND_CLAUDE_CLI || t.kind == KIND_CLAUDE {
        return true;
    }
    let Some(u) = crate::gourl::parse(gotext::trim_space(&t.base_url)) else { return true };
    let h = hostname(&String::from_utf8_lossy(&u.host));
    if h.is_empty() {
        return true;
    }
    let h = gotext::to_lower(&h);
    if h == "localhost" {
        return false;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => !a.is_loopback(),
        Ok(std::net::IpAddr::V6(a)) => !(a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v| v.is_loopback())),
        Err(_) => true,
    }
}

/// Go's `URL.Hostname`: the host without its port and brackets.
fn hostname(host: &str) -> String {
    let mut h = host;
    if let Some(c) = h.rfind(':') {
        let port = &h[c + 1..];
        if port.bytes().all(|b| b.is_ascii_digit()) {
            h = &h[..c];
        }
    }
    if h.starts_with('[') && h.ends_with(']') && h.len() >= 2 {
        h = &h[1..h.len() - 1];
    }
    h.to_string()
}

/// A shorter-lived handle on the caller's stream callback, for one attempt.
fn reborrow<'a>(on: &'a mut Option<&mut dyn FnMut(StreamEvent)>) -> Option<&'a mut dyn FnMut(StreamEvent)> {
    match on {
        Some(f) => Some(&mut **f),
        None => None,
    }
}

enum Verdict {
    Ok,
    Unusable(String),
    /// fine except for resource pressure (advisory: the last resort)
    PressureOnly(String),
}

impl Router {
    /// Go's `New`: the config must have at least one tier.
    pub fn new(mut opt: Options) -> Result<Router, String> {
        if opt.config.tiers.is_empty() {
            return Err("tiers: no tiers configured".into());
        }
        if opt.health_timeout.is_zero() {
            opt.health_timeout = Duration::from_secs(2);
        }
        if opt.health_ttl.is_zero() {
            opt.health_ttl = Duration::from_secs(30);
        }
        if opt.wake_poll.is_zero() {
            opt.wake_poll = Duration::from_secs(2);
        }
        let sampler = opt.sampler.take().unwrap_or_else(|| Box::new(SystemSampler::new(opt.config.resource.max_cpu_percent > 0.0)));
        let mut known = Vec::new();
        if !opt.base.api_key.is_empty() {
            known.push(opt.base.api_key.clone());
        }
        for t in &opt.config.tiers {
            if !t.api_key_env.is_empty() {
                let v = (opt.getenv)(&t.api_key_env);
                if !v.is_empty() {
                    known.push(v);
                }
            }
        }
        let st = State { confirm: opt.confirm.take(), ..State::default() };
        Ok(Router { opt, sampler, known, st: Mutex::new(st) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The configured tiers.
    pub fn tiers(&self) -> &[Tier] {
        &self.opt.config.tiers
    }

    /// The tier the next call starts on.
    pub fn current(&self) -> String {
        let st = self.lock();
        self.name(st.cur)
    }

    /// Installs the operator-confirmation function.
    pub fn set_confirm(&self, f: Option<ConfirmFunc>) {
        self.lock().confirm = f;
    }

    fn now(&self) -> Time {
        match &self.opt.now {
            Some(f) => f(),
            None => Time::now(),
        }
    }

    /// Any secret value this process holds removed from text that may be shown or stored.
    fn scrub(&self, s: &str) -> String {
        let mut s = s.to_string();
        for k in &self.known {
            if k.len() >= 8 {
                s = s.replace(k.as_str(), "[redacted-key]");
            }
        }
        s
    }

    fn name(&self, i: usize) -> String {
        self.opt.config.tiers[i].name.clone()
    }

    /// Shows an event to the operator and queues it for the session file.
    fn post(&self, st: &mut State, mut e: Event, record: bool) {
        let line = match e.get("line") {
            Some(Any::String(l)) => l.clone(),
            _ => String::new(),
        };
        e.insert("line".into(), s(&self.scrub(&line)));
        if let Some(Any::String(r)) = e.get("reason") {
            let r = self.scrub(r);
            e.insert("reason".into(), s(&r));
        }
        e.insert("time".into(), s(&self.now().format_rfc3339_utc()));
        if let Some(emit) = st.emit.as_mut() {
            emit(e.clone());
        }
        if record {
            st.events.push(e);
        }
    }

    fn announce(&self, st: &mut State, from: usize, to: usize, reason: &str) {
        let t = &self.opt.config.tiers[to];
        let mut line = format!("[tier] escalated {} -> {}: {reason}", self.name(from), self.name(to));
        if t.kind == KIND_CLAUDE_CLI {
            line += " (handing the task to the claude CLI)";
        }
        let e = ev(&[
            ("type", s("tier_escalation")),
            ("from", s(&self.name(from))),
            ("to", s(&self.name(to))),
            ("kind", s(&t.kind)),
            ("reason", s(reason)),
            ("line", s(&line)),
        ]);
        self.post(st, e, true);
        st.cur = to;
        st.highest = st.highest.max(to);
    }

    fn refuse(&self, st: &mut State, i: usize, reason: &str) {
        let e = ev(&[
            ("type", s("tier_refused")),
            ("tier", s(&self.name(i))),
            ("reason", s(reason)),
            ("line", s(&format!("[tier] not using {}: {reason}", self.name(i)))),
        ]);
        self.post(st, e, true);
    }

    /// Go's `BeginTurn`: back to the lowest tier at the start of every turn, unless the operator asked for /tier up,
    /// which applies to this turn only.
    pub fn begin_turn(&self, emit: Option<Box<dyn FnMut(Event)>>) {
        let mut st = self.lock();
        st.emit = emit;
        if let Some(target) = st.pending.take() {
            if target != st.cur {
                let cur = st.cur;
                self.announce(&mut st, cur, target, "operator ran /tier up");
            }
            st.cur = target;
            return;
        }
        st.cur = 0;
    }

    /// Go's `RequestUp`: the operator's /tier up; the next turn starts one tier above where the last one ended (typing
    /// it is the operator's consent for a tier whose auto is false). The target tier's name.
    pub fn request_up(&self) -> Result<String, String> {
        let mut st = self.lock();
        let base = st.pending.unwrap_or(st.cur);
        for (next, t) in self.opt.config.tiers.iter().enumerate().skip(base + 1) {
            if !t.enabled {
                continue;
            }
            st.approved.insert(next);
            st.pending = Some(next);
            return Ok(t.name.clone());
        }
        Err(format!("no higher tier is available (on {})", self.name(base)))
    }

    /// Go's `RequestDown`: the operator's /tier down.
    pub fn request_down(&self) -> String {
        let mut st = self.lock();
        st.pending = None;
        st.cur = 0;
        self.name(0)
    }

    /// Go's `Record`: what to store in the session file's plugin_state[SESSION_KEY] when a turn ends (None: nothing, so a
    /// session that never left the first tier keeps the same bytes).
    pub fn record(&self, prev: Option<&Any>) -> Option<Event> {
        let mut st = self.lock();
        st.emit = None;
        let pm = match prev {
            Some(Any::Object(m)) => Some(m),
            _ => None,
        };
        if pm.is_none() && st.events.is_empty() && st.cur == 0 && st.redacted.is_empty() {
            return None;
        }
        let mut list: Vec<Any> = match pm.and_then(|m| m.get("escalations")) {
            Some(Any::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        for e in &st.events {
            list.push(Any::Object(e.clone()));
        }
        if list.len() > MAX_RECORDED_EVENTS {
            list.drain(..list.len() - MAX_RECORDED_EVENTS);
        }
        let mut highest = st.highest as i64;
        if let Some(Any::Number(n)) = pm.and_then(|m| m.get("highest_tier_index")) {
            highest = highest.max(crate::config::go_int(*n));
        }
        highest = highest.min(self.opt.config.tiers.len() as i64 - 1);
        let mut reds: BTreeMap<String, Any> = match pm.and_then(|m| m.get("redactions")) {
            Some(Any::Object(o)) => o.clone(),
            _ => BTreeMap::new(),
        };
        for (k, n) in &st.redacted {
            let prev_n = match reds.get(k) {
                Some(Any::Number(v)) => crate::config::go_int(*v),
                _ => 0,
            };
            reds.insert(k.clone(), Any::Number((prev_n + n) as f64));
        }
        let mut rec = ev(&[
            ("current_tier", s(&self.name(st.cur))),
            ("highest_tier", s(&self.name(highest.max(0) as usize))),
            ("highest_tier_index", Any::Number(highest as f64)),
            ("escalations", Any::Array(list)),
        ]);
        if !reds.is_empty() {
            rec.insert("redactions".into(), Any::Object(reds));
        }
        st.events.clear();
        st.redacted.clear();
        Some(rec)
    }

    // ---- the model call ----------------------------------------------------------------------------------------------

    fn client(&self, st: &mut State, i: usize) -> Result<(), String> {
        if st.clients.contains_key(&i) {
            return Ok(());
        }
        let t = &self.opt.config.tiers[i];
        let mut cfg = self.opt.base.clone();
        cfg.api_key = String::new();
        let c = match t.kind.as_str() {
            KIND_CLAUDE_CLI => Tc::Cli(Box::new(CliClient {
                tier: t.clone(),
                workdir: self.opt.workdir.clone(),
                environ: self.opt.environ.clone(),
                secrets: self.known.clone(),
                last: Default::default(),
            })),
            KIND_CLAUDE => {
                let key = (self.opt.getenv)(&t.api_key_env);
                if key.is_empty() {
                    return Err(format!("environment variable {} is not set", t.api_key_env));
                }
                cfg.api_key = key;
                cfg.model.clone_from(&t.model);
                cfg.base_url.clone_from(&t.base_url);
                let mut c = AnthropicClient::new(cfg);
                c.tls.clone_from(&self.opt.tls);
                Tc::Anthropic(c)
            }
            _ => {
                if i == 0 && t.base_url.is_empty() && t.model.is_empty() && self.opt.base_client.is_some() {
                    Tc::Base
                } else {
                    if !t.base_url.is_empty() {
                        cfg.base_url.clone_from(&t.base_url);
                    }
                    if !t.model.is_empty() {
                        cfg.model.clone_from(&t.model);
                    }
                    if !t.api_key_env.is_empty() {
                        cfg.api_key = (self.opt.getenv)(&t.api_key_env);
                    } else if i == 0 {
                        cfg.api_key.clone_from(&self.opt.base.api_key);
                    }
                    let mut c = Client::new(cfg);
                    c.tls.clone_from(&self.opt.tls);
                    Tc::OpenAI(c)
                }
            }
        };
        st.clients.insert(i, c);
        Ok(())
    }

    /// Go's `cliGate`: whether a claude_cli tier may take this request ("" = allowed).
    fn cli_gate(&self, st: &State, i: usize, trigger: &str) -> String {
        let t = &self.opt.config.tiers[i];
        if !t.triggers.iter().flatten().any(|tr| tr == trigger) {
            return format!("{trigger} is not one of its escalation triggers");
        }
        if t.max_escalations > 0 && st.cli_runs.get(&i).copied().unwrap_or(0) >= t.max_escalations {
            return format!("escalation cap reached ({} hand-off(s) this session)", t.max_escalations);
        }
        String::new()
    }

    /// Go's `probe`: one health request (the tier's models endpoint, a short timeout).
    fn probe(&self, st: &mut State, ctx: &Ctx, i: usize) -> (bool, String) {
        let t = &self.opt.config.tiers[i];
        let target;
        let mut hdr: Vec<(&str, String)> = Vec::new();
        match t.kind.as_str() {
            KIND_CLAUDE_CLI => {
                let p = if t.cli_path.is_empty() { DEFAULT_CLI_PATH } else { &t.cli_path };
                if !found(p) {
                    return (false, format!("claude CLI {} not found", gotext::quote(p)));
                }
                return (true, String::new());
            }
            KIND_CLAUDE => {
                if let Err(e) = self.client(st, i) {
                    return (false, e);
                }
                let Some(Tc::Anthropic(ac)) = st.clients.get(&i) else { return (false, String::new()) };
                target = ac.models_endpoint();
                hdr.extend(ac.headers());
            }
            _ => {
                let base = if t.base_url.is_empty() { &self.opt.base.base_url } else { &t.base_url };
                if base.is_empty() {
                    return (true, String::new()); // nothing to probe (an injected transport)
                }
                target = format!("{}/models", base.trim_end_matches('/'));
                let key = if !t.api_key_env.is_empty() {
                    (self.opt.getenv)(&t.api_key_env)
                } else if i == 0 {
                    self.opt.base.api_key.clone()
                } else {
                    String::new()
                };
                if !key.is_empty() {
                    hdr.push(("Authorization", format!("Bearer {key}")));
                }
            }
        }
        let parsed = crate::gourl::parse(&target);
        let host = match &parsed {
            Some(u) if !u.host.is_empty() => String::from_utf8_lossy(&u.host).into_owned(),
            _ => target.clone(),
        };
        if parsed.is_none() {
            return (false, "bad base_url".into());
        }
        if ctx.err().is_some() {
            return (false, "cancelled".into()); // Go's client does not dial for a context already ended
        }
        let headers: Vec<(&str, &str)> = hdr.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let o = Opts { dial: Dial::Tcp, method: "GET", url: &target, headers: &headers, body: None, tls: self.opt.tls.clone() };
        let deadline: gohttp::Deadline = Rc::new(Cell::new(Instant::now() + self.opt.health_timeout));
        let status = match gohttp::open(&o, ctx, &deadline) {
            Err(_) => {
                if ctx.err().is_some() {
                    return (false, "cancelled".into());
                }
                return (false, format!("unreachable ({host})"));
            }
            Ok(r) => r.status,
        };
        if status >= 500 {
            return (false, format!("unhealthy (HTTP {status} from {host})"));
        }
        if t.kind == KIND_CLAUDE && (status == 401 || status == 403) {
            return (false, format!("API key rejected (HTTP {status})"));
        }
        (true, String::new())
    }

    /// Go's `healthy`: probe with a cache (successes and failures alike).
    fn healthy(&self, st: &mut State, ctx: &Ctx, i: usize) -> (bool, String) {
        if let Some(h) = st.health.get(&i) {
            if self.now().sub_nanos(&h.when) < self.opt.health_ttl.as_nanos() as i64 {
                return (h.ok, h.why.clone());
            }
        }
        let (ok, why) = self.probe(st, ctx, i);
        let when = self.now();
        st.health.insert(i, Health { ok, why: why.clone(), when });
        (ok, why)
    }

    fn mark_down(&self, st: &mut State, i: usize, why: &str) {
        let when = self.now();
        st.health.insert(i, Health { ok: false, why: why.into(), when });
    }

    /// Go's `tryWake`: the tier's configured wake entry (never anything else), then a wait for it to answer.
    fn try_wake(&self, st: &mut State, ctx: &Ctx, i: usize) -> bool {
        let t = &self.opt.config.tiers[i];
        if gotext::trim_space(&t.wake).is_empty() {
            return false;
        }
        if let Some(last) = st.wake_at.get(&i) {
            if self.now().sub_nanos(last) < 120_000_000_000 {
                return false;
            }
        }
        let at = self.now();
        st.wake_at.insert(i, at);
        let Some(wake) = &self.opt.wake else {
            let e = ev(&[
                ("type", s("tier_wake")),
                ("tier", s(&t.name)),
                ("ok", Any::Bool(false)),
                ("reason", s("no wake runner available")),
                ("line", s(&format!("[tier] {} is down and has a wake entry, but it was not run: no wake runner", t.name))),
            ]);
            self.post(st, e, false);
            return false;
        };
        if let Err(err) = wake(ctx, t) {
            let e = ev(&[
                ("type", s("tier_wake")),
                ("tier", s(&t.name)),
                ("ok", Any::Bool(false)),
                ("reason", s(&err)),
                ("line", s(&format!("[tier] wake for {} failed: {err}", t.name))),
            ]);
            self.post(st, e, false);
            return false;
        }
        let e = ev(&[
            ("type", s("tier_wake")),
            ("tier", s(&t.name)),
            ("ok", Any::Bool(true)),
            ("line", s(&format!("[tier] wake sent to {}; waiting for it to answer", t.name))),
        ]);
        self.post(st, e, false);
        let mut wait = crate::config::go_int(t.wake_wait * 1e9);
        if wait <= 0 {
            wait = 30_000_000_000;
        }
        let deadline = self.now().add_nanos(wait);
        loop {
            let (ok, why) = self.probe(st, ctx, i);
            if ok {
                let when = self.now();
                st.health.insert(i, Health { ok: true, why: String::new(), when });
                return true;
            } else if why == "cancelled" {
                return false;
            }
            if !self.now().before(&deadline) {
                return false;
            }
            if !ctx.sleep(self.opt.wake_poll) {
                return false;
            }
        }
    }

    fn usable(&self, st: &mut State, ctx: &Ctx, i: usize, req: &Request) -> Verdict {
        let t = &self.opt.config.tiers[i];
        if !t.enabled {
            return Verdict::Unusable("disabled in config".into());
        }
        if t.kind == KIND_CLAUDE {
            if let Err(e) = self.client(st, i) {
                return Verdict::Unusable(e);
            }
        }
        let pressure = if i == 0 { self.opt.config.resource.pressure(&self.sampler.sample(ctx)) } else { String::new() };
        let (ok, why) = self.healthy(st, ctx, i);
        if !ok && !self.try_wake(st, ctx, i) {
            return Verdict::Unusable(why);
        }
        if t.max_context > 0 {
            let est = estimate_tokens(req);
            if est > t.max_context {
                return Verdict::Unusable(format!("the request (~{est} tokens) does not fit its {}-token context window", t.max_context));
            }
        }
        if !pressure.is_empty() {
            return Verdict::PressureOnly(format!("resource pressure: {pressure}"));
        }
        Verdict::Ok
    }

    /// Go's `attempt`: guards and sends one request to tier i. None: try the next tier (the reason is in `reasons`).
    #[allow(clippy::too_many_arguments)]
    fn attempt(
        &self,
        st: &mut State,
        ctx: &Ctx,
        i: usize,
        req: &Request,
        mut on: Option<&mut dyn FnMut(StreamEvent)>,
        reasons: &mut HashMap<usize, String>,
        cli_reason: Option<&str>,
    ) -> Option<Result<Turn, Error>> {
        let t = &self.opt.config.tiers[i];
        // Heimdall's grant gate: while it is on, a tier that sends the conversation off this computer is used only with
        // the owner's approval in Heimdall's root-owned file, whatever its auto setting; without one, a request is filed and
        // this tier is skipped. The one place every hand-off passes.
        if i > 0 && off_machine(t) {
            let g = match &self.opt.grants {
                Some(f) => f(),
                None => heimdall::load_grants(),
            };
            if g.on {
                if !g.escalation_allowed(&t.name, "", &self.now()) {
                    if st.asked.insert(i) {
                        let how = match heimdall::request_grant("escalation", &t.name, "", "the local model could not finish the task") {
                            Ok(h) => h,
                            Err(e) => format!("Heimdall could not take the request ({e})"),
                        };
                        reasons.insert(i, format!("needs the owner's approval in Heimdall: {how}"));
                    } else {
                        reasons.insert(i, "needs the owner's approval in Heimdall (requested)".into());
                    }
                    let r = reasons[&i].clone();
                    self.refuse(st, i, &r);
                    return None;
                }
                st.approved.insert(i); // the owner's approval in Heimdall is the operator's consent
            }
        }
        if !t.auto && !st.approved.contains(&i) {
            if st.denied.contains(&i) {
                reasons.insert(i, "the operator declined to use it this session".into());
                return None;
            }
            let prompt = format!("Send this conversation to tier {} ({})? [y/N] ", gotext::quote(&t.name), t.kind);
            match st.confirm.as_mut().map(|c| c(&prompt)) {
                None => {
                    let r = "needs operator confirmation (auto is false) and none can be asked";
                    reasons.insert(i, r.into());
                    self.refuse(st, i, r);
                    return None;
                }
                Some(true) => {
                    st.approved.insert(i);
                }
                Some(false) => {
                    st.denied.insert(i);
                    let r = "the operator declined to use it this session";
                    reasons.insert(i, r.into());
                    self.refuse(st, i, r);
                    return None;
                }
            }
        }
        let (sent, redactions) = match sanitize(req, &t.allow_data, &self.known) {
            Ok(x) => x,
            Err(e) => {
                let r = format!("refused: {e}");
                reasons.insert(i, r.clone());
                self.refuse(st, i, &r);
                return None;
            }
        };
        if let Err(e) = self.client(st, i) {
            reasons.insert(i, e);
            return None;
        }
        if redactions > 0 {
            *st.redacted.entry(t.name.clone()).or_insert(0) += redactions as i64;
            let e = ev(&[
                ("type", s("tier_redaction")),
                ("tier", s(&t.name)),
                ("count", Any::Number(redactions as f64)),
                ("line", s(&format!("[tier] {}: {}", t.name, describe_guard(redactions)))),
            ]);
            self.post(st, e, false);
        }
        let is_cli = t.kind == KIND_CLAUDE_CLI;
        if is_cli {
            *st.cli_runs.entry(i).or_insert(0) += 1;
        }
        let mut tc = st.clients.remove(&i).expect("client made above");
        let out = loop {
            st.sent_any = true;
            let started = Cell::new(false);
            let has_on = on.is_some();
            let r = {
                let mut wrapped = |e: StreamEvent| {
                    started.set(true);
                    if let Some(f) = on.as_mut() {
                        f(e)
                    }
                };
                let on2: Option<&mut dyn FnMut(StreamEvent)> = if has_on { Some(&mut wrapped) } else { None };
                match &mut tc {
                    Tc::Base => match &self.opt.base_client {
                        Some(b) => b.complete(ctx, &sent, on2),
                        None => Err(Error::Backend(BackendError { message: "no transport".into(), status: 0 })),
                    },
                    Tc::OpenAI(c) => c.complete(ctx, &sent, on2),
                    Tc::Anthropic(c) => c.complete(ctx, &sent, on2),
                    Tc::Cli(c) => c.complete(ctx, &sent, cli_reason, on2).map_err(Error::Cli),
                }
            };
            let err = match r {
                Ok(turn) => {
                    st.timeouts.insert(i, 0);
                    let when = self.now();
                    st.health.insert(i, Health { ok: true, why: String::new(), when });
                    break Some(Ok(turn));
                }
                Err(e) => e,
            };
            if ctx.err().is_some() || started.get() {
                break Some(Err(err));
            }
            let Error::Backend(be) = &err else { break Some(Err(err)) };
            if be.status == 0 && be.message.contains("timed out") {
                let n = st.timeouts.entry(i).or_insert(0);
                *n += 1;
                if *n < 2 {
                    continue; // one more try on the same tier before calling it repeated
                }
                self.mark_down(st, i, "repeated timeouts");
                reasons.insert(i, "repeated timeouts".into());
            } else if be.status == 0 {
                self.mark_down(st, i, "unreachable");
                reasons.insert(i, "backend unreachable".into());
            } else if matches!(be.status, 502..=504) {
                self.mark_down(st, i, &format!("HTTP {}", be.status));
                reasons.insert(i, format!("backend unavailable (HTTP {})", be.status));
            } else {
                break Some(Err(err));
            }
            break None;
        };
        if let Tc::Cli(c) = &tc {
            self.record_cli(st, i, c);
        }
        st.clients.insert(i, tc);
        out
    }

    /// Go's `recordCLI`: the outcome of a claude_cli hand-off in the session record, shown to the operator.
    fn record_cli(&self, st: &mut State, i: usize, c: &CliClient) {
        let t = &self.opt.config.tiers[i];
        let l = &c.last;
        let mut e = ev(&[
            ("type", s("tier_cli_result")),
            ("action", s("escalate_claude_code")),
            ("tier", s(&t.name)),
            ("ok", Any::Bool(l.ok)),
            ("cli_session_id", s(&l.session_id)),
            ("total_cost_usd", Any::Number(l.cost_usd)),
            ("stdout_preview", s(&l.stdout)),
            ("stderr_preview", s(&l.stderr)),
        ]);
        if l.ok {
            e.insert("line".into(), s(&format!("[tier] {}: the claude CLI finished the task (cost ${:.4})", t.name, l.cost_usd)));
        } else {
            e.insert("error".into(), s(&l.err));
            e.insert("line".into(), s(&format!("[tier] {}: the claude CLI hand-off failed: {}", t.name, l.err)));
        }
        self.post(st, e, true);
    }

    /// Go's `Complete`: `req` to the lowest usable tier at or above the current one.
    pub fn complete(&self, ctx: &Ctx, req: &Request, mut on: Option<&mut dyn FnMut(StreamEvent)>) -> Result<Turn, Error> {
        let mut guard = self.lock();
        let st = &mut *guard;
        let tiers = &self.opt.config.tiers;
        let mut reasons: HashMap<usize, String> = HashMap::new();
        st.sent_any = false;
        let mut soft: Option<usize> = None;
        let from = st.cur;
        let mut i = st.cur;
        while i < tiers.len() {
            match self.usable(st, ctx, i, req) {
                Verdict::Unusable(why) => {
                    reasons.insert(i, why);
                    i += 1;
                    continue;
                }
                Verdict::PressureOnly(why) => {
                    reasons.insert(i, why);
                    soft = Some(i);
                    i += 1;
                    continue;
                }
                Verdict::Ok => {}
            }
            let trigger = if soft.is_some() { TRIGGER_RESOURCE_EXHAUSTED } else { TRIGGER_BACKEND_ERROR };
            if tiers[i].kind == KIND_CLAUDE_CLI {
                let why = self.cli_gate(st, i, trigger);
                if !why.is_empty() {
                    reasons.insert(i, why);
                    i += 1;
                    continue;
                }
            }
            let mut reason = "operator asked for this tier".to_string();
            if i > st.cur {
                reason = reasons.get(&st.cur).cloned().unwrap_or_default();
                for k in st.cur + 1..i {
                    reason += &format!("; {} skipped: {}", self.name(k), reasons.get(&k).cloned().unwrap_or_default());
                }
                let cur = st.cur;
                self.announce(st, cur, i, &reason);
            }
            let cli_reason = (tiers[i].kind == KIND_CLAUDE_CLI).then_some(reason.as_str());
            if let Some(r) = self.attempt(st, ctx, i, req, reborrow(&mut on), &mut reasons, cli_reason) {
                if r.is_ok() {
                    st.cur = i;
                }
                return r;
            }
            i += 1;
        }
        if let Some(sf) = soft {
            // resource pressure is advisory: with nowhere better to go, carry on here
            let why = reasons.get(&sf).cloned().unwrap_or_default();
            let e = ev(&[
                ("type", s("tier_notice")),
                ("tier", s(&self.name(sf))),
                ("reason", s(&why)),
                ("line", s(&format!("[tier] {why}; no higher tier could take the request, so staying on {}", self.name(sf)))),
            ]);
            self.post(st, e, false);
            if let Some(r) = self.attempt(st, ctx, sf, req, reborrow(&mut on), &mut reasons, None) {
                if r.is_ok() {
                    st.cur = sf;
                }
                return r;
            }
        }
        let parts: Vec<String> =
            (from..tiers.len()).map(|i| format!("{}: {}", self.name(i), reasons.get(&i).cloned().unwrap_or_default())).collect();
        let mut msg = format!("no compute tier could run this request ({})", parts.join("; "));
        if !st.sent_any {
            msg += ". Nothing was sent to any tier.";
        }
        Err(Error::Backend(BackendError { message: self.scrub(&msg), status: 0 }))
    }

    // ---- status --------------------------------------------------------------------------------------------------------

    /// Go's `Status`: every enabled tier probed (the cache bypassed); only health requests, never conversation data.
    pub fn status(&self, ctx: &Ctx) -> Vec<TierStatus> {
        let mut st = self.lock();
        let mut out = Vec::new();
        for (i, t) in self.opt.config.tiers.iter().enumerate() {
            let mut s = TierStatus {
                name: t.name.clone(),
                kind: t.kind.clone(),
                base_url: t.base_url.clone(),
                model: t.model.clone(),
                api_key_env: t.api_key_env.clone(),
                allow_data: t.allow_data.string(),
                max_context: t.max_context,
                wake: !gotext::trim_space(&t.wake).is_empty(),
                enabled: t.enabled,
                auto: t.auto,
                current: i == st.cur,
                ..TierStatus::default()
            };
            if !t.api_key_env.is_empty() {
                s.key_set = !(self.opt.getenv)(&t.api_key_env).is_empty();
            }
            if t.kind == KIND_CLAUDE_CLI {
                s.cli_path = if t.cli_path.is_empty() { DEFAULT_CLI_PATH.into() } else { t.cli_path.clone() };
                s.cli_found = found(&s.cli_path);
            }
            if s.base_url.is_empty() && i == 0 {
                s.base_url.clone_from(&self.opt.base.base_url);
            }
            if s.model.is_empty() && t.kind != KIND_CLAUDE && t.kind != KIND_CLAUDE_CLI {
                s.model.clone_from(&self.opt.base.model);
            }
            if !t.enabled {
                s.why = "disabled in config".into();
            } else if t.kind == KIND_CLAUDE && !s.key_set {
                s.why = format!("environment variable {} is not set", t.api_key_env);
            } else {
                let (ok, why) = self.probe(&mut st, ctx, i);
                let when = self.now();
                st.health.insert(i, Health { ok, why: why.clone(), when });
                (s.healthy, s.why) = (ok, why);
            }
            out.push(s);
        }
        out
    }

    /// Go's `Pressure`: one resource sample against the thresholds.
    pub fn pressure(&self, ctx: &Ctx) -> (Sample, String) {
        let s = self.sampler.sample(ctx);
        let p = self.opt.config.resource.pressure(&s);
        (s, p)
    }

    /// The configured resource limits.
    pub fn thresholds(&self) -> Thresholds {
        self.opt.config.resource
    }
}

impl Completer for Router {
    fn complete(&self, ctx: &Ctx, req: &Request, on: Option<&mut dyn FnMut(StreamEvent)>) -> Result<Turn, Error> {
        Router::complete(self, ctx, req, on)
    }
}
