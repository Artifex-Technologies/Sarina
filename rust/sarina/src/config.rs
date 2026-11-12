//! Go: `go/config` (config.go, context.go): the model and runtime configuration, the environment that feeds it, and the
//! tolerant loaders a saved session file is read back with. `AccessTierFromEnv` (Heimdall's grant cap) is not here yet: it
//! comes with the permission gate.

use crate::goany::Any;
use crate::gojson::{self, Obj};
use crate::gopath;
use crate::gotext::{self, lossy};
use std::collections::BTreeMap;

pub const DEFAULT_MODEL: &str = "Qwen/Qwen3-Coder-30B-A3B-Instruct";
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000/v1";
pub const DEFAULT_API_KEY: &str = "local-token";
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 120.0;

/// USD per million tokens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_creation_per_million: f64,
    pub cache_read_per_million: f64,
}

/// The OpenAI-compatible backend. The API key is never written to any JSON form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelConfig {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub temperature: f64,
    pub timeout_seconds: f64,
    pub pricing: Pricing,
    /// models tried in order on the same server (SARINA_FALLBACK_MODELS); `omitempty`
    pub fallbacks: Vec<String>,
}

/// What the side-effecting tools may do. `allow_privileged` is never set and never written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Permissions {
    pub allow_file_write: bool,
    pub allow_shell_commands: bool,
    pub allow_destructive_shell_commands: bool,
    pub allow_privileged: bool,
}

/// A session's limits; None is "unset" (Go's nil pointer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BudgetConfig {
    pub max_total_tokens: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_reasoning_tokens: Option<i64>,
    pub max_total_cost_usd: Option<f64>,
    pub max_tool_calls: Option<i64>,
    pub max_delegated_tasks: Option<i64>,
    pub max_model_calls: Option<i64>,
    pub max_session_turns: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceThresholds {
    pub max_memory_percent: Option<f64>,
    pub max_cpu_percent: Option<f64>,
    pub max_gpu_memory_percent: Option<f64>,
    pub min_disk_free_mb: Option<f64>,
}

/// Kept for the file format; escalation itself is not in the Go core.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EscalationConfig {
    pub enabled: bool,
    pub cli_path: String,
    pub trigger_stop_reasons: Vec<String>,
    pub max_escalations: i64,
    pub timeout_seconds: f64,
    pub resource_thresholds: ResourceThresholds,
    pub extra_cli_args: Vec<String>,
}

/// Structured output asked of the backend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputSchema {
    pub name: String,
    pub schema: BTreeMap<String, Any>,
    pub strict: bool,
}

/// How long sessions are kept inside the context window. The zero value is "all defaults, everything on".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextConfig {
    pub disable_notes: bool,
    pub notes_every_turns: i64,
    pub notes_window_percent: i64,
    pub disable_clearing: bool,
    pub clear_window_percent: i64,
    pub clear_keep_recent: i64,
    pub clear_min_tokens: i64,
    pub disable_llm_compaction: bool,
    pub disable_discovery: bool,
}

/// The resolved form of a ContextConfig.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextSettings {
    pub notes: bool,
    pub notes_every_turns: i64,
    pub notes_window_fraction: f64,
    pub clear: bool,
    pub clear_window_fraction: f64,
    pub clear_keep_recent: i64,
    pub clear_min_tokens: i64,
    pub llm_compaction: bool,
    pub discovery: bool,
}

/// The agent loop's settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeConfig {
    pub cwd: String,
    pub max_turns: i64,
    pub command_timeout_seconds: f64,
    pub max_output_chars: i64,
    pub stream_model_responses: bool,
    pub auto_snip_threshold_tokens: Option<i64>,
    pub auto_compact_threshold_tokens: Option<i64>,
    pub compact_preserve_messages: i64,
    pub permissions: Permissions,
    pub additional_working_directories: Vec<String>,
    pub disable_claude_md_discovery: bool,
    pub budget_config: BudgetConfig,
    pub escalation_config: EscalationConfig,
    pub output_schema: Option<OutputSchema>,
    pub session_directory: String,
    pub scratchpad_root: String,
    pub context_management: Option<ContextConfig>,
}

