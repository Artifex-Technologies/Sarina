//! Go: `cli/chat.go`, `slash.go`, `plancmd.go`, the conversation parts of `skillcmd.go`, `tiercmd.go` (`tierRouter`,
//! `stdinConfirm`, `linesConfirm`, `/tier`) and `memorycmd.go` (`memory-review`, the review when a conversation ends):
//! `sarina chat`, `sarina resume`, `sarina skill list|run` and `sarina memory-review`, on `agent::run`.
//!
//! The agent gets every tool the Go build offers by default (the built-ins, memory, Limbo, and `sarina_doctor`, whose
//! check-up runs in the Go build: `doctor_tool`). The optional tool sets that are not ported yet (vault, system, crypto
//! and steganography, Trachtenberg, quantum, reverse engineering, the forge, the council) make [`needs_go`] answer, and
//! the command is then handed to the Go build whole.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::chatopts::ChatOpts;
use super::{Env, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::agent::run::{
    Agent, Options as AgentOptions, PlanDecision, RunResult, PLAN_APPROVE, PLAN_REJECT, PLAN_REVISE, STOP_BACKEND_ERROR,
    STOP_BUDGET_EXCEEDED, STOP_INTERRUPTED, STOP_MAX_TURNS, STOP_PLAN_PENDING, STOP_PROMPT_TOO_LONG,
};
use crate::agent::Event;
use crate::config::ModelConfig;
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::goflag::HELP;
use crate::gotext;
use crate::llm::Completer;
use crate::session::Session;
use crate::skills::discover::Catalog;
use crate::skills::recall::Library;
use crate::tiers::router::Router;
use crate::tools::registry::{Builder, Context, Output, Tool};

pub const EXIT_PLAN_PENDING: i32 = 3;
pub const EXIT_INTERRUPTED: i32 = 130;

/// Go's `DraftWithdrawn`: printed in a streamed reply where a check sent the draft back.
pub const DRAFT_WITHDRAWN: &str = "\u{1e}draft withdrawn\u{1e}";

// ---- which build answers ----------------------------------------------------------------------------------------------

/// Why this command line needs the Go build: an optional tool set that is not ported is switched on, so the Go build
/// would offer the model tools the port does not have. None: the port offers exactly the Go build's tools.
pub fn needs_go(getenv: &dyn Fn(&str) -> String, home: &str) -> Option<String> {
    for (k, on) in [
        ("SARINA_CRYPTO", &["on", "auto"][..]),
        ("SARINA_MATH", &["on", "auto"][..]),
        ("SARINA_QUANTUM", &["on"][..]),
        ("SARINA_REVERSE", &["on"][..]),
        ("SARINA_SYSTEM", &["on"][..]),
        ("SARINA_FORGE", &["on"][..]),
    ] {
        if on.contains(&getenv(k).as_str()) {
            return Some(format!("{k}={}", getenv(k)));
        }
    }
    if !getenv("SARINA_VAULT").is_empty() {
        return Some("SARINA_VAULT is set".into());
    }
    // the council's tool exists only with a council file; any file at all goes to the Go build, which reads it
    let p = {
        let v = getenv("SARINA_COUNCIL_FILE");
        let v = gotext::trim_space(&v);
        if !v.is_empty() {
            v.to_owned()
        } else {
            let h = if home.is_empty() { getenv("HOME") } else { home.to_owned() };
            if h.is_empty() {
                String::new()
            } else {
                format!("{h}/.sarina/council.json")
            }
        }
    };
    if !p.is_empty() && std::fs::symlink_metadata(&p).is_ok() {
        return Some(format!("a council file ({p})"));
    }
    None
}

// ---- output ------------------------------------------------------------------------------------------------------------

/// Where a conversation writes: the Env's sink (or its buffers), shared with the event hook and the confirmations.
#[derive(Clone)]
struct Io(Arc<Mutex<Env>>);

impl Io {
    fn out(&self, s: &str) {
        let mut e = self.0.lock().unwrap_or_else(|p| p.into_inner());
        e.stdout.extend_from_slice(s.as_bytes());
        e.flush();
    }
    fn err(&self, s: &str) {
        let mut e = self.0.lock().unwrap_or_else(|p| p.into_inner());
        e.stderr.extend_from_slice(s.as_bytes());
        e.flush();
    }
    fn outln(&self, s: &str) {
        self.out(&format!("{s}\n"));
    }
    fn errln(&self, s: &str) {
        self.err(&format!("{s}\n"));
    }
}

// ---- Ctrl-C ---------------------------------------------------------------------------------------------------------

/// What the interactive loop waits for: a line, the end of input, or Ctrl-C.
enum Input {
    Line(String),
    Eof,
    Interrupt,
}

/// Ctrl-C (Go's `env.Interrupts`): while a turn runs it cancels the turn; otherwise, and while a question to the operator
/// waits during a turn, it reaches whoever waits for input.
struct Interrupts {
    turn: Mutex<Option<Ctx>>,
    asking: AtomicBool,
    tx: Mutex<Option<Sender<Input>>>,
}

impl Interrupts {
    fn new() -> Arc<Interrupts> {
        Arc::new(Interrupts { turn: Mutex::new(None), asking: AtomicBool::new(false), tx: Mutex::new(None) })
    }

    /// SIGINT blocked in every thread, and one thread that takes it (as Go's signal.Notify, the default action is gone).
    fn install(self: &Arc<Self>) {
        // SAFETY: a signal set built and used locally; pthread_sigmask on the calling thread only.
        unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGINT);
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        }
        let me = self.clone();
        std::thread::spawn(move || loop {
            let mut sig: libc::c_int = 0;
            // SAFETY: as above; sigwait fills `sig`.
            let r = unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGINT);
                libc::sigwait(&set, &mut sig)
            };
            if r == 0 {
                me.fire();
            }
        });
    }

    fn fire(&self) {
        let running = self.turn.lock().unwrap_or_else(|p| p.into_inner()).clone();
        if let Some(ctx) = &running {
            ctx.cancel();
        }
        if running.is_none() || self.asking.load(Ordering::SeqCst) {
            if let Some(tx) = self.tx.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
                let _ = tx.send(Input::Interrupt);
            }
        }
    }

    /// Runs `f` as a turn: Ctrl-C cancels its context.
    fn turn<T>(&self, f: impl FnOnce(&Ctx) -> T) -> T {
        let ctx = Ctx::background();
        *self.turn.lock().unwrap_or_else(|p| p.into_inner()) = Some(ctx.clone());
        let r = f(&ctx);
        *self.turn.lock().unwrap_or_else(|p| p.into_inner()) = None;
        r
    }
}

