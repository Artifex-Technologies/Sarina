//! Go: `llm/anthropic.go`: the Anthropic Messages API client. The OpenAI-shaped request is translated to Messages blocks
//! (`build_body`) and the answer back (`parse_anthropic`); `complete` makes the call with Go's one retry for a failed
//! connection, 429 and 5xx, the timer over the whole call, the caller's context, and replays a finished answer as events
//! when a stream was asked for (the wire is not streamed). The API key is sent only as `x-api-key` and scrubbed from
//! every error text, the server's own included (in Go and Rust since 2026-11-12: an error in a 200 answer kept it).

use super::client::{bad_header, deadline_after, error_body, read_bounded, timeout_nanos, Error, Failure, DEFAULT_RETRY_DELAY};
use super::openai::{BackendError, Request, StreamEvent, MAX_RESPONSE_BYTES};
use super::types::{compact_ascii, usage_from_provider, ToolCall, Turn};
use crate::config::ModelConfig;
use crate::goany::Any;
use crate::goctx::{Ctx, CANCELED};
use crate::gohttp::{self, Dial, Fail, Opts};
use crate::gojson;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The public Messages API host.
pub const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// The Messages API requires max_tokens.
const DEFAULT_MAX_TOKENS: i64 = 8192;

/// Go's `anthropicID`: any tool-call id in the characters the API accepts (each other character one `_`; "call" for none).
pub fn anthropic_id(id: &str) -> String {
    let s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    if s.is_empty() {
        "call".into()
    } else {
        s
    }
}

