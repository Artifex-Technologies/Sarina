//! Go: `go/agent/agent.go` with the agent's parts of `plan.go`, `notes.go`, `summarize.go`, `window.go` and `tiers.go`: the
//! turn loop. A run calls the model, dispatches its tool calls through the registry, checks the answer (the carry-on
//! nudges, then the one claim check), keeps the transcript inside the window (clearing, snipping, notes, a summary) and
//! saves the session file after every prompt, and runs read-only sub-agents for the `agent` tool (`subagent.go`).

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::claims::*;
use super::compact::*;
use super::limits::*;
use super::prompt::*;
use super::secrets::{hide_all, hide_secrets_read, StreamRedactor};
use super::Event;
use crate::agents::{Catalog, Profile, MAX_TURNS_LIMIT};
use crate::config::{as_int, BudgetConfig, ContextConfig, ContextSettings, ModelConfig, Permissions, RuntimeConfig};
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gopath;
use crate::gotext::{self, trim_space};
use crate::gotime::Time;
use crate::llm::client::Error as LlmError;
use crate::llm::openai::{Request, StreamEvent};
use crate::llm::types::{self as llm, ToolCall, ToolSpec, Turn, Usage};
use crate::llm::Completer;
use crate::session::message::{
    not_run_message, tool_result_body, KIND_COMPACTION_REPLAY, KIND_COMPACT_BOUNDARY, KIND_CONTINUATION, KIND_FILE_HISTORY_REPLAY,
    KIND_SNIPPED, REMINDER_CLOSE, REMINDER_OPEN, STATE_FINAL, STATE_STREAMING,
};
use crate::session::redact::redact;
use crate::session::{Message, OrderedMap, PlanState, Session, PLAN_ACTIVE, PLAN_NORMAL};
use crate::tools::fsutil::state_dir;
use crate::tools::registry::{Approver, Builder, Class, Confirm, Context, Output, Permission, Registry, Tool, ToolResult};

/// Go's `maxNotesBytes`: a session's notes are read back up to 1 MiB.
const MAX_NOTES_BYTES: u64 = 1 << 20;

pub const STOP_MAX_TURNS: &str = "max_turns";
pub const STOP_BUDGET_EXCEEDED: &str = "budget_exceeded";
pub const STOP_BACKEND_ERROR: &str = "backend_error";
pub const STOP_PROMPT_TOO_LONG: &str = "prompt_too_long";
pub const STOP_INTERRUPTED: &str = "interrupted";
pub const STOP_PLAN_PENDING: &str = "plan_pending";

/// Go's `ToolAgent`: the model-facing sub-agent tool.
pub const TOOL_AGENT: &str = "agent";
/// The agent tool's handler result that hands the call to the loop.
const SUBAGENT_MARKER: &str = "__sarina_subagent_call";
const DEFAULT_CHILD_TURNS: i64 = 6;
const DEFAULT_CHILD_TOOL_CALLS: i64 = 40;

/// Go's `childDenied`: tools a child never gets.
fn child_denied(name: &str) -> bool {
    matches!(
        name,
        TOOL_AGENT | "skill" | "TaskOutput" | "TaskStop" | "todo_write" | "update_plan" | "plan_get" | "enter_plan_mode" | "exit_plan_mode"
    ) || name.starts_with("task_")
}

pub const PLAN_APPROVE: &str = "approve";
pub const PLAN_REVISE: &str = "revise";
pub const PLAN_REJECT: &str = "reject";

/// Go's `PlanDecision`: the operator's answer to a submitted plan; an empty action is "no answer now".
#[derive(Clone, Debug, Default)]
pub struct PlanDecision {
    pub action: String,
    pub feedback: String,
}

/// Go's `WindowDiscoverer`: the backend's context window (0 = not reported) and where it came from.
pub trait WindowDiscoverer {
    fn context_window(&self, model: &ModelConfig) -> (i64, String);
}

/// Go's `TierTracker` (the tiers Router is one).
pub trait TierTracker {
    fn begin_turn(&self, emit: Box<dyn FnMut(Event)>);
    fn record(&self, prev: Option<&Any>) -> Option<Event>;
}

/// The gate notes and summaries pass before they are put back in front of the model: (blocked, reason).
pub type Scanner = Arc<dyn Fn(&str) -> (bool, String)>;

/// A live listener for the loop's events (streamed output).
pub type EventHook = Arc<dyn Fn(&Event)>;

/// Go's `PlanDecider`: the operator's interactive answer to a submitted plan.
pub type PlanDecider = Arc<dyn Fn(&str) -> PlanDecision>;

/// Go's `agent.Options` (the parts ported so far).
#[derive(Clone)]
pub struct Options {
    pub model: ModelConfig,
    pub runtime: RuntimeConfig,
    pub client: Arc<dyn Completer>,
    pub tools: Builder,
    pub prompt: PromptOptions,
    /// 0 = SARINA_CONTEXT_WINDOW, else what the backend reports, else inferred from the model's name
    pub context_window: i64,
    pub now: Arc<dyn Fn() -> Time>,
    pub home: String,
    pub on_event: Option<EventHook>,
    pub getenv: Arc<dyn Fn(&str) -> String>,
    pub discoverer: Option<Arc<dyn WindowDiscoverer>>,
    pub scanner: Option<Scanner>,
    pub tiers: Option<Arc<dyn TierTracker>>,
    pub session_env: BTreeMap<String, String>,
    pub parent_link: Option<BTreeMap<String, Any>>,
    pub approver: Option<Approver>,
    /// Go's `Options.Confirm`: lets a tool ask the operator one yes/no question per call (None: no one can be asked).
    /// Sub-agents get the parent's, as its Approver.
    pub confirm: Option<Confirm>,
    /// read-only profiles the `agent` tool can run as bounded child agents (a child never gets the tool)
    pub subagents: Option<Arc<Catalog>>,
}

/// Go's `Result`.
#[derive(Clone, Debug, Default)]
pub struct RunResult {
    pub final_output: String,
    pub turns: i64,
    pub tool_calls: i64,
    pub transcript: Vec<Message>,
    pub events: Vec<Event>,
    pub usage: Usage,
    pub total_cost_usd: f64,
    pub stop_reason: String,
    pub file_history: Vec<BTreeMap<String, Any>>,
    pub session_id: String,
    pub session_path: String,
    pub scratchpad_directory: String,
    pub plan: String,
}

/// The agent (Go's `Agent`).
pub struct Agent {
    opts: Options,
    registry: Registry,
    tool_ctx: Context,
    plan: PlanState,
    plan_on: Arc<AtomicBool>,
    decider: Option<PlanDecider>,
    narrow: Option<Vec<String>>,
    ctx_settings: ContextSettings,
    subagent_call: Arc<Mutex<Option<BTreeMap<String, Any>>>>,
    /// what the tier tracker emitted (during a model call), taken into the run's events in order
    tier_events: Arc<Mutex<Vec<Event>>>,
    pub cumulative_usage: Usage,
    pub cumulative_cost_usd: f64,
    /// the registry's tools as each rebuild leaves them, for a reader outside the loop ([`Agent::watch_registry`])
    registry_watch: Option<RegistryWatch>,
}

/// Go's `Registry()` read while the loop runs (the GUI names a starting tool's effect from the registry as it is then):
/// each rebuilt registry's tools and what they need.
pub type RegistryWatch = Arc<Mutex<BTreeMap<String, crate::tools::registry::Permission>>>;

/// Go's `run`: one Run or Resume in progress.
struct Run {
    sess: Session,
    events: Vec<Event>,
    usage: Usage,
    cost: f64,
    counts: Counters,
    base_turn: i64,
    turns_run: i64,
    esc: i64,
    done: HashSet<String>,
    segments: Vec<String>,
    last_text: String,
    no_think: bool,
    tool_turns_since: i64,
    compact_failures: i64,
}

fn ev(pairs: Vec<(&str, Any)>) -> Event {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn i(n: i64) -> Any {
    Any::Int(n)
}

/// A usage as an event holds it: Go puts the struct itself in the map, so JSON writes its fields in the struct's order, not
/// sorted; the text is kept as it is written (`Any::Num` is written verbatim).
fn usage_any(u: &Usage) -> Any {
    Any::Num(u.to_json())
}

/// Go's `firstNonNil` and `%v` of a history value.
fn sprint(v: Option<&Any>) -> String {
    match v {
        None => "<nil>".into(),
        Some(a) => crate::session::sprint(a),
    }
}

/// Go's `session.NewID`: 16 random bytes as a UUID4, in hex.
pub fn new_session_id() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 16];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Go's `t.Format("2006-01-02")` in the time's own zone.
fn date(t: &Time) -> String {
    Time::from_unix(t.unix() + t.offset() as i64, 0).format_rfc3339_utc()[..10].to_owned()
}

fn clip_runes(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_owned(),
    }
}

fn chunks(s: &str, size: usize) -> Vec<String> {
    let r: Vec<char> = s.chars().collect();
    r.chunks(size).map(|c| c.iter().collect()).collect()
}

fn is_finish_cut(f: &str) -> bool {
    f == "length" || f == "max_tokens"
}

const NOTES_PROMPT: &str = "You maintain the working notes for a coding-agent session so the agent can keep going after its older conversation is discarded. You will receive the previous notes (possibly empty) and the newest part of the transcript. Rewrite the notes so they are accurate and complete as of now.\n\nReply with the notes only: markdown with exactly these six second-level headings, in this order, and nothing before or after them:\n\n## Task / goal\n## Current state\n## Files touched\n## Decisions\n## Open questions\n## Next steps\n\nRules: be concrete (paths, commands, error text, identifiers). Keep what the previous notes got right, correct what is now wrong, drop what no longer matters. Record the user's requests and constraints faithfully. Write \"(none)\" under a heading with nothing to say. Never copy secrets, tokens or passwords into the notes. Do not call tools. Keep it under about 600 words.";

const SUMMARY_PROMPT: &str = "You are condensing the older part of a coding-agent conversation so the agent can continue with a much shorter history. You will receive the transcript to condense, and sometimes working notes written earlier. Everything you write replaces that older history, so anything you leave out is lost to the agent.\n\nAnswer in plain text only. Do not call tools. Put the summary between <summary> and </summary> tags, using these headings in this order:\n\n1. User's goal and standing instructions\n2. Everything the user asked for, in order (every user message, close to verbatim)\n3. Work done so far (what was tried, what worked)\n4. Files and commands involved, and why they matter (paths, names, key snippets)\n5. Errors met and how they were fixed or worked around\n6. Decisions taken and facts learned that must not be re-discovered\n7. Work still pending\n8. Exact current state and the very next step, consistent with the user's latest request\n\nBe specific and compact. Never include secrets, tokens or passwords; refer to them by name only.";

const MIN_WINDOW_TURNS: i64 = 2;
const NOTES_TOOL_RESULT_MAX: usize = 1500;
const BREAKER_LIMIT: i64 = 3;
const MAX_SUMMARY_RETRY: i64 = 3;
const SUMMARY_TOOL_CHARS: usize = 2000;

impl Agent {
    /// Go's `New`.
    pub fn new(mut opts: Options) -> Result<Agent, String> {
        if opts.runtime.cwd.is_empty() {
            return Err("agent: runtime cwd is required".into());
        }
        if opts.home.is_empty() {
            opts.home = (opts.getenv)("HOME");
        }
        let ctx_settings = ContextConfig::settings(opts.runtime.context_management.as_ref(), &*opts.getenv);
        let plan_on = Arc::new(AtomicBool::new(false));
        let subagent_call = Arc::new(Mutex::new(None));
        if let (Some(cat), None) = (&opts.subagents, &opts.parent_link) {
            let t = Agent::agent_tool(cat, subagent_call.clone());
            opts.tools.extra.push(t);
        }
        let mut a = Agent {
            subagent_call,
            tier_events: Arc::new(Mutex::new(Vec::new())),
            registry: Registry::new(Vec::new()),
            tool_ctx: Context::default(),
            plan: PlanState { mode: PLAN_NORMAL.into(), ..PlanState::default() },
            plan_on,
            decider: None,
            narrow: None,
            ctx_settings,
            cumulative_usage: Usage::default(),
            cumulative_cost_usd: 0.0,
            registry_watch: None,
            opts,
        };
        let cwd = a.opts.runtime.cwd.clone();
        a.rebuild_tools(&cwd)?;
        Ok(a)
    }