// ---- reading the operator's answers -----------------------------------------------------------------------------------

/// Go's `bufio.Reader` over standard input: reads of up to 4096 bytes, so a reader may take more than one line from a
/// pipe (and another reader then sees what is left), as each of Go's `stdinConfirm`s does.
struct StdinLines {
    buf: Vec<u8>,
    eof: bool,
}

impl StdinLines {
    fn new() -> StdinLines {
        StdinLines { buf: Vec::new(), eof: false }
    }

    /// Go's `ReadString('\n')`: a line with its newline, or what is left at the end.
    fn read_line(&mut self) -> (Vec<u8>, bool) {
        loop {
            if let Some(i) = self.buf.iter().position(|&c| c == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return (line, true);
            }
            if self.eof {
                return (std::mem::take(&mut self.buf), false);
            }
            let mut chunk = [0u8; 4096];
            // fd 0 read directly: Rust's own stdin buffer would take more than Go's 4096
            let mut f = std::mem::ManuallyDrop::new(unsafe_stdin());
            match f.read(&mut chunk) {
                Ok(0) | Err(_) => self.eof = true,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
            }
        }
    }
}

fn unsafe_stdin() -> std::fs::File {
    use std::os::fd::FromRawFd;
    // SAFETY: fd 0 is this process's standard input; the File is never dropped (ManuallyDrop), so it is not closed.
    unsafe { std::fs::File::from_raw_fd(0) }
}

/// Go's `stdinConfirm`: the question on stderr, one line read from standard input; yes is "y" or "yes".
fn stdin_confirm(io: &Io) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    let reader = Arc::new(Mutex::new(StdinLines::new()));
    let io = io.clone();
    let io = Arc::new(Mutex::new(io));
    Arc::new(move |prompt: &str| {
        io.lock().unwrap_or_else(|p| p.into_inner()).err(prompt);
        let (line, _) = reader.lock().unwrap_or_else(|p| p.into_inner()).read_line();
        let a = gotext::to_lower(gotext::trim_space(&gotext::lossy(&line)));
        a == "y" || a == "yes"
    })
}

/// The interactive input: one thread reads lines (Go's goroutine) into the channel the loop, the plan question and the
/// confirmations read.
struct Lines {
    rx: Mutex<Receiver<Input>>,
}

impl Lines {
    fn start(ints: &Arc<Interrupts>) -> Arc<Lines> {
        let (tx, rx) = std::sync::mpsc::channel();
        *ints.tx.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx.clone());
        std::thread::spawn(move || {
            let mut r = StdinLines::new();
            loop {
                let (line, nl) = r.read_line();
                if !line.is_empty() {
                    let mut s = gotext::lossy(&line);
                    while s.ends_with('\n') || s.ends_with('\r') {
                        s.pop();
                    }
                    if tx.send(Input::Line(s)).is_err() {
                        return;
                    }
                }
                if !nl {
                    let _ = tx.send(Input::Eof);
                    return;
                }
            }
        });
        Arc::new(Lines { rx: Mutex::new(rx) })
    }

    fn next(&self) -> Input {
        self.rx.lock().unwrap_or_else(|p| p.into_inner()).recv().unwrap_or(Input::Eof)
    }
}

/// Go's `linesConfirm`: the question on stdout, the answer from the interactive input; Ctrl-C is a no.
fn lines_confirm(io: &Io, lines: &Arc<Lines>, ints: &Arc<Interrupts>) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    let io = Arc::new(Mutex::new(io.clone()));
    let lines = lines.clone();
    let ints = ints.clone();
    Arc::new(move |prompt: &str| {
        let io = io.lock().unwrap_or_else(|p| p.into_inner()).clone();
        io.out(prompt);
        ints.asking.store(true, Ordering::SeqCst);
        let got = lines.next();
        ints.asking.store(false, Ordering::SeqCst);
        match got {
            Input::Line(l) => {
                let a = gotext::to_lower(gotext::trim_space(&l));
                a == "y" || a == "yes"
            }
            Input::Interrupt => {
                io.outln("");
                false
            }
            Input::Eof => false,
        }
    })
}

/// Go's `askPlanDecision`: the plan shown, a decision read; no answer leaves the plan pending.
fn ask_plan_decision(io: &Io, lines: &Lines, ints: &Interrupts, plan: &str) -> PlanDecision {
    io.out(&format!("\n--- plan awaiting your decision ---\n{plan}\n-----------------------------------\n"));
    let read = |prompt: &str| -> Option<String> {
        io.out(prompt);
        ints.asking.store(true, Ordering::SeqCst);
        let got = lines.next();
        ints.asking.store(false, Ordering::SeqCst);
        match got {
            Input::Line(l) => Some(gotext::trim_space(&l).to_owned()),
            Input::Interrupt => {
                io.outln("");
                None
            }
            Input::Eof => None,
        }
    };
    loop {
        let Some(ans) = read("approve, revise or reject? [a/r/j] ") else { return PlanDecision::default() };
        match gotext::to_lower(&ans).as_str() {
            "a" | "approve" | "y" | "yes" => return PlanDecision { action: PLAN_APPROVE.into(), feedback: String::new() },
            "j" | "reject" | "n" | "no" => return PlanDecision { action: PLAN_REJECT.into(), feedback: String::new() },
            "r" | "revise" => {
                let Some(fb) = read("what should change? ") else { return PlanDecision::default() };
                return PlanDecision { action: PLAN_REVISE.into(), feedback: fb };
            }
            _ => {}
        }
    }
}

// ---- the agent ---------------------------------------------------------------------------------------------------------

/// A command's state (the rest of Go's `opts`).
pub struct Chat {
    pub o: ChatOpts,
    io: Io,
    getenv: Arc<dyn Fn(&str) -> String + Send + Sync>,
    home: String,
    cat: Option<Arc<Catalog>>,
    lib: Option<Arc<Library>>,
    router: Option<Arc<Router>>,
    client: Option<Arc<dyn Completer>>,
    mcp: Option<crate::mcp::manager::Manager>,
    no_slash: bool,
    auto_review: bool,
    /// Ctrl-C, taken before any thread starts (MCP servers' readers, the model clients) so none of them gets it
    ints: Arc<Interrupts>,
}

