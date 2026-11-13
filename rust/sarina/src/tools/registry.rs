//! Go: `go/tools/registry.go`, `permission.go` and the plan-mode filter of `plan.go`: the registry the agent loop
//! dispatches to, the one permission gate in front of every handler, the allowlist, the operator's deny rule, and what a
//! call returns in each case (testdata/parity/registry.json). Streaming handlers and the provider list come with the tools
//! themselves.

use crate::config::Permissions;
use crate::goany::Any;
use crate::gotext;
use crate::llm::types::ToolSpec;
use crate::tools::schema::validate_args;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// What a tool needs from the operator before it may run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Permission {
    /// a read-only tool
    #[default]
    None,
    /// changes files or persistent state (--allow-write)
    Write,
    /// runs or controls processes (--allow-shell)
    Shell,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Write => "write",
            Permission::Shell => "shell",
            Permission::None => "none",
        }
    }
}

/// Whether a tool may run in plan mode. Unset is treated as mutating, so a forgotten tool is never offered there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Class {
    #[default]
    Unset,
    ReadOnly,
    Mutating,
    /// enter_plan_mode (offered outside plan mode) and exit_plan_mode (only inside it)
    PlanControl,
}

pub const TOOL_ENTER_PLAN_MODE: &str = "enter_plan_mode";
pub const TOOL_EXIT_PLAN_MODE: &str = "exit_plan_mode";

/// One call awaiting the operator's consent.
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalRequest {
    pub tool: String,
    pub arguments: BTreeMap<String, Any>,
    pub needs: Permission,
}

/// Answers an ApprovalRequest: allow, or refuse with an optional reason the model reads.
pub type Approver = Arc<dyn Fn(&ApprovalRequest) -> (bool, String) + Send + Sync>;

/// One yes/no question to the operator, asked on every call (Go's `Runtimes[RuntimeOperatorConfirm]`, a
/// `func(prompt string) bool`). Deliberately not the [`Approver`], which can carry an "allow for this session" grant: a
/// tool that sends data off the machine (council_ask) must be asked about every time, and does nothing online when this
/// is None (no one can be asked).
pub type Confirm = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What a handler receives besides its arguments (the parts ported so far).
#[derive(Clone, Default)]
pub struct Context {
    /// the workspace folder (absolute, links followed)
    pub root: String,
    /// what a tool's answer is cut to (0 or less: not cut)
    pub max_output_chars: i64,
    /// the model's tool-call batch (the agent raises it before each one): a file read and written in the same step was
    /// written before its content was seen
    pub step: i64,
    /// the file tools' memory for this context (Go keeps it in Context.Runtimes); clones share it
    pub state: Arc<crate::tools::files::FileState>,
    /// the call's context (Go passes it to every handler): a cancel stops a running command
    pub ctx: crate::goctx::Ctx,
    /// a foreground command's time when the call sets none (zero or less: the default, 30 s)
    pub command_timeout: std::time::Duration,
    /// the parent environment ("KEY=value" entries) children start from; None is this process's
    pub environ: Option<crate::tools::exec::Environ>,
    /// variables the operator set for this session, applied to children after the secret filter
    pub session_env: BTreeMap<String, String>,
    pub extra_env: BTreeMap<String, String>,
    /// where background processes live (Go's Runtimes[RuntimeProcs]); None is the process-wide manager
    pub procs: Option<Arc<crate::tools::exec::ProcManager>>,
    /// where saved sessions live ("" = <workspace>/.port_sessions/agent)
    pub session_directory: String,
    /// the memory runtime the memory tools use (Go's Runtimes["memory"]); None: the workspace's
    pub memory: Option<Arc<crate::memory::Runtime>>,
    /// the home folder for the user-scope memory file (Go's Runtimes["memory_home"], tests); "" = $HOME
    pub memory_home: String,
    /// whether plan mode is on, as the agent reports it (Go's RuntimePlanActive); None is unknown
    pub plan_active: Option<crate::tools::plan::PlanActive>,
    pub permissions: Permissions,
    /// asked before a tool that needs a permission or changes state runs (after the gate and the schema check)
    pub approver: Option<Approver>,
    /// the per-call operator question; None: no one can be asked
    pub confirm: Option<Confirm>,
}

/// Why a handler failed: refused by configuration (Go's PermissionDeniedError, found anywhere in the error chain) or any
/// other failure. The text is the whole error's.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolError {
    Denied(String),
    Failed(String),
}

/// A handler's answer: text, metadata, and the error if it failed (Go returns the metadata with the error too).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub content: String,
    pub metadata: BTreeMap<String, Any>,
    pub error: Option<ToolError>,
}

pub type Handler = Arc<dyn Fn(&BTreeMap<String, Any>, &Context) -> Output + Send + Sync>;