    pub fn set_event_hook(&mut self, f: Option<EventHook>) {
        self.opts.on_event = f;
    }

    /// Go's `SetConfirm`: installs (or with None removes) the per-call operator question; only that channel changes,
    /// and a rebuilt tool context keeps it.
    pub fn set_confirm(&mut self, f: Option<Confirm>) {
        self.opts.confirm.clone_from(&f);
        self.tool_ctx.confirm = f;
    }

    pub fn set_plan_decider(&mut self, d: Option<PlanDecider>) {
        self.decider = d;
    }

    pub fn tool_count(&self) -> usize {
        self.registry.len()
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn plan_state(&self) -> &PlanState {
        &self.plan
    }

    /// Go's `rebuildTools`: the registry and tool context for a workspace.
    fn rebuild_tools(&mut self, cwd: &str) -> Result<(), String> {
        let mut abs = gopath::abs(cwd.as_bytes()).map_err(|e| e.to_string())?;
        if let Ok(real) = gopath::eval_symlinks(&abs) {
            abs = real;
        }
        let abs = gotext::lossy(&abs);
        self.opts.runtime.cwd = abs.clone();
        let rt = &self.opts.runtime;
        let plan_on = self.plan_on.clone();
        self.tool_ctx = Context {
            root: abs,
            command_timeout: std::time::Duration::from_nanos(crate::config::go_int(rt.command_timeout_seconds * 1e9).max(0) as u64),
            max_output_chars: rt.max_output_chars,
            permissions: rt.permissions.clone(),
            session_directory: rt.session_directory.clone(),
            session_env: self.opts.session_env.clone(),
            approver: self.opts.approver.clone(),
            confirm: self.opts.confirm.clone(),
            plan_active: Some(Arc::new(move || plan_on.load(Ordering::SeqCst))),
            ..Context::default()
        };
        self.refresh_registry();
        Ok(())
    }

    /// Go's `effectiveBuilder`.
    fn effective_builder(&self) -> Builder {
        let mut b = self.opts.tools.clone();
        b.plan_mode = b.plan_mode || self.plan.active();
        b.perms = Some(self.opts.runtime.permissions.clone());
        b.narrow = match (&b.narrow, &self.narrow) {
            (None, n) => n.clone(),
            (Some(fixed), Some(n)) => Some(fixed.iter().filter(|x| n.contains(x)).cloned().collect()),
            (Some(fixed), None) => Some(fixed.clone()),
        };
        b
    }

    fn refresh_registry(&mut self) {
        self.registry = self.effective_builder().build();
        if let Some(w) = &self.registry_watch {
            *w.lock().unwrap_or_else(|p| p.into_inner()) = self.registry.requirements();
        }
    }

    /// The registry's tools as every rebuild leaves them (Go's `Registry()` from another goroutine).
    pub fn watch_registry(&mut self) -> RegistryWatch {
        let w = Arc::new(Mutex::new(self.registry.requirements()));
        self.registry_watch = Some(w.clone());
        w
    }

    /// Go's `baseToolNames`: the allowlist's tools alone, for the system prompt.
    fn base_tool_names(&self) -> Vec<String> {
        let mut b = self.opts.tools.clone();
        b.perms = Some(self.opts.runtime.permissions.clone());
        b.build().names()
    }

    /// Go's `NarrowTools`.
    pub fn narrow_tools(&mut self, names: Option<Vec<String>>) {
        let Some(names) = names else { return };
        self.narrow = Some(match &self.narrow {
            None => names,
            Some(cur) => cur.iter().filter(|n| names.contains(n)).cloned().collect(),
        });
        self.refresh_registry();
    }

    fn end_run(&mut self) {
        if self.narrow.is_some() {
            self.narrow = None;
            self.refresh_registry();
        }
    }

    /// Go's `setPlan`.
    fn set_plan(&mut self, mut p: PlanState, s: Option<&mut Session>) {
        if p.mode.is_empty() {
            p.mode = PLAN_NORMAL.into();
        }
        self.plan = p;
        self.plan_on.store(self.plan.active(), Ordering::SeqCst);
        if let Some(s) = s {
            s.set_plan(&self.plan);
        }
        self.refresh_registry();
    }

    /// Go's `EnterPlan`.
    pub fn enter_plan(&mut self, s: Option<&mut Session>) {
        if self.plan.active() {
            return;
        }
        let mut p = self.plan.clone();
        p.mode = PLAN_ACTIVE.into();
        p.pending.clear();
        match s {
            Some(s) => {
                self.set_plan(p, Some(s));
                let id = s.next_message_id("plan_mode");
                s.append(Message {
                    role: "user".into(),
                    content: reminder(PLAN_MODE_NOTICE),
                    message_id: id,
                    metadata: [("kind".to_owned(), s_any("plan_mode_entered"))].into(),
                    ..Message::default()
                });
            }
            None => self.set_plan(p, None),
        }
    }

    /// Go's `applyDecision`.
    fn apply_decision(&mut self, s: Option<&mut Session>, d: &PlanDecision) -> Result<String, String> {
        let mut p = self.plan.clone();
        if !p.active() {
            return Err("plan mode is not on".into());
        }
        match d.action.as_str() {
            PLAN_APPROVE => {
                if p.pending.is_empty() {
                    return Err("there is no submitted plan to approve".into());
                }
                let plan = std::mem::take(&mut p.pending);
                p.mode = PLAN_NORMAL.into();
                p.note = plan.clone();
                self.set_plan(p, s);
                Ok(format!(
                    "The operator approved the plan. Plan mode is off and your normal tools are available again. Approved plan:\n{plan}"
                ))
            }
            PLAN_REVISE => {
                if p.pending.is_empty() {
                    return Err("there is no submitted plan to revise".into());
                }
                p.pending.clear();
                self.set_plan(p, s);
                let fb = trim_space(&d.feedback);
                let fb = if fb.is_empty() { "(no details given)" } else { fb };
                Ok(format!("The operator did not approve the plan and asks for changes:\n{fb}\nPlan mode is still on. Revise the plan and submit it again with exit_plan_mode."))
            }
            PLAN_REJECT => {
                p.mode = PLAN_NORMAL.into();
                p.pending.clear();
                self.set_plan(p, s);
                Ok("The operator rejected the plan. Plan mode is off and the plan is discarded; do not carry it out.".into())
            }
            a => Err(format!("unknown plan decision {} (want approve, revise or reject)", gotext::quote(a))),
        }
    }

    /// Go's `DecidePlan`.
    pub fn decide_plan(&mut self, mut s: Option<&mut Session>, d: &PlanDecision) -> Result<String, String> {
        let text = self.apply_decision(s.as_deref_mut(), d)?;
        if let Some(s) = s {
            let id = s.next_message_id("plan_decision");
            s.append(Message {
                role: "user".into(),
                content: reminder(&text),
                message_id: id,
                metadata: [("kind".to_owned(), s_any("plan_decision")), ("decision".to_owned(), s_any(&d.action))].into(),
                ..Message::default()
            });
        }
        Ok(text)
    }

    /// Go's `AdoptPlan`.
    pub fn adopt_plan(&mut self, s: &Session) {
        self.set_plan(s.plan(), None);
    }

    /// Go's `onPlanAction`: the plan a tool reported, maybe awaiting approval (then the run ends).
    fn on_plan_action(&mut self, sess: &mut Session, action: &str, plan_arg: Option<&Any>, res: &mut ToolResult) -> String {
        match action {
            "enter" => {
                let mut p = self.plan.clone();
                p.mode = PLAN_ACTIVE.into();
                p.pending.clear();
                self.set_plan(p, Some(sess));
                String::new()
            }
            "exit" => {
                let plan = match plan_arg {
                    Some(Any::String(p)) => p.clone(),
                    _ => String::new(),
                };
                let mut p = self.plan.clone();
                p.pending = plan.clone();
                self.set_plan(p, Some(sess));
                if let Some(dec) = self.decider.clone() {
                    let d = dec(&plan);
                    if !d.action.is_empty() {
                        if let Ok(text) = self.apply_decision(Some(sess), &d) {
                            res.content = text;
                            res.metadata.insert("plan_decision".into(), s_any(&d.action));
                            return String::new();
                        }
                    }
                }
                res.content = "Plan submitted. It is waiting for the operator's decision; plan mode stays on until then.".into();
                plan
            }
            _ => String::new(),
        }
    }

    /// Go's `offeredTools`.
    fn offered_tools(&self) -> Vec<String> {
        self.registry
            .names()
            .into_iter()
            .filter(|n| {
                !(n.starts_with("task_")
                    || n.starts_with("plan_")
                    || matches!(
                        n.as_str(),
                        "TaskOutput" | "TaskStop" | "todo_write" | "update_plan" | "enter_plan_mode" | "exit_plan_mode"
                    ))
            })
            .collect()
    }

    /// Go's `resolveWindow`.
    fn resolve_window(&self) -> (i64, String) {
        if self.opts.context_window > 0 {
            return (self.opts.context_window, "flag".into());
        }
        let n = crate::config::context_window_from_env(&*self.opts.getenv);
        if n > 0 {
            return (n, "env".into());
        }
        if self.ctx_settings.discovery {
            if let Some(d) = &self.opts.discoverer {
                let (n, src) = d.context_window(&self.opts.model);
                if n > 0 {
                    return (n, format!("backend:{src}"));
                }
            }
        }
        (infer_context_window(&self.opts.model.model), "inferred".into())
    }

    fn window(&self) -> i64 {
        self.resolve_window().0
    }

    fn scan(&self, text: &str) -> (bool, String) {
        match &self.opts.scanner {
            Some(f) => f(text),
            None => {
                let v = crate::guard::Gate::default_gate().scan(text.as_bytes(), &*self.opts.getenv);
                (v.blocked, v.reason)
            }
        }
    }

    // ---- sessions ------------------------------------------------------------------------------------------------------

    /// Go's `Run`.
    pub fn run(&mut self, ctx: &Ctx, prompt: &str) -> (Option<RunResult>, Option<String>) {
        self.refresh_registry();
        match self.new_session_for(prompt) {
            Ok(s) => self.drive(ctx, s, prompt, false),
            Err(e) => {
                self.end_run();
                (None, Some(e))
            }
        }
    }

    /// Go's `Resume`.
    pub fn resume(&mut self, ctx: &Ctx, prompt: &str, mut stored: Session) -> (Option<RunResult>, Option<String>) {
        stored.migrate_retired_tools();
        stored.messages = crate::session::message::repair_pairing(&stored.messages);
        self.set_plan(stored.plan(), None);
        self.append_replays(&mut stored);
        let rt = &self.opts.runtime;
        if state_dir(&rt.cwd, &rt.scratchpad_root).is_ok() && stored.scratchpad_directory.is_empty() {
            let dir = join(&rt.scratchpad_root, &stored.id);
            if crate::goos::mkdir_all(dir.as_bytes()).is_ok() {
                stored.scratchpad_directory = dir;
            }
        }
        self.drive(ctx, stored, prompt, true)
    }

    fn new_session_for(&mut self, prompt: &str) -> Result<Session, String> {
        let rt = self.opts.runtime.clone();
        let mut s = Session { id: new_session_id(), model: self.opts.model.clone(), runtime: rt.clone(), ..Session::default() };
        state_dir(&rt.cwd, &rt.scratchpad_root).map_err(|e| format!("cannot create scratchpad: {e}"))?;
        let scratch = join(&rt.scratchpad_root, &s.id);
        crate::goos::mkdir_all(scratch.as_bytes()).map_err(|e| format!("cannot create scratchpad: {e}"))?;
        s.scratchpad_directory = scratch.clone();
        let mut user_ctx = OrderedMap::default();
        let sys_ctx = OrderedMap::default();
        user_ctx.set("currentDate", &format!("Today's date is {}.", date(&(self.opts.now)())));
        if !rt.disable_claude_md_discovery {
            let g = discover_guidance(&rt.cwd, &rt.additional_working_directories, &self.opts.home);
            if !g.is_empty() {
                user_ctx.set("claudeMd", &g);
            }
            let block = curated_memory(&rt.cwd, &self.opts.home, prompt);
            if !block.is_empty() {
                user_ctx.set("curatedMemory", &block);
            }
        }
        let gs = git_snapshot(&rt.cwd);
        if !gs.is_empty() {
            user_ctx.set("gitStatus", &gs);
        }
        user_ctx.set(
            "scratchpad",
            &format!("Scratch space for temporary working files you create yourself (use it instead of /tmp): {scratch}. Files the user asks for belong in the workspace, not here."),
        );
        let env = EnvInfo {
            cwd: rt.cwd.clone(),
            is_git: is_git_repo(&rt.cwd),
            extra_dirs: rt.additional_working_directories.clone(),
            model: self.opts.model.model.clone(),
        };
        s.system_prompt_parts = build_system_prompt_parts(&self.opts.prompt, &env, &self.base_tool_names(), &rt.permissions);
        s.user_context = user_ctx.clone();
        s.system_context = sys_ctx.clone();
        s.set_plan(&self.plan);
        if let Some(link) = &self.opts.parent_link {
            s.plugin_state.insert("subagent".into(), Any::Object(link.clone()));
        }
        let sys = render_system_message(&s.system_prompt_parts, &sys_ctx);
        s.append(Message { role: "system".into(), content: sys, message_id: "system_0".into(), ..Message::default() });
        if !user_ctx.0.is_empty() {
            s.append(Message {
                role: "user".into(),
                content: render_user_context(&user_ctx),
                message_id: "user_context_0".into(),
                ..Message::default()
            });
        }
        if self.plan.active() {
            let id = s.next_message_id("plan_mode");
            s.append(Message {
                role: "user".into(),
                content: reminder(PLAN_MODE_NOTICE),
                message_id: id,
                metadata: [("kind".to_owned(), s_any("plan_mode_entered"))].into(),
                ..Message::default()
            });
        }
        Ok(s)
    }

    /// Go's `appendReplays`: the file-history and compaction replay reminders on resume.
    fn append_replays(&self, s: &mut Session) {
        let n = s.file_history.len();
        if n > 0 && !replay_count_matches(&s.messages, n as i64) {
            let mut order: Vec<String> = Vec::new();
            let mut snaps = 0;
            for e in &s.file_history {
                if let Some(Any::String(p)) = e.get("path") {
                    if !order.contains(p) {
                        order.push(p.clone());
                    }
                }
                if e.contains_key("after_snapshot_id") {
                    snaps += 1;
                }
            }
            let mut b = format!("{REMINDER_OPEN}\n");
            b.push_str(&format!("This session already has {n} recorded tool actions touching {} distinct paths.\n", order.len()));
            if !order.is_empty() {
                b.push_str(&format!("Paths: {}", order.iter().take(4).cloned().collect::<Vec<_>>().join(", ")));
                if order.len() > 4 {
                    b.push_str(&format!(" +{} more", order.len() - 4));
                }
                b.push('\n');
            }
            let start = n.saturating_sub(10);
            for e in &s.file_history[start..] {
                let name = e.get("tool_name").or_else(|| e.get("action"));
                b.push_str(&format!(
                    "- {} #{} turn {} path={} command={}\n",
                    if name.is_some() { sprint(name) } else { String::new() },
                    sprint(e.get("history_entry_id")),
                    sprint(e.get("turn_index")),
                    sprint(e.get("path")),
                    sprint(e.get("command"))
                ));
            }
            if start > 0 {
                b.push_str(&format!("({start} older entries omitted)\n"));
            }
            b.push_str("Use this record to avoid repeating edits or commands that already happened.\n");
            b.push_str(REMINDER_CLOSE);
            s.append(Message {
                role: "user".into(),
                content: b,
                message_id: format!("file_history_replay_{n}"),
                metadata: [
                    ("kind".to_owned(), s_any(KIND_FILE_HISTORY_REPLAY)),
                    ("file_history_count".to_owned(), i(n as i64)),
                    ("file_history_unique_paths".to_owned(), i(order.len() as i64)),
                    ("file_history_snapshot_count".to_owned(), i(snaps)),
                ]
                .into(),
                ..Message::default()
            });
        }
        let (mut boundaries, mut snipped, mut has_replay) = (0, 0, false);
        let mut latest: Option<usize> = None;
        for (idx, m) in s.messages.iter().enumerate() {
            match m.kind() {
                KIND_COMPACT_BOUNDARY => {
                    boundaries += 1;
                    latest = Some(idx);
                }
                KIND_SNIPPED => snipped += 1,
                KIND_COMPACTION_REPLAY => has_replay = true,
                _ => {}
            }
        }
        if (boundaries > 0 || snipped > 0) && !has_replay {
            let mut b = format!("{REMINDER_OPEN}\n");
            b.push_str(&format!(
                "Earlier history in this session was reduced: {boundaries} compaction boundaries, {snipped} snipped messages.\n"
            ));
            if let Some(l) = latest {
                let m = &s.messages[l];
                b.push_str(&format!("Latest boundary: {} (depth {}).\n", m.message_id, sprint(m.metadata.get("compaction_depth"))));
            }
            b.push_str("Treat the surviving transcript and the summaries in it as authoritative.\n");
            b.push_str(REMINDER_CLOSE);
            s.append(Message {
                role: "user".into(),
                content: b,
                message_id: format!("compaction_replay_{boundaries}_{snipped}"),
                metadata: [("kind".to_owned(), s_any(KIND_COMPACTION_REPLAY))].into(),
                ..Message::default()
            });
        }
    }

    // ---- the run -------------------------------------------------------------------------------------------------------

    /// The tier tracker's events since the last look, into the run's (already shown to the live hook).
    fn take_tier_events(&self, r: &mut Run) {
        r.events.extend(self.tier_events.lock().unwrap_or_else(|p| p.into_inner()).drain(..));
    }

    fn emit(&self, r: &mut Run, e: Event) {
        if let Some(f) = &self.opts.on_event {
            f(&e);
        }
        r.events.push(e);
    }

    fn budget_text(&self, r: &mut Run) -> String {
        r.counts.usage = r.usage;
        r.counts.cost = r.cost;
        r.counts.session_turns = r.base_turn + r.turns_run;
        check_budget(&self.opts.runtime.budget_config, &r.counts)
    }

    fn drive(&mut self, ctx: &Ctx, mut sess: Session, prompt: &str, resumed: bool) -> (Option<RunResult>, Option<String>) {
        let rt = self.opts.runtime.clone();
        sess.model = self.opts.model.clone();
        sess.runtime = rt.clone();
        let mut r = Run {
            sess,
            events: Vec::new(),
            usage: Usage::default(),
            cost: 0.0,
            counts: Counters::default(),
            base_turn: 0,
            turns_run: 0,
            esc: 0,
            done: HashSet::new(),
            segments: Vec::new(),
            last_text: String::new(),
            no_think: trim_space(prompt).ends_with("/no_think"),
            tool_turns_since: 0,
            compact_failures: 0,
        };
        self.tool_ctx.ctx = ctx.clone();
        if let Some(t) = self.opts.tiers.clone() {
            let hook = self.opts.on_event.clone();
            let buf = self.tier_events.clone();
            t.begin_turn(Box::new(move |e| {
                if let Some(f) = &hook {
                    f(&e);
                }
                buf.lock().unwrap_or_else(|p| p.into_inner()).push(e);
            }));
            self.take_tier_events(&mut r);
        }
        self.refresh_registry();
        if resumed {
            r.usage = r.sess.usage;
            r.counts.tool_calls = r.sess.tool_calls;
            r.counts.model_calls = r.sess.budget_state.model_calls;
            r.esc = r.sess.budget_state.escalations;
            r.base_turn = r.sess.turns;
            r.counts.delegated = r.sess.file_history.iter().filter(|e| e.get("history_kind") == Some(&s_any("delegation"))).count() as i64;
        }
        r.cost = cost(&self.opts.model.pricing, &r.usage);
        let (start_usage, start_cost) = (r.usage, r.cost);
        let (n, src) = self.resolve_window();
        if src.starts_with("backend:") {
            self.emit(&mut r, ev(vec![("type", s("context_window")), ("tokens", i(n)), ("source", s(&src))]));
        }
        let id = r.sess.next_message_id("user");
        r.sess.append(Message { role: "user".into(), content: prompt.into(), message_id: id, ..Message::default() });

        macro_rules! finish {
            ($stop:expr, $out:expr, $turns:expr) => {
                return self.finish(r, $stop, $out, $turns, start_usage, start_cost)
            };
        }

        let why = self.budget_text(&mut r);
        if !why.is_empty() {
            finish!(STOP_BUDGET_EXCEEDED, &why, 0);
        }

        let max_turns = rt.max_turns;
        let (hist_start, msg_start) = (r.sess.file_history.len(), r.sess.messages.len());
        let (mut claim_checked, mut empty_retries, mut nudges, mut repeat_retries, mut cut_empty) = (false, 0, 0, 0, 0);
        let mut nudged_from = String::new();
        let mut sent_nudges: HashSet<String> = HashSet::new();
        let mut turn = 1;
        while turn <= max_turns {
            if ctx.err().is_some() {
                finish!(STOP_INTERRUPTED, "Interrupted before the model was called.", turn - 1);
            }
            let preserve = rt.compact_preserve_messages.max(0);
            let spec_tokens = tool_spec_tokens(&self.registry.specs());
            if let Some(e) = clear_tool_outputs(&mut r.sess, self.window(), spec_tokens, &self.ctx_settings, turn) {
                self.emit(&mut r, e);
            }
            if let Some(t) = rt.auto_snip_threshold_tokens.filter(|t| *t > 0) {
                let (_, evs) = reduce(&mut r.sess, &ReduceOpts { target: t, preserve, turn, ..Default::default() });
                self.emit_all(&mut r, evs);
            }
            if let Some(t) = rt.auto_compact_threshold_tokens.filter(|t| *t > 0) {
                let (_, evs) = reduce(&mut r.sess, &ReduceOpts { target: t, preserve, turn, ..Default::default() });
                self.emit_all(&mut r, evs);
                if message_tokens(&r.sess.messages) > t && !self.notes_compact(&mut r, turn, preserve, false) {
                    let notes = self.vetted_notes(&mut r, turn);
                    let (_, evs) =
                        reduce(&mut r.sess, &ReduceOpts { target: t, allow_compaction: true, preserve, turn, notes, ..Default::default() });
                    self.emit_all(&mut r, evs);
                }
            }
            if let Some(text) = self.preflight(ctx, &mut r, turn, preserve) {
                finish!(STOP_PROMPT_TOO_LONG, &text, turn - 1);
            }
            self.emit(&mut r, ev(vec![("type", s("turn_start")), ("turn_index", i(turn))]));
            let t = match self.call_model(ctx, &mut r, turn, preserve) {
                Ok(t) => t,
                Err(e) => {
                    if ctx.err().is_some() {
                        finish!(STOP_INTERRUPTED, "Interrupted while waiting for the model.", turn - 1);
                    }
                    if e.contains("repeat limit") && repeat_retries < 1 && turn < max_turns {
                        repeat_retries += 1;
                        r.sess.append(Message {
                            role: "user".into(),
                            message_id: format!("repeat_retry_{turn}"),
                            content: "Your last reply started repeating itself and was stopped by the server. Carry on from where you were, briefly: make the next tool call or give the answer, without repeating text.".into(),
                            ..Message::default()
                        });
                        self.emit(&mut r, ev(vec![("type", s("repeat_retry")), ("turn_index", i(turn))]));
                        turn += 1;
                        continue;
                    }
                    finish!(STOP_BACKEND_ERROR, &e, turn - 1);
                }
            };
            self.emit(
                &mut r,
                ev(vec![
                    ("type", s("assistant_message")),
                    ("turn_index", i(turn)),
                    ("content", s(&t.content)),
                    ("tool_calls", i(t.tool_calls.len() as i64)),
                ]),
            );
            r.counts.model_calls += 1;
            r.turns_run += 1;
            r.usage = r.usage.add(&t.usage);
            r.cost = cost(&self.opts.model.pricing, &r.usage);
            r.last_text = t.content.clone();

            let why = self.budget_text(&mut r);
            if !why.is_empty() {
                answer_pending(&mut r, &t.tool_calls, &format!("not run: {why}"));
                finish!(STOP_BUDGET_EXCEEDED, &why, turn);
            }

            if t.tool_calls.is_empty() {
                let msgs_from = msg_start.min(r.sess.messages.len());
                if trim_space(&t.content).is_empty() && is_finish_cut(&t.finish_reason) {
                    if cut_empty < 2 && turn < max_turns {
                        cut_empty += 1;
                        r.sess.append(Message {
                            role: "user".into(),
                            message_id: format!("empty_reply_{turn}"),
                            content: "Your last reply was all thinking and hit the length limit before any text or tool call. Do not think it through again: make the next tool call now, or answer in words from what you have. /no_think".into(),
                            ..Message::default()
                        });
                        self.emit(&mut r, ev(vec![("type", s("empty_reply_retry")), ("turn_index", i(turn)), ("reason", s("length"))]));
                        turn += 1;
                        continue;
                    }
                    let mut out = r.segments.join("");
                    if trim_space(&out).is_empty() {
                        out = "Stopped: my replies kept running past the length limit before I could answer. Ask again, or ask for one part of it.".into();
                    }
                    let stop = t.finish_reason.clone();
                    finish!(&stop, &out, turn);
                }
                if trim_space(&t.content).is_empty() && r.segments.is_empty() && empty_retries < 2 && turn < max_turns {
                    empty_retries += 1;
                    let names = self.offered_tools().join(", ");
                    let mut text = format!("Your last reply had no text and no tool call. If you planned a call to one of your tools ({names}), make it now; a tool that is not in that list does not exist here. Otherwise answer.");
                    if empty_retries == 2 {
                        text = "Your last two replies were empty. Answer in words now: what you can tell from what you have, and if the user asked for something none of your tools can do, say so plainly and give the command they can run themselves. Do not describe results you have not seen. /no_think".into();
                    }
                    r.sess.append(Message {
                        role: "user".into(),
                        message_id: format!("empty_reply_{turn}"),
                        content: text,
                        ..Message::default()
                    });
                    self.emit(&mut r, ev(vec![("type", s("empty_reply_retry")), ("turn_index", i(turn))]));
                    turn += 1;
                    continue;
                }
                r.segments.push(t.content.clone());
                if !nudged_from.is_empty() && !is_finish_cut(&t.finish_reason) {
                    r.segments = vec![std::mem::take(&mut nudged_from)];
                    let first = r.segments[0].clone();
                    self.emit(&mut r, ev(vec![("type", s("nudge_withdrawn")), ("turn_index", i(turn)), ("answer", s(&first))]));
                }
                if is_finish_cut(&t.finish_reason) {
                    let idx = r.segments.len() as i64;
                    r.sess.append(Message {
                        role: "user".into(),
                        message_id: format!("continuation_{turn}"),
                        content: "Your last reply hit the output length limit and was cut off. Carry on from exactly where it stopped and do not repeat what you already wrote.".into(),
                        metadata: [("kind".to_owned(), s_any(KIND_CONTINUATION)), ("continuation_index".to_owned(), i(idx))].into(),
                        ..Message::default()
                    });
                    self.emit(
                        &mut r,
                        ev(vec![
                            ("type", s("continuation_request")),
                            ("turn_index", i(turn)),
                            ("reason", s(&t.finish_reason)),
                            ("continuation_index", i(idx)),
                        ]),
                    );
                    turn += 1;
                    continue;
                }
                let offered = self.offered_tools();
                let answer = r.segments.join("");
                let msgs = r.sess.messages[msgs_from..].to_vec();
                let nudge = |a: &mut Agent, r: &mut Run, text: String, nudges: &mut i64| {
                    *nudges += 1;
                    r.sess.append(Message {
                        role: "user".into(),
                        message_id: format!("continue_{turn}"),
                        content: text,
                        ..Message::default()
                    });
                    a.emit(r, ev(vec![("type", s("continue_nudge")), ("turn_index", i(turn))]));
                    r.segments.clear();
                };
                if nudges < 2 && turn < max_turns && claims_no_tools(&answer, &msgs, &offered) {
                    nudge(self, &mut r, claims_no_tools_text(&offered), &mut nudges);
                    turn += 1;
                    continue;
                }
                if nudges < 2 && turn < max_turns {
                    let name = tool_named_not_called(&answer, &msgs, &offered);
                    if !name.is_empty() && !sent_nudges.contains(&format!("named:{name}")) {
                        sent_nudges.insert(format!("named:{name}"));
                        nudged_from = answer.clone();
                        nudge(self, &mut r, format!("Carry on: you named {name} as the next step, and it is your own tool. Call it now if it does what was asked, then answer with what it did."), &mut nudges);
                        turn += 1;
                        continue;
                    }
                    let hist = r.sess.file_history[hist_start.min(r.sess.file_history.len())..].to_vec();
                    let f = fix_pending_after_failed_check(&answer, &hist);
                    if !f.is_empty() && !edit_loop_stopped(&msgs) {
                        nudge(self, &mut r, fix_pending_text(&f), &mut nudges);
                        turn += 1;
                        continue;
                    }
                    if !sent_nudges.contains("check_code") && unreproduced_failure(prompt, &msgs, &offered) {
                        sent_nudges.insert("check_code".into());
                        nudge(self, &mut r, UNREPRODUCED_FAILURE_TEXT.into(), &mut nudges);
                        turn += 1;
                        continue;
                    }
                    let (name, line) = unread_definition(&answer, &msgs);
                    if !name.is_empty() && !sent_nudges.contains(&format!("unread:{name}")) {
                        sent_nudges.insert(format!("unread:{name}"));
                        nudge(self, &mut r, unread_definition_text(&name, line), &mut nudges);
                        turn += 1;
                        continue;
                    }
                    let can_read = self.base_tool_names().iter().any(|n| n == "read_file");
                    let f = denied_readable(&answer, prompt, &rt.cwd, &msgs, can_read);
                    if !f.is_empty() && !sent_nudges.contains(&format!("denied:{f}")) {
                        sent_nudges.insert(format!("denied:{f}"));
                        nudge(self, &mut r, denied_readable_text(&f), &mut nudges);
                        turn += 1;
                        continue;
                    }
                    let (name, how) = undecompiled_function(&answer, &msgs);
                    if !name.is_empty() && !sent_nudges.contains(&format!("decompile:{name}")) {
                        sent_nudges.insert(format!("decompile:{name}"));
                        nudge(self, &mut r, undecompiled_function_text(&name, &how), &mut nudges);
                        turn += 1;
                        continue;
                    }
                }
                if nudges < 2 && turn < max_turns && announced_not_done(&answer) {
                    nudge(self, &mut r, ANNOUNCED_TEXT.into(), &mut nudges);
                    turn += 1;
                    continue;
                }
                if !claim_checked && turn < max_turns {
                    let text = self.claim_check(&r, &answer, prompt, msg_start, hist_start);
                    if !text.is_empty() {
                        claim_checked = true;
                        r.sess.append(Message {
                            role: "user".into(),
                            message_id: format!("claim_check_{turn}"),
                            content: text,
                            ..Message::default()
                        });
                        self.emit(&mut r, ev(vec![("type", s("claim_check")), ("turn_index", i(turn))]));
                        r.segments.clear();
                        turn += 1;
                        continue;
                    }
                }
                let mut answer = unwrap_reminder(&r.segments.join(""));
                let msgs = r.sess.messages[msg_start.min(r.sess.messages.len())..].to_vec();
                let (clean, changed) = hide_secrets_read(&answer, &msgs);
                if changed {
                    answer = clean;
                    if let Some(m) = r.sess.messages.iter_mut().rev().find(|m| m.role == "assistant") {
                        m.content = hide_all(&m.content, &msgs);
                    }
                    self.emit(&mut r, ev(vec![("type", s("secret_hidden")), ("turn_index", i(turn))]));
                }
                let stop = t.finish_reason.clone();
                finish!(&stop, &answer, turn);
            }

            r.segments.clear();
            nudged_from.clear();
            let (stop, out) = self.run_tool_calls(&mut r, &t.tool_calls, turn);
            if !stop.is_empty() {
                finish!(&stop, &out, turn);
            }
            self.maybe_update_notes(ctx, &mut r, turn);
            turn += 1;
        }
        let mut out = r.segments.join("");
        if out.is_empty() {
            out = r.last_text.clone();
        }
        if out.is_empty() {
            out = "Stopped: the turn limit was reached before the task finished.".into();
        }
        finish!(STOP_MAX_TURNS, &out, max_turns)
    }

    /// The one claim check, in Go's order: the text to send back, or "".
    fn claim_check(&self, r: &Run, answer: &str, prompt: &str, msg_start: usize, hist_start: usize) -> String {
        let rt = &self.opts.runtime;
        let msgs = &r.sess.messages[msg_start.min(r.sess.messages.len())..];
        let hist = &r.sess.file_history[hist_start.min(r.sess.file_history.len())..];
        let base = self.base_tool_names();
        let with_rem = format!("{prompt}\n{}", reminders(&r.sess.messages));
        if claims_tool_work(answer, prompt, msgs) {
            return TOOL_WORK_TEXT.into();
        }
        if !exfil_in_answer(answer).is_empty() {
            return EXFIL_IN_ANSWER_TEXT.into();
        }
        let what = suggests_security_off(answer);
        if !what.is_empty() {
            return security_off_text(&what);
        }
        let why = code_works_unverified(answer, hist);
        if !why.is_empty() {
            return code_works_text(&why);
        }
        let can_write = base.iter().any(|n| n == "edit_file" || n == "write_file");
        if fix_described_not_made(prompt, answer, can_write, hist) && !write_refused(msgs) {
            return FIX_DESCRIBED_TEXT.into();
        }
        let f = task_instead_of_file(prompt, &rt.cwd, msgs, hist);
        if !f.is_empty() {
            return task_instead_of_file_text(&f);
        }
        let f = unbacked_file_claim(answer, hist);
        if !f.is_empty() {
            return claim_check_text(&f);
        }
        let f = unreported_change(answer, hist);
        if !f.is_empty() {
            return unreported_text(&f);
        }
        let (l, omitted) = invented_code(answer, prompt, msgs);
        if !l.is_empty() {
            return invented_code_text(&l, &omitted);
        }
        let q = ungrounded_quote(answer, prompt, msgs);
        if !q.is_empty() && !own_tool_named(&q, &base) {
            return ungrounded_text(&q);
        }
        if decompile_claimed(answer, msgs) {
            return DECOMPILE_CLAIMED_TEXT.into();
        }
        let c = unsourced_command(answer, prompt, msgs);
        if !c.is_empty() {
            return unsourced_text(&c);
        }
        let h = invented_history(answer, &r.sess.messages, msg_start);
        if !h.is_empty() {
            return invented_history_text(&h);
        }
        if default_output_for_one_app(answer, prompt) {
            return ONE_APP_SILENT_TEXT.into();
        }
        if up_to_date_unseen(answer, &evidence_of(&with_rem, msgs).0) {
            return UP_TO_DATE_TEXT.into();
        }
        let id = ungrounded_identifier(answer, &with_rem, msgs);
        if !id.is_empty() {
            return ungrounded_identifier_text(&id);
        }
        if digest_not_one_way(answer, &with_rem, msgs) {
            return DIGEST_NOT_ONE_WAY_TEXT.into();
        }
        let f = checked_unseen_file(answer, msgs);
        if !f.is_empty() {
            return checked_unseen_text(&f);
        }
        let why = failed_write_hidden(answer, msgs);
        if !why.is_empty() {
            return failed_write_hidden_text(&why);
        }
        if tool_error_ignored(answer, msgs) {
            return TOOL_ERROR_IGNORED_TEXT.into();
        }
        String::new()
    }

    fn emit_all(&self, r: &mut Run, evs: Vec<Event>) {
        for e in evs {
            self.emit(r, e);
        }
    }

    /// Go's `finish`: counters recorded, the session written, the result built.
    fn finish(
        &mut self,
        mut r: Run,
        stop: &str,
        out: &str,
        turns: i64,
        start_usage: Usage,
        start_cost: f64,
    ) -> (Option<RunResult>, Option<String>) {
        if let Some(t) = &self.opts.tiers {
            if let Some(rec) = t.record(r.sess.plugin_state.get("compute_tiers")) {
                r.sess.plugin_state.insert("compute_tiers".into(), Any::Object(rec));
            }
        }
        r.sess.set_plan(&self.plan);
        let s = &mut r.sess;
        s.turns = r.base_turn + turns;
        s.tool_calls = r.counts.tool_calls;
        s.usage = r.usage;
        s.total_cost_usd = r.cost;
        s.budget_state = crate::session::BudgetState {
            model_calls: r.counts.model_calls,
            session_turns: s.turns,
            tool_calls: r.counts.tool_calls,
            delegated_tasks: r.counts.delegated,
            escalations: r.esc,
        };
        self.cumulative_usage = self.cumulative_usage.add(&Usage {
            input_tokens: r.usage.input_tokens - start_usage.input_tokens,
            output_tokens: r.usage.output_tokens - start_usage.output_tokens,
            cache_creation_input_tokens: r.usage.cache_creation_input_tokens - start_usage.cache_creation_input_tokens,
            cache_read_input_tokens: r.usage.cache_read_input_tokens - start_usage.cache_read_input_tokens,
            reasoning_tokens: r.usage.reasoning_tokens - start_usage.reasoning_tokens,
        });
        self.cumulative_cost_usd += r.cost - start_cost;
        let mut res = RunResult {
            final_output: out.to_owned(),
            turns,
            tool_calls: r.counts.tool_calls,
            transcript: s.messages.clone(),
            events: std::mem::take(&mut r.events),
            usage: r.usage,
            total_cost_usd: r.cost,
            stop_reason: stop.to_owned(),
            file_history: s.file_history.clone(),
            session_id: s.id.clone(),
            scratchpad_directory: s.scratchpad_directory.clone(),
            ..RunResult::default()
        };
        if stop == STOP_PLAN_PENDING {
            res.plan = s.plan().pending;
        }
        self.end_run();
        let dir = match state_dir(&self.opts.runtime.cwd, &self.opts.runtime.session_directory) {
            Ok(d) => d,
            Err(e) => return (Some(res), Some(format!("saving session: {e}"))),
        };
        let getenv = self.opts.getenv.clone();
        let real_env = |k: &str| std::env::var(k).unwrap_or_default();
        let _ = getenv;
        match r.sess.save(&dir, &real_env) {
            Ok(p) => {
                res.session_path = p;
                (Some(res), None)
            }
            Err(e) => (Some(res), Some(format!("saving session: {e}"))),
        }
    }

    /// Go's `preflight`: the explanation when the next request cannot fit even after reduction.
    fn preflight(&mut self, ctx: &Ctx, r: &mut Run, turn: i64, preserve: i64) -> Option<String> {
        let rt = self.opts.runtime.clone();
        let specs = self.registry.specs();
        let mut snap = take_snapshot(self.window(), &r.sess.messages, &specs, &rt);
        if !snap.exceeds_soft && !snap.exceeds_hard {
            return None;
        }
        self.emit(
            r,
            ev(vec![
                ("type", s("prompt_length_check")),
                ("turn_index", i(turn)),
                ("projected_tokens", i(snap.projected)),
                ("soft_limit", i(snap.soft)),
                ("hard_limit", i(snap.hard)),
                ("overflow_tokens", i(snap.overflow)),
                ("exceeds_hard", Any::Bool(snap.exceeds_hard)),
            ]),
        );
        let target = if snap.exceeds_hard { snap.hard } else { snap.soft };
        let overhead = snap.projected - message_tokens(&r.sess.messages);
        let goal = (target - overhead).max(0);
        let (mut changed, evs) = reduce(&mut r.sess, &ReduceOpts { target: goal, preserve, turn, ..Default::default() });
        self.emit_all(r, evs);
        let mut strategy = "snip".to_owned();
        if message_tokens(&r.sess.messages) > goal {
            let how = self.compact_history(ctx, r, turn, preserve, true);
            if !how.is_empty() {
                changed = true;
                strategy = how;
            }
        }
        if changed {
            snap = take_snapshot(self.window(), &r.sess.messages, &specs, &rt);
            self.emit(
                r,
                ev(vec![
                    ("type", s("prompt_length_recovery")),
                    ("turn_index", i(turn)),
                    ("strategy", s(&strategy)),
                    ("projected_tokens", i(snap.projected)),
                    ("exceeds_hard", Any::Bool(snap.exceeds_hard)),
                ]),
            );
            if !snap.exceeds_soft && !snap.exceeds_hard {
                return None;
            }
        }
        if snap.exceeds_hard {
            let (win, src) = self.resolve_window();
            let mut msg = format!(
                "Stopped before calling the model: the conversation is about {} tokens, over the hard limit of {} (soft limit {}) for this context window of {win} tokens ({}).",
                snap.projected,
                snap.hard,
                snap.soft,
                describe_window_source(&src)
            );
            if win <= 8192 {
                msg.push_str(" That window is small for an agent: the tool definitions and system prompt alone take several thousand tokens. Raise the backend's context (Ollama: a Modelfile with `PARAMETER num_ctx 16384`, or set OLLAMA_CONTEXT_LENGTH=16384) or pass --context-window if the backend really has more.");
            }
            return Some(msg);
        }
        self.emit(
            r,
            ev(vec![
                ("type", s("prompt_length_warning")),
                ("turn_index", i(turn)),
                ("projected_tokens", i(snap.projected)),
                ("soft_limit", i(snap.soft)),
                ("hard_limit", i(snap.hard)),
            ]),
        );
        None
    }

    /// Go's `request`.
    fn request(&self, r: &Run) -> Request {
        let rt = &self.opts.runtime;
        let mut req = Request {
            messages: r.sess.messages.iter().map(Message::wire).collect(),
            tools: self.registry.specs(),
            schema: rt.output_schema.clone(),
            stream: rt.stream_model_responses,
            no_think: r.no_think,
            max_tokens: 0,
        };
        if let Some(n) = rt.budget_config.max_output_tokens.filter(|n| *n > 0) {
            req.max_tokens = n;
        }
        let n = crate::config::request_max_tokens_from_env(&|k: &str| std::env::var(k).unwrap_or_default());
        if n > 0 && (req.max_tokens == 0 || n < req.max_tokens) {
            req.max_tokens = n;
        }
        req
    }

    /// Go's `callModel`: one model call (with the single reactive-compaction retry), the assistant message committed.
    fn call_model(&mut self, ctx: &Ctx, r: &mut Run, turn: i64, preserve: i64) -> Result<Turn, String> {
        let attempt = |a: &Agent, r: &mut Run| -> (Result<Turn, LlmError>, Request, Option<usize>) {
            let req = a.request(r);
            let mut stream_idx: Option<usize> = None;
            let client = a.opts.client.clone();
            let result = if req.stream {
                let mut red = StreamRedactor::new(&r.sess.messages);
                let mut on = |e: StreamEvent| {
                    a.take_tier_events(r);
                    if let Some(red) = red.as_mut() {
                        if e.kind == "content_delta" {
                            let mut out = e.clone();
                            out.delta = red.push(&e.delta);
                            if !out.delta.is_empty() {
                                a.emit(r, stream_event_dict(&out));
                            }
                        } else {
                            let rest = red.flush();
                            if !rest.is_empty() {
                                a.emit(
                                    r,
                                    stream_event_dict(&StreamEvent { kind: "content_delta".into(), delta: rest, ..Default::default() }),
                                );
                            }
                            a.emit(r, stream_event_dict(&e));
                        }
                    } else {
                        a.emit(r, stream_event_dict(&e));
                    }
                    match e.kind.as_str() {
                        "message_start" => {
                            let id = r.sess.next_message_id("assistant");
                            stream_idx = Some(r.sess.append(Message {
                                role: "assistant".into(),
                                message_id: id,
                                state: STATE_STREAMING.into(),
                                ..Message::default()
                            }));
                        }
                        "content_delta" => {
                            if let Some(idx) = stream_idx {
                                let m = &mut r.sess.messages[idx];
                                m.content.push_str(&e.delta);
                                m.mutate("assistant_delta_append");
                            }
                        }
                        "tool_call_delta" => {
                            if let Some(idx) = stream_idx {
                                r.sess.messages[idx].mutate("assistant_tool_call_delta");
                            }
                        }
                        _ => {}
                    }
                };
                client.complete(ctx, &req, Some(&mut on))
            } else {
                client.complete(ctx, &req, None)
            };
            a.take_tier_events(r);
            if result.is_err() {
                if let Some(idx) = stream_idx {
                    if idx + 1 == r.sess.messages.len() {
                        r.sess.messages.truncate(idx);
                        stream_idx = None;
                    }
                }
            }
            (result, req, stream_idx)
        };
        let (mut result, mut req, mut stream_idx) = attempt(self, r);
        if let Err(e) = &result {
            if ctx.err().is_none() && is_prompt_too_long(e.text()) {
                let half = (preserve / 2).max(1);
                let mut changed = self.notes_compact(r, turn, half, true);
                if !changed {
                    let notes = self.vetted_notes(r, turn);
                    let (c, evs) =
                        reduce(&mut r.sess, &ReduceOpts { target: 0, allow_compaction: true, reactive: true, preserve: half, turn, notes });
                    changed = c;
                    self.emit_all(r, evs);
                }
                if changed {
                    (result, req, stream_idx) = attempt(self, r);
                    if result.is_ok() {
                        self.emit(r, ev(vec![("type", s("reactive_compact_retry")), ("turn_index", i(turn))]));
                    }
                }
            }
        }
        let mut t = result.map_err(|e| e.text().to_owned())?;
        if !t.usage_reported {
            let input = message_tokens(&r.sess.messages) + tool_spec_tokens(&req.tools);
            let mut out = count_tokens(&t.content);
            for c in &t.tool_calls {
                out += count_tokens(&c.name) + count_tokens(&c.arguments_json());
            }
            t.usage = Usage { input_tokens: input, output_tokens: out, reasoning_tokens: t.usage.reasoning_tokens, ..Usage::default() };
        }
        let mut msg = Message {
            role: "assistant".into(),
            content: t.content.clone(),
            state: STATE_FINAL.into(),
            stop_reason: t.finish_reason.clone(),
            usage: t.usage,
            tool_calls: t.tool_calls.iter().map(ToolCall::wire).collect(),
            ..Message::default()
        };
        let idx = match stream_idx {
            Some(idx) => {
                let cur = &r.sess.messages[idx];
                msg.message_id = cur.message_id.clone();
                msg.metadata = cur.metadata.clone();
                r.sess.messages[idx] = msg;
                r.sess.messages[idx].mutate("assistant_finalize");
                idx
            }
            None => {
                msg.message_id = r.sess.next_message_id("assistant");
                r.sess.append(msg)
            }
        };
        if !t.usage_reported {
            r.sess.messages[idx].set_meta("usage_estimated", Any::Bool(true));
        }
        if !t.reasoning.is_empty() {
            r.sess.messages[idx].set_meta("has_reasoning", Any::Bool(true));
        }
        Ok(t)
    }

    /// Go's `runToolCalls`: a turn's calls in order; a stop reason when the run must end.
    fn run_tool_calls(&mut self, r: &mut Run, calls: &[ToolCall], turn: i64) -> (String, String) {
        self.tool_ctx.step += 1;
        for (ci, call) in calls.iter().enumerate() {
            if self.tool_ctx.ctx.err().is_some() {
                answer_pending(r, &calls[ci..], "not run: the session was interrupted");
                return (STOP_INTERRUPTED.into(), "Interrupted during tool execution.".into());
            }
            if r.done.contains(&call.id) {
                let id = r.sess.next_message_id("tool");
                let mut meta = BTreeMap::new();
                meta.insert("error_kind".to_owned(), s_any("duplicate_call"));
                r.sess.append(Message {
                    role: "tool".into(),
                    name: call.name.clone(),
                    tool_call_id: call.id.clone(),
                    state: STATE_FINAL.into(),
                    message_id: id,
                    stop_reason: "tool_duplicate".into(),
                    content: tool_result_body(
                        &call.name,
                        false,
                        "this tool call id was already executed earlier in this run; it was not run again",
                        &meta,
                    ),
                    metadata: [
                        ("tool_name".to_owned(), s_any(&call.name)),
                        ("tool_call_id".to_owned(), s_any(&call.id)),
                        ("phase".to_owned(), s_any("completed")),
                    ]
                    .into(),
                    ..Message::default()
                });
                continue;
            }
            r.counts.tool_calls += 1;
            if call.name == TOOL_AGENT && self.registry.get(TOOL_AGENT).is_some() {
                r.counts.delegated += 1;
            }
            let why = self.budget_text(r);
            if !why.is_empty() {
                self.emit(
                    r,
                    ev(vec![
                        ("type", s("task_budget_exceeded")),
                        ("turn_index", i(turn)),
                        ("tool_name", s(&call.name)),
                        ("tool_call_id", s(&call.id)),
                        ("reason", s(&why)),
                    ]),
                );
                answer_pending(r, &calls[ci..], &format!("not run: {why}"));
                return (STOP_BUDGET_EXCEEDED.into(), why);
            }
            let mid = r.sess.next_message_id("tool");
            let idx = r.sess.append(Message {
                role: "tool".into(),
                name: call.name.clone(),
                tool_call_id: call.id.clone(),
                state: STATE_STREAMING.into(),
                message_id: mid.clone(),
                metadata: [
                    ("tool_name".to_owned(), s_any(&call.name)),
                    ("tool_call_id".to_owned(), s_any(&call.id)),
                    ("phase".to_owned(), s_any("starting")),
                ]
                .into(),
                ..Message::default()
            });
            let args = match &call.arguments {
                Some(m) => Any::Object(m.clone()),
                None => Any::Null,
            };
            self.emit(
                r,
                ev(vec![
                    ("type", s("tool_start")),
                    ("turn_index", i(turn)),
                    ("tool_name", s(&call.name)),
                    ("tool_call_id", s(&call.id)),
                    ("message_id", s(&mid)),
                    ("arguments", args),
                ]),
            );
            let stream = self.opts.runtime.stream_model_responses;
            let mut res = if !call.parse_error.is_empty() {
                let mut meta = BTreeMap::new();
                meta.insert("error_kind".to_owned(), s_any("invalid_arguments"));
                ToolResult {
                    name: call.name.clone(),
                    ok: false,
                    content: format!(
                        "The arguments for {} could not be used: {}. Received: {}. Send the call again with a valid JSON object.",
                        call.name,
                        call.parse_error,
                        collapse(&call.raw_arguments, 200)
                    ),
                    metadata: meta,
                }
            } else if stream {
                let deltas: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
                let d2 = deltas.clone();
                let (name, id) = (call.name.clone(), call.id.clone());
                let emit: crate::tools::registry::Emit = Arc::new(move |stream: &str, text: &str| {
                    d2.lock().unwrap_or_else(|p| p.into_inner()).push(ev(vec![
                        ("type", s("tool_delta")),
                        ("turn_index", i(turn)),
                        ("tool_name", s(&name)),
                        ("tool_call_id", s(&id)),
                        ("stream", s(stream)),
                        ("delta", s(text)),
                    ]));
                });
                let res = self.registry.execute_stream(&call.name, call.arguments.clone(), &self.tool_ctx, &emit);
                let got: Vec<Event> = deltas.lock().unwrap_or_else(|p| p.into_inner()).drain(..).collect();
                self.emit_all(r, got);
                res
            } else {
                self.registry.execute(&call.name, call.arguments.clone(), &self.tool_ctx)
            };
            if res.metadata.get(SUBAGENT_MARKER) == Some(&Any::Bool(true)) {
                let args = self.subagent_call.lock().unwrap_or_else(|p| p.into_inner()).take().unwrap_or_default();
                let ctx = self.tool_ctx.ctx.clone();
                res = self.run_subagent(&ctx, r, &args, &call.id);
            }
            if stream && res.metadata.get("streamed") != Some(&Any::Bool(true)) {
                for chunk in chunks(&res.content, 400) {
                    self.emit(
                        r,
                        ev(vec![
                            ("type", s("tool_delta")),
                            ("turn_index", i(turn)),
                            ("tool_name", s(&call.name)),
                            ("tool_call_id", s(&call.id)),
                            ("stream", s("tool")),
                            ("delta", s(&chunk)),
                        ]),
                    );
                }
            }
            let mut plan_pending = String::new();
            if res.ok {
                let act = match res.metadata.get("plan_action") {
                    Some(Any::String(a)) => a.clone(),
                    _ => String::new(),
                };
                {
                    if !act.is_empty() {
                        let plan_arg = call.arguments.as_ref().and_then(|a| a.get("plan")).cloned();
                        plan_pending = self.on_plan_action(&mut r.sess, &act, plan_arg.as_ref(), &mut res);
                    }
                }
                if let Some(Any::Array(names)) = res.metadata.get("skill_allowed_tools") {
                    let names: Vec<String> =
                        names.iter().filter_map(|n| if let Any::String(s) = n { Some(s.clone()) } else { None }).collect();
                    self.narrow_tools(Some(names));
                }
            }
            if res.metadata.get("error_kind") == Some(&s_any("permission_denied")) {
                self.emit(
                    r,
                    ev(vec![
                        ("type", s("tool_permission_denial")),
                        ("turn_index", i(turn)),
                        ("tool_name", s(&call.name)),
                        ("tool_call_id", s(&call.id)),
                        ("reason", s(&res.content)),
                        ("source", s("tool_runtime")),
                    ]),
                );
            }
            {
                let m = &mut r.sess.messages[idx];
                m.content = redact(&tool_result_body(&call.name, res.ok, &res.content, &res.metadata));
                m.state = STATE_FINAL.into();
                m.stop_reason = "tool_completed".into();
                for (k, v) in &res.metadata {
                    m.metadata.insert(k.clone(), v.clone());
                }
                m.metadata.insert("tool_name".into(), s_any(&call.name));
                m.metadata.insert("tool_call_id".into(), s_any(&call.id));
                m.metadata.insert("phase".into(), s_any("completed"));
            }
            r.done.insert(call.id.clone());
            self.emit(
                r,
                ev(vec![
                    ("type", s("tool_result")),
                    ("turn_index", i(turn)),
                    ("tool_name", s(&call.name)),
                    ("tool_call_id", s(&call.id)),
                    ("ok", Any::Bool(res.ok)),
                    ("metadata", Any::Object(res.metadata.clone())),
                    ("content", s(&clip_runes(&redact(&res.content), 20000))),
                ]),
            );
            self.record_history(r, call, &res, turn);
            if !plan_pending.is_empty() {
                answer_pending(r, &calls[ci + 1..], "not run: the plan is waiting for the operator's decision");
                return (STOP_PLAN_PENDING.into(), plan_pending);
            }
            if let Some(Any::String(cwd)) = res.metadata.get("cwd_update") {
                if res.ok && !cwd.is_empty() && std::fs::metadata(cwd).is_ok_and(|m| m.is_dir()) && self.rebuild_tools(&cwd.clone()).is_ok()
                {
                    r.sess.runtime = self.opts.runtime.clone();
                }
            }
        }
        (String::new(), String::new())
    }

    /// Go's `recordHistory`.
    fn record_history(&self, r: &mut Run, call: &ToolCall, res: &ToolResult, turn: i64) {
        let stamp = (self.opts.now)().format_rfc3339_utc();
        let entry_id = format!("{turn}:{}:{}", call.id, call.name);
        if res.metadata.get("delegation") == Some(&Any::Bool(true)) {
            let g = |k: &str| res.metadata.get(k).cloned().unwrap_or(Any::Null);
            r.sess.file_history.push(ev(vec![
                ("history_kind", s("delegation")),
                ("timestamp", s(&stamp)),
                ("turn_index", i(turn)),
                ("tool_call_id", s(&call.id)),
                ("tool_name", s(&call.name)),
                ("ok", Any::Bool(res.ok)),
                ("history_entry_id", s(&entry_id)),
                ("child_session_id", g("child_session_id")),
                ("subagent_profile", g("subagent_profile")),
                ("child_stop_reason", g("child_stop_reason")),
                ("result_preview", s(&collapse(&res.content, 220))),
            ]));
            return;
        }
        let path = match res.metadata.get("path") {
            Some(Any::String(p)) => Some(p.clone()),
            _ => None,
        };
        let has_cmd = matches!(res.metadata.get("command"), Some(Any::String(_)));
        if path.is_none() && !has_cmd {
            return;
        }
        let mut e = res.metadata.clone();
        e.insert("timestamp".into(), s(&stamp));
        e.insert("turn_index".into(), i(turn));
        e.insert("tool_call_id".into(), s(&call.id));
        e.insert("tool_name".into(), s(&call.name));
        e.insert("ok".into(), Any::Bool(res.ok));
        e.insert("history_entry_id".into(), s(&entry_id));
        e.insert("result_preview".into(), s(&collapse(&res.content, 220)));
        match path {
            Some(p) => {
                e.insert("history_kind".into(), s("file_change"));
                e.insert("changed_paths".into(), Any::Array(vec![s(&p)]));
                for side in ["before", "after"] {
                    if let Some(Any::String(h)) = res.metadata.get(&format!("{side}_sha256")) {
                        if !h.is_empty() {
                            let cut = h.char_indices().nth(12).map_or(h.len(), |(k, _)| k);
                            e.insert(format!("{side}_snapshot_id"), s(&format!("{p}:{}", &h[..cut])));
                        }
                    }
                }
            }
            None => {
                e.insert("history_kind".into(), s("shell"));
            }
        }
        r.sess.file_history.push(e);
    }

    // ---- notes and compaction with a model -----------------------------------------------------------------------------

    /// Go's `notesPath`: "" when the session folder may not be used.
    pub fn notes_path(&self, id: &str) -> String {
        match state_dir(&self.opts.runtime.cwd, &self.opts.runtime.session_directory) {
            Ok(dir) => join(&dir, &format!("{id}.notes.md")),
            Err(_) => String::new(),
        }
    }

    fn read_notes(&self, id: &str) -> String {
        crate::session::readfile::read_regular(self.notes_path(id).as_bytes(), MAX_NOTES_BYTES)
            .map(|b| trim_space(&gotext::lossy(&b)).to_owned())
            .unwrap_or_default()
    }

    /// Go's `charge`: a side call booked against the run.
    fn charge(&self, r: &mut Run, t: &Turn, req: &Request) -> Usage {
        let mut u = t.usage;
        if !t.usage_reported {
            let input = req.messages.iter().map(|m| count_tokens(&m.content)).sum();
            u = Usage { input_tokens: input, output_tokens: count_tokens(&t.content), ..Usage::default() };
        }
        r.usage = r.usage.add(&u);
        r.cost = cost(&self.opts.model.pricing, &r.usage);
        r.counts.model_calls += 1;
        u
    }

    fn notes_due(&self, r: &Run) -> Option<&'static str> {
        let st = &self.ctx_settings;
        if !st.notes || r.tool_turns_since == 0 {
            return None;
        }
        if r.tool_turns_since >= st.notes_every_turns {
            return Some("cadence");
        }
        if r.tool_turns_since >= MIN_WINDOW_TURNS {
            let w = self.window();
            let used = message_tokens(&r.sess.messages) + tool_spec_tokens(&self.registry.specs());
            if used as f64 >= st.notes_window_fraction * w as f64 {
                return Some("window");
            }
        }
        None
    }

