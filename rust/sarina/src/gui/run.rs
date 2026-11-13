//! Go: `gui/run.go`: one agent run at a time, its events as a re-attachable stream, the operator's approvals of tool
//! calls and plans while a run waits, cancelling.
//!
//! A run's agent lives on a thread of its own (built there, as the service's); the approver, the plan decider and the
//! event hook it calls reach the server through its lock.

use std::collections::{BTreeMap, HashSet};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use regex::Regex;

use super::state::{bad, ApiErr, APPROVAL_ASK};
use super::{detail, fail, json_any, json_str, obj, read_json, s, write_json, write_raw, Server};
use crate::agent::run::{PlanDecision, RunResult, PLAN_APPROVE, PLAN_REJECT, PLAN_REVISE};
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gohttpd::{Exchange, Response};
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext;
use crate::runkit::{self, Scrub};
use crate::tools::registry::{ApprovalRequest, Permission};

pub const STATE_IDLE: &str = "idle";
pub const STATE_RUNNING: &str = "running";
pub const STATE_WAITING: &str = "waiting_approval";
pub const STATE_FINISHED: &str = "finished";

/// Go's `run`: one agent turn in flight (or the last one that finished).
pub struct Run {
    pub id: String,
    pub ctx: Ctx,
    pub started: SystemTime,
    pub scrub: Scrub,
    m: Mutex<RunState>,
    done: Condvar,
}

#[derive(Default)]
struct RunState {
    finished: bool,
    waiting: i64,
    sess_id: String,
    /// tools the operator allowed for the rest of the run
    grants: HashSet<String>,
    /// the result's JSON, once done
    result: Option<String>,
    /// the error (scrubbed) and its type, once done
    err: Option<(String, String)>,
}

impl Run {
    fn st(&self) -> std::sync::MutexGuard<'_, RunState> {
        self.m.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn running(&self) -> bool {
        !self.st().finished
    }

    pub fn state(&self) -> &'static str {
        let st = self.st();
        if st.finished {
            STATE_FINISHED
        } else if st.waiting > 0 {
            STATE_WAITING
        } else {
            STATE_RUNNING
        }
    }

    pub fn current_session(&self) -> String {
        self.st().sess_id.clone()
    }

    fn finish(&self) {
        self.st().finished = true;
        self.done.notify_all();
    }

    /// Waits for the run's end; false when `give_up` says to stop waiting first (checked every 50 ms).
    fn wait(&self, mut give_up: impl FnMut() -> bool) -> bool {
        let mut st = self.st();
        loop {
            if st.finished {
                return true;
            }
            st = self.done.wait_timeout(st, Duration::from_millis(50)).unwrap_or_else(|p| p.into_inner()).0;
            if !st.finished && give_up() {
                return false;
            }
        }
    }
}

/// Go's `approval`.
pub struct Approval {
    pub id: String,
    pub kind: String,
    pub run_id: String,
    pub session_id: String,
    pub tool: String,
    pub arguments: Any,
    pub plan: String,
    pub summary: String,
    pub risk: String,
    pub created: String,
    decide: SyncSender<(String, String)>,
}

/// Go's `clip`: at most `n` characters, "..." after a cut.
pub(super) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push_str("...");
    out
}

/// Go's `summarize`.
fn summarize(tool: &str, args: &BTreeMap<String, Any>) -> String {
    if let (Some(Any::String(c)), "bash") = (args.get("command"), tool) {
        return format!("run: {}", clip(c, 300));
    }
    if let Some(Any::String(p)) = args.get("path") {
        return format!("{tool} on {p}");
    }
    let parts: Vec<String> = args.iter().map(|(k, v)| format!("{k}={}", clip(&crate::session::sprint(v), 60))).collect();
    clip(&format!("{tool}({})", parts.join(", ")), 300)
}

/// Go's `riskFor`.
fn risk_for(req: &ApprovalRequest, root: &str) -> String {
    match req.needs {
        Permission::Shell => {
            if let Some(Any::String(c)) = req.arguments.get("command") {
                let v = crate::tools::destructive::check_destructive(c.as_bytes(), root.as_bytes());
                if v.blocked {
                    return format!("high: this command looks destructive ({})", gotext::lossy(&v.reason));
                }
            }
            "high: runs a command on this machine".into()
        }
        Permission::Write => "medium: changes files or saved state".into(),
        _ => "medium: this tool changes state".into(),
    }
}