// ---- the JSON forms (field order, omitempty and null as Go's struct tags) ----

impl Pricing {
    pub fn to_json(&self, html: bool) -> Result<String, gojson::Error> {
        Obj::new(html)
            .f64("input_cost_per_million_tokens_usd", self.input_per_million)
            .f64("output_cost_per_million_tokens_usd", self.output_per_million)
            .f64("cache_creation_input_cost_per_million_tokens_usd", self.cache_creation_per_million)
            .f64("cache_read_input_cost_per_million_tokens_usd", self.cache_read_per_million)
            .end()
    }
}

impl ModelConfig {
    pub fn to_json(&self, html: bool) -> Result<String, gojson::Error> {
        let o = Obj::new(html)
            .str("model", &self.model)
            .str("base_url", &self.base_url)
            .f64("temperature", self.temperature)
            .f64("timeout_seconds", self.timeout_seconds)
            .with("pricing", self.pricing.to_json(html));
        let o = if self.fallbacks.is_empty() { o } else { o.strs("fallbacks", &self.fallbacks) };
        o.end()
    }
}

impl ContextConfig {
    pub fn to_json(&self, html: bool) -> Result<String, gojson::Error> {
        Obj::new(html)
            .bool_omit("disable_notes", self.disable_notes)
            .int_omit("notes_every_turns", self.notes_every_turns)
            .int_omit("notes_window_percent", self.notes_window_percent)
            .bool_omit("disable_clearing", self.disable_clearing)
            .int_omit("clear_window_percent", self.clear_window_percent)
            .int_omit("clear_keep_recent", self.clear_keep_recent)
            .int_omit("clear_min_tokens", self.clear_min_tokens)
            .bool_omit("disable_llm_compaction", self.disable_llm_compaction)
            .bool_omit("disable_context_discovery", self.disable_discovery)
            .end()
    }

    /// Whether nothing differs from the defaults (`IsZero`).
    pub fn is_zero(&self) -> bool {
        *self == ContextConfig::default()
    }

    /// Go's `Settings`: defaults, the stored config and the environment. A feature is off if either the config or the
    /// environment turns it off; a number comes from the config, then the environment, then the default.
    pub fn settings(c: Option<&ContextConfig>, getenv: &dyn Fn(&str) -> String) -> ContextSettings {
        let v = c.cloned().unwrap_or_default();
        let pick = |from_cfg: i64, env: &str, def: i64| {
            if from_cfg > 0 {
                return from_cfg;
            }
            let n = env_positive(getenv, env);
            if n > 0 {
                n
            } else {
                def
            }
        };
        let pct = |n: i64, def: i64| {
            let n = if n <= 0 || n > 100 { def } else { n };
            n as f64 / 100.0
        };
        ContextSettings {
            notes: !v.disable_notes && !env_off(getenv, "SARINA_SESSION_NOTES"),
            notes_every_turns: pick(v.notes_every_turns, "SARINA_NOTES_EVERY", DEFAULT_NOTES_EVERY_TURNS),
            notes_window_fraction: pct(v.notes_window_percent, DEFAULT_NOTES_WINDOW_PERCENT),
            clear: !v.disable_clearing && !env_off(getenv, "SARINA_CLEAR_TOOL_OUTPUTS"),
            clear_window_fraction: pct(v.clear_window_percent, DEFAULT_CLEAR_WINDOW_PERCENT),
            clear_keep_recent: pick(v.clear_keep_recent, "", DEFAULT_CLEAR_KEEP_RECENT),
            clear_min_tokens: pick(v.clear_min_tokens, "", DEFAULT_CLEAR_MIN_TOKENS),
            llm_compaction: !v.disable_llm_compaction && !env_off(getenv, "SARINA_LLM_COMPACTION"),
            discovery: !v.disable_discovery && !env_off(getenv, "SARINA_CONTEXT_DISCOVERY"),
        }
    }
}

pub const DEFAULT_NOTES_EVERY_TURNS: i64 = 6;
pub const DEFAULT_NOTES_WINDOW_PERCENT: i64 = 50;
pub const DEFAULT_CLEAR_WINDOW_PERCENT: i64 = 60;
pub const DEFAULT_CLEAR_KEEP_RECENT: i64 = 4;
pub const DEFAULT_CLEAR_MIN_TOKENS: i64 = 500;
pub const DEFAULT_COMPACT_BREAKER_LIMIT: i64 = 3;