    fn maybe_update_notes(&mut self, ctx: &Ctx, r: &mut Run, turn: i64) {
        if !self.ctx_settings.notes {
            return;
        }
        r.tool_turns_since += 1;
        let Some(why) = self.notes_due(r) else { return };
        if ctx.err().is_some() {
            return;
        }
        r.tool_turns_since = 0;
        if let Err(e) = self.update_notes(ctx, r, turn, why) {
            self.emit(r, ev(vec![("type", s("session_notes_failed")), ("turn_index", i(turn)), ("reason", s(&e))]));
        }
    }

    fn update_notes(&mut self, ctx: &Ctx, r: &mut Run, turn: i64, why: &str) -> Result<(), String> {
        let prefix = protected_prefix(&r.sess.messages) as i64;
        let covered = notes_coverage(&r.sess).0.max(prefix);
        if covered >= r.sess.messages.len() as i64 {
            return Ok(());
        }
        let window = self.window();
        let fresh = tail_within(&r.sess.messages[covered as usize..], NOTES_TOOL_RESULT_MAX, window.max(4096) * 2).to_vec();
        let mut prev = self.read_notes(&r.sess.id);
        if prev.is_empty() {
            prev = "(no notes yet)".into();
        }
        let req = Request {
            messages: vec![
                llm::Message { role: "system".into(), content: NOTES_PROMPT.into(), ..Default::default() },
                llm::Message {
                    role: "user".into(),
                    content: format!(
                        "Previous notes:\n{prev}\n\nNewest part of the transcript:\n{}\nWrite the updated notes now.",
                        render_transcript(&fresh, NOTES_TOOL_RESULT_MAX)
                    ),
                    ..Default::default()
                },
            ],
            max_tokens: side_max_tokens(window, 2048),
            tools: Vec::new(),
            schema: None,
            stream: false,
            no_think: false,
        };
        let got = self.opts.client.complete(ctx, &req, None);
        self.take_tier_events(r);
        let t = got.map_err(|e| format!("notes model call failed: {}", e.text()))?;
        let u = self.charge(r, &t, &req);
        let text = normalise_notes(&t.content).ok_or("notes reply had no usable sections")?;
        let path = self.notes_path(&r.sess.id);
        if path.is_empty() {
            return Err("cannot write notes: the session folder leads out of the workspace".into());
        }
        write_file_atomic(&path, text.as_bytes()).map_err(|e| format!("cannot write notes: {e}"))?;
        let n = r.sess.messages.len() as i64;
        set_notes_coverage(&mut r.sess, n);
        self.emit(
            r,
            ev(vec![
                ("type", s("session_notes_updated")),
                ("turn_index", i(turn)),
                ("trigger", s(why)),
                ("covered_messages", i(n)),
                ("usage_tokens", i(u.total())),
                ("path", s(&path)),
            ]),
        );
        Ok(())
    }

