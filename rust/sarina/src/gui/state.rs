//! Go: `gui/state.go`: the operator's settings, their snapshot for the page (never the API key) and the partial update
//! with its checks, all or nothing.

use std::collections::BTreeMap;

use crate::config::{default_runtime, go_int, model_from_env, ModelConfig, OutputSchema, RuntimeConfig, DEFAULT_TIMEOUT_SECONDS};
use crate::goany::Any;
use crate::gojson::Value;
use crate::gopath;
use crate::gotext;

/// A request failure with its HTTP status (Go's `apiErr`); a 422 carries the `detail` list.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiErr {
    pub code: u16,
    pub msg: String,
}

pub fn bad(msg: impl Into<String>) -> ApiErr {
    ApiErr { code: 400, msg: msg.into() }
}

/// Go's `wrongType`.
pub fn wrong_type(key: &str, want: &str) -> ApiErr {
    ApiErr { code: 422, msg: format!("{key}: expected {want}") }
}

pub const APPROVAL_ASK: &str = "ask";
pub const APPROVAL_AUTO: &str = "auto";

/// Go's `State`.
#[derive(Clone, Default)]
pub struct State {
    pub model: String,
    pub base_url: String,
    pub api_key: String,
    pub cwd: String,
    pub session_dir: String,
    pub session_dir_explicit: bool,
    pub allow_shell: bool,
    pub allow_write: bool,
    pub approval_mode: String,
    pub temperature: f64,
    pub timeout_seconds: f64,
    pub stream: bool,
    pub max_turns: i64,
    pub max_total_tokens: Option<i64>,
    pub max_input_tokens: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub max_reasoning_tokens: Option<i64>,
    pub max_tool_calls: Option<i64>,
    pub max_delegated_tasks: Option<i64>,
    pub max_model_calls: Option<i64>,
    pub max_session_turns: Option<i64>,
    pub max_total_cost_usd: Option<f64>,
    pub system_prompt: String,
    pub append_system_prompt: String,
    pub override_system_prompt: String,
    pub response_schema: Option<BTreeMap<String, Any>>,
    pub response_schema_name: String,
    pub response_schema_strict: bool,
    pub auto_snip: Option<i64>,
    pub auto_compact: Option<i64>,
    pub compact_preserve: i64,
    pub disable_claude_md: bool,
    pub extra_dirs: Vec<String>,
    pub active_session_id: String,
}

/// Go's `DefaultState`.
pub fn default_state(getenv: &dyn Fn(&str) -> String, cwd: &str) -> State {
    let m = model_from_env(getenv);
    let rt = default_runtime(cwd);
    State {
        model: m.model,
        base_url: m.base_url,
        api_key: m.api_key,
        cwd: rt.cwd,
        session_dir: rt.session_directory,
        approval_mode: APPROVAL_ASK.into(),
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        stream: true,
        max_turns: 12,
        compact_preserve: 4,
        response_schema_name: "response".into(),
        ..State::default()
    }
}

fn opt_int(v: Option<i64>) -> Any {
    v.map_or(Any::Null, Any::Int)
}

fn opt_text(s: &str) -> Any {
    if s.is_empty() {
        Any::Null
    } else {
        Any::String(s.to_owned())
    }
}