fn trust_fn() -> Arc<dyn Fn(&str) -> String + Send + Sync> {
    let c = Arc::new(crate::trust::Checker::new(crate::trust::default_dir(&|k: &str| std::env::var(k).unwrap_or_default()).as_bytes()));
    Arc::new(move |p: &str| c.refuse(p.as_bytes()))
}

impl Chat {
    fn new(name: &'static str, env: &mut Env) -> Chat {
        let getenv: Arc<dyn Fn(&str) -> String + Send + Sync> = match &env.getenv {
            Some(g) => {
                let g = g.clone();
                Arc::new(move |k: &str| g(k))
            }
            None => Arc::new(|k: &str| Env::default().getenv(k)),
        };
        let o = ChatOpts::new(name, &*getenv);
        let home = env.home.clone();
        let io = Io(Arc::new(Mutex::new(std::mem::take(env))));
        let ints = Interrupts::new();
        ints.install();
        Chat { o, io, getenv, home, cat: None, lib: None, router: None, client: None, mcp: None, no_slash: false, auto_review: false, ints }
    }

    /// Hands the output back to the caller's Env.
    fn finish(self, env: &mut Env, code: i32) -> i32 {
        if let Some(m) = &self.mcp {
            m.close();
        }
        let mut e = self.io.0.lock().unwrap_or_else(|p| p.into_inner());
        e.flush();
        *env = std::mem::take(&mut *e);
        code
    }

    fn home_dir(&self) -> String {
        if self.home.is_empty() {
            (self.getenv)("HOME")
        } else {
            self.home.clone()
        }
    }