    /// Go's `vettedNotes`: the session's notes when they exist and pass the scanner.
    fn vetted_notes(&self, r: &mut Run, turn: i64) -> String {
        if !self.ctx_settings.notes {
            return String::new();
        }
        let text = self.read_notes(&r.sess.id);
        if text.is_empty() {
            return String::new();
        }
        let (blocked, reason) = self.scan(&text);
        if blocked {
            self.emit(r, ev(vec![("type", s("session_notes_blocked")), ("turn_index", i(turn)), ("reason", s(&reason))]));
            return String::new();
        }
        text
    }

    /// Go's `notesCompact`: the history folded into fresh notes, with no model call.
    fn notes_compact(&self, r: &mut Run, turn: i64, preserve: i64, reactive: bool) -> bool {
        let prefix = protected_prefix(&r.sess.messages);
        let cut = pair_safe_cut(&r.sess.messages, prefix, preserve);
        if cut <= prefix {
            return false;
        }
        let (covered, ok) = notes_coverage(&r.sess);
        if !ok || covered < cut as i64 {
            return false;
        }
        let notes = self.vetted_notes(r, turn);
        if notes.is_empty() {
            return false;
        }
        let body =
            folded_body(cut - prefix, &notes, "Treat the messages after this note as the working set; rely on them over these notes.");
        let e = fold_history(
            &mut r.sess,
            prefix,
            cut,
            &body,
            &ReduceOpts { preserve, turn, reactive, ..Default::default() },
            "auto_compact_summary",
            vec![("summary_source", s("notes")), ("summary_model_calls", i(0))],
        );
        self.emit(r, e);
        true
    }