fn env_off(getenv: &dyn Fn(&str) -> String, name: &str) -> bool {
    matches!(gotext::to_lower(gotext::trim_space(&getenv(name))).as_str(), "0" | "false" | "off" | "no")
}

fn env_positive(getenv: &dyn Fn(&str) -> String, name: &str) -> i64 {
    match gotext::atoi(gotext::trim_space(&getenv(name)).as_bytes()) {
        Some(n) if n > 0 => n,
        _ => 0,
    }
}

impl RuntimeConfig {
    /// The session file's form of the runtime config (after `Normalized`: the lists are written as `[]`, never null).
    pub fn to_json(&self, html: bool) -> Result<String, gojson::Error> {
        let p = &self.permissions;
        let perms = Obj::new(html)
            .bool("allow_file_write", p.allow_file_write)
            .bool("allow_shell_commands", p.allow_shell_commands)
            .bool("allow_destructive_shell_commands", p.allow_destructive_shell_commands)
            .end();
        let b = &self.budget_config;
        let budget = Obj::new(html)
            .int_ptr("max_total_tokens", b.max_total_tokens)
            .int_ptr("max_input_tokens", b.max_input_tokens)
            .int_ptr("max_output_tokens", b.max_output_tokens)
            .int_ptr("max_reasoning_tokens", b.max_reasoning_tokens)
            .f64_ptr("max_total_cost_usd", b.max_total_cost_usd)
            .int_ptr("max_tool_calls", b.max_tool_calls)
            .int_ptr("max_delegated_tasks", b.max_delegated_tasks)
            .int_ptr("max_model_calls", b.max_model_calls)
            .int_ptr("max_session_turns", b.max_session_turns)
            .end();
        let e = &self.escalation_config;
        let rt = &e.resource_thresholds;
        let thresholds = Obj::new(html)
            .f64_ptr("max_memory_percent", rt.max_memory_percent)
            .f64_ptr("max_cpu_percent", rt.max_cpu_percent)
            .f64_ptr("max_gpu_memory_percent", rt.max_gpu_memory_percent)
            .f64_ptr("min_disk_free_mb", rt.min_disk_free_mb)
            .end();
        let escalation = Obj::new(html)
            .bool("enabled", e.enabled)
            .str("cli_path", &e.cli_path)
            .strs("trigger_stop_reasons", &e.trigger_stop_reasons)
            .int("max_escalations", e.max_escalations)
            .f64("timeout_seconds", e.timeout_seconds)
            .with("resource_thresholds", thresholds)
            .strs("extra_cli_args", &e.extra_cli_args)
            .end();
        let schema = match &self.output_schema {
            None => Ok("null".to_owned()),
            Some(s) => Obj::new(html)
                .str("name", &s.name)
                .with("schema", gojson::any_json(&Any::Object(s.schema.clone()), html))
                .bool("strict", s.strict)
                .end(),
        };
        let o = Obj::new(html)
            .str("cwd", &self.cwd)
            .int("max_turns", self.max_turns)
            .f64("command_timeout_seconds", self.command_timeout_seconds)
            .int("max_output_chars", self.max_output_chars)
            .bool("stream_model_responses", self.stream_model_responses)
            .int_ptr("auto_snip_threshold_tokens", self.auto_snip_threshold_tokens)
            .int_ptr("auto_compact_threshold_tokens", self.auto_compact_threshold_tokens)
            .int("compact_preserve_messages", self.compact_preserve_messages)
            .with("permissions", perms)
            .strs("additional_working_directories", &self.additional_working_directories)
            .bool("disable_claude_md_discovery", self.disable_claude_md_discovery)
            .with("budget_config", budget)
            .with("escalation_config", escalation)
            .with("output_schema", schema)
            .str("session_directory", &self.session_directory)
            .str("scratchpad_root", &self.scratchpad_root);
        match &self.context_management {
            Some(c) => o.with("context_management", c.to_json(html)).end(),
            None => o.end(),
        }
    }
}

