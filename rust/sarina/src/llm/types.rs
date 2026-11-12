//! Go: `go/llm/types.go`: token usage, the wire shape of messages and tool calls, tool definitions.

use crate::config::as_int;
use crate::goany::Any;
use crate::gojson::{self, Obj};
use std::collections::BTreeMap;

/// Token counts. Go's ints: sums wrap as Go's do.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    /// the server's count, or (Ollama) the reasoning's length in bytes / 4
    pub reasoning_tokens: i64,
}

impl Usage {
    /// Input + output + cache creation + cache read (reasoning is part of output).
    pub fn total(&self) -> i64 {
        self.input_tokens
            .wrapping_add(self.output_tokens)
            .wrapping_add(self.cache_creation_input_tokens)
            .wrapping_add(self.cache_read_input_tokens)
    }

    pub fn add(&self, o: &Usage) -> Usage {
        Usage {
            input_tokens: self.input_tokens.wrapping_add(o.input_tokens),
            output_tokens: self.output_tokens.wrapping_add(o.output_tokens),
            cache_creation_input_tokens: self.cache_creation_input_tokens.wrapping_add(o.cache_creation_input_tokens),
            cache_read_input_tokens: self.cache_read_input_tokens.wrapping_add(o.cache_read_input_tokens),
            reasoning_tokens: self.reasoning_tokens.wrapping_add(o.reasoning_tokens),
        }
    }

    pub fn is_zero(&self) -> bool {
        self.total() == 0 && self.reasoning_tokens == 0
    }

    /// Go's `MarshalJSON`: the five counts and the derived total_tokens.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"input_tokens\":{},\"output_tokens\":{},\"cache_creation_input_tokens\":{},\"cache_read_input_tokens\":{},\"reasoning_tokens\":{},\"total_tokens\":{}}}",
            self.input_tokens,
            self.output_tokens,
            self.cache_creation_input_tokens,
            self.cache_read_input_tokens,
            self.reasoning_tokens,
            self.total()
        )
    }

    /// Go's `UnmarshalJSON`: missing keys, numeric strings and non-objects are tolerated (a document that does not read is
    /// all zeros).
    pub fn from_json(b: &[u8]) -> Usage {
        let (v, err) = gojson::unmarshal_any(b);
        if err.is_some() {
            return Usage::default();
        }
        let m = crate::config::as_map(Some(&v));
        let g = |k: &str| as_int(m.get(k)).unwrap_or(0);
        Usage {
            input_tokens: g("input_tokens"),
            output_tokens: g("output_tokens"),
            cache_creation_input_tokens: g("cache_creation_input_tokens"),
            cache_read_input_tokens: g("cache_read_input_tokens"),
            reasoning_tokens: g("reasoning_tokens"),
        }
    }
}

/// Go's `UsageFromProvider`: a provider's "usage" object (OpenAI, Ollama or Anthropic names); anything else is zeros.
pub fn usage_from_provider(v: Option<&Any>) -> Usage {
    let Some(Any::Object(m)) = v else { return Usage::default() };
    let num = |k: &str| match m.get(k) {
        Some(Any::Bool(_)) => 0,
        x => as_int(x).unwrap_or(0),
    };
    let first = |keys: &[&str]| keys.iter().map(|k| num(k)).find(|&n| n != 0).unwrap_or(0);
    let mut u = Usage {
        input_tokens: first(&["input_tokens", "prompt_tokens", "prompt_eval_count"]),
        output_tokens: first(&["output_tokens", "completion_tokens", "eval_count"]),
        cache_creation_input_tokens: num("cache_creation_input_tokens"),
        cache_read_input_tokens: num("cache_read_input_tokens"),
        reasoning_tokens: num("reasoning_tokens"),
    };
    if let (Some(Any::Object(d)), 0) = (m.get("completion_tokens_details"), u.reasoning_tokens) {
        u.reasoning_tokens = as_int(d.get("reasoning_tokens")).unwrap_or(0);
    }
    if let (Some(Any::Object(d)), 0) = (m.get("prompt_tokens_details"), u.cache_read_input_tokens) {
        u.cache_read_input_tokens = as_int(d.get("cached_tokens")).unwrap_or(0);
    }
    u
}