impl State {
    /// Go's `PermissionMode`.
    pub fn permission_mode(&self) -> &'static str {
        match (self.allow_shell, self.allow_write) {
            (true, true) => "write+shell",
            (true, false) => "shell",
            (false, true) => "write",
            _ => "read-only",
        }
    }

    /// Go's `Snapshot`: the GET /api/state body (never the API key).
    pub fn snapshot(&self) -> BTreeMap<String, Any> {
        let s = |v: &str| Any::String(v.to_owned());
        let mut m = BTreeMap::new();
        let mut put = |k: &str, v: Any| {
            m.insert(k.to_owned(), v);
        };
        put("model", s(&self.model));
        put("base_url", s(&self.base_url));
        put("api_key_set", Any::Bool(!self.api_key.is_empty()));
        put("cwd", s(&self.cwd));
        put("session_directory", s(&self.session_dir));
        put("allow_shell", Any::Bool(self.allow_shell));
        put("allow_write", Any::Bool(self.allow_write));
        put("approval_mode", s(&self.approval_mode));
        put("permission_mode", s(self.permission_mode()));
        put("temperature", Any::Number(self.temperature));
        put("timeout_seconds", Any::Number(self.timeout_seconds));
        put("stream_model_responses", Any::Bool(self.stream));
        put("max_turns", Any::Int(self.max_turns));
        put("max_total_tokens", opt_int(self.max_total_tokens));
        put("max_input_tokens", opt_int(self.max_input_tokens));
        put("max_output_tokens", opt_int(self.max_output_tokens));
        put("max_reasoning_tokens", opt_int(self.max_reasoning_tokens));
        put("max_tool_calls", opt_int(self.max_tool_calls));
        put("max_delegated_tasks", opt_int(self.max_delegated_tasks));
        put("max_model_calls", opt_int(self.max_model_calls));
        put("max_session_turns", opt_int(self.max_session_turns));
        put("max_total_cost_usd", self.max_total_cost_usd.map_or(Any::Null, Any::Number));
        put("system_prompt", opt_text(&self.system_prompt));
        put("append_system_prompt", opt_text(&self.append_system_prompt));
        put("override_system_prompt", opt_text(&self.override_system_prompt));
        put("response_schema", self.response_schema.clone().map_or(Any::Null, Any::Object));
        put("response_schema_name", s(&self.response_schema_name));
        put("response_schema_strict", Any::Bool(self.response_schema_strict));
        put("auto_snip_threshold_tokens", opt_int(self.auto_snip));
        put("auto_compact_threshold_tokens", opt_int(self.auto_compact));
        put("compact_preserve_messages", Any::Int(self.compact_preserve));
        put("disable_claude_md_discovery", Any::Bool(self.disable_claude_md));
        put("additional_working_directories", Any::Array(self.extra_dirs.iter().map(|d| s(d)).collect()));
        put("active_session_id", opt_text(&self.active_session_id));
        m
    }

    /// Go's `ModelRuntime`.
    pub fn model_runtime(&self) -> (ModelConfig, RuntimeConfig) {
        let m = ModelConfig {
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            temperature: self.temperature,
            timeout_seconds: self.timeout_seconds,
            ..ModelConfig::default()
        };
        let mut rt = default_runtime(&self.cwd);
        rt.session_directory = self.session_dir.clone();
        rt.max_turns = self.max_turns;
        rt.stream_model_responses = self.stream;
        rt.auto_snip_threshold_tokens = self.auto_snip;
        rt.auto_compact_threshold_tokens = self.auto_compact;
        rt.compact_preserve_messages = self.compact_preserve;
        rt.permissions.allow_shell_commands = self.allow_shell;
        rt.permissions.allow_file_write = self.allow_write;
        rt.additional_working_directories = self.extra_dirs.clone();
        rt.disable_claude_md_discovery = self.disable_claude_md;
        let b = &mut rt.budget_config;
        b.max_total_tokens = self.max_total_tokens;
        b.max_input_tokens = self.max_input_tokens;
        b.max_output_tokens = self.max_output_tokens;
        b.max_reasoning_tokens = self.max_reasoning_tokens;
        b.max_tool_calls = self.max_tool_calls;
        b.max_delegated_tasks = self.max_delegated_tasks;
        b.max_model_calls = self.max_model_calls;
        b.max_session_turns = self.max_session_turns;
        b.max_total_cost_usd = self.max_total_cost_usd;
        if let Some(schema) = &self.response_schema {
            rt.output_schema =
                Some(OutputSchema { name: self.response_schema_name.clone(), schema: schema.clone(), strict: self.response_schema_strict });
        }
        (m, rt)
    }

    /// Go's `PromptOptions`.
    pub fn prompt_options(&self) -> crate::agent::prompt::PromptOptions {
        crate::agent::prompt::PromptOptions {
            system: self.system_prompt.clone(),
            append: self.append_system_prompt.clone(),
            override_: self.override_system_prompt.clone(),
        }
    }

    /// Go's `apply`: a partial update (omitted keys unchanged, an explicit null clears), all or nothing. The keys are
    /// taken in sorted order (Go's map order is random; with two bad keys Go may name either).
    pub fn apply(&mut self, patch: &BTreeMap<String, Value>, confirmed: bool) -> Result<(), ApiErr> {
        let mut n = self.clone();
        for (key, raw) in patch {
            apply_one(&mut n, key, raw)?;
        }
        let widened = (n.allow_shell && !self.allow_shell)
            || (n.allow_write && !self.allow_write)
            || (n.approval_mode == APPROVAL_AUTO && self.approval_mode != APPROVAL_AUTO && (n.allow_shell || n.allow_write));
        if widened && !confirmed {
            return Err(bad(
                "turning on shell or write access (or switching to auto-approve) needs an explicit confirmation: send confirm_permissions=true",
            ));
        }
        *self = n;
        Ok(())
    }
}