    /// Go's `catalog`: skills discovered once; what the operator must know goes to stderr.
    fn catalog(&mut self) -> Arc<Catalog> {
        if let Some(c) = &self.cat {
            return c.clone();
        }
        let cwd = self.o.fs.str("cwd");
        let ws = crate::gopath::abs(cwd.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or(cwd);
        let g = self.getenv.clone();
        let cat = crate::skills::discover::discover(&crate::skills::discover::Options {
            home: self.home_dir(),
            getenv: Some(Arc::new(move |k: &str| g(k))),
            workspace: ws,
            trust_workspace: self.o.fs.is_true("trust-workspace-skills"),
            scanner: None,
            system_dir: String::new(),
            trust: Some(trust_fn()),
        });
        for n in &cat.notices {
            self.io.errln(&format!("[notice] {n}"));
        }
        for c in &cat.collisions {
            self.io.errln(&format!("[skills] {} is defined twice; using {}, ignoring {}", gotext::quote(&c.name), c.kept, c.shadowed));
        }
        if !cat.problems.is_empty() {
            self.io.errln(&format!("[skills] {} skill file(s) were not loaded; 'sarina skill list' shows why", cat.problems.len()));
        }
        let cat = Arc::new(cat);
        self.cat = Some(cat.clone());
        cat
    }

    /// Go's `library`: the long skills' search, with the local embedding model.
    fn library(&mut self) -> Arc<Library> {
        if let Some(l) = &self.lib {
            return l.clone();
        }
        let cat = self.catalog();
        let emb = crate::embed::default_embedder(&*self.getenv);
        let lib =
            Arc::new(Library::new(cat, Some(Arc::new(emb)), &crate::skills::recall::default_index_dir(&*self.getenv, &self.home_dir())));
        self.lib = Some(lib.clone());
        lib
    }

    /// Go's `tierRouter`: the operator's tiers, or None (only the local backend).
    fn tier_router(&mut self, m: &ModelConfig, client: Box<dyn Completer>, rt: &crate::config::RuntimeConfig) -> Option<Arc<Router>> {
        use crate::tiers::config::{from_escalation, load};
        let g = self.getenv.clone();
        let (mut cfg, err) = load(&self.home, Some(&*g));
        if let Some(e) = err {
            self.io.errln(&format!("[tier] ignoring the tiers config, staying on the local backend only: {e}"));
            return None;
        }
        if let Some(mut eo) = self.o.escalation() {
            eo.accept_edits = rt.permissions.allow_file_write || rt.permissions.allow_shell_commands;
            eo.extra_dirs = Some(rt.additional_working_directories.clone());
            cfg = from_escalation(&cfg, &crate::config::EscalationConfig::default(), &eo, Some(&*g));
        }
        for w in cfg.warnings.clone().unwrap_or_default() {
            self.io.errln(&format!("[tier] warning: {w}"));
        }
        if !cfg.multi() {
            return None;
        }
        let g2 = g.clone();
        Router::new(crate::tiers::router::Options {
            config: cfg,
            base: m.clone(),
            base_client: Some(client),
            getenv: Box::new(move |k: &str| g2(k)),
            workdir: rt.cwd.clone(),
            wake: Some(crate::tiers::wake::new_wake(rt.permissions.clone(), rt.cwd.clone())),
            ..crate::tiers::router::Options::default()
        })
        .ok()
        .map(Arc::new)
    }

    /// Go's `startMCP`.
    fn start_mcp(&mut self, trusted: bool, rt: &crate::config::RuntimeConfig, session_env: &BTreeMap<String, String>) -> Vec<Tool> {
        if !trusted {
            self.io.errln("[notice] MCP servers are disabled: the operator policy sets trusted=false");
            return Vec::new();
        }
        let g = self.getenv.clone();
        let cfg = crate::mcp::config::load(&crate::mcp::config::LoadOptions {
            home: self.home_dir(),
            getenv: Some(Arc::new(move |k: &str| g(k))),
            workspace: rt.cwd.clone(),
            trust_workspace: self.o.fs.is_true("trust-workspace-mcp"),
            trust: Some(trust_fn()),
        });
        for n in &cfg.notices {
            self.io.errln(&format!("[notice] {n}"));
        }
        for p in &cfg.problems {
            self.io.errln(&format!("[mcp] {p}"));
        }
        if cfg.servers.is_empty() {
            return Vec::new();
        }
        let tc = Context { session_env: session_env.clone(), ..Context::default() };
        let env: Vec<String> = crate::tools::exec::child_env(&tc).into_iter().map(|b| gotext::lossy(&b)).collect();
        let mut mgr = crate::mcp::manager::Manager::new(
            Some(&cfg),
            crate::mcp::manager::Options {
                env,
                max_result_chars: rt.max_output_chars.max(0) as usize,
                init_timeout: std::time::Duration::ZERO,
                request_timeout: std::time::Duration::ZERO,
                spawn: None,
            },
        );
        mgr.connect(&Ctx::background());
        for p in &mgr.problems {
            self.io.errln(&format!("[mcp] {p}"));
        }
        let tools = mgr.tools();
        self.mcp = Some(mgr);
        tools
    }

    /// Go's `opts.agent`.
    fn agent(&mut self, stored: Option<&Session>) -> Result<Agent, String> {
        let tier = crate::config::access_tier_from_env(&|k: &str| std::env::var(k).unwrap_or_default());
        let (m, mut rt, allow) = self.o.build(&*self.getenv, stored, tier)?;
        let base: Box<dyn Completer> = Box::new(crate::llm::client::Client::new(m.clone()));
        let mut client: Arc<dyn Completer> = Arc::new(crate::llm::client::Client::new(m.clone()));
        let mut tracker: Option<Arc<dyn crate::agent::run::TierTracker>> = None;
        self.router = self.tier_router(&m, base, &rt);
        if let Some(r) = &self.router {
            client = r.clone();
            tracker = Some(r.clone());
        }
        self.client = Some(client.clone());
        let g = self.getenv.clone();
        let trust = trust_fn();
        let pol = crate::policy::load(&crate::policy::Options {
            home: self.home_dir(),
            getenv: &*g,
            workspace: rt.cwd.clone(),
            trust: Some(&*trust),
        });
        if !pol.fatal.is_empty() {
            return Err(pol.fatal.join("; "));
        }
        for n in &pol.notices {
            self.io.errln(&format!("[notice] {n}"));
        }
        pol.apply_budget(&mut rt.budget_config);
        let mut session_env = BTreeMap::new();
        for name in &pol.safe_env {
            let v = g(name);
            if !v.is_empty() {
                session_env.insert(name.clone(), v);
            }
        }
        let pol = Arc::new(pol);
        let p2 = pol.clone();
        let mut builder =
            Builder { allowlist: allow, builtins: builtins(), deny: Some(Arc::new(move |t: &str| p2.denied(t))), ..Builder::default() };
        let cat = self.catalog();
        let lib = self.library();
        if let Some(t) = crate::skills::tool::skill_tool(cat.clone(), Some(lib.clone())) {
            builder.extra.push(t);
        }
        builder.extra.extend(crate::skills::tool::library_tools(cat, lib));
        let mcp_tools = self.start_mcp(pol.trusted, &rt, &session_env);
        builder.extra.extend(mcp_tools);
        let profiles =
            crate::agents::load(&crate::agents::Options { home: self.home_dir(), getenv: &*g, trusted: pol.trusted, trust: Some(&*trust) });
        for p in &profiles.problems {
            self.io.errln(&format!("[agents] {p}"));
        }
        let g3 = g.clone();
        Agent::new(AgentOptions {
            model: m,
            runtime: rt,
            client,
            tools: builder,
            prompt: crate::agent::prompt::PromptOptions {
                system: self.o.fs.str("system-prompt"),
                append: self.o.fs.str("append-system-prompt"),
                override_: self.o.fs.str("override-system-prompt"),
            },
            context_window: self.o.fs.num("context-window"),
            now: Arc::new(crate::gotime::Time::now),
            home: self.home.clone(),
            on_event: None,
            getenv: Arc::new(move |k: &str| g3(k)),
            discoverer: Some(Arc::new(crate::llm::discover::Discoverer::default())),
            scanner: None,
            tiers: tracker,
            session_env,
            parent_link: None,
            approver: None,
            confirm: None,
            subagents: Some(Arc::new(profiles)),
        })
    }
}

/// Go's stable built-ins: the default tools and every provider's, in the Go build's order (go/cmd/sarina's
/// TestParityToolDefs records it): the file, shell, to-do and plan tools, memory, Limbo, sarina_doctor.
pub fn builtins() -> Vec<Tool> {
    let mut files = crate::tools::files::file_tools();
    let take = |v: &mut Vec<Tool>, n: &str| {
        let i = v.iter().position(|t| t.spec.name == n).expect("file tool");
        v.remove(i)
    };
    let mut out =
        vec![take(&mut files, "read_file"), take(&mut files, "list_dir"), take(&mut files, "write_file"), take(&mut files, "edit_file")];
    out.push(crate::tools::verify::check_code_tool());
    out.push(take(&mut files, "glob_search"));
    out.push(take(&mut files, "grep_search"));
    out.push(crate::tools::git::git_tool());
    out.extend(crate::tools::exec::shell_tools());
    out.extend(crate::tools::todo::todo_tools());
    out.extend(crate::tools::plan::plan_tools());
    out.extend(crate::memory::tools());
    out.extend(crate::limbo::tools());
    out.push(doctor_tool());
    out
}

/// The most a check-up's report may be (it is a JSON list of short findings).
const MAX_DOCTOR_OUTPUT: usize = 4 << 20;

/// `sarina_doctor` (Go's `doctor.Tools`): the read-only check-up runs in the Go build (`doctor --json`), which is where
/// the check-up is until it is ported; the answer is its JSON list, `doctor_failed` whether a check failed.
pub fn doctor_tool() -> Tool {
    crate::tools::defs::tool(
        "sarina_doctor",
        Arc::new(|_args: &BTreeMap<String, Any>, tc: &Context| {
            let go = super::entry::go_binary(&|k: &str| std::env::var_os(k).map(|v| v.into_encoded_bytes()).unwrap_or_default());
            let go = gotext::lossy(&go);
            let args = vec![
                "doctor".to_owned(),
                "--json".to_owned(),
                "--cwd".to_owned(),
                tc.root.clone(),
                "--session-dir".to_owned(),
                tc.session_directory.clone(),
            ];
            let mut out = Vec::new();
            let mut over = false;
            let mut sink = |stream: &'static str, b: &[u8]| {
                if stream == "stdout" {
                    let room = MAX_DOCTOR_OUTPUT - out.len();
                    over |= b.len() > room;
                    out.extend_from_slice(&b[..b.len().min(room)]);
                }
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            let run = crate::tools::exec::run_group(
                &go,
                &args,
                &crate::tools::exec::parent_env(),
                None,
                deadline,
                &tc.ctx,
                crate::tools::exec::GroupKill::Group,
                &mut sink,
            );
            let code = run.as_ref().ok().and_then(|e| e.status).and_then(|s| s.code());
            match code {
                Some(c @ (0 | 1)) if !over => {
                    let mut text = gotext::lossy(&out);
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    let mut md = BTreeMap::new();
                    md.insert("doctor_failed".to_owned(), Any::Bool(c == 1));
                    Output { content: text, metadata: md, error: None }
                }
                _ => {
                    let why = match &run {
                        Err(e) => e.clone(),
                        Ok(_) if over => format!("its report is larger than {MAX_DOCTOR_OUTPUT} bytes"),
                        Ok(e) => format!("it ended with {:?}", e.status),
                    };
                    Output {
                        error: Some(crate::tools::registry::ToolError::Failed(format!(
                            "the check-up could not run in the Go build ({go}): {why}"
                        ))),
                        ..Output::default()
                    }
                }
            }
        }),
    )
}

// ---- a turn and its report -------------------------------------------------------------------------------------------

/// Go's `chatIO.onEvent`: streamed text, withdrawn drafts, tool starts and tier lines as they happen.
fn event_hook(io: &Io, streamed: &Arc<AtomicBool>) -> crate::agent::run::EventHook {
    let io = Arc::new(Mutex::new(io.clone()));
    let streamed = streamed.clone();
    Arc::new(move |e: &Event| {
        let io = io.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let s = |k: &str| e.get(k).map(crate::session::sprint).unwrap_or_else(|| "<nil>".into());
        if let Some(Any::String(t)) = e.get("type") {
            match t.as_str() {
                "content_delta" => {
                    streamed.store(true, Ordering::SeqCst);
                    io.out(&s("delta"));
                }
                "claim_check" => {
                    if streamed.load(Ordering::SeqCst) {
                        io.out(&format!("\n[{DRAFT_WITHDRAWN}]\n"));
                    }
                }
                "nudge_withdrawn" => {
                    if streamed.load(Ordering::SeqCst) {
                        io.out(&format!("\n[{DRAFT_WITHDRAWN}]\n{}", s("answer")));
                    }
                }
                "tool_start" => io.err(&format!("[tool] {}\n", s("tool_name"))),
                "tier_escalation" | "tier_refused" | "tier_redaction" | "tier_wake" | "tier_notice" => io.errln(&s("line")),
                _ => {}
            }
        }
    })
}

/// Go's `report`: the answer, then why the run stopped, as the exit status.
fn report(io: &Io, streamed: &AtomicBool, res: Option<&RunResult>, err: Option<&str>) -> i32 {
    if let Some(e) = err {
        io.errln(&format!("sarina: {e}"));
    }
    let Some(res) = res else { return EXIT_FAILURE };
    if streamed.load(Ordering::SeqCst) {
        io.outln("");
    } else if !res.final_output.is_empty() {
        io.outln(&res.final_output);
    }
    streamed.store(false, Ordering::SeqCst);
    match res.stop_reason.as_str() {
        STOP_INTERRUPTED => {
            io.errln("[interrupted]");
            return EXIT_INTERRUPTED;
        }
        STOP_PLAN_PENDING => {
            io.errln(&format!(
                "[plan pending approval] decide with: sarina resume --approve-plan|--reject-plan|--revise-plan \"feedback\" {}",
                res.session_id
            ));
            return EXIT_PLAN_PENDING;
        }
        STOP_BACKEND_ERROR | STOP_PROMPT_TOO_LONG | STOP_BUDGET_EXCEEDED | STOP_MAX_TURNS => {
            io.errln(&format!("[stopped: {}]", res.stop_reason));
            return EXIT_FAILURE;
        }
        _ => {}
    }
    if err.is_some() {
        EXIT_FAILURE
    } else {
        EXIT_OK
    }
}

// ---- slash commands ----------------------------------------------------------------------------------------------------

/// Go's `SlashResult`.
#[derive(Default)]
struct SlashResult {
    text: String,
    prompt: String,
    narrow: Option<Vec<String>>,
}

const SLASH: [(&str, &str); 4] = [
    ("help", "list the slash commands"),
    ("plan", "plan mode: /plan (enter), /plan show, /plan approve, /plan revise <feedback>, /plan reject"),
    ("skill", "run a skill: /skill <name> [args]; /skill alone lists them"),
    ("tier", "compute tiers: /tier status, /tier up (next turn starts one tier higher), /tier down"),
];

fn plain_name(n: &str) -> bool {
    !n.is_empty() && n.chars().all(|r| r.is_ascii_alphanumeric() || matches!(r, ':' | '-' | '_'))
}

fn plan_status(pending: &str) -> String {
    if pending.is_empty() {
        "Plan mode is on. No plan has been submitted yet.".into()
    } else {
        format!("Plan mode is on. Submitted plan awaiting your decision:\n{pending}")
    }
}

impl Chat {
    /// Go's `dispatchSlash`: None for an ordinary prompt.
    fn dispatch_slash(&mut self, a: &mut Agent, current: &mut Option<Session>, input: &str) -> Option<SlashResult> {
        let input = gotext::trim_space(input);
        let body = input.strip_prefix('/')?;
        let (name, args) = match body.split_once(' ') {
            Some((n, a)) => (n, gotext::trim_space(a)),
            None => (body, ""),
        };
        if name.is_empty() {
            return Some(SlashResult { text: "Slash commands look like /command [args]. Try /help.".into(), ..Default::default() });
        }
        let r = match gotext::to_lower(name).as_str() {
            "help" => {
                let mut b = String::from("# Slash Commands\n");
                for (n, d) in SLASH {
                    b.push_str(&format!("- /{n}: {d}\n"));
                }
                b.push_str("These are handled locally; the model is not called for them.");
                SlashResult { text: b, ..Default::default() }
            }
            "plan" => self.plan_slash(a, current, args),
            "skill" => self.skill_slash(args),
            "tier" => self.tier_slash(args),
            _ if plain_name(name) => SlashResult { text: format!("Unknown skill: {name}"), ..Default::default() },
            _ => return None,
        };
        Some(r)
    }