fn obj(kv: Vec<(&str, Any)>) -> Any {
    Any::Object(kv.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn text_block(s: &str) -> Any {
    obj(vec![("type", Any::String("text".into())), ("text", Any::String(s.into()))])
}

/// Go's `parseAnthropic`.
pub fn parse_anthropic(b: &[u8]) -> Result<Turn, BackendError> {
    let bad = |m: &str| BackendError { message: m.into(), status: 0 };
    let root = match gojson::unmarshal_any(b) {
        (v, None) => match v.into_object() {
            Ok(m) => m,
            Err(_) => return Err(bad("backend returned invalid JSON")),
        },
        _ => return Err(bad("backend returned invalid JSON")),
    };
    if let Some(Any::String(t)) = root.get("type") {
        if t == "error" {
            let e = root.get("error").cloned().unwrap_or(Any::Null);
            return Err(bad(&format!("backend error: {}", error_text(&e))));
        }
    }
    let Some(Any::Array(blocks)) = root.get("content") else { return Err(bad("backend response had no content")) };
    let mut t = Turn::default();
    let (mut text, mut thinking) = (String::new(), String::new());
    let s = |v: Option<&Any>| match v {
        Some(Any::String(x)) => x.clone(),
        _ => String::new(),
    };
    for item in blocks {
        let Any::Object(blk) = item else { continue };
        let Some(Any::String(kind)) = blk.get("type") else { continue };
        match kind.as_str() {
            "text" => text.push_str(&s(blk.get("text"))),
            "thinking" => thinking.push_str(&s(blk.get("thinking"))),
            "tool_use" => {
                let name = s(blk.get("name"));
                if name.is_empty() {
                    return Err(bad("tool call missing function name"));
                }
                let input = match blk.get("input") {
                    Some(Any::Object(m)) => m.clone(),
                    _ => BTreeMap::new(),
                };
                let raw = compact_ascii(&Any::Object(input.clone()));
                t.tool_calls.push(ToolCall {
                    id: s(blk.get("id")),
                    name,
                    arguments: Some(input),
                    raw_arguments: raw,
                    ..ToolCall::default()
                });
            }
            _ => {}
        }
    }
    t.content = text;
    t.reasoning = thinking;
    if let Some(Any::String(sr)) = root.get("stop_reason") {
        if !sr.is_empty() {
            t.has_finish_reason = true;
            t.finish_reason = match sr.as_str() {
                "end_turn" | "stop_sequence" | "pause_turn" => "stop".into(),
                "tool_use" => "tool_calls".into(),
                "max_tokens" => "length".into(),
                "refusal" => "content_filter".into(),
                other => other.into(),
            };
        }
    }
    t.usage = usage_from_provider(root.get("usage"));
    t.usage_reported = !t.usage.is_zero();
    t.raw = Some(root);
    Ok(t)
}

/// Go's `errorText`.
fn error_text(v: &Any) -> String {
    match v {
        Any::String(s) => s.clone(),
        Any::Object(m) => match m.get("message") {
            Some(Any::String(s)) => s.clone(),
            _ => gojson::marshal_any(v).unwrap_or_default(),
        },
        _ => gojson::marshal_any(v).unwrap_or_default(),
    }
}

/// Go's `replay`: a finished answer as the events a stream would have given.
fn replay(t: &Turn, on: &mut dyn FnMut(StreamEvent)) {
    on(StreamEvent { kind: "message_start".into(), ..StreamEvent::default() });
    if !t.content.is_empty() {
        on(StreamEvent { kind: "content_delta".into(), delta: t.content.clone(), ..StreamEvent::default() });
    }
    if t.usage_reported {
        on(StreamEvent { kind: "usage".into(), usage: Some(t.usage), ..StreamEvent::default() });
    }
    on(StreamEvent { kind: "message_stop".into(), finish_reason: t.finish_reason.clone(), ..StreamEvent::default() });
}

/// Go's `AnthropicClient`.
pub struct AnthropicClient {
    pub config: ModelConfig,
    /// the wait before the retry; zero = 750 ms
    pub retry_delay: Duration,
    /// the authorities for https; None = the system's
    pub tls: Option<Arc<rustls::ClientConfig>>,
    /// the answer's bound (tests set it low)
    pub max_response: usize,
}

impl AnthropicClient {
    pub fn new(config: ModelConfig) -> AnthropicClient {
        AnthropicClient { config, retry_delay: Duration::ZERO, tls: None, max_response: MAX_RESPONSE_BYTES }
    }

    fn base(&self) -> String {
        let b = self.config.base_url.trim_end_matches('/');
        if b.is_empty() {
            DEFAULT_ANTHROPIC_URL.into()
        } else {
            b.into()
        }
    }

    pub fn endpoint(&self) -> String {
        let b = self.base();
        if b.ends_with("/v1") {
            format!("{b}/messages")
        } else {
            format!("{b}/v1/messages")
        }
    }

    /// The cheap, free URL a health check can GET.
    pub fn models_endpoint(&self) -> String {
        let b = self.base();
        if b.ends_with("/v1") {
            format!("{b}/models")
        } else {
            format!("{b}/v1/models")
        }
    }

    /// The authentication and version headers (Go's `SetHeaders`, names as Go's canonical form writes them).
    pub fn headers(&self) -> [(&'static str, String); 2] {
        [("X-Api-Key", self.config.api_key.clone()), ("Anthropic-Version", ANTHROPIC_VERSION.into())]
    }

    fn scrub(&self, s: &str) -> String {
        if self.config.api_key.is_empty() {
            s.to_string()
        } else {
            s.replace(&self.config.api_key, "[redacted-key]")
        }
    }

    /// Go's `BuildBody`: the Messages API body (an Encoder without HTML escaping, its newline kept).
    pub fn build_body(&self, req: &Request) -> Result<String, String> {
        let mut system: Vec<String> = Vec::new();
        let mut msgs: Vec<(String, Vec<Any>)> = Vec::new();
        let push = |msgs: &mut Vec<(String, Vec<Any>)>, role: &str, blocks: Vec<Any>| {
            if blocks.is_empty() {
                return;
            }
            if let Some(last) = msgs.last_mut() {
                if last.0 == role {
                    last.1.extend(blocks);
                    return;
                }
            }
            msgs.push((role.to_string(), blocks));
        };
        let trim = crate::gotext::trim_space;
        for m in &req.messages {
            match m.role.as_str() {
                "system" => {
                    if !trim(&m.content).is_empty() {
                        system.push(m.content.clone());
                    }
                }
                "assistant" => {
                    let mut blocks = Vec::new();
                    if !trim(&m.content).is_empty() {
                        blocks.push(text_block(&m.content));
                    }
                    for tc in &m.tool_calls {
                        let input = match gojson::unmarshal_any(tc.function.arguments.as_bytes()) {
                            (v, None) => v.into_object().unwrap_or_default(),
                            _ => BTreeMap::new(),
                        };
                        blocks.push(obj(vec![
                            ("type", Any::String("tool_use".into())),
                            ("id", Any::String(anthropic_id(&tc.id))),
                            ("name", Any::String(tc.function.name.clone())),
                            ("input", Any::Object(input)),
                        ]));
                    }
                    push(&mut msgs, "assistant", blocks);
                }
                "tool" => {
                    let content = if trim(&m.content).is_empty() { "(no output)".to_string() } else { m.content.clone() };
                    push(
                        &mut msgs,
                        "user",
                        vec![obj(vec![
                            ("type", Any::String("tool_result".into())),
                            ("tool_use_id", Any::String(anthropic_id(&m.tool_call_id))),
                            ("content", Any::String(content)),
                        ])],
                    );
                }
                // user and anything unknown
                _ => {
                    if !trim(&m.content).is_empty() {
                        push(&mut msgs, "user", vec![text_block(&m.content)]);
                    }
                }
            }
        }
        if msgs.first().is_none_or(|m| m.0 != "user") {
            msgs.insert(0, ("user".into(), vec![text_block("(continuing the conversation)")]));
        }
        if let Some(s) = &req.schema {
            // json.Marshal: HTML escaped; an error leaves it empty
            let raw = gojson::marshal_any(&Any::Object(s.schema.clone())).unwrap_or_default();
            system.push(format!(
                "Reply with a single JSON document and nothing else. It must conform to this JSON Schema (named {}): {raw}",
                s.name
            ));
        }
        let max_tokens = if req.max_tokens <= 0 { DEFAULT_MAX_TOKENS } else { req.max_tokens };
        let e = |e: gojson::Error| e.0;
        let mut out = String::from("{\"model\":");
        gojson::write_str(&mut out, &self.config.model, false);
        out.push_str(&format!(",\"max_tokens\":{max_tokens}"));
        let system = system.join("\n\n");
        if !system.is_empty() {
            out.push_str(",\"system\":");
            gojson::write_str(&mut out, &system, false);
        }
        out.push_str(",\"messages\":[");
        for (i, (role, blocks)) in msgs.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"role\":");
            gojson::write_str(&mut out, role, false);
            out.push_str(",\"content\":");
            out.push_str(&gojson::any_json(&Any::Array(blocks.clone()), false).map_err(e)?);
            out.push('}');
        }
        out.push(']');
        if self.config.temperature > 0.0 {
            out.push_str(",\"temperature\":");
            gojson::write_f64(&mut out, self.config.temperature).map_err(e)?;
        }
        if !req.tools.is_empty() {
            let tools: Vec<Any> = req
                .tools
                .iter()
                .map(|t| {
                    let params = match &t.parameters {
                        Some(p) => Any::Object(p.clone()),
                        None => obj(vec![("type", Any::String("object".into())), ("properties", Any::Object(BTreeMap::new()))]),
                    };
                    obj(vec![
                        ("name", Any::String(t.name.clone())),
                        ("description", Any::String(t.description.clone())),
                        ("input_schema", params),
                    ])
                })
                .collect();
            out.push_str(",\"tools\":");
            out.push_str(&gojson::any_json(&Any::Array(tools), false).map_err(e)?);
            out.push_str(",\"tool_choice\":{\"type\":\"auto\"}");
        }
        out.push_str("}\n");
        Ok(out)
    }

    /// Go's `Complete`: one model call; with `req.stream` the finished answer is replayed to `on` as events.
    pub fn complete(&self, ctx: &Ctx, req: &Request, on: Option<&mut dyn FnMut(StreamEvent)>) -> Result<Turn, Error> {
        let body =
            self.build_body(req).map_err(|e| Error::Backend(BackendError { message: format!("cannot encode request: {e}"), status: 0 }))?;
        let mut last = None;
        for _attempt in 0..2 {
            let (r, retryable) = self.once(ctx, body.as_bytes());
            match r {
                Ok(t) => {
                    if req.stream {
                        if let Some(on) = on {
                            replay(&t, on);
                        }
                    }
                    return Ok(t);
                }
                Err(e) => last = Some(e),
            }
            if !retryable || ctx.err().is_some() {
                break;
            }
            let delay = if self.retry_delay.is_zero() { DEFAULT_RETRY_DELAY } else { self.retry_delay };
            if !ctx.sleep(delay) {
                return Err(Error::Ctx(ctx.err().unwrap_or(CANCELED)));
            }
        }
        Err(last.unwrap_or(Error::Ctx(CANCELED)))
    }

    fn once(&self, ctx: &Ctx, body: &[u8]) -> (Result<Turn, Error>, bool) {
        let timeout = timeout_nanos(self.config.timeout_seconds);
        let deadline: gohttp::Deadline = Rc::new(Cell::new(deadline_after(timeout)));
        let endpoint = self.endpoint();
        let base = self.base();
        let fail = |f: Failure, retryable: bool| -> (Result<Turn, Error>, bool) {
            if let Some(e) = ctx.err() {
                return (Err(Error::Ctx(e)), false);
            }
            let be = |m: String| Error::Backend(BackendError { message: self.scrub(&m), status: 0 });
            if Instant::now() >= deadline.get() {
                let d = crate::gotime::duration_string(timeout);
                return (Err(be(format!("request to {endpoint} timed out after {d}"))), false);
            }
            match f {
                Failure::Backend(b) => (Err(Error::Backend(BackendError { message: self.scrub(&b.message), status: b.status })), retryable),
                Failure::Transport(t) => (Err(be(format!("cannot reach backend at {base}: {t}"))), retryable),
            }
        };
        let [(k1, v1), (k2, v2)] = self.headers();
        let headers = [(k1, v1.as_str()), (k2, v2.as_str()), ("Content-Type", "application/json")];
        if let Some(e) = bad_header(&headers) {
            return fail(Failure::Transport(gohttp::go_url_error("POST", &endpoint, &e)), true);
        }
        let o = Opts { dial: Dial::Tcp, method: "POST", url: &endpoint, headers: &headers, body: Some(body), tls: self.tls.clone() };
        let mut resp = match gohttp::open(&o, ctx, &deadline) {
            Ok(r) => r,
            Err(f) => {
                let what = match f {
                    Fail::Other(s) => s,
                    Fail::Deadline | Fail::Ctx => ctx.err().unwrap_or("context deadline exceeded").to_string(),
                };
                return fail(Failure::Transport(gohttp::go_url_error("POST", &endpoint, &what)), true);
            }
        };
        if !(200..=299).contains(&resp.status) {
            let b = error_body(&mut resp.body);
            let status = i64::from(resp.status);
            let message = format!("backend returned HTTP {status}: {}", crate::gotext::to_valid_utf8(&b));
            return fail(Failure::Backend(BackendError { message, status }), status >= 500 || status == 429);
        }
        if resp.encoded {
            return fail(Failure::Transport("the answer is compressed (Content-Encoding), which this client does not read".into()), false);
        }
        let b = match read_bounded(&mut resp.body, self.max_response) {
            Ok(b) => b,
            Err(f) => return fail(f, false),
        };
        match parse_anthropic(&b) {
            Ok(t) => (Ok(t), false),
            Err(e) => (Err(Error::Backend(BackendError { message: self.scrub(&e.message), status: e.status })), false),
        }
    }
}