/// Receives live output from a streaming tool: the stream ("stdout" or "stderr") and a piece of text.
pub type Emit = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// A handler that can also report output while it runs (Go's StreamHandler).
pub type StreamHandler = Arc<dyn Fn(&BTreeMap<String, Any>, &Context, &Emit) -> Output + Send + Sync>;

/// A name, a model-visible description and schema, a handler, the permission the call needs and the plan-mode class.
#[derive(Clone)]
pub struct Tool {
    pub spec: ToolSpec,
    pub handler: Handler,
    pub requires: Permission,
    pub class: Class,
    /// used instead of the handler by execute_stream
    pub stream: Option<StreamHandler>,
}

/// The result of one call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolResult {
    pub name: String,
    pub ok: bool,
    pub content: String,
    pub metadata: BTreeMap<String, Any>,
}

/// Go's `checkPermission`: the single gate in front of every handler.
pub fn check_permission(t: &Tool, p: &Permissions) -> Result<(), String> {
    match t.requires {
        Permission::Write if !p.allow_file_write => Err(format!(
            "{} changes files or saved state, and file writing is disabled. The operator must re-run with --allow-write to enable it.",
            t.spec.name
        )),
        Permission::Shell if !p.allow_shell_commands => Err(format!(
            "{} runs or controls processes, and shell commands are disabled. The operator must re-run with --allow-shell to enable them.",
            t.spec.name
        )),
        _ => Ok(()),
    }
}

/// Go's `needsApproval`.
pub fn needs_approval(t: &Tool) -> bool {
    t.requires != Permission::None || matches!(t.class, Class::Mutating | Class::Unset)
}

pub type Deny = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// An ordered set of tools; later tools replace earlier ones of the same name.
#[derive(Clone, Default)]
pub struct Registry {
    order: Vec<String>,
    tools: HashMap<String, Tool>,
    /// the operator policy's deny rule (a reason, or "" for allowed): denied tools are not offered and refused when called
    deny: Option<Deny>,
    /// when set, tools whose permission is not granted are not offered
    perms: Option<Permissions>,
}

fn md(kind: &str) -> BTreeMap<String, Any> {
    BTreeMap::from([("error_kind".to_owned(), Any::String(kind.into()))])
}

impl Registry {
    pub fn new(ts: impl IntoIterator<Item = Tool>) -> Registry {
        let mut r = Registry::default();
        for t in ts {
            r.register(t);
        }
        r
    }

    /// Sets the operator policy's deny rule (what the Builder does last).
    pub fn set_deny(&mut self, deny: Option<Deny>) {
        self.deny = deny;
    }

    /// Adds or replaces a tool.
    pub fn register(&mut self, t: Tool) {
        if !self.tools.contains_key(&t.spec.name) {
            self.order.push(t.spec.name.clone());
        }
        self.tools.insert(t.spec.name.clone(), t);
    }

    fn hidden(&self, name: &str) -> bool {
        if self.deny.as_ref().is_some_and(|d| !d(name).is_empty()) {
            return true;
        }
        match (&self.perms, self.tools.get(name).map(|t| t.requires)) {
            (Some(p), Some(Permission::Write)) => !p.allow_file_write,
            (Some(p), Some(Permission::Shell)) => !p.allow_shell_commands,
            _ => false,
        }
    }

    /// The offered tool names in registration order.
    /// Every tool held (hidden ones too) and what it needs: what Go's `Get(name).Requires` would answer.
    pub fn requirements(&self) -> std::collections::BTreeMap<String, Permission> {
        self.tools.iter().map(|(n, t)| (n.clone(), t.requires)).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.order.iter().filter(|n| !self.hidden(n)).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.names().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }

