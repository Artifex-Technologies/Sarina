//! Go: `cli/chat.go`'s `opts`: the flags chat, resume and skill run share (`newOpts`), and `build`, which turns them,
//! the environment and a stored session into the model and runtime configuration and the tool allowlist.

use crate::config::{default_runtime, model_from_env, ModelConfig, OutputSchema, RuntimeConfig, DEFAULT_TIMEOUT_SECONDS};
use crate::goany::Any;
use crate::goflag::FlagSet;
use crate::gojson;
use crate::gotext;
use crate::session::Session;
use crate::tiers::config::EscalationOptions;

/// The shared flags (Go's `opts` as `newOpts` makes it).
pub struct ChatOpts {
    pub fs: FlagSet,
    /// --accept-foreign-session was needed: the session being resumed belongs to another account
    pub foreign_loaded: bool,
}

impl ChatOpts {
    /// Go's `newOpts`: the flags in Go's order, with the environment's model settings as defaults.
    pub fn new(name: &'static str, getenv: &dyn Fn(&str) -> String) -> ChatOpts {
        let d = model_from_env(getenv);
        let mut f = FlagSet::new(name);
        f.string("model", &d.model, "model name (env OPENAI_MODEL)");
        f.string("base-url", &d.base_url, "OpenAI-compatible base URL (env OPENAI_BASE_URL)");
        // no default shown: the usage printed for -h or a mistyped flag would show the key (build reads OPENAI_API_KEY)
        f.string("api-key", "", "API key (env OPENAI_API_KEY); never written to session files");
        f.float("temperature", d.temperature, "sampling temperature (default: SARINA_TEMPERATURE, else 0)");
        f.float("timeout-seconds", DEFAULT_TIMEOUT_SECONDS, "per-request timeout");
        f.float("input-cost-per-million", 0.0, "USD per million input tokens");
        f.float("output-cost-per-million", 0.0, "USD per million output tokens");
        f.string("cwd", ".", "workspace directory");
        f.int("max-turns", 12, "model turns allowed per call");
        f.bool("stream", false, "stream the model's reply as it is produced");
        f.int("auto-snip-threshold", 0, "snip old bulky messages above this many tokens");
        f.int("auto-compact-threshold", 0, "compact history above this many tokens");
        f.int("compact-preserve-messages", 4, "recent messages kept verbatim when compacting");
        f.bool("allow-write", false, "permit file-writing tools");
        f.bool("allow-shell", false, "permit shell tools");
        f.bool("unsafe", false, "permit destructive shell commands");
        f.list("add-dir", "extra working directory (repeatable)");
        f.bool("disable-claude-md", false, "skip CLAUDE.md discovery");
        f.string("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)");
        f.string("scratchpad-root", "", "scratchpad root (default <cwd>/.port_sessions/scratchpad)");
        f.string("system-prompt", "", "replace the default system prompt");
        f.string("append-system-prompt", "", "text added after the system prompt");
        f.string("override-system-prompt", "", "use only this as the system prompt");
        f.int("context-window", 0, "context window in tokens (env SARINA_CONTEXT_WINDOW)");
        f.string("tools", "", "comma-separated tool allowlist");
        f.bool("no-tools", false, "offer the model no tools at all");
        f.int("max-total-tokens", 0, "stop above this many total tokens");
        f.int("max-input-tokens", 0, "input token limit");
        f.int("max-output-tokens", 0, "output token limit (also sent as max_tokens)");
        f.int("max-tool-calls", 0, "stop above this many tool calls");
        f.int("max-model-calls", 0, "stop above this many model calls");
        f.int("max-session-turns", 0, "stop above this many turns in the whole session");
        f.float("max-cost-usd", 0.0, "stop above this estimated cost");
        f.string("response-schema-file", "", "JSON schema file for structured output");
        f.string("response-schema-name", "response", "name of the response schema");
        f.bool("response-schema-strict", false, "ask for strict schema adherence");
        f.bool("interactive", false, "hold a conversation even if a prompt is given");
        f.bool(
            "trust-workspace-skills",
            false,
            "also load skills from <cwd>/.sarina/skills (off by default: a repository must not inject prompts)",
        );
        f.bool("trust-workspace-mcp", false, "also load MCP server definitions from files in <cwd> (off by default: they start programs)");
        f.bool("no-session-notes", false, "do not keep running session notes (env SARINA_SESSION_NOTES=0)");
        f.int("notes-every", 0, "refresh session notes every N tool-using turns (default 6; env SARINA_NOTES_EVERY)");
        f.bool(
            "accept-foreign-session",
            false,
            "resume: allow a session file that belongs to another account (its history is then whatever that account wrote)",
        );
        f.bool("no-auto-memory-review", false, "do not review the session for memory proposals when an interactive conversation ends");
        f.bool(
            "no-clear-tool-outputs",
            false,
            "do not clear old bulky tool outputs as the context fills (env SARINA_CLEAR_TOOL_OUTPUTS=0)",
        );
        f.bool(
            "no-llm-compaction",
            false,
            "compact with the heuristic digest only, never a summarising model call (env SARINA_LLM_COMPACTION=0)",
        );
        f.bool("no-context-discovery", false, "do not ask the backend for its context window (env SARINA_CONTEXT_DISCOVERY=0)");
        super::tiercmd::register_escalation(&mut f);
        ChatOpts { fs: f, foreign_loaded: false }
    }