// ---- the environment ----

/// Go's `ModelFromEnv`.
pub fn model_from_env(getenv: &dyn Fn(&str) -> String) -> ModelConfig {
    let mut m = ModelConfig {
        model: DEFAULT_MODEL.into(),
        base_url: DEFAULT_BASE_URL.into(),
        api_key: DEFAULT_API_KEY.into(),
        temperature: 0.0,
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        ..ModelConfig::default()
    };
    for (key, field) in [("OPENAI_MODEL", &mut m.model), ("OPENAI_BASE_URL", &mut m.base_url), ("OPENAI_API_KEY", &mut m.api_key)] {
        let v = getenv(key);
        if !v.is_empty() {
            *field = v;
        }
    }
    for f in getenv("SARINA_FALLBACK_MODELS").split(',') {
        let f = gotext::trim_space(f);
        if !f.is_empty() && f != m.model && m.fallbacks.len() < 4 {
            m.fallbacks.push(f.to_owned());
        }
    }
    // thinking models loop at temperature 0; their makers recommend about 0.6
    if let Ok(f) = gotext::parse_float(gotext::trim_space(&getenv("SARINA_TEMPERATURE"))) {
        if (0.0..=2.0).contains(&f) {
            m.temperature = f;
        }
    }
    m
}

fn positive_env(getenv: &dyn Fn(&str) -> String, name: &str) -> i64 {
    match gotext::atoi(gotext::trim_space(&getenv(name)).as_bytes()) {
        Some(n) if n > 0 => n,
        _ => 0,
    }
}

/// Go's `RequestMaxTokensFromEnv` (SARINA_REQUEST_MAX_TOKENS; 0 = not set).
pub fn request_max_tokens_from_env(getenv: &dyn Fn(&str) -> String) -> i64 {
    positive_env(getenv, "SARINA_REQUEST_MAX_TOKENS")
}

/// Go's `ContextWindowFromEnv` (SARINA_CONTEXT_WINDOW; 0 = not set).
pub fn context_window_from_env(getenv: &dyn Fn(&str) -> String) -> i64 {
    positive_env(getenv, "SARINA_CONTEXT_WINDOW")
}

fn default_escalation() -> EscalationConfig {
    EscalationConfig {
        cli_path: "claude".into(),
        trigger_stop_reasons: vec!["backend_error".into(), "resource_exhausted".into()],
        max_escalations: 1,
        timeout_seconds: 300.0,
        ..EscalationConfig::default()
    }
}

/// Go's `DefaultRuntime`: the session and scratchpad folders anchored at the workspace (made absolute against the process's
/// folder, as `filepath.Abs`).
pub fn default_runtime(cwd: &str) -> RuntimeConfig {
    let abs = gopath::abs(cwd.as_bytes()).unwrap_or_else(|_| cwd.as_bytes().to_vec());
    RuntimeConfig {
        cwd: lossy(&abs),
        max_turns: 12,
        command_timeout_seconds: 30.0,
        max_output_chars: 12000,
        compact_preserve_messages: 4,
        escalation_config: default_escalation(),
        session_directory: lossy(&gopath::join(&[&abs, b".port_sessions", b"agent"])),
        scratchpad_root: lossy(&gopath::join(&[&abs, b".port_sessions", b"scratchpad"])),
        ..RuntimeConfig::default()
    }
}

// ---- tolerant loaders ----

/// Go on amd64 converting a float64 to int: truncation, and the "integer indefinite" value (the smallest int) for NaN or
/// anything out of range.
pub fn go_int(f: f64) -> i64 {
    if f.is_nan() || f >= 9_223_372_036_854_775_808.0 || f < -9_223_372_036_854_775_808.0 {
        i64::MIN
    } else {
        f as i64
    }
}

/// Go's `AsInt`: numbers and numeric strings (never bools).
pub fn as_int(v: Option<&Any>) -> Option<i64> {
    match v? {
        Any::Number(f) => Some(go_int(*f)),
        Any::Int(i) => Some(*i),
        Any::String(s) => {
            let s = gotext::trim_space(s);
            if let Some(n) = gotext::atoi(s.as_bytes()) {
                return Some(n);
            }
            gotext::parse_float(s).ok().map(go_int)
        }
        _ => None,
    }
}