    /// Go's `compactHistory`: how the transcript was shrunk ("" for not at all).
    fn compact_history(&mut self, ctx: &Ctx, r: &mut Run, turn: i64, preserve: i64, allow_llm: bool) -> String {
        let prefix = protected_prefix(&r.sess.messages);
        let cut = pair_safe_cut(&r.sess.messages, prefix, preserve);
        if cut <= prefix {
            return String::new();
        }
        if self.notes_compact(r, turn, preserve, false) {
            return "notes".into();
        }
        let notes = self.vetted_notes(r, turn);
        if allow_llm && self.ctx_settings.llm_compaction && ctx.err().is_none() {
            if r.compact_failures >= BREAKER_LIMIT {
                let f = r.compact_failures;
                self.emit(r, ev(vec![("type", s("auto_compact_circuit_breaker")), ("turn_index", i(turn)), ("failures", i(f))]));
            } else if self.llm_compact(ctx, r, turn, prefix, cut, preserve, &notes) {
                return "llm".into();
            }
        }
        if ctx.err().is_some() {
            return String::new();
        }
        match compact_pass(&mut r.sess, &ReduceOpts { preserve, turn, notes, ..Default::default() }) {
            Some(e) => {
                self.emit(r, e);
                "heuristic".into()
            }
            None => String::new(),
        }
    }

