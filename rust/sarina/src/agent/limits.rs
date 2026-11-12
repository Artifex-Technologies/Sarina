//! Go: `go/agent/limits.go` and `window.go`'s words: token estimates, the context window guessed from a model's name, the
//! pre-flight arithmetic of one request, the price of the usage and the budgets.

use crate::config::{BudgetConfig, Pricing, RuntimeConfig};
use crate::llm::types::{ToolSpec, Usage, WireToolCall};
use crate::session::Message;

/// Go's `CountTokens`: ceil(chars/4), at least 1 for text that is not empty.
pub fn count_tokens(s: &str) -> i64 {
    let n = s.chars().count() as i64;
    if n == 0 {
        0
    } else {
        (n + 3) / 4
    }
}

/// Go's `InferContextWindow`.
pub fn infer_context_window(model: &str) -> i64 {
    let m = crate::gotext::to_lower(model);
    if m.contains("qwen3-coder") || m.contains("devstral") {
        256000
    } else if m.contains("qwen") {
        131072
    } else if m.contains("claude") {
        200000
    } else {
        128000
    }
}

/// Go's `Snapshot`: the pre-flight arithmetic for one request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub window: i64,
    pub projected: i64,
    pub message_tokens: i64,
    pub framing: i64,
    pub reserved_output: i64,
    pub buffer: i64,
    pub reserved_schema: i64,
    pub hard: i64,
    pub soft: i64,
    pub overflow: i64,
    pub soft_overflow: i64,
    pub exceeds_hard: bool,
    pub exceeds_soft: bool,
}

impl Snapshot {
    /// json.Marshal of Go's struct (its field names).
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"Window":{},"Projected":{},"MessageTokens":{},"Framing":{},"ReservedOutput":{},"Buffer":{},"ReservedSchema":{},"Hard":{},"Soft":{},"Overflow":{},"SoftOverflow":{},"ExceedsHard":{},"ExceedsSoft":{}}}"#,
            self.window,
            self.projected,
            self.message_tokens,
            self.framing,
            self.reserved_output,
            self.buffer,
            self.reserved_schema,
            self.hard,
            self.soft,
            self.overflow,
            self.soft_overflow,
            self.exceeds_hard,
            self.exceeds_soft
        )
    }
}

fn wire_call_tokens(c: &WireToolCall) -> i64 {
    count_tokens(&c.function.name) + count_tokens(&c.function.arguments) + count_tokens(&c.id)
}

/// Go's `MessageTokens`: the content of every message plus its tool calls.
pub fn message_tokens(msgs: &[Message]) -> i64 {
    msgs.iter().map(|m| count_tokens(&m.content) + m.tool_calls.iter().map(wire_call_tokens).sum::<i64>()).sum()
}

/// Go's `toolSpecTokens`.
pub fn tool_spec_tokens(specs: &[ToolSpec]) -> i64 {
    specs.iter().filter_map(|s| s.to_json().ok()).map(|b| count_tokens(&b)).sum()
}

const DEFAULT_OUTPUT_RESERVE: i64 = 4096;
const MIN_OUTPUT_RESERVE: i64 = 1024;
const SCHEMA_OVERHEAD: i64 = 256;
const COMPACT_BUFFER_MAX: i64 = 13000;

/// Go's `TakeSnapshot`.
pub fn take_snapshot(window: i64, msgs: &[Message], specs: &[ToolSpec], rt: &RuntimeConfig) -> Snapshot {
    let mut s = Snapshot { window, message_tokens: message_tokens(msgs) + tool_spec_tokens(specs), ..Snapshot::default() };
    for m in msgs {
        s.framing += 5;
        if !m.name.is_empty() {
            s.framing += 2;
        }
        s.framing += 12 * m.tool_calls.len() as i64;
    }
    s.framing += 3;
    s.reserved_output = match rt.budget_config.max_output_tokens {
        Some(n) => n.max(0),
        None => DEFAULT_OUTPUT_RESERVE.min((window / 16).max(MIN_OUTPUT_RESERVE)),
    };
    if rt.output_schema.is_some() {
        s.reserved_schema = SCHEMA_OVERHEAD;
    }
    s.hard = (window - s.reserved_output - s.reserved_schema).max(0);
    if let Some(n) = rt.budget_config.max_input_tokens {
        s.hard = s.hard.min(n.max(0));
    }
    s.buffer = COMPACT_BUFFER_MAX.min((window / 10).max(1024));
    s.soft = (s.hard - s.buffer).max(0);
    s.projected = s.message_tokens + s.framing;
    s.overflow = (s.projected - s.hard).max(0);
    s.soft_overflow = (s.projected - s.soft).max(0);
    s.exceeds_hard = s.overflow > 0;
    s.exceeds_soft = s.soft_overflow > 0;
    s
}

/// Go's `counters`: what the budgets are checked against.
#[derive(Clone, Debug, Default)]
pub struct Counters {
    pub usage: Usage,
    pub cost: f64,
    pub tool_calls: i64,
    pub delegated: i64,
    pub model_calls: i64,
    pub session_turns: i64,
}

/// Go's `Cost`.
pub fn cost(p: &Pricing, u: &Usage) -> f64 {
    u.input_tokens as f64 / 1e6 * p.input_per_million
        + u.output_tokens as f64 / 1e6 * p.output_per_million
        + u.cache_creation_input_tokens as f64 / 1e6 * p.cache_creation_per_million
        + u.cache_read_input_tokens as f64 / 1e6 * p.cache_read_per_million
}

/// Go's `checkBudget`: the first budget breached (strictly over), in Go's order, or "".
pub fn check_budget(b: &BudgetConfig, c: &Counters) -> String {
    let over = |limit: Option<i64>, have: i64, what: &str| match limit {
        Some(l) if have > l => format!("{what} budget exceeded: {have} used, limit {l}"),
        _ => String::new(),
    };
    for s in [
        over(b.max_total_tokens, c.usage.total(), "token (total)"),
        over(b.max_input_tokens, c.usage.input_tokens, "token (input)"),
        over(b.max_output_tokens, c.usage.output_tokens, "token (output)"),
        over(b.max_reasoning_tokens, c.usage.reasoning_tokens, "token (reasoning)"),
    ] {
        if !s.is_empty() {
            return s;
        }
    }
    if let Some(l) = b.max_total_cost_usd {
        if c.cost > l {
            return format!("cost budget exceeded: ${:.6} spent, limit ${:.6}", c.cost, l);
        }
    }
    for s in [
        over(b.max_tool_calls, c.tool_calls, "tool-call"),
        over(b.max_delegated_tasks, c.delegated, "delegated-task"),
        over(b.max_model_calls, c.model_calls, "model-call"),
        over(b.max_session_turns, c.session_turns, "session-turn"),
    ] {
        if !s.is_empty() {
            return s;
        }
    }
    String::new()
}

/// Go's `describeWindowSource`: where the window came from, in words for the operator.
pub fn describe_window_source(src: &str) -> String {
    match src {
        "flag" => "set with --context-window".into(),
        "env" => "set with SARINA_CONTEXT_WINDOW".into(),
        _ => match src.strip_prefix("backend:") {
            Some(rest) => format!("reported by the backend ({rest})"),
            None => "inferred from the model name".into(),
        },
    }
}