    fn plan_slash(&mut self, a: &mut Agent, current: &mut Option<Session>, args: &str) -> SlashResult {
        let (sub, rest) = match args.split_once(' ') {
            Some((s, r)) => (s, gotext::trim_space(r)),
            None => (args, ""),
        };
        let text = |t: String| SlashResult { text: t, ..Default::default() };
        let st = a.plan_state().clone();
        match gotext::to_lower(sub).as_str() {
            "" | "on" | "enter" => {
                if st.active() {
                    return text(plan_status(&st.pending));
                }
                a.enter_plan(current.as_mut());
                text("Plan mode is on: the model can only read and search until you approve a plan.".into())
            }
            "show" | "status" => {
                if !st.active() {
                    return text("Plan mode is off.".into());
                }
                text(plan_status(&st.pending))
            }
            s @ ("approve" | "revise" | "reject") => {
                let d = PlanDecision { action: s.to_owned(), feedback: rest.to_owned() };
                if let Err(e) = a.decide_plan(current.as_mut(), &d) {
                    return text(format!("Cannot {}: {e}", d.action));
                }
                text(match s {
                    PLAN_APPROVE => "Plan approved. Plan mode is off; send a message to let the model proceed.".into(),
                    PLAN_REVISE => "Feedback recorded. Plan mode stays on; send a message to let the model revise.".into(),
                    _ => "Plan rejected. Plan mode is off.".into(),
                })
            }
            _ => text(format!(
                "Unknown /plan option {}. Use /plan, /plan show, /plan approve, /plan revise <feedback> or /plan reject.",
                gotext::quote(sub)
            )),
        }
    }