/// The function half of a wire tool call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// The OpenAI shape of a tool call inside an assistant message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WireToolCall {
    pub id: String,
    pub kind: String,
    pub function: FunctionCall,
}

impl WireToolCall {
    pub fn to_json(&self, html: bool) -> String {
        let f = Obj::new(html).str("name", &self.function.name).str("arguments", &self.function.arguments).end();
        Obj::new(html).str("id", &self.id).str("type", &self.kind).with("function", f).end().unwrap_or_default()
    }
}

/// Exactly what is sent to the backend for one message.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
    pub name: String,
    pub tool_call_id: String,
    pub tool_calls: Vec<WireToolCall>,
}

/// A list of tool calls as Go writes `[]WireToolCall`.
pub fn tool_calls_json(calls: &[WireToolCall], html: bool) -> String {
    let parts: Vec<String> = calls.iter().map(|c| c.to_json(html)).collect();
    format!("[{}]", parts.join(","))
}

impl Message {
    /// The struct's JSON (`name`, `tool_call_id` and `tool_calls` omitted when empty).
    pub fn to_json(&self, html: bool) -> String {
        let o = Obj::new(html)
            .str("role", &self.role)
            .str("content", &self.content)
            .str_omit("name", &self.name)
            .str_omit("tool_call_id", &self.tool_call_id);
        let o = if self.tool_calls.is_empty() { o } else { o.raw("tool_calls", &tool_calls_json(&self.tool_calls, html)) };
        o.end().unwrap_or_default()
    }
}

/// A parsed model tool call. When the arguments could not be read even after repair, `parse_error` is set and
/// `arguments` is None (Go's nil map).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Option<BTreeMap<String, Any>>,
    pub raw_arguments: String,
    pub parse_error: String,
}

impl ToolCall {
    /// Go's `ArgumentsJSON`: compact, without HTML escaping, everything past ASCII as `\u` escapes; `{}` for none.
    pub fn arguments_json(&self) -> String {
        match &self.arguments {
            None => "{}".into(),
            Some(m) => compact_ascii(&Any::Object(m.clone())),
        }
    }

    /// Go's `Wire`.
    pub fn wire(&self) -> WireToolCall {
        WireToolCall {
            id: self.id.clone(),
            kind: "function".into(),
            function: FunctionCall { name: self.name.clone(), arguments: self.arguments_json() },
        }
    }
}

/// Go's `compactASCII`: `{}` when the value cannot be written.
pub fn compact_ascii(v: &Any) -> String {
    match gojson::any_json(v, false) {
        Ok(s) => gojson::ascii_only(&s),
        Err(_) => "{}".into(),
    }
}

/// A model-visible function definition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// None is Go's nil map: an empty object schema is sent instead
    pub parameters: Option<BTreeMap<String, Any>>,
}

impl ToolSpec {
    /// Go's `MarshalJSON`: the OpenAI function-tool shape, written by json.Marshal (keys sorted, HTML escaping on).
    pub fn to_json(&self) -> Result<String, gojson::Error> {
        let params = match &self.parameters {
            Some(p) => Any::Object(p.clone()),
            None => Any::Object(BTreeMap::from([
                ("type".to_owned(), Any::String("object".into())),
                ("properties".to_owned(), Any::Object(BTreeMap::new())),
            ])),
        };
        let function = BTreeMap::from([
            ("name".to_owned(), Any::String(self.name.clone())),
            ("description".to_owned(), Any::String(self.description.clone())),
            ("parameters".to_owned(), params),
        ]);
        let v = Any::Object(BTreeMap::from([
            ("type".to_owned(), Any::String("function".into())),
            ("function".to_owned(), Any::Object(function)),
        ]));
        gojson::marshal_any(&v)
    }
}