/// Go's `json.Unmarshal(raw, &s)` for a string: null leaves "".
fn as_string(key: &str, raw: &Value) -> Result<String, ApiErr> {
    match raw {
        Value::String(s) => Ok(s.clone()),
        Value::Null => Ok(String::new()),
        _ => Err(wrong_type(key, "a string")),
    }
}

fn as_bool(key: &str, raw: &Value) -> Result<bool, ApiErr> {
    match raw {
        Value::Bool(b) => Ok(*b),
        Value::Null => Ok(false),
        _ => Err(wrong_type(key, "a boolean")),
    }
}

/// Go's `json.Unmarshal(raw, &f)` for a float64: null leaves 0; a number past float64's range fails.
fn as_float(key: &str, raw: &Value) -> Result<f64, ApiErr> {
    match raw {
        Value::Number(t) => gotext::parse_float(t).map_err(|_| wrong_type(key, "a number")),
        Value::Null => Ok(0.0),
        _ => Err(wrong_type(key, "a number")),
    }
}

/// Go's `optPositiveInt`: null, or a positive whole number.
fn opt_positive_int(key: &str, raw: &Value) -> Result<Option<i64>, ApiErr> {
    if matches!(raw, Value::Null) {
        return Ok(None);
    }
    let f = as_float(key, raw)?;
    if f <= 0.0 || f != f.trunc() || f > 1e12 {
        return Err(bad(format!("{key} must be a positive whole number or null")));
    }
    Ok(Some(go_int(f)))
}

/// Go's `os.UserHomeDir` (HOME; "" with an error when unset).
fn user_home() -> String {
    std::env::var_os("HOME").map(|v| gotext::lossy(std::os::unix::ffi::OsStrExt::as_bytes(v.as_os_str()))).unwrap_or_default()
}

/// Go's `absDir`: an existing folder, absolute, `~` expanded, links followed.
pub fn abs_dir(p: &str) -> Result<String, ApiErr> {
    let mut p = gotext::trim_space(p).to_owned();
    if p.is_empty() {
        return Err(bad("directory must not be empty"));
    }
    if p == "~" || p.starts_with("~/") {
        p = gotext::lossy(&gopath::join(&[user_home().as_bytes(), &p.as_bytes()[1..]]));
    }
    let a = match gopath::abs(p.as_bytes()) {
        Ok(a) => gotext::lossy(&a),
        Err(_) => return Err(bad(format!("bad directory {}", gotext::quote(&p)))),
    };
    if !std::fs::metadata(&a).map(|m| m.is_dir()).unwrap_or(false) {
        return Err(bad(format!("{a} does not exist or is not a directory")));
    }
    Ok(match gopath::eval_symlinks(a.as_bytes()) {
        Ok(real) => gotext::lossy(&real),
        Err(_) => a,
    })
}