    fn skill_slash(&mut self, args: &str) -> SlashResult {
        let cat = self.catalog();
        let (name, rest) = match args.split_once(' ') {
            Some((n, r)) => (n, r),
            None => (args, ""),
        };
        if name.is_empty() {
            let mut b = String::new();
            for s in cat.skills.iter().filter(|s| s.user_invocable) {
                b.push_str(&format!("- {}: {}\n", s.name, s.description));
            }
            if b.is_empty() {
                return SlashResult { text: "No skills are available. Put skill files in ~/.sarina/skills/.".into(), ..Default::default() };
            }
            return SlashResult { text: format!("Skills (use /skill <name> [args]):\n{}", b.trim_end_matches('\n')), ..Default::default() };
        }
        let s = match find_user_skill(&cat, name) {
            Ok(s) => s.clone(),
            Err(e) => {
                let mut c = e.chars();
                let first = c.next().map(|f| f.to_uppercase().collect::<String>()).unwrap_or_default();
                return SlashResult { text: format!("{first}{}", c.as_str()), ..Default::default() };
            }
        };
        let lib = self.library();
        SlashResult {
            prompt: s.expand_with(gotext::trim_space(rest), Some(&lib)),
            narrow: s.has_allowed.then(|| s.allowed_tools.clone()),
            ..Default::default()
        }
    }

    fn tier_slash(&mut self, args: &str) -> SlashResult {
        let text = |t: String| SlashResult { text: t, ..Default::default() };
        let Some(r) = self.router.clone() else {
            return text("Compute tiers are not configured: only the local backend is used. Define tiers in ~/.sarina/tiers.json.".into());
        };
        let t = gotext::trim_space(args);
        let sub = t.split_once(' ').map_or(t, |(s, _)| s);
        match gotext::to_lower(sub).as_str() {
            "" | "status" => {
                text(super::tiercmd::render_tier_status(&r.status(&Ctx::background()), "", &super::tiercmd::resource_line(&r)))
            }
            "up" => match r.request_up() {
                Ok(name) => text(format!("The next turn will start on tier {name}. Later turns go back to the lowest healthy tier.")),
                Err(e) => text(format!("Cannot go up: {e}.")),
            },
            "down" => text(format!("Back on tier {}; any pending /tier up is cancelled.", r.request_down())),
            _ => text(format!("Unknown /tier option {}. Use /tier status, /tier up or /tier down.", gotext::quote(sub))),
        }
    }
}

/// Go's `findUserSkill`.
fn find_user_skill<'a>(cat: &'a Catalog, name: &str) -> Result<&'a crate::skills::parse::Skill, String> {
    let Some(s) = cat.get(name) else {
        return Err(format!("unknown skill {} (run 'sarina skill list' to see what exists)", gotext::quote(name)));
    };
    if !s.user_invocable {
        return Err(format!("skill {} is not user-invocable", gotext::quote(name)));
    }
    Ok(s)
}

// ---- the conversation ------------------------------------------------------------------------------------------------

impl Chat {
    /// Go's `conversation`: one turn when a prompt is given (unless --interactive), otherwise a read-eval loop on stdin.
    fn conversation(&mut self, a: &mut Agent, stored: Option<Session>, first: &str) -> i32 {
        let code = self.converse(a, stored, first);
        crate::tools::exec::stop_all_background();
        if let Some(m) = self.mcp.take() {
            m.close();
        }
        code
    }

    fn converse(&mut self, a: &mut Agent, stored: Option<Session>, first: &str) -> i32 {
        let ints = self.ints.clone();
        let streamed = Arc::new(AtomicBool::new(false));
        a.set_confirm(Some(stdin_confirm(&self.io)));
        if let Some(r) = &self.router {
            let c = stdin_confirm(&self.io);
            r.set_confirm(Some(Box::new(move |p: &str| c(p))));
        }
        a.set_event_hook(Some(event_hook(&self.io, &streamed)));
        let mut current = stored;
        let io = self.io.clone();

        let turn = |me: &mut Chat, a: &mut Agent, current: &mut Option<Session>, mut prompt: String| -> i32 {
            if !me.no_slash {
                if let Some(r) = me.dispatch_slash(a, current, &prompt) {
                    if !r.text.is_empty() {
                        io.outln(&r.text);
                    }
                    if r.prompt.is_empty() {
                        return EXIT_OK;
                    }
                    prompt = r.prompt;
                    a.narrow_tools(r.narrow);
                }
            }
            let (res, err) = ints.turn(|ctx| match current.clone() {
                None => a.run(ctx, &prompt),
                Some(s) => a.resume(ctx, &prompt, s),
            });
            if let Some(r) = &res {
                if !r.session_path.is_empty() {
                    if let Ok(s) = crate::session::readfile::load(&r.session_path) {
                        *current = Some(s);
                    }
                }
            }
            report(&io, &streamed, res.as_ref(), err.as_deref())
        };

        if !first.is_empty() && !self.o.fs.is_true("interactive") {
            return turn(self, a, &mut current, first.to_owned());
        }

        let lines = Lines::start(&ints);
        {
            let (io2, l2, i2) = (Arc::new(Mutex::new(self.io.clone())), lines.clone(), ints.clone());
            a.set_plan_decider(Some(Arc::new(move |plan: &str| {
                let io = io2.lock().unwrap_or_else(|p| p.into_inner()).clone();
                ask_plan_decision(&io, &l2, &i2, plan)
            })));
        }
        a.set_confirm(Some(lines_confirm(&self.io, &lines, &ints)));
        if let Some(r) = &self.router {
            let c = lines_confirm(&self.io, &lines, &ints);
            r.set_confirm(Some(Box::new(move |p: &str| c(p))));
        }
        let mut last = EXIT_OK;
        let mut pending = first.to_owned();
        loop {
            if pending.is_empty() {
                io.out("> ");
                match lines.next() {
                    Input::Interrupt => {
                        io.outln("");
                        return self.end_review(&ints, current.as_ref(), EXIT_INTERRUPTED);
                    }
                    Input::Eof => return self.end_review(&ints, current.as_ref(), last),
                    Input::Line(l) => pending = gotext::trim_space(&l).to_owned(),
                }
            }
            match pending.as_str() {
                "" => continue,
                "/exit" | "/quit" => return self.end_review(&ints, current.as_ref(), last),
                _ => {}
            }
            let p = std::mem::take(&mut pending);
            let code = turn(self, a, &mut current, p);
            last = if code == EXIT_INTERRUPTED { EXIT_OK } else { code };
        }
    }

