//! Go: `runkit`: what the HTTP front ends (service and GUI) share: an agent built from plain settings ([`build`]), a run's
//! result and transcript in the wire shape both send ([`entry`], [`transcript`], [`result`]), and the scrubber that takes
//! secrets out of anything leaving the process ([`scrubber`]).

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::agent::run::{Agent, Options as AgentOptions, RunResult, TierTracker};
use crate::config::{ModelConfig, RuntimeConfig};
use crate::goany::Any;
use crate::goregexp;
use crate::gostruct::{self, Field, Ty, Val};
use crate::llm::Completer;
use crate::session::message::Message;
use crate::tiers::config::EscalationOptions;
use crate::tiers::router::Router;
use crate::tools::registry::{Approver, Builder, Tool};

static BEARER_RE: LazyLock<Regex> = LazyLock::new(|| goregexp::compile(r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=\-]{12,}").expect("bearer"));
static JSON_ASSIGN_RE: LazyLock<Regex> = LazyLock::new(|| {
    goregexp::compile(r#"(?i)("[A-Za-z0-9_]*(?:api[_-]?key|secret|token|passw(?:or)?d)[A-Za-z0-9_]*"\s*:\s*)"(?:[^"\\]|\\.){6,}""#)
        .expect("json assign")
});
static ASSIGN_RE: LazyLock<Regex> = LazyLock::new(|| {
    goregexp::compile(
        r#"(?i)\b([A-Za-z0-9_]*(?:api[_-]?key|secret|token|passw(?:or)?d)[A-Za-z0-9_]*)(\s*[=:]\s*)("[^"\s]{6,}"|'[^'\s]{6,}'|[^\s"',;]{6,})"#,
    )
    .expect("assign")
});

/// Removes secrets from text.
pub type Scrub = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Go's `Scrubber`: the literal values given (API keys and tokens this process holds, at least 6 characters), the
/// credential shapes the session redactor knows, bearer tokens, secret-named keys in JSON and key=value assignments.
pub fn scrubber(literals: &[&str]) -> Scrub {
    let lits: Vec<String> = literals.iter().filter(|l| l.len() >= 6).map(|l| l.to_string()).collect();
    Arc::new(move |s: &str| {
        let mut s = s.to_owned();
        for l in &lits {
            s = s.replace(l.as_str(), "[REDACTED]");
        }
        let s = crate::session::redact::redact(&s);
        let s = BEARER_RE.replace_all(&s, "$1 [REDACTED]").into_owned();
        let s = JSON_ASSIGN_RE.replace_all(&s, r#"$1"[REDACTED]""#).into_owned();
        ASSIGN_RE.replace_all(&s, "$1$2[REDACTED]").into_owned()
    })
}

fn scrub_value(v: &Any, scrub: &dyn Fn(&str) -> String) -> Any {
    match v {
        Any::String(s) => Any::String(scrub(s)),
        Any::Object(m) => Any::Object(m.iter().map(|(k, x)| (k.clone(), scrub_value(x, scrub))).collect()),
        Any::Array(a) => Any::Array(a.iter().map(|x| scrub_value(x, scrub)).collect()),
        other => other.clone(),
    }
}

/// Go's `ScrubValue`: `scrub` applied to every string inside a JSON-like value.
pub fn scrub_any(v: &Any, scrub: &dyn Fn(&str) -> String) -> Any {
    scrub_value(v, scrub)
}

static TOOL_BODY: Ty = Ty::Struct(
    r#"struct { OK *bool "json:\"ok\""; Content string "json:\"content\"" }"#,
    "",
    &[Field("ok", &Ty::Ptr(&Ty::Bool)), Field("content", &Ty::Str)],
);

/// Go's `Entry`: one transcript message in the wire shape: role and content always; name, tool_call_id, tool_calls,
/// metadata and message_id when not empty. A tool entry's metadata says `ok` (from the tool's own result body, else
/// false unless the metadata has it). Every string goes through `scrub`.
pub fn entry(m: &Message, scrub: Option<&dyn Fn(&str) -> String>) -> Any {
    let id = |s: &str| s.to_owned();
    let scrub: &dyn Fn(&str) -> String = scrub.unwrap_or(&id);
    let mut e = BTreeMap::new();
    e.insert("role".into(), Any::String(m.role.clone()));
    e.insert("content".into(), Any::String(scrub(&m.content)));
    if !m.name.is_empty() {
        e.insert("name".into(), Any::String(m.name.clone()));
    }
    if !m.tool_call_id.is_empty() {
        e.insert("tool_call_id".into(), Any::String(m.tool_call_id.clone()));
    }
    if !m.tool_calls.is_empty() {
        let calls = m
            .tool_calls
            .iter()
            .map(|c| {
                let mut f = BTreeMap::new();
                f.insert("name".to_owned(), Any::String(c.function.name.clone()));
                f.insert("arguments".to_owned(), Any::String(scrub(&c.function.arguments)));
                let mut o = BTreeMap::new();
                o.insert("id".to_owned(), Any::String(c.id.clone()));
                o.insert("type".to_owned(), Any::String("function".into()));
                o.insert("function".to_owned(), Any::Object(f));
                Any::Object(o)
            })
            .collect();
        e.insert("tool_calls".into(), Any::Array(calls));
    }
    let mut md = m.metadata.clone();
    if m.role == "tool" {
        let (v, err) = gostruct::unmarshal(&TOOL_BODY, m.content.as_bytes());
        match (err, gostruct::field(&TOOL_BODY, &v, "ok")) {
            (None, Some(Val::Ptr(Some(b)))) if matches!(**b, Val::Bool(_)) => {
                let Val::Bool(ok) = **b else { unreachable!() };
                md.insert("ok".into(), Any::Bool(ok));
            }
            _ => {
                md.entry("ok".into()).or_insert(Any::Bool(false));
            }
        }
    }
    if !md.is_empty() {
        e.insert("metadata".into(), scrub_value(&Any::Object(md), scrub));
    }
    if !m.message_id.is_empty() {
        e.insert("message_id".into(), Any::String(m.message_id.clone()));
    }
    Any::Object(e)
}

/// Go's `Transcript`.
pub fn transcript(msgs: &[Message], scrub: Option<&dyn Fn(&str) -> String>) -> Any {
    Any::Array(msgs.iter().map(|m| entry(m, scrub)).collect())
}

/// Go's `json.Marshal(Result(res, scrub))`: a run result as both servers answer it, its keys in Go's (sorted) order, the
/// usage in its struct's order; `stop_reason` null when the backend gave none.
pub fn result_json(res: &RunResult, scrub: Option<&dyn Fn(&str) -> String>) -> Result<String, crate::gojson::Error> {
    let id = |s: &str| s.to_owned();
    let sc: &dyn Fn(&str) -> String = scrub.unwrap_or(&id);
    let j = crate::gojson::marshal_any;
    let stop = if res.stop_reason.is_empty() { Any::Null } else { Any::String(res.stop_reason.clone()) };
    Ok(format!(
        "{{\"final_output\":{},\"session_id\":{},\"stop_reason\":{},\"tool_calls\":{},\"total_cost_usd\":{},\"transcript\":{},\"turns\":{},\"usage\":{}}}",
        j(&Any::String(sc(&res.final_output)))?,
        j(&Any::String(res.session_id.clone()))?,
        j(&stop)?,
        res.tool_calls,
        j(&Any::Number(res.total_cost_usd))?,
        j(&transcript(&res.transcript, Some(sc)))?,
        res.turns,
        res.usage.to_json()
    ))
}

// ---- building an agent -------------------------------------------------------------------------------------------------

/// Go's `Spec`: everything needed to build one agent.
pub struct Spec {
    pub model: ModelConfig,
    pub runtime: RuntimeConfig,
    /// None: all tools; Some (even empty): exactly those
    pub allowlist: Option<Vec<String>>,
    pub prompt: crate::agent::prompt::PromptOptions,
    /// "" = the user's real home
    pub home: String,
    /// None: every variable reads as ""
    pub getenv: Option<crate::cli::Getenv>,
    /// the model transport; None: the HTTP client
    pub client: Option<Arc<dyn Completer>>,
    pub approver: Option<Approver>,
    /// let the agent ask the backend for its context window
    pub discover: bool,
    /// consult ~/.sarina/tiers.json
    pub use_tiers: bool,
    /// adds the claude_cli tier (operator flags only, never request data)
    pub escalation: Option<EscalationOptions>,
    /// gives the model the read-only `agent` tool
    pub subagents: bool,
    /// the stable built-in tools; None: the Go build's (`cli::chat::builtins`)
    pub builtins: Option<Vec<Tool>>,
    /// the owner's signed list (Go's `trust.Default()`); None: the default folder's
    pub trust: Option<crate::cli::Getenv>,
}

/// Go's `Built`.
pub struct Built {
    pub agent: Agent,
    /// None unless more than one tier is configured
    pub router: Option<Arc<Router>>,
    pub notices: Vec<String>,
}

/// Go's `Build`. The allowlist is part of the agent's tool builder, so it survives every rebuild of the registry.
pub fn build(s: Spec) -> Result<Built, String> {
    let getenv: Arc<dyn Fn(&str) -> String + Send + Sync> = s.getenv.clone().unwrap_or_else(|| Arc::new(|_: &str| String::new()));
    let mut notices = Vec::new();
    let m = s.model.clone();
    let mut rt = s.runtime.clone();
    let base: Arc<dyn Completer> = s.client.clone().unwrap_or_else(|| Arc::new(crate::llm::client::Client::new(m.clone())));
    let mut client = base.clone();
    let mut tracker: Option<Arc<dyn TierTracker>> = None;
    let mut router = None;
    if s.use_tiers || s.escalation.is_some() {
        let (mut cfg, err) =
            if s.use_tiers { crate::tiers::config::load(&s.home, Some(&*getenv)) } else { (crate::tiers::config::default(), None) };
        if let Some(e) = err {
            notices.push(format!("ignoring the tiers config, staying on the local backend only: {e}"));
        } else {
            if let Some(mut o) = s.escalation.clone() {
                o.accept_edits = rt.permissions.allow_file_write || rt.permissions.allow_shell_commands;
                o.extra_dirs = Some(rt.additional_working_directories.clone());
                cfg = crate::tiers::config::from_escalation(&cfg, &crate::config::EscalationConfig::default(), &o, Some(&*getenv));
            }
            for w in cfg.warnings.clone().unwrap_or_default() {
                notices.push(format!("tier warning: {w}"));
            }
            if cfg.multi() {
                let g = getenv.clone();
                let b2 = base.clone();
                let r = Router::new(crate::tiers::router::Options {
                    config: cfg,
                    base: m.clone(),
                    base_client: Some(Box::new(ArcCompleter(b2))),
                    getenv: Box::new(move |k: &str| g(k)),
                    workdir: rt.cwd.clone(),
                    wake: Some(crate::tiers::wake::new_wake(rt.permissions.clone(), rt.cwd.clone())),
                    ..crate::tiers::router::Options::default()
                });
                if let Ok(r) = r {
                    // one Router is the agent's transport and its tier tracker, on the agent's thread
                    #[allow(clippy::arc_with_non_send_sync)]
                    let r = Arc::new(r);
                    client = r.clone();
                    tracker = Some(r.clone());
                    router = Some(r);
                }
            }
        }
    }
    let trust = s.trust.clone().unwrap_or_else(|| {
        let c = Arc::new(crate::trust::Checker::new(crate::trust::default_dir(&|k: &str| std::env::var(k).unwrap_or_default()).as_bytes()));
        Arc::new(move |p: &str| c.refuse(p.as_bytes()))
    });
    let pol = crate::policy::load(&crate::policy::Options {
        home: s.home.clone(),
        getenv: &*getenv,
        workspace: rt.cwd.clone(),
        trust: Some(&*trust),
    });
    if !pol.fatal.is_empty() {
        return Err(pol.fatal.join("; "));
    }
    notices.extend(pol.notices.iter().cloned());
    pol.apply_budget(&mut rt.budget_config);
    let mut session_env = BTreeMap::new();
    for name in &pol.safe_env {
        let v = getenv(name);
        if !v.is_empty() {
            session_env.insert(name.clone(), v);
        }
    }
    let discoverer: Option<Arc<dyn crate::agent::run::WindowDiscoverer>> =
        if s.discover { Some(Arc::new(crate::llm::discover::Discoverer::default())) } else { None };
    let profiles = s.subagents.then(|| {
        Arc::new(crate::agents::load(&crate::agents::Options {
            home: s.home.clone(),
            getenv: &*getenv,
            trusted: pol.trusted,
            trust: Some(&*trust),
        }))
    });
    let pol = Arc::new(pol);
    let p2 = pol.clone();
    let g = getenv.clone();
    let agent = Agent::new(AgentOptions {
        model: m,
        runtime: rt,
        client,
        tools: Builder {
            allowlist: s.allowlist.clone(),
            builtins: s.builtins.unwrap_or_else(crate::cli::chat::builtins),
            deny: Some(Arc::new(move |t: &str| p2.denied(t))),
            ..Builder::default()
        },
        prompt: s.prompt,
        context_window: 0,
        now: Arc::new(crate::gotime::Time::now),
        home: s.home,
        on_event: None,
        getenv: Arc::new(move |k: &str| g(k)),
        discoverer,
        scanner: None,
        tiers: tracker,
        session_env,
        parent_link: None,
        approver: s.approver,
        confirm: None,
        subagents: profiles,
    })?;
    Ok(Built { agent, router, notices })
}

/// A shared transport handed to the Router, which wants its own box.
struct ArcCompleter(Arc<dyn Completer>);

impl Completer for ArcCompleter {
    fn complete(
        &self,
        ctx: &crate::goctx::Ctx,
        req: &crate::llm::openai::Request,
        on: Option<&mut dyn FnMut(crate::llm::openai::StreamEvent)>,
    ) -> Result<crate::llm::types::Turn, crate::llm::client::Error> {
        self.0.complete(ctx, req, on)
    }
}