fn apply_one(n: &mut State, key: &str, raw: &Value) -> Result<(), ApiErr> {
    let is_null = matches!(raw, Value::Null);
    match key {
        "model" | "base_url" => {
            let s = as_string(key, raw)?;
            let t = gotext::trim_space(&s);
            if t.is_empty() {
                return Err(bad(format!("{key} must not be empty")));
            }
            if key == "model" {
                n.model = t.to_owned();
            } else {
                n.base_url = t.to_owned();
            }
        }
        "api_key" => {
            if !is_null {
                let s = as_string(key, raw)?;
                if !s.is_empty() {
                    n.api_key = s;
                }
            }
        }
        "cwd" => {
            let s = as_string(key, raw)?;
            let a = abs_dir(&s)?;
            n.cwd = a.clone();
            if !n.session_dir_explicit {
                n.session_dir = default_runtime(&a).session_directory;
            }
        }
        "session_directory" => {
            let s = if is_null { String::new() } else { as_string(key, raw)? };
            let t = gotext::trim_space(&s);
            if t.is_empty() {
                n.session_dir_explicit = false;
                n.session_dir = default_runtime(&n.cwd).session_directory;
            } else {
                match gopath::abs(t.as_bytes()) {
                    Ok(a) => {
                        n.session_dir_explicit = true;
                        n.session_dir = gotext::lossy(&a);
                    }
                    Err(_) => return Err(bad("bad session directory")),
                }
            }
        }
        "allow_shell" => n.allow_shell = as_bool(key, raw)?,
        "allow_write" => n.allow_write = as_bool(key, raw)?,
        "approval_mode" => {
            let s = as_string(key, raw)?;
            if s != APPROVAL_ASK && s != APPROVAL_AUTO {
                return Err(bad(format!("approval_mode must be {} or {}", gotext::quote(APPROVAL_ASK), gotext::quote(APPROVAL_AUTO))));
            }
            n.approval_mode = s;
        }
        "temperature" => {
            let f = as_float(key, raw)?;
            if f < 0.0 {
                return Err(bad("temperature must be >= 0"));
            }
            n.temperature = f;
        }
        "timeout_seconds" => {
            let f = as_float(key, raw)?;
            if f <= 0.0 {
                return Err(bad("timeout_seconds must be > 0"));
            }
            n.timeout_seconds = f;
        }
        "max_turns" => {
            let f = as_float(key, raw)?;
            if f < 1.0 || f != f.trunc() || f > 1e6 {
                return Err(bad("max_turns must be a whole number >= 1"));
            }
            n.max_turns = go_int(f);
        }
        "stream_model_responses" => n.stream = as_bool(key, raw)?,
        "max_total_tokens" => n.max_total_tokens = opt_positive_int(key, raw)?,
        "max_input_tokens" => n.max_input_tokens = opt_positive_int(key, raw)?,
        "max_output_tokens" => n.max_output_tokens = opt_positive_int(key, raw)?,
        "max_reasoning_tokens" => n.max_reasoning_tokens = opt_positive_int(key, raw)?,
        "max_tool_calls" => n.max_tool_calls = opt_positive_int(key, raw)?,
        "max_delegated_tasks" => n.max_delegated_tasks = opt_positive_int(key, raw)?,
        "max_model_calls" => n.max_model_calls = opt_positive_int(key, raw)?,
        "max_session_turns" => n.max_session_turns = opt_positive_int(key, raw)?,
        "auto_snip_threshold_tokens" => n.auto_snip = opt_positive_int(key, raw)?,
        "auto_compact_threshold_tokens" => n.auto_compact = opt_positive_int(key, raw)?,
        "max_total_cost_usd" => {
            if is_null {
                n.max_total_cost_usd = None;
            } else {
                let f = as_float(key, raw)?;
                if f <= 0.0 {
                    return Err(bad("max_total_cost_usd must be positive or null"));
                }
                n.max_total_cost_usd = Some(f);
            }
        }
        "system_prompt" | "append_system_prompt" | "override_system_prompt" => {
            let s = if is_null { String::new() } else { as_string(key, raw)? };
            match key {
                "system_prompt" => n.system_prompt = s,
                "append_system_prompt" => n.append_system_prompt = s,
                _ => n.override_system_prompt = s,
            }
        }
        "response_schema" => {
            if is_null {
                n.response_schema = None;
            } else {
                let (v, err) = crate::gojson::to_any(raw);
                match (v.into_object(), err) {
                    (Ok(m), None) => n.response_schema = Some(m),
                    _ => return Err(bad("response_schema must be a JSON object or null")),
                }
            }
        }
        "response_schema_name" => {
            let s = as_string(key, raw)?;
            let t = gotext::trim_space(&s);
            if t.is_empty() {
                return Err(bad("response_schema_name must not be empty"));
            }
            n.response_schema_name = t.to_owned();
        }
        "response_schema_strict" => n.response_schema_strict = as_bool(key, raw)?,
        "compact_preserve_messages" => {
            let f = as_float(key, raw)?;
            if f < 0.0 || f != f.trunc() {
                return Err(bad("compact_preserve_messages must be a whole number >= 0"));
            }
            n.compact_preserve = go_int(f);
        }
        "disable_claude_md_discovery" => n.disable_claude_md = as_bool(key, raw)?,
        "additional_working_directories" => {
            let mut list = Vec::new();
            if !is_null {
                match raw {
                    Value::Array(items) => {
                        for it in items {
                            match it {
                                Value::String(s) => list.push(s.clone()),
                                Value::Null => list.push(String::new()),
                                _ => return Err(wrong_type(key, "a list of strings")),
                            }
                        }
                    }
                    _ => return Err(wrong_type(key, "a list of strings")),
                }
            }
            n.extra_dirs = Vec::new();
            for d in list {
                if gotext::trim_space(&d).is_empty() {
                    continue;
                }
                n.extra_dirs.push(abs_dir(&d)?);
            }
        }
        _ => {}
    }
    Ok(())
}