/// Go's `AsFloat`.
pub fn as_float(v: Option<&Any>) -> Option<f64> {
    match v? {
        Any::Number(f) => Some(*f),
        Any::Int(i) => Some(*i as f64),
        Any::String(s) => gotext::parse_float(gotext::trim_space(s)).ok(),
        _ => None,
    }
}

static EMPTY: BTreeMap<String, Any> = BTreeMap::new();

/// Go's `AsMap`: the object, or an empty one.
pub fn as_map(v: Option<&Any>) -> &BTreeMap<String, Any> {
    match v {
        Some(Any::Object(m)) => m,
        _ => &EMPTY,
    }
}

fn int_or(m: &BTreeMap<String, Any>, k: &str, def: i64) -> i64 {
    as_int(m.get(k)).unwrap_or(def)
}

fn float_or(m: &BTreeMap<String, Any>, k: &str, def: f64) -> f64 {
    as_float(m.get(k)).unwrap_or(def)
}

fn bool_or(m: &BTreeMap<String, Any>, k: &str, def: bool) -> bool {
    match m.get(k) {
        Some(Any::Bool(b)) => *b,
        _ => def,
    }
}

fn str_or(m: &BTreeMap<String, Any>, k: &str, def: &str) -> String {
    match m.get(k) {
        Some(Any::String(s)) => s.clone(),
        _ => def.to_owned(),
    }
}