    /// The model-visible definitions in registration order.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.order.iter().filter(|n| !self.hidden(n)).map(|n| self.tools[n].spec.clone()).collect()
    }

    fn list(&self) -> Vec<Tool> {
        self.order.iter().map(|n| self.tools[n].clone()).collect()
    }

    /// Go's `Filter`: None is no restriction; a list (even empty) keeps only the names in it. Unknown names grant nothing.
    pub fn filter(&self, allow: Option<&[String]>) -> Registry {
        match allow {
            None => Registry::new(self.list()),
            Some(allow) => Registry::new(self.order.iter().filter(|n| allow.contains(n)).map(|n| self.tools[n].clone())),
        }
    }

    /// Go's `ForMode`: in plan mode only the read-only tools and exit_plan_mode; outside it everything but exit_plan_mode.
    pub fn for_mode(&self, plan: bool) -> Registry {
        Registry::new(
            self.order
                .iter()
                .filter(|n| {
                    let t = &self.tools[n.as_str()];
                    if n.as_str() == TOOL_EXIT_PLAN_MODE {
                        plan
                    } else if plan {
                        t.class == Class::ReadOnly
                    } else {
                        true
                    }
                })
                .map(|n| self.tools[n].clone()),
        )
    }

    /// Go's `Execute`: never fails; failures are results the model reads. Order: unknown tool, the operator's policy, the
    /// permission gate, the schema, the approver, the handler (a panic in it is a result too).
    pub fn execute(&self, name: &str, args: Option<BTreeMap<String, Any>>, tc: &Context) -> ToolResult {
        self.run(name, args, tc, None)
    }

    /// Go's `ExecuteStream`: Execute for a caller that wants live output; a tool with a stream handler reports through
    /// emit, and its result says `streamed`.
    pub fn execute_stream(&self, name: &str, args: Option<BTreeMap<String, Any>>, tc: &Context, emit: &Emit) -> ToolResult {
        self.run(name, args, tc, Some(emit))
    }

    fn run(&self, name: &str, args: Option<BTreeMap<String, Any>>, tc: &Context, emit: Option<&Emit>) -> ToolResult {
        let Some(t) = self.get(name) else {
            return ToolResult { name: name.into(), content: format!("unknown tool {}", gotext::quote(name)), ..ToolResult::default() };
        };
        if let Some(deny) = &self.deny {
            let why = deny(name);
            if !why.is_empty() {
                return ToolResult { name: name.into(), content: why, metadata: md("permission_denied"), ..ToolResult::default() };
            }
        }
        if let Err(why) = check_permission(t, &tc.permissions) {
            return ToolResult { name: name.into(), content: why, metadata: md("permission_denied"), ..ToolResult::default() };
        }
        let args = match validate_args(t.spec.parameters.as_ref(), args.unwrap_or_default()) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult {
                    name: name.into(),
                    content: format!("invalid arguments for {name}: {e}"),
                    metadata: md("tool_execution_error"),
                    ..ToolResult::default()
                }
            }
        };
        if let (Some(approver), true) = (&tc.approver, needs_approval(t)) {
            let (allow, mut why) = approver(&ApprovalRequest { tool: name.into(), arguments: args.clone(), needs: t.requires });
            if !allow {
                if why.is_empty() {
                    why = "the operator did not approve this call".into();
                }
                return ToolResult {
                    name: name.into(),
                    content: format!("denied by the operator: {why}"),
                    metadata: md("permission_denied"),
                    ..ToolResult::default()
                };
            }
        }
        let handler = t.handler.clone();
        let streamed = emit.is_some() && t.stream.is_some();
        let call = || match (emit, &t.stream) {
            (Some(e), Some(st)) => st(&args, tc, e),
            _ => handler(&args, tc),
        };
        let out = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)) {
            Ok(out) => out,
            Err(p) => {
                let what =
                    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                return ToolResult {
                    name: name.into(),
                    content: format!("tool {name} failed internally: {what}"),
                    metadata: md("tool_execution_error"),
                    ..ToolResult::default()
                };
            }
        };
        let mut metadata = out.metadata;
        if streamed {
            metadata.insert("streamed".into(), Any::Bool(true));
        }
        match out.error {
            Some(e) => {
                let (kind, text) = match e {
                    ToolError::Denied(t) => ("permission_denied", t),
                    ToolError::Failed(t) => ("tool_execution_error", t),
                };
                metadata.insert("error_kind".into(), Any::String(kind.into()));
                ToolResult { name: name.into(), ok: false, content: text, metadata }
            }
            None => ToolResult { name: name.into(), ok: true, content: out.content, metadata },
        }
    }
}

/// Assembles a registry and applies the allowlist on every build, so a rebuild cannot undo it (Go's `Builder`).
#[derive(Clone, Default)]
pub struct Builder {
    /// None = all tools; Some (even empty) = exactly those names
    pub allowlist: Option<Vec<String>>,
    /// the built-in tools and every provider's (Go's stableBuiltins), before `extra`
    pub builtins: Vec<Tool>,
    pub extra: Vec<Tool>,
    /// the per-question tools, after everything else (the cached prompt prefix ends at the first difference)
    pub per_prompt: Vec<Tool>,
    /// keeps only these names (empty = none); can only remove what the allowlist left
    pub narrow: Option<Vec<String>>,
    /// read-only tools plus exit_plan_mode
    pub plan_mode: bool,
    pub deny: Option<Deny>,
    /// tools whose permission is not granted are not offered (a call that arrives anyway is still refused by the gate)
    pub perms: Option<Permissions>,
}

impl Builder {
    pub fn build(&self) -> Registry {
        let mut reg = Registry::new(self.builtins.iter().chain(&self.extra).chain(&self.per_prompt).cloned());
        reg = reg.filter(self.allowlist.as_deref());
        if let Some(n) = &self.narrow {
            reg = reg.filter(Some(n));
        }
        reg = reg.for_mode(self.plan_mode);
        reg.perms = self.perms.clone();
        reg.deny = self.deny.clone();
        reg
    }
}