static PASTE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\[Pasted text #([0-9]+)[^\]]*\]").expect("paste pattern"));

/// Go's `expandPastes`.
fn expand_pastes(prompt: &str, contents: &BTreeMap<String, String>) -> String {
    if contents.is_empty() {
        return prompt.to_owned();
    }
    PASTE_RE
        .replace_all(prompt, |c: &regex::Captures| match contents.get(&c[1]) {
            Some(t) if !t.is_empty() => t.clone(),
            _ => c[0].to_owned(),
        })
        .into_owned()
}

static STR: Ty = Ty::Ptr(&Ty::Str);
static PASTED: Ty = Ty::Struct("gui.pasted", "pasted", &[Field("type", &Ty::Str), Field("content", &Ty::Str)]);
static PASTED_MAP: Ty = Ty::Map(&PASTED);
/// Go's `chatBody`.
static CHAT: Ty = Ty::Struct(
    "gui.chatBody",
    "chatBody",
    &[Field("prompt", &STR), Field("resume_session_id", &STR), Field("pasted_contents", &PASTED_MAP)],
);
static DECISION: Ty = Ty::Struct(
    r#"struct { Decision string "json:\"decision\""; Reason string "json:\"reason\"" }"#,
    "",
    &[Field("decision", &Ty::Str), Field("reason", &Ty::Str)],
);

fn opt_str(v: Option<&Val>) -> Option<String> {
    match v {
        Some(Val::Ptr(Some(b))) => match &**b {
            Val::Str(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A run's start, what the HTTP side waits for: the run, or the error its request gets.
type Started = Result<Arc<Run>, ApiErr>;

impl Server {
    /// Go's `onAgentEvent`: the agent's events as the console's.
    fn on_agent_event(&self, r: &Run, effects: &crate::agent::run::RegistryWatch, e: &BTreeMap<String, Any>) {
        let typ = match e.get("type") {
            Some(Any::String(t)) => t.as_str(),
            _ => "",
        };
        let str = |k: &str| match e.get(k) {
            Some(Any::String(v)) => v.clone(),
            _ => String::new(),
        };
        let raw = |k: &str| json_any(e.get(k).unwrap_or(&Any::Null));
        let sc = |t: &str| json_str(&(r.scrub)(t));
        match typ {
            "turn_start" => self.emit(&r.id, "turn_start", vec![("turn", raw("turn_index"))]),
            "content_delta" => self.emit(&r.id, "text_delta", vec![("delta", sc(&str("delta")))]),
            "assistant_message" => self.emit(
                &r.id,
                "assistant_message",
                vec![("turn", raw("turn_index")), ("content", sc(&str("content"))), ("tool_calls", raw("tool_calls"))],
            ),
            "tool_start" => {
                let name = str("tool_name");
                let effect = match effects.lock().unwrap_or_else(|p| p.into_inner()).get(&name) {
                    Some(Permission::Shell) => "shell",
                    Some(Permission::Write) => "write",
                    _ => "read",
                };
                let args = runkit::scrub_any(e.get("arguments").unwrap_or(&Any::Null), &*r.scrub);
                self.emit(
                    &r.id,
                    "tool_call",
                    vec![
                        ("tool_call_id", json_str(&str("tool_call_id"))),
                        ("name", json_str(&name)),
                        ("effect", json_str(effect)),
                        ("arguments", json_any(&args)),
                    ],
                );
            }
            "tool_delta" => self.emit(
                &r.id,
                "tool_delta",
                vec![("tool_call_id", json_str(&str("tool_call_id"))), ("stream", json_str(&str("stream"))), ("delta", sc(&str("delta")))],
            ),
            "tool_result" => {
                let mut md = BTreeMap::new();
                if let Some(Any::Object(m)) = e.get("metadata") {
                    if let Some(k) = m.get("error_kind") {
                        md.insert("error_kind".to_owned(), k.clone());
                    }
                }
                self.emit(
                    &r.id,
                    "tool_result",
                    vec![
                        ("tool_call_id", json_str(&str("tool_call_id"))),
                        ("name", json_str(&str("tool_name"))),
                        ("ok", raw("ok")),
                        ("content", sc(&str("content"))),
                        ("metadata", json_any(&Any::Object(md))),
                    ],
                );
            }
            "tier_escalation" | "tier_refused" | "tier_redaction" | "tier_wake" | "tier_notice" => {
                self.emit(&r.id, "info", vec![("name", json_str(typ)), ("text", sc(&str("line")))])
            }
            _ => {
                for p in [
                    "auto_compact",
                    "prompt_length",
                    "reactive_compact",
                    "tool_output_cleared",
                    "session_notes",
                    "subagent_",
                    "continuation_request",
                    "task_budget_exceeded",
                ] {
                    if typ.starts_with(p) {
                        self.emit(&r.id, "info", vec![("name", json_str(typ))]);
                        return;
                    }
                }
            }
        }
    }

    /// Go's `approvalView`.
    fn approval_view(&self, ap: &Approval, scrub: &dyn Fn(&str) -> String) -> Any {
        let mut m = BTreeMap::new();
        for (k, v) in [
            ("approval_id", &ap.id),
            ("kind", &ap.kind),
            ("run_id", &ap.run_id),
            ("session_id", &ap.session_id),
            ("risk", &ap.risk),
            ("created_at", &ap.created),
        ] {
            m.insert(k.to_owned(), s(v));
        }
        m.insert("summary".into(), s(&scrub(&ap.summary)));
        if ap.kind == "tool" {
            m.insert("tool_name".into(), s(&ap.tool));
            m.insert("arguments".into(), runkit::scrub_any(&ap.arguments, scrub));
        } else {
            m.insert("plan".into(), s(&scrub(&ap.plan)));
        }
        Any::Object(m)
    }

    /// Go's `ask`: a pending approval, waited for until the operator answers, the run is cancelled or the timeout
    /// passes (a denial). The decision and whether the operator gave it.
    #[allow(clippy::too_many_arguments)]
    fn ask(&self, r: &Run, kind: &str, tool: &str, arguments: Any, plan: &str, summary: String, risk: String) -> ((String, String), bool) {
        let (tx, rx) = sync_channel::<(String, String)>(1);
        let ap = {
            let mut g = self.lock();
            g.next_app += 1;
            let id = format!("a{}-{}", g.next_app, &crate::agent::run::new_session_id()[..8]);
            let ap = Arc::new(Approval {
                id: id.clone(),
                kind: kind.into(),
                run_id: r.id.clone(),
                session_id: r.current_session(),
                tool: tool.into(),
                arguments,
                plan: plan.into(),
                summary,
                risk,
                created: super::now_rfc3339(),
                decide: tx,
            });
            g.approvals.insert(id, ap.clone());
            ap
        };
        r.st().waiting += 1;
        self.emit(&r.id, "approval_request", vec![("approval", json_any(&self.approval_view(&ap, &*r.scrub)))]);
        let until = Instant::now() + self.cfg.approval_timeout;
        let (d, answered) = loop {
            if r.ctx.err().is_some() {
                break (("deny".to_owned(), "the run was cancelled".to_owned()), false);
            }
            let now = Instant::now();
            if now >= until {
                break (("deny".to_owned(), "no answer before the approval timeout".to_owned()), false);
            }
            match rx.recv_timeout((until - now).min(Duration::from_millis(20))) {
                Ok(d) => break (d, true),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break (("deny".to_owned(), "the run was cancelled".to_owned()), false),
            }
        };
        self.emit(
            &r.id,
            "approval_resolved",
            vec![("approval_id", json_str(&ap.id)), ("decision", json_str(&d.0)), ("answered", answered.to_string())],
        );
        r.st().waiting -= 1;
        self.lock().approvals.remove(&ap.id);
        (d, answered)
    }

    pub(super) fn list_approvals(&self, w: &mut Response) {
        let scrub = self.scrubber();
        let mut list: Vec<Arc<Approval>> = self.lock().approvals.values().cloned().collect();
        list.sort_by_key(|a| a.created.clone() + &a.id);
        let out: Vec<Any> = list.iter().map(|ap| self.approval_view(ap, &*scrub)).collect();
        write_json(w, 200, &Any::Array(out));
    }

    pub(super) fn answer_approval(&self, ex: &mut Exchange, w: &mut Response, id: &str) {
        let Some(body) = read_json(ex, w) else { return };
        let Some(v) = super::decode(&body, &DECISION, w) else { return };
        let decision = gostruct::str_field(&DECISION, &v, "decision").to_owned();
        let reason = gostruct::str_field(&DECISION, &v, "reason").to_owned();
        let Some(ap) = self.lock().approvals.get(id).cloned() else {
            detail(w, 404, "no pending approval with that id (it may have been answered or timed out)");
            return;
        };
        let valid: &[&str] =
            if ap.kind == "plan" { &[PLAN_APPROVE, PLAN_REVISE, PLAN_REJECT] } else { &["allow_once", "allow_session", "deny"] };
        if !valid.contains(&decision.as_str()) {
            detail(w, 400, &format!("decision is not valid for a {} approval", ap.kind));
            return;
        }
        if ap.kind == "plan" && decision == PLAN_REVISE && gotext::trim_space(&reason).is_empty() {
            detail(w, 400, "revising a plan needs feedback in reason");
            return;
        }
        if ap.decide.try_send((decision.clone(), reason)).is_ok() {
            write_json(w, 200, &obj(vec![("approval_id", s(&ap.id)), ("decision", s(&decision))]));
        } else {
            detail(w, 409, "already answered");
        }
    }

    /// Go's `startRun`: checks, the run slot taken, the agent built from the current settings on a thread of its own, the
    /// turn run there.
    fn start_run(self: &Arc<Self>, v: &Val) -> Started {
        let prompt = opt_str(gostruct::field(&CHAT, v, "prompt")).unwrap_or_default();
        if prompt.is_empty() {
            return Err(ApiErr { code: 422, msg: "prompt: field required (min length 1)".into() });
        }
        let mut pastes = BTreeMap::new();
        if let Some(Val::Map(Some(m))) = gostruct::field(&CHAT, v, "pasted_contents") {
            for (k, p) in m {
                pastes.insert(k.clone(), gostruct::str_field(&PASTED, p, "content").to_owned());
            }
        }
        let prompt = gotext::trim_space(&expand_pastes(&prompt, &pastes)).to_owned();
        if prompt.is_empty() {
            return Err(bad("prompt must not be blank"));
        }
        let resume = opt_str(gostruct::field(&CHAT, v, "resume_session_id")).filter(|r| !r.is_empty());
        // the single run slot, taken first so two requests cannot both start
        let (st, prev, r) = {
            let mut g = self.lock();
            if g.run.as_ref().is_some_and(|r| r.running()) {
                return Err(ApiErr { code: 409, msg: "a run is already in progress; stop it first".into() });
            }
            let st = g.st.clone();
            let r = Arc::new(Run {
                id: crate::agent::run::new_session_id()[..12].to_owned(),
                ctx: Ctx::background(),
                started: SystemTime::now(),
                scrub: runkit::scrubber(&[&st.api_key, &self.token]),
                m: Mutex::new(RunState::default()),
                done: Condvar::new(),
            });
            let prev = g.run.replace(r.clone());
            (st, prev, r)
        };
        let release = |s: &Server| {
            r.ctx.cancel();
            let mut g = s.lock();
            if g.run.as_ref().is_some_and(|x| Arc::ptr_eq(x, &r)) {
                g.run = prev.clone();
            }
        };
        let mut stored = None;
        if let Some(id) = &resume {
            if !super::api::valid_id(id) {
                release(self);
                return Err(ApiErr { code: 404, msg: "session to resume not found".into() });
            }
            match crate::session::readfile::load_by_id(&st.session_dir, id) {
                Ok(s) => stored = Some(s),
                Err(_) => {
                    release(self);
                    return Err(ApiErr { code: 404, msg: "session to resume not found".into() });
                }
            }
        }
        if let Some(text) = crate::cli::chat::local_slash(&prompt) {
            self.finish_local(&r, &prompt, &text, stored.as_ref().map(|s| s.id.clone()));
            return Ok(r);
        }
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let srv = self.clone();
        let run = r.clone();
        let spawned = std::thread::Builder::new().name("gui-run".into()).stack_size(8 << 20).spawn(move || {
            srv.run_thread(run, st, stored, prompt, ready_tx);
        });
        if let Err(e) = spawned {
            release(self);
            return Err(bad(format!("cannot start the run: {e}")));
        }
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(r),
            Ok(Err(e)) => {
                release(self);
                Err(bad(e))
            }
            Err(_) => {
                release(self);
                Err(bad("cannot start the run"))
            }
        }
    }

    /// The run's thread: the agent built (its error back to the request), the start announced, the turn run, the end
    /// recorded and announced.
    fn run_thread(
        self: Arc<Self>,
        r: Arc<Run>,
        st: super::state::State,
        stored: Option<crate::session::Session>,
        prompt: String,
        ready: std::sync::mpsc::Sender<Result<(), String>>,
    ) {
        let (m, rt) = st.model_runtime();
        let approver: Option<crate::tools::registry::Approver> = (st.approval_mode == APPROVAL_ASK).then(|| {
            let srv = self.clone();
            let run = r.clone();
            let root = rt.cwd.clone();
            let f: crate::tools::registry::Approver = Arc::new(move |req: &ApprovalRequest| {
                if run.st().grants.contains(&req.tool) {
                    return (true, String::new());
                }
                let args = Any::Object(req.arguments.clone());
                let ((d, reason), _) =
                    srv.ask(&run, "tool", &req.tool, args, "", summarize(&req.tool, &req.arguments), risk_for(req, &root));
                match d.as_str() {
                    "allow_session" => {
                        run.st().grants.insert(req.tool.clone());
                        (true, String::new())
                    }
                    "allow_once" => (true, String::new()),
                    _ => (false, reason),
                }
            });
            f
        });
        let built = runkit::build(runkit::Spec {
            model: m,
            runtime: rt,
            allowlist: None,
            prompt: st.prompt_options(),
            home: self.cfg.home.clone(),
            getenv: Some(self.cfg.getenv.clone()),
            client: None,
            approver,
            discover: self.cfg.discover,
            use_tiers: true,
            escalation: self.cfg.escalation.clone(),
            subagents: true,
            builtins: None,
            trust: None,
        });
        let mut built = match built {
            Ok(b) => b,
            Err(e) => {
                let _ = ready.send(Err((r.scrub)(&e)));
                return;
            }
        };
        let agent = &mut built.agent;
        if let Some(s) = &stored {
            r.st().sess_id = s.id.clone();
        }
        let effects = agent.watch_registry();
        let (srv, run) = (self.clone(), r.clone());
        agent.set_event_hook(Some(Arc::new(move |e: &BTreeMap<String, Any>| srv.on_agent_event(&run, &effects, e))));
        let (srv, run) = (self.clone(), r.clone());
        agent.set_plan_decider(Some(Arc::new(move |plan: &str| {
            let summary = "The agent submitted a plan and is waiting for your decision.".to_owned();
            let ((d, reason), answered) =
                srv.ask(&run, "plan", "", Any::Null, plan, summary, "none: nothing changes until you approve".into());
            if !answered {
                return PlanDecision::default();
            }
            PlanDecision { action: d, feedback: reason }
        })));
        self.emit(&r.id, "run_start", vec![("prompt", json_str(&(r.scrub)(&prompt))), ("resumed", stored.is_some().to_string())]);
        for n in &built.notices {
            self.emit(&r.id, "info", vec![("name", json_str("notice")), ("text", json_str(&(r.scrub)(n)))]);
        }
        let _ = ready.send(Ok(()));
        let agent = &mut built.agent;
        let (res, err) = match stored {
            Some(s) => agent.resume(&r.ctx, &prompt, s),
            None => agent.run(&r.ctx, &prompt),
        };
        let mut failure = None;
        match (res, err) {
            (Some(res), None) => self.record_result(&r, &res),
            (_, Some(e)) => failure = Some(e),
            (None, None) => failure = Some("the run produced no result".into()),
        }
        if let Some(e) = &failure {
            r.st().err = Some(((r.scrub)(e), runkit::error_type(e).to_owned()));
        }
        r.finish();
        r.ctx.cancel();
        if let Some(e) = failure {
            self.emit(&r.id, "error", vec![("error", json_str(&(r.scrub)(&e))), ("error_type", json_str(runkit::error_type(&e)))]);
        }
        let result = r.st().result.clone().unwrap_or_else(|| "null".into());
        self.emit(&r.id, "done", vec![("result", result)]);
    }

    fn record_result(&self, r: &Run, res: &RunResult) {
        let sc = |x: &str| (r.scrub)(x);
        let json = runkit::result_json(res, Some(&sc)).unwrap_or_else(|_| "null".into());
        {
            let mut st = r.st();
            st.result = Some(json);
            st.sess_id = res.session_id.clone();
        }
        self.lock().st.active_session_id = res.session_id.clone();
        self.emit(
            &r.id,
            "usage",
            vec![
                ("usage", res.usage.to_json()),
                ("total_cost_usd", json_any(&Any::Number(res.total_cost_usd))),
                ("turns", res.turns.to_string()),
                ("tool_calls", res.tool_calls.to_string()),
            ],
        );
    }

    /// Go's `finishLocal`: a slash command the console answers itself.
    fn finish_local(&self, r: &Run, prompt: &str, text: &str, sid: Option<String>) {
        let resumed = sid.is_some();
        let sid = sid.unwrap_or_default();
        r.st().sess_id = sid.clone();
        let n = |v: i64| Any::Int(v);
        let usage = obj(vec![
            ("input_tokens", n(0)),
            ("output_tokens", n(0)),
            ("cache_creation_input_tokens", n(0)),
            ("cache_read_input_tokens", n(0)),
            ("reasoning_tokens", n(0)),
            ("total_tokens", n(0)),
        ]);
        let transcript = Any::Array(vec![
            obj(vec![("role", s("user")), ("content", s(prompt))]),
            obj(vec![("role", s("assistant")), ("content", s(text))]),
        ]);
        let result = json_any(&obj(vec![
            ("final_output", s(text)),
            ("turns", n(0)),
            ("tool_calls", n(0)),
            ("session_id", s(&sid)),
            ("usage", usage),
            ("total_cost_usd", n(0)),
            ("stop_reason", s("slash_command")),
            ("transcript", transcript),
        ]));
        r.st().result = Some(result.clone());
        self.emit(&r.id, "run_start", vec![("prompt", json_str(prompt)), ("resumed", resumed.to_string())]);
        self.emit(&r.id, "assistant_message", vec![("turn", "0".into()), ("content", json_str(text)), ("tool_calls", "0".into())]);
        r.finish();
        r.ctx.cancel();
        self.emit(&r.id, "done", vec![("result", result)]);
    }

    /// Go's `chat`: a run started; with Accept: text/event-stream the answer is its event stream (until its "done"),
    /// else the result once it ends (a client that hangs up cancels it).
    pub(super) fn chat(self: &Arc<Self>, ex: &mut Exchange, w: &mut Response) {
        let Some(body) = read_json(ex, w) else { return };
        let Some(v) = super::decode(&body, &CHAT, w) else { return };
        let run = match self.start_run(&v) {
            Ok(r) => r,
            Err(e) => return fail(w, &e),
        };
        if String::from_utf8_lossy(ex.req.header("Accept")).contains("text/event-stream") {
            self.stream(ex, w, 0, Some(&run.id));
            return;
        }
        let mut watching = true;
        let finished = run.wait(|| {
            if watching {
                match ex.peer_gone() {
                    Some(true) => return true,
                    Some(false) => watching = false,
                    None => {}
                }
            }
            false
        });
        if !finished {
            run.ctx.cancel(); // a script that hung up does not leave an agent running
            run.wait(|| false);
        }
        let st = run.st();
        if let Some((e, t)) = &st.err {
            write_json(w, 500, &obj(vec![("error", s(e)), ("error_type", s(t))]));
            return;
        }
        write_raw(w, 200, st.result.clone().unwrap_or_else(|| "null".into()));
    }

    pub(super) fn cancel(&self, w: &mut Response) {
        let r = self.lock().run.clone();
        match r {
            Some(r) if r.running() => {
                r.ctx.cancel();
                write_json(w, 200, &obj(vec![("cancelled", Any::Bool(true)), ("run_id", s(&r.id))]));
            }
            _ => write_json(w, 200, &obj(vec![("cancelled", Any::Bool(false))])),
        }
    }

    pub(super) fn run_status(&self, w: &mut Response) {
        let scrub = self.scrubber();
        let (r, last, pending) = {
            let g = self.lock();
            (g.run.clone(), g.seq, g.approvals.values().cloned().collect::<Vec<_>>())
        };
        let mut out = BTreeMap::new();
        out.insert("state".to_owned(), s(STATE_IDLE));
        out.insert("run_id".to_owned(), Any::Null);
        out.insert("session_id".to_owned(), Any::Null);
        out.insert("last_seq".to_owned(), Any::Int(last));
        if let Some(r) = r {
            out.insert("state".into(), s(r.state()));
            out.insert("run_id".into(), s(&r.id));
            out.insert("started_at".into(), s(&super::rfc3339(r.started)));
            let sid = r.current_session();
            if !sid.is_empty() {
                out.insert("session_id".into(), s(&sid));
            }
        }
        out.insert("pending_approvals".into(), Any::Array(pending.iter().map(|ap| self.approval_view(ap, &*scrub)).collect()));
        write_json(w, 200, &Any::Object(out));
    }

    /// Go's `eventStream`: GET /api/events?after=N replays what the page missed, then follows live.
    pub(super) fn event_stream(&self, ex: &mut Exchange, w: &mut Response) {
        let after = sscan_int(&super::query_get(&ex.req.url.raw_query, "after"));
        self.stream(ex, w, after, None);
    }

    /// Go's `stream`: server-sent events after `after`; with a run id only that run's, ending after its "done". A
    /// comment every 15 s keeps the connection alive; it ends when the client hangs up.
    pub(super) fn stream(&self, ex: &mut Exchange, w: &mut Response, mut after: i64, run_id: Option<&str>) {
        let mut head = Response::new(200);
        head.header = std::mem::take(&mut w.header);
        head.set("Content-Type", "text/event-stream");
        head.set("Cache-Control", "no-store");
        head.set("X-Accel-Buffering", "no");
        ex.begin_stream(&head);
        let mut next_beat = Instant::now() + Duration::from_secs(15);
        let mut watching = true;
        loop {
            let (evs, seen) = {
                let g = self.lock();
                (g.events.iter().filter(|e| e.seq > after).cloned().collect::<Vec<_>>(), g.seq)
            };
            let mut buf = String::new();
            for e in evs {
                after = e.seq;
                if let Some(id) = run_id {
                    if e.run_id != id {
                        continue;
                    }
                }
                buf.push_str(&format!("id: {}\nevent: {}\ndata: {}\n\n", e.seq, e.typ, e.json));
                if run_id.is_some() && e.typ == "done" {
                    ex.stream_write(buf.as_bytes());
                    return;
                }
            }
            ex.stream_write(buf.as_bytes());
            // wait for an event, the keep-alive or the client's going
            loop {
                let now = Instant::now();
                if now >= next_beat {
                    next_beat += Duration::from_secs(15);
                    ex.stream_write(b": keep-alive\n\n");
                    break;
                }
                if watching {
                    match ex.peer_gone() {
                        Some(true) => return,
                        Some(false) => watching = false,
                        None => {}
                    }
                }
                let g = self.lock();
                if g.seq != seen {
                    break;
                }
                let wait = (next_beat - now).min(Duration::from_millis(50));
                let g = self.wait_event(g, wait);
                if g.seq != seen {
                    break;
                }
            }
        }
    }
}

/// Go's `fmt.Sscanf(s, "%d", &n)`: a decimal after spaces (with a sign); 0 when there is none or it does not fit.
fn sscan_int(s: &str) -> i64 {
    let t = s.trim_start_matches([' ', '\t', '\r']);
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let start = i;
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    if i == start {
        return 0;
    }
    t[..i].parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(sscan_int("12"), 12);
        assert_eq!(sscan_int(" -3x"), -3);
        assert_eq!(sscan_int("1_0"), 0);
        assert_eq!(sscan_int(""), 0);
        assert_eq!(clip("abcdef", 3), "abc...");
        let mut m = BTreeMap::new();
        m.insert("1".to_owned(), "first".to_owned());
        assert_eq!(expand_pastes("a [Pasted text #1 +2 lines] [Pasted text #2]", &m), "a first [Pasted text #2]");
        let mut args = BTreeMap::new();
        args.insert("b".to_owned(), Any::Number(5.0));
        args.insert("a".to_owned(), s("x"));
        assert_eq!(summarize("t", &args), "t(a=x, b=5)");
    }
}