    /// The escalation flags' options (Go's `o.esc.Options()`).
    pub fn escalation(&self) -> Option<EscalationOptions> {
        super::tiercmd::escalation_options(&self.fs)
    }

    /// Go's `build`. `tier` is the access ladder's applied tier (`config.AccessTierFromEnv`, read by the caller). With a
    /// stored session its values are the base and only flags given override them; permissions are only ever widened.
    pub fn build(
        &self,
        getenv: &dyn Fn(&str) -> String,
        stored: Option<&Session>,
        tier: i64,
    ) -> Result<(ModelConfig, RuntimeConfig, Option<Vec<String>>), String> {
        let fs = &self.fs;
        let set = |n: &str| fs.is_set(n);
        let (mut m, mut rt) = match stored {
            Some(s) => {
                let (mut m, mut rt) = (s.model.clone(), s.runtime.clone());
                m.api_key = model_from_env(getenv).api_key;
                if self.foreign_loaded {
                    // another account's file is a transcript, not a configuration: the server, the permissions, the extra
                    // folders, escalation and where files are written come from this run
                    let fresh = model_from_env(getenv);
                    m.base_url = fresh.base_url;
                    m.fallbacks = fresh.fallbacks;
                    let d = default_runtime(&rt.cwd);
                    rt.permissions = d.permissions;
                    rt.escalation_config = d.escalation_config;
                    rt.additional_working_directories = Vec::new();
                    rt.session_directory = d.session_directory;
                    rt.scratchpad_root = d.scratchpad_root;
                }
                (m, rt)
            }
            None => (model_from_env(getenv), default_runtime(&fs.str("cwd"))),
        };
        let pick = |n: &str| stored.is_none() || set(n);
        if pick("model") {
            m.model = fs.str("model");
        }
        if pick("base-url") {
            m.base_url = fs.str("base-url");
        }
        if set("api-key") {
            m.api_key = fs.str("api-key");
        }
        if pick("temperature") {
            m.temperature = fs.f64("temperature");
        }
        if pick("timeout-seconds") {
            m.timeout_seconds = fs.f64("timeout-seconds");
        }
        if set("input-cost-per-million") {
            m.pricing.input_per_million = fs.f64("input-cost-per-million");
        }
        if set("output-cost-per-million") {
            m.pricing.output_per_million = fs.f64("output-cost-per-million");
        }
        if stored.is_none() || set("cwd") {
            let d = default_runtime(&fs.str("cwd"));
            rt.cwd = d.cwd;
            if stored.is_none() || !set("session-dir") {
                rt.session_directory = d.session_directory;
            }
            if stored.is_none() || !set("scratchpad-root") {
                rt.scratchpad_root = d.scratchpad_root;
            }
        }
        if set("session-dir") {
            rt.session_directory = fs.str("session-dir");
        }
        if set("scratchpad-root") {
            rt.scratchpad_root = fs.str("scratchpad-root");
        }
        if pick("max-turns") {
            rt.max_turns = fs.num("max-turns");
        }
        if pick("stream") {
            rt.stream_model_responses = fs.is_true("stream");
        }
        if set("auto-snip-threshold") {
            rt.auto_snip_threshold_tokens = Some(fs.num("auto-snip-threshold"));
        }
        if set("auto-compact-threshold") {
            rt.auto_compact_threshold_tokens = Some(fs.num("auto-compact-threshold"));
        }
        if pick("compact-preserve-messages") {
            rt.compact_preserve_messages = fs.num("compact-preserve-messages").max(0);
        }
        let p = &mut rt.permissions;
        p.allow_file_write = p.allow_file_write || fs.is_true("allow-write") || tier >= 1;
        p.allow_shell_commands = p.allow_shell_commands || fs.is_true("allow-shell") || tier >= 3;
        p.allow_destructive_shell_commands = p.allow_destructive_shell_commands || fs.is_true("unsafe");
        let dirs = fs.values("add-dir");
        if !dirs.is_empty() {
            rt.additional_working_directories = dirs;
        }
        rt.disable_claude_md_discovery = rt.disable_claude_md_discovery || fs.is_true("disable-claude-md");
        let b = &mut rt.budget_config;
        for (flag, slot) in [
            ("max-total-tokens", &mut b.max_total_tokens),
            ("max-input-tokens", &mut b.max_input_tokens),
            ("max-output-tokens", &mut b.max_output_tokens),
            ("max-tool-calls", &mut b.max_tool_calls),
            ("max-model-calls", &mut b.max_model_calls),
            ("max-session-turns", &mut b.max_session_turns),
        ] {
            if set(flag) {
                *slot = Some(fs.num(flag));
            }
        }
        if set("max-cost-usd") {
            b.max_total_cost_usd = Some(fs.f64("max-cost-usd"));
        }
        let schema_file = fs.str("response-schema-file");
        if !schema_file.is_empty() {
            let raw = read_file(&schema_file).map_err(|e| format!("cannot read schema file: {e}"))?;
            let schema = schema_object(&raw).map_err(|e| format!("schema file is not a JSON object: {e}"))?;
            rt.output_schema =
                Some(OutputSchema { name: fs.str("response-schema-name"), schema, strict: fs.is_true("response-schema-strict") });
        }
        let mut cm = rt.context_management.clone().unwrap_or_default();
        cm.disable_notes = cm.disable_notes || fs.is_true("no-session-notes");
        cm.disable_clearing = cm.disable_clearing || fs.is_true("no-clear-tool-outputs");
        cm.disable_llm_compaction = cm.disable_llm_compaction || fs.is_true("no-llm-compaction");
        cm.disable_discovery = cm.disable_discovery || fs.is_true("no-context-discovery");
        if set("notes-every") && fs.num("notes-every") > 0 {
            cm.notes_every_turns = fs.num("notes-every");
        }
        rt.context_management = if cm.is_zero() { None } else { Some(cm) };
        let allow = if fs.is_true("no-tools") {
            Some(Vec::new())
        } else if set("tools") {
            Some(fs.str("tools").split(',').map(|n| gotext::trim_space(n).to_owned()).filter(|n| !n.is_empty()).collect())
        } else {
            None
        };
        Ok((m, rt, allow))
    }
}

/// `os.ReadFile` with Go's error texts (the operator's own file).
fn read_file(p: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut f = std::fs::File::open(crate::gopath::os_path(p.as_bytes())).map_err(|e| crate::goerr::path("open", p.as_bytes(), &e))?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| crate::goerr::path("read", p.as_bytes(), &e))?;
    Ok(b)
}

/// `json.Unmarshal(raw, &map[string]any{})`: an object (or null: no schema), else Go's error.
fn schema_object(raw: &[u8]) -> Result<std::collections::BTreeMap<String, Any>, String> {
    let (doc, err) = gojson::unmarshal_any(raw);
    if let Some(e) = err {
        return Err(e.0);
    }
    let kind = match &doc {
        Any::Object(_) | Any::Null => "",
        Any::Bool(_) => "bool",
        Any::String(_) => "string",
        Any::Array(_) => "array",
        _ => "number",
    };
    if !kind.is_empty() {
        return Err(format!("json: cannot unmarshal {kind} into Go value of type map[string]interface {{}}"));
    }
    Ok(doc.into_object().unwrap_or_default())
}