    /// Go's `finish`: the memory review when an interactive conversation ends (unless --no-auto-memory-review).
    fn end_review(&mut self, ints: &Interrupts, current: Option<&Session>, code: i32) -> i32 {
        if self.auto_review {
            if let (Some(s), Some(client)) = (current, self.client.clone()) {
                if !s.id.is_empty() {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
                    let out = ints.turn(|turn_ctx| {
                        let ctx = Ctx::with_deadline(deadline);
                        let watch = turn_ctx.clone();
                        let c2 = ctx.clone();
                        let stop = Arc::new(AtomicBool::new(false));
                        let s2 = stop.clone();
                        let t = std::thread::spawn(move || {
                            while !s2.load(Ordering::SeqCst) {
                                if watch.err().is_some() {
                                    c2.cancel();
                                    return;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(50));
                            }
                        });
                        let r = crate::memory::review::review_outcome(&ctx, &s.runtime.cwd, &s.id, &s.messages, &*client);
                        stop.store(true, Ordering::SeqCst);
                        let _ = t.join();
                        r
                    });
                    match out {
                        Ok(line) => self.io.outln(&line),
                        Err(e) => self.io.outln(&format!("memory_review_skipped=error ({e})")),
                    }
                }
            }
        }
        code
    }
}

// ---- the commands ------------------------------------------------------------------------------------------------------

/// `sarina chat [flags] [prompt...]`.
pub fn chat_command(env: &mut Env, args: &[String]) -> i32 {
    let mut c = Chat::new("chat", env);
    let mut errb = Vec::new();
    let parsed = crate::flagx::parse(&mut c.o.fs, args, &mut errb);
    c.io.err(&gotext::lossy(&errb));
    let positional = match parsed {
        Ok(p) => p,
        Err(e) => return c.finish(env, if e == HELP { EXIT_OK } else { EXIT_USAGE }),
    };
    c.auto_review = !c.o.fs.is_true("no-auto-memory-review");
    let mut a = match c.agent(None) {
        Ok(a) => a,
        Err(e) => {
            c.io.errln(&format!("sarina: {e}"));
            return c.finish(env, EXIT_USAGE);
        }
    };
    let prompt = gotext::trim_space(&positional.join(" ")).to_owned();
    let code = c.conversation(&mut a, None, &prompt);
    c.finish(env, code)
}

/// Go's `ownedByOther`: the file's owner when it is not this account.
fn owned_by_other(path: &str) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let st = std::fs::metadata(crate::gopath::os_path(path.as_bytes())).ok()?;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    (st.uid() != me).then_some(st.uid())
}

/// `sarina resume [flags] <session_id> [prompt...]`.
pub fn resume_command(env: &mut Env, args: &[String]) -> i32 {
    let mut c = Chat::new("resume", env);
    c.o.fs.bool("approve-plan", false, "approve the plan this session is waiting on, then continue");
    c.o.fs.bool("reject-plan", false, "reject the plan this session is waiting on (plan mode ends), then continue");
    c.o.fs.string("revise-plan", "", "ask for changes to the pending plan (feedback text); plan mode stays on");
    let mut errb = Vec::new();
    let parsed = crate::flagx::parse(&mut c.o.fs, args, &mut errb);
    c.io.err(&gotext::lossy(&errb));
    let rest = match parsed {
        Ok(p) => p,
        Err(e) => return c.finish(env, if e == HELP { EXIT_OK } else { EXIT_USAGE }),
    };
    let Some(id) = rest.first().cloned() else {
        c.io.errln("usage: sarina resume [flags] <session_id> [prompt...]");
        return c.finish(env, EXIT_USAGE);
    };
    let mut dir = c.o.fs.str("session-dir");
    if dir.is_empty() {
        dir = crate::config::default_runtime(&c.o.fs.str("cwd")).session_directory;
    }
    if let Some(owner) = owned_by_other(&crate::session::path(&dir, &id)) {
        if !c.o.fs.is_true("accept-foreign-session") {
            c.io.errln(&format!(
                "sarina: session {id} belongs to another account (uid {owner}), which could have written anything into it; add --accept-foreign-session to resume it as history only"
            ));
            return c.finish(env, EXIT_FAILURE);
        }
        c.o.foreign_loaded = true;
    }
    let mut stored = match crate::session::readfile::load_by_id(&dir, &id) {
        Ok(s) => s,
        Err(e) => {
            c.io.errln(&format!("sarina: cannot load session {id}: {e}"));
            return c.finish(env, EXIT_FAILURE);
        }
    };
    c.auto_review = !c.o.fs.is_true("no-auto-memory-review");
    let mut a = match c.agent(Some(&stored)) {
        Ok(a) => a,
        Err(e) => {
            c.io.errln(&format!("sarina: {e}"));
            return c.finish(env, EXIT_USAGE);
        }
    };
    let mut prompt = gotext::trim_space(&rest[1..].join(" ")).to_owned();
    // Go's planFlagDecision: at most one of the three
    let mut n = 0;
    let mut d = PlanDecision::default();
    if c.o.fs.is_true("approve-plan") {
        n += 1;
        d = PlanDecision { action: PLAN_APPROVE.into(), feedback: String::new() };
    }
    if c.o.fs.is_true("reject-plan") {
        n += 1;
        d = PlanDecision { action: PLAN_REJECT.into(), feedback: String::new() };
    }
    if c.o.fs.is_set("revise-plan") {
        n += 1;
        d = PlanDecision { action: PLAN_REVISE.into(), feedback: c.o.fs.str("revise-plan") };
    }
    if n > 1 {
        c.io.errln("sarina: give only one of --approve-plan, --reject-plan, --revise-plan");
        if let Some(m) = c.mcp.take() {
            m.close();
        }
        return c.finish(env, EXIT_USAGE);
    }
    if n == 1 {
        a.adopt_plan(&stored);
        if let Err(e) = a.decide_plan(Some(&mut stored), &d) {
            c.io.errln(&format!("sarina: cannot apply the plan decision: {e}"));
            return c.finish(env, EXIT_FAILURE);
        }
        c.io.errln(&format!("[plan decision: {}]", d.action));
        if prompt.is_empty() {
            prompt = "Carry on, following the operator's plan decision above.".into();
        }
    }
    let code = c.conversation(&mut a, Some(stored), &prompt);
    c.finish(env, code)
}