fn str_list(v: Option<&Any>, def: Vec<String>) -> Vec<String> {
    match v {
        Some(Any::Array(items)) => items
            .iter()
            .filter_map(|x| match x {
                Any::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => def,
    }
}

/// Go's `ModelFromMap`: "model" is required; the API key is not read from a file even if present.
pub fn model_from_map(m: &BTreeMap<String, Any>) -> Option<ModelConfig> {
    let model = match m.get("model") {
        Some(Any::String(s)) if !s.is_empty() => s.clone(),
        _ => return None,
    };
    let p = as_map(m.get("pricing"));
    Some(ModelConfig {
        model,
        base_url: str_or(m, "base_url", DEFAULT_BASE_URL),
        temperature: float_or(m, "temperature", 0.0),
        timeout_seconds: float_or(m, "timeout_seconds", DEFAULT_TIMEOUT_SECONDS),
        pricing: Pricing {
            input_per_million: float_or(p, "input_cost_per_million_tokens_usd", 0.0),
            output_per_million: float_or(p, "output_cost_per_million_tokens_usd", 0.0),
            cache_creation_per_million: float_or(p, "cache_creation_input_cost_per_million_tokens_usd", 0.0),
            cache_read_per_million: float_or(p, "cache_read_input_cost_per_million_tokens_usd", 0.0),
        },
        ..ModelConfig::default()
    })
}

/// Go's `RuntimeFromMap`: "cwd" is required; everything else falls back to the defaults.
pub fn runtime_from_map(m: &BTreeMap<String, Any>) -> Option<RuntimeConfig> {
    let cwd = match m.get("cwd") {
        Some(Any::String(s)) if !s.is_empty() => s.clone(),
        _ => return None,
    };
    let mut r = default_runtime(&cwd);
    r.cwd.clone_from(&cwd);
    r.max_turns = int_or(m, "max_turns", r.max_turns);
    r.command_timeout_seconds = float_or(m, "command_timeout_seconds", r.command_timeout_seconds);
    r.max_output_chars = int_or(m, "max_output_chars", r.max_output_chars);
    r.stream_model_responses = bool_or(m, "stream_model_responses", false);
    r.auto_snip_threshold_tokens = as_int(m.get("auto_snip_threshold_tokens"));
    r.auto_compact_threshold_tokens = as_int(m.get("auto_compact_threshold_tokens"));
    r.compact_preserve_messages = int_or(m, "compact_preserve_messages", r.compact_preserve_messages);
    let pm = as_map(m.get("permissions"));
    r.permissions = Permissions {
        allow_file_write: bool_or(pm, "allow_file_write", false),
        allow_shell_commands: bool_or(pm, "allow_shell_commands", false),
        allow_destructive_shell_commands: bool_or(pm, "allow_destructive_shell_commands", false),
        allow_privileged: false,
    };
    r.additional_working_directories = str_list(m.get("additional_working_directories"), Vec::new());
    r.disable_claude_md_discovery = bool_or(m, "disable_claude_md_discovery", false);
    let bm = as_map(m.get("budget_config"));
    r.budget_config = BudgetConfig {
        max_total_tokens: as_int(bm.get("max_total_tokens")),
        max_input_tokens: as_int(bm.get("max_input_tokens")),
        max_output_tokens: as_int(bm.get("max_output_tokens")),
        max_reasoning_tokens: as_int(bm.get("max_reasoning_tokens")),
        max_total_cost_usd: as_float(bm.get("max_total_cost_usd")),
        max_tool_calls: as_int(bm.get("max_tool_calls")),
        max_delegated_tasks: as_int(bm.get("max_delegated_tasks")),
        max_model_calls: as_int(bm.get("max_model_calls")),
        max_session_turns: as_int(bm.get("max_session_turns")),
    };
    let em = as_map(m.get("escalation_config"));
    let rt = as_map(em.get("resource_thresholds"));
    let mut esc = default_escalation();
    esc.enabled = bool_or(em, "enabled", false);
    esc.cli_path = str_or(em, "cli_path", &esc.cli_path);
    esc.trigger_stop_reasons = str_list(em.get("trigger_stop_reasons"), esc.trigger_stop_reasons);
    esc.max_escalations = int_or(em, "max_escalations", esc.max_escalations);
    esc.timeout_seconds = float_or(em, "timeout_seconds", esc.timeout_seconds);
    esc.extra_cli_args = str_list(em.get("extra_cli_args"), esc.extra_cli_args);
    esc.resource_thresholds = ResourceThresholds {
        max_memory_percent: as_float(rt.get("max_memory_percent")),
        max_cpu_percent: as_float(rt.get("max_cpu_percent")),
        max_gpu_memory_percent: as_float(rt.get("max_gpu_memory_percent")),
        min_disk_free_mb: as_float(rt.get("min_disk_free_mb")),
    };
    r.escalation_config = esc;
    if let Some(Any::Object(sm)) = m.get("output_schema") {
        let name = match sm.get("name") {
            Some(Any::String(s)) => s.clone(),
            _ => String::new(),
        };
        if let (false, Some(Any::Object(schema))) = (name.is_empty(), sm.get("schema")) {
            r.output_schema = Some(OutputSchema { name, schema: schema.clone(), strict: bool_or(sm, "strict", false) });
        }
    }
    r.session_directory = str_or(m, "session_directory", &r.session_directory);
    if r.session_directory.is_empty() {
        r.session_directory = lossy(&gopath::join(&[cwd.as_bytes(), b".port_sessions", b"agent"]));
    }
    r.context_management = context_from_map(as_map(m.get("context_management")));
    r.scratchpad_root = str_or(m, "scratchpad_root", &r.scratchpad_root);
    if r.scratchpad_root.is_empty() {
        r.scratchpad_root = lossy(&gopath::join(&[cwd.as_bytes(), b".port_sessions", b"scratchpad"]));
    }
    Some(r)
}

fn context_from_map(m: &BTreeMap<String, Any>) -> Option<ContextConfig> {
    if m.is_empty() {
        return None;
    }
    let c = ContextConfig {
        disable_notes: bool_or(m, "disable_notes", false),
        notes_every_turns: int_or(m, "notes_every_turns", 0),
        notes_window_percent: int_or(m, "notes_window_percent", 0),
        disable_clearing: bool_or(m, "disable_clearing", false),
        clear_window_percent: int_or(m, "clear_window_percent", 0),
        clear_keep_recent: int_or(m, "clear_keep_recent", 0),
        clear_min_tokens: int_or(m, "clear_min_tokens", 0),
        disable_llm_compaction: bool_or(m, "disable_llm_compaction", false),
        disable_discovery: bool_or(m, "disable_context_discovery", false),
    };
    (!c.is_zero()).then_some(c)
}