    /// Go's `llmCompact`.
    #[allow(clippy::too_many_arguments)]
    fn llm_compact(&mut self, ctx: &Ctx, r: &mut Run, turn: i64, prefix: usize, cut: usize, preserve: i64, notes: &str) -> bool {
        let window = self.window();
        let mut groups = group_messages(&r.sess.messages[prefix..cut]);
        let count = cut - prefix;
        let budget_chars = window.max(2048) * 3;
        let mut usage = Usage::default();
        let (mut retries, mut dropped) = (0i64, 0usize);
        let fail = |a: &Agent, r: &mut Run, reason: &str| {
            r.compact_failures += 1;
            let c = r.compact_failures;
            a.emit(r, ev(vec![("type", s("auto_compact_failed")), ("turn_index", i(turn)), ("reason", s(reason)), ("count", i(c))]));
            false
        };
        let drop = |groups: &mut Vec<Vec<Message>>, dropped: &mut usize| -> bool {
            if groups.len() < 2 {
                return false;
            }
            let n = (groups.len() / 5).max(1).min(groups.len() - 1);
            *dropped += groups[..n].iter().map(Vec::len).sum::<usize>();
            groups.drain(..n);
            true
        };
        loop {
            let flat = |g: &[Vec<Message>]| -> Vec<Message> { g.iter().flatten().cloned().collect() };
            let mut text = render_transcript(&flat(&groups), SUMMARY_TOOL_CHARS);
            while text.chars().count() as i64 > budget_chars && drop(&mut groups, &mut dropped) {
                text = render_transcript(&flat(&groups), SUMMARY_TOOL_CHARS);
            }
            let mut b = String::new();
            if !notes.is_empty() {
                b.push_str(&format!("Working notes written earlier (they may lag the newest messages):\n{notes}\n\n"));
            }
            if dropped > 0 {
                b.push_str(&format!("(The oldest {dropped} messages did not fit and are not shown.)\n\n"));
            }
            b.push_str(&format!("Transcript to condense:\n{text}\nWrite the summary now, as plain text, with no tool calls."));
            let req = Request {
                messages: vec![
                    llm::Message { role: "system".into(), content: SUMMARY_PROMPT.into(), ..Default::default() },
                    llm::Message { role: "user".into(), content: b, ..Default::default() },
                ],
                max_tokens: side_max_tokens(window, 3000),
                tools: Vec::new(),
                schema: None,
                stream: false,
                no_think: false,
            };
            let mut too_long = false;
            let got = self.opts.client.complete(ctx, &req, None);
            self.take_tier_events(r);
            let reply = match got {
                Err(e) => {
                    if ctx.err().is_some() {
                        return false;
                    }
                    if !err_too_long(e.text()) {
                        return fail(self, r, &format!("model call failed: {}", e.text()));
                    }
                    too_long = true;
                    None
                }
                Ok(t) => {
                    usage = usage.add(&self.charge(r, &t, &req));
                    if reply_too_long(&t.content) && t.content.len() < 400 {
                        too_long = true;
                    }
                    Some(t)
                }
            };
            if too_long {
                if retries >= MAX_SUMMARY_RETRY || !drop(&mut groups, &mut dropped) {
                    return fail(self, r, "compact request itself too long");
                }
                retries += 1;
                continue;
            }
            let summary = clean_summary(&reply.map(|t| t.content).unwrap_or_default());
            if summary.is_empty() {
                return fail(self, r, "empty_response");
            }
            let (blocked, reason) = self.scan(&summary);
            if blocked {
                return fail(self, r, &format!("summary rejected by the security scan: {reason}"));
            }
            let body = folded_body(
                count,
                &format!("Summary of that history:\n{summary}"),
                "Treat the messages after this note as the working set; rely on them over this summary.",
            );
            let e = fold_history(
                &mut r.sess,
                prefix,
                cut,
                &body,
                &ReduceOpts { preserve, turn, ..Default::default() },
                "auto_compact_summary",
                vec![
                    ("summary_source", s("llm")),
                    ("summary_model_calls", i(retries + 1)),
                    ("summary_usage_tokens", i(usage.total())),
                    ("ptl_retries", i(retries)),
                ],
            );
            r.compact_failures = 0;
            self.emit(r, e);
            return true;
        }
    }