/// `sarina skill list|run`.
pub fn skill_command(env: &mut Env, args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            env.eprintln("usage: sarina skill list [--trust-workspace-skills]\n       sarina skill run [flags] <name> [arguments...]");
            EXIT_USAGE
        }
        Some("list") => skill_list(env, &args[1..]),
        Some("run") => skill_run(env, &args[1..]),
        Some(other) => {
            env.eprintln(&format!("sarina skill: unknown subcommand {} (want list or run)", gotext::quote(other)));
            EXIT_USAGE
        }
    }
}

fn skill_list(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("skill list");
    fs.string("cwd", ".", "workspace directory");
    fs.bool("trust-workspace-skills", false, "also load skills from <cwd>/.sarina/skills");
    if let Err(e) = fs.parse(args, &mut env.stderr) {
        return if e == HELP { EXIT_OK } else { EXIT_USAGE };
    }
    let cwd = fs.str("cwd");
    let ws = crate::gopath::abs(cwd.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or(cwd);
    let getenv: Arc<dyn Fn(&str) -> String + Send + Sync> = match &env.getenv {
        Some(g) => {
            let g = g.clone();
            Arc::new(move |k: &str| g(k))
        }
        None => Arc::new(|k: &str| Env::default().getenv(k)),
    };
    let cat = crate::skills::discover::discover(&crate::skills::discover::Options {
        home: env.home_dir(),
        getenv: Some(getenv),
        workspace: ws,
        trust_workspace: fs.is_true("trust-workspace-skills"),
        scanner: None,
        system_dir: String::new(),
        trust: Some(trust_fn()),
    });
    for n in &cat.notices {
        env.println(&format!("NOTICE: {n}"));
    }
    if cat.skills.is_empty() {
        env.println("No skills found. Put skill files in ~/.sarina/skills/ or a directory named by SARINA_SKILLS_DIR.");
    }
    let yn = |b: bool| if b { "yes" } else { "no" };
    for s in &cat.skills {
        env.println(&format!(
            "{}\n    {}\n    source: {} ({})   model-invocable: {}   user-invocable: {}",
            s.name,
            s.description,
            s.source,
            s.path,
            yn(s.model_invocable),
            yn(s.user_invocable)
        ));
        if s.has_allowed {
            env.println(&format!("    allowed tools: {}", s.allowed_tools.join(", ")));
        }
    }
    for c in &cat.collisions {
        env.println(&format!("COLLISION: {} is defined twice; using {}, ignoring {}", gotext::quote(&c.name), c.kept, c.shadowed));
    }
    for p in &cat.problems {
        env.println(&format!("NOT LOADED: {}: {}", p.path, p.message));
    }
    EXIT_OK
}

fn skill_run(env: &mut Env, args: &[String]) -> i32 {
    let mut c = Chat::new("skill run", env);
    let mut errb = Vec::new();
    let parsed = crate::flagx::parse(&mut c.o.fs, args, &mut errb);
    c.io.err(&gotext::lossy(&errb));
    let rest = match parsed {
        Ok(p) => p,
        Err(e) => return c.finish(env, if e == HELP { EXIT_OK } else { EXIT_USAGE }),
    };
    if rest.is_empty() {
        c.io.errln("usage: sarina skill run [flags] <name> [arguments...]");
        return c.finish(env, EXIT_USAGE);
    }
    let cat = c.catalog();
    let s = match find_user_skill(&cat, &rest[0]) {
        Ok(s) => s.clone(),
        Err(e) => {
            c.io.errln(&format!("sarina: {e}"));
            return c.finish(env, EXIT_FAILURE);
        }
    };
    let mut a = match c.agent(None) {
        Ok(a) => a,
        Err(e) => {
            c.io.errln(&format!("sarina: {e}"));
            return c.finish(env, EXIT_USAGE);
        }
    };
    if s.has_allowed {
        a.narrow_tools(Some(s.allowed_tools.clone()));
    }
    c.no_slash = true;
    let lib = c.library();
    let prompt = s.expand_with(&rest[1..].join(" "), Some(&lib));
    let code = c.conversation(&mut a, None, &prompt);
    c.finish(env, code)
}

/// `sarina memory-review [flags] <session_id>`: one review pass over a saved session with the model.
pub fn memory_review_command(env: &mut Env, args: &[String]) -> i32 {
    let getenv = env.getenv_owned();
    let mut o = ChatOpts::new("memory-review", &*getenv);
    let left = match o.fs.parse(args, &mut env.stderr) {
        Ok(r) => r,
        Err(e) => return if e == HELP { EXIT_OK } else { EXIT_USAGE },
    };
    if left.len() != 1 {
        env.eprintln("usage: sarina memory-review [flags] <session_id>");
        return EXIT_USAGE;
    }
    let id = &left[0];
    let mut dir = o.fs.str("session-dir");
    if dir.is_empty() {
        dir = crate::config::default_runtime(&o.fs.str("cwd")).session_directory;
    }
    let sess = match crate::session::readfile::load_by_id(&dir, id) {
        Ok(s) => s,
        Err(e) => {
            env.eprintln(&format!("sarina: cannot load session {id}: {e}"));
            return EXIT_FAILURE;
        }
    };
    let tier = crate::config::access_tier_from_env(&|k: &str| std::env::var(k).unwrap_or_default());
    let m = match o.build(&*getenv, None, tier) {
        Ok((m, _, _)) => m,
        Err(e) => {
            env.eprintln(&format!("sarina: {e}"));
            return EXIT_USAGE;
        }
    };
    let client = crate::llm::client::Client::new(m);
    let cwd = crate::config::default_runtime(&o.fs.str("cwd")).cwd;
    match crate::memory::review::review_outcome(&Ctx::background(), &cwd, &sess.id, &sess.messages, &client) {
        Ok(line) => {
            env.println(&line);
            EXIT_OK
        }
        Err(e) => {
            env.eprintln(&format!("sarina: memory review failed: {e}"));
            EXIT_FAILURE
        }
    }
}