    // ---- sub-agents (Go: subagent.go) ------------------------------------------------------------------------------

    /// Go's `agentTool`: the `agent` tool's definition. Its handler only hands the checked arguments to the loop, which
    /// runs the child (a handler cannot reach the agent that runs it).
    fn agent_tool(cat: &Catalog, slot: Arc<Mutex<Option<BTreeMap<String, Any>>>>) -> Tool {
        let mut desc = String::from("Delegate a separate, self-contained investigation to a read-only helper that returns one report. Do not use it for a single read or search; call read_file, grep_search or glob_search directly. The helper does not see this conversation. Profiles:\n");
        for p in &cat.profiles {
            desc.push_str(&format!("- {}: {}\n", p.name, p.description));
        }
        let o = |pairs: Vec<(&str, Any)>| Any::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        let num = |n: i64| Any::Number(n as f64);
        let params = o(vec![
            ("type", s("object")),
            (
                "properties",
                o(vec![
                    ("profile", o(vec![("type", s("string")), ("enum", Any::Array(cat.profiles.iter().map(|p| s(&p.name)).collect()))])),
                    (
                        "prompt",
                        o(vec![("type", s("string")), ("description", s("Complete task, with all context.")), ("minLength", num(1))]),
                    ),
                    ("description", o(vec![("type", s("string")), ("description", s("Short label."))])),
                    (
                        "max_turns",
                        o(vec![
                            ("type", s("integer")),
                            ("description", s(&format!("1 to {MAX_TURNS_LIMIT}."))),
                            ("minimum", num(1)),
                            ("maximum", num(MAX_TURNS_LIMIT)),
                        ]),
                    ),
                ]),
            ),
            ("required", Any::Array(vec![s("profile"), s("prompt")])),
        ]);
        Tool {
            spec: ToolSpec {
                name: TOOL_AGENT.into(),
                description: desc.trim_end_matches('\n').to_owned(),
                parameters: params.into_object().ok(),
            },
            handler: Arc::new(move |args, _| {
                *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(args.clone());
                Output { content: String::new(), metadata: [(SUBAGENT_MARKER.to_owned(), Any::Bool(true))].into(), error: None }
            }),
            requires: Permission::None,
            class: Class::ReadOnly,
            stream: None,
        }
    }

    /// Go's `childTools`: the parent's current tools a child may use (read-only ones, bash only when the profile and the
    /// parent allow shell commands); it never adds a name.
    fn child_tools(&self, p: &Profile) -> Vec<String> {
        let mut out = Vec::new();
        for n in self.registry.names() {
            if child_denied(&n) || p.tools.as_ref().is_some_and(|t| !t.contains(&n)) {
                continue;
            }
            let Some(t) = self.registry.get(&n) else { continue };
            if t.class == Class::ReadOnly || (n == "bash" && p.shell && self.opts.runtime.permissions.allow_shell_commands) {
                out.push(n);
            }
        }
        out
    }

    /// Go's `childBudget`: the parent's remaining allowance where it has a limit, a fixed tool-call cap, the turn limit, no
    /// delegation.
    fn child_budget(&self, r: &Run, turns: i64) -> BudgetConfig {
        let b = &self.opts.runtime.budget_config;
        let left = |limit: Option<i64>, have: i64| limit.map(|l| (l - have).max(0));
        let mut tc = DEFAULT_CHILD_TOOL_CALLS;
        if let Some(l) = b.max_tool_calls {
            tc = tc.min((l - r.counts.tool_calls).max(0));
        }
        BudgetConfig {
            max_input_tokens: b.max_input_tokens,
            max_output_tokens: b.max_output_tokens,
            max_total_tokens: left(b.max_total_tokens, r.usage.total()),
            max_reasoning_tokens: left(b.max_reasoning_tokens, r.usage.reasoning_tokens),
            max_model_calls: left(b.max_model_calls, r.counts.model_calls),
            max_total_cost_usd: b.max_total_cost_usd.map(|l| (l - cost(&self.opts.model.pricing, &r.usage)).max(0.0)),
            max_tool_calls: Some(tc),
            max_session_turns: Some(turns),
            max_delegated_tasks: Some(0),
        }
    }

    /// Go's `runSubagent`: the child's report, or why it could not run, with the delegation's metadata.
    fn run_subagent(&mut self, ctx: &Ctx, r: &mut Run, args: &BTreeMap<String, Any>, call_id: &str) -> ToolResult {
        let fail = |content: String, mut metadata: BTreeMap<String, Any>| {
            metadata.insert("error_kind".into(), s("tool_execution_error"));
            ToolResult { name: TOOL_AGENT.into(), ok: false, content, metadata }
        };
        let get = |k: &str| match args.get(k) {
            Some(Any::String(v)) => v.clone(),
            _ => String::new(),
        };
        let (name, prompt, label) = (get("profile"), get("prompt"), get("description"));
        let Some(cat) = self.opts.subagents.clone() else { return fail("no agent profiles".into(), BTreeMap::new()) };
        let Some(p) = cat.get(&name).cloned() else {
            return fail(
                format!("no agent profile named {}; available: {}", gotext::quote(&name), cat.names().join(", ")),
                BTreeMap::new(),
            );
        };
        if trim_space(&prompt).is_empty() {
            return fail("prompt must not be empty".into(), BTreeMap::new());
        }
        let mut rt = self.opts.runtime.clone();
        let mut turns = DEFAULT_CHILD_TURNS.min(rt.max_turns);
        if p.max_turns > 0 {
            turns = p.max_turns;
        }
        if let Some(n) = as_int(args.get("max_turns")).filter(|n| *n > 0) {
            turns = n;
        }
        turns = turns.min(MAX_TURNS_LIMIT).min(rt.max_turns).max(1);
        rt.max_turns = turns;
        rt.permissions = Permissions { allow_shell_commands: rt.permissions.allow_shell_commands && p.shell, ..Permissions::default() };
        rt.disable_claude_md_discovery = rt.disable_claude_md_discovery || !p.project_context;
        rt.stream_model_responses = false;
        rt.output_schema = None;
        rt.budget_config = self.child_budget(r, turns);
        let disable_clearing = rt.context_management.as_ref().is_some_and(|c| c.disable_clearing);
        rt.context_management = Some(ContextConfig {
            disable_notes: true,
            disable_llm_compaction: true,
            disable_discovery: true,
            disable_clearing,
            ..ContextConfig::default()
        });
        let mut builder = self.opts.tools.clone();
        builder.narrow = Some(self.child_tools(&p));
        builder.plan_mode = self.plan.active();
        let link: BTreeMap<String, Any> = [
            ("parent_session_id".to_owned(), s(&r.sess.id)),
            ("parent_tool_call_id".to_owned(), s(call_id)),
            ("profile".to_owned(), s(&p.name)),
            ("depth".to_owned(), i(1)),
        ]
        .into();
        let child_prompt =
            format!("{}\n\nWorking directory: {}\nYour final message is returned to the agent that started you.", p.prompt, rt.cwd);
        let opts = Options {
            runtime: rt,
            tools: builder,
            prompt: PromptOptions { override_: child_prompt, ..PromptOptions::default() },
            on_event: None,
            tiers: None,
            parent_link: Some(link),
            subagents: None,
            ..self.opts.clone()
        };
        let mut child = match Agent::new(opts) {
            Ok(c) => c,
            Err(e) => return fail(format!("cannot start the {} helper: {e}", p.name), BTreeMap::new()),
        };
        let names = child.registry.names();
        self.emit(
            r,
            ev(vec![
                ("type", s("subagent_start")),
                ("profile", s(&p.name)),
                ("label", s(&label)),
                ("tools", Any::Array(names.iter().map(|n| s(n)).collect())),
                ("max_turns", i(turns)),
            ]),
        );
        let (res, err) = child.run(ctx, &prompt);
        let Some(res) = res else {
            return fail(format!("the {} helper failed: {}", p.name, err.unwrap_or_default()), BTreeMap::new());
        };
        r.usage = r.usage.add(&res.usage);
        r.cost = cost(&self.opts.model.pricing, &r.usage);
        self.emit(
            r,
            ev(vec![
                ("type", s("subagent_end")),
                ("profile", s(&p.name)),
                ("child_session_id", s(&res.session_id)),
                ("stop_reason", s(&res.stop_reason)),
                ("turns", i(res.turns)),
                ("tool_calls", i(res.tool_calls)),
            ]),
        );
        let md: BTreeMap<String, Any> = [
            ("delegation".to_owned(), Any::Bool(true)),
            ("child_session_id".to_owned(), s(&res.session_id)),
            ("subagent_profile".to_owned(), s(&p.name)),
            ("child_stop_reason".to_owned(), s(&res.stop_reason)),
            ("child_turns".to_owned(), i(res.turns)),
            ("child_tool_calls".to_owned(), i(res.tool_calls)),
        ]
        .into();
        if res.stop_reason == STOP_BACKEND_ERROR || res.stop_reason == STOP_PROMPT_TOO_LONG {
            return fail(format!("the {} helper stopped early ({}): {}", p.name, res.stop_reason, res.final_output), md);
        }
        let limit = if self.tool_ctx.max_output_chars > 0 { self.tool_ctx.max_output_chars as usize } else { 12000 };
        let mut out = res.final_output.clone();
        if let Some((k, _)) = out.char_indices().nth(limit) {
            out = format!("{}\n...[helper report truncated]", &out[..k]);
        }
        let header = format!(
            "{} helper finished (stop: {}, turns: {}, tool calls: {}, session: {})",
            p.name, res.stop_reason, res.turns, res.tool_calls, res.session_id
        );
        if trim_space(&out).is_empty() {
            out = "(the helper returned no text)".into();
        }
        ToolResult { name: TOOL_AGENT.into(), ok: true, content: format!("{header}\n\n{out}"), metadata: md }
    }
}

fn s_any(v: &str) -> Any {
    Any::String(v.to_owned())
}

fn join(a: &str, b: &str) -> String {
    gotext::lossy(&gopath::join(&[a.as_bytes(), b.as_bytes()]))
}

fn replay_count_matches(msgs: &[Message], n: i64) -> bool {
    match msgs.iter().rev().find(|m| m.kind() == KIND_FILE_HISTORY_REPLAY) {
        Some(m) => as_int(m.metadata.get("file_history_count")).unwrap_or(0) == n,
        None => false,
    }
}

/// Go's `answerPending`: every call not yet answered gets a result saying it was not run.
fn answer_pending(r: &mut Run, calls: &[ToolCall], reason: &str) {
    for c in calls {
        if r.done.contains(&c.id) {
            continue;
        }
        let id = r.sess.next_message_id("tool");
        r.sess.append(not_run_message(&c.id, &c.name, reason, &id));
        r.done.insert(c.id.clone());
    }
}

/// Go's `streamEventDict`.
fn stream_event_dict(e: &StreamEvent) -> Event {
    let mut d = ev(vec![
        ("type", s(&e.kind)),
        ("delta", s(&e.delta)),
        ("tool_call_index", Any::Null),
        ("tool_call_id", Any::Null),
        ("tool_name", Any::Null),
        ("arguments_delta", Any::Null),
        ("finish_reason", Any::Null),
        ("usage", Any::Null),
        ("raw_event", Any::Null),
    ]);
    match e.kind.as_str() {
        "tool_call_delta" => {
            d.insert("tool_call_index".into(), i(e.tool_call_index));
            d.insert("tool_call_id".into(), s(&e.tool_call_id));
            d.insert("tool_name".into(), s(&e.tool_name));
            d.insert("arguments_delta".into(), s(&e.arguments_delta));
        }
        "usage" => {
            if let Some(u) = &e.usage {
                d.insert("usage".into(), usage_any(u));
            }
        }
        "message_stop" => {
            d.insert("finish_reason".into(), s(&e.finish_reason));
        }
        _ => {}
    }
    d
}

/// Go's `writeFileAtomic`: a temporary file next to path (mode 0600), renamed over it.
pub fn write_file_atomic(path: &str, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let dir = gopath::dir(path.as_bytes());
    crate::goos::mkdir_all(&dir)?;
    let base = gotext::lossy(&gopath::base(path.as_bytes()));
    let (mut f, name) = crate::goos::create_temp(&dir, &format!("{base}.*.tmp"))?;
    let tmp = gopath::os_path(&name).to_path_buf();
    if let Err(e) = f.write_all(data) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    drop(f);
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}

/// The tiers Router tells the loop where each turn runs (Go: the Router satisfies TierTracker).
impl TierTracker for crate::tiers::router::Router {
    fn begin_turn(&self, emit: Box<dyn FnMut(Event)>) {
        crate::tiers::router::Router::begin_turn(self, Some(emit));
    }

    fn record(&self, prev: Option<&Any>) -> Option<Event> {
        crate::tiers::router::Router::record(self, prev)
    }
}

/// The backend's context window (Go: llm.Discoverer satisfies WindowDiscoverer, asked without a deadline).
impl WindowDiscoverer for crate::llm::discover::Discoverer {
    fn context_window(&self, model: &ModelConfig) -> (i64, String) {
        crate::llm::discover::Discoverer::context_window(self, &Ctx::background(), model)
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;

    struct NoModel;

    impl Completer for NoModel {
        fn complete(&self, _: &Ctx, _: &Request, _: Option<&mut dyn FnMut(StreamEvent)>) -> Result<Turn, LlmError> {
            Err(LlmError::Backend(crate::llm::openai::BackendError { message: "no model in this test".into(), status: 0 }))
        }
    }

    /// Go's `TestSetConfirmAddsOnlyItsOwnRuntimeKey`.
    #[test]
    fn set_confirm_adds_only_its_own_channel() {
        let dir = std::env::temp_dir().join(format!("sarina-confirm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().into_owned();
        let opts = Options {
            model: ModelConfig { model: "test".into(), base_url: "http://127.0.0.1:1/v1".into(), ..ModelConfig::default() },
            runtime: crate::config::default_runtime(&d),
            client: Arc::new(NoModel),
            tools: Builder::default(),
            prompt: PromptOptions::default(),
            context_window: 0,
            now: Arc::new(|| Time::from_unix(1_791_115_200, 0)),
            home: d.clone(),
            on_event: None,
            getenv: Arc::new(|_: &str| String::new()),
            discoverer: None,
            scanner: None,
            tiers: None,
            session_env: BTreeMap::new(),
            parent_link: None,
            approver: None,
            confirm: None,
            subagents: None,
        };
        let mut a = Agent::new(opts).unwrap();
        assert!(a.tool_ctx.confirm.is_none(), "no confirm channel was installed, yet one is there");
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = asked.clone();
        a.set_confirm(Some(Arc::new(move |p: &str| {
            log.lock().unwrap().push(p.to_owned());
            true
        })));
        let ask = |a: &Agent, p: &str| a.tool_ctx.confirm.as_ref().map(|f| f(p));
        assert_eq!(ask(&a, "send?"), Some(true), "the confirm channel was not installed");
        assert!(a.tool_ctx.plan_active.is_some(), "set_confirm dropped the plan-mode probe");
        a.rebuild_tools(&d).unwrap();
        assert_eq!(ask(&a, "again?"), Some(true), "rebuilding the tool context lost the confirm channel");
        assert_eq!(*asked.lock().unwrap(), ["send?", "again?"], "each call asks once");
        a.set_confirm(None);
        assert!(a.tool_ctx.confirm.is_none(), "set_confirm(None) left the channel installed");
        a.rebuild_tools(&d).unwrap();
        assert!(a.tool_ctx.confirm.is_none(), "a rebuilt context brought the removed channel back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
