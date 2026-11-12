//! Go: `llm/client.go`'s wire format: the request body for an OpenAI-compatible `/chat/completions` endpoint
//! (`BuildBody`), the answer read into a turn (`parseResponse`) and a streamed answer read event by event
//! (`readStream`). Every read is bounded (Go and Rust, 2026-11-12): an answer of at most `MAX_RESPONSE_BYTES`, streamed or
//! not, and a streamed line of at most `MAX_STREAM_LINE`, each refused past its bound with its own error. The transport
//! (and TLS for the online tiers) comes next.

use super::repair::parse_arguments;
use super::types::{usage_from_provider, Message, ToolCall, ToolSpec, Turn, Usage};
use crate::config::OutputSchema;
use crate::goany::Any;
use crate::gojson;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};

/// An answer's bound (256 MiB; a long answer is a few hundred KiB).
pub const MAX_RESPONSE_BYTES: usize = 256 << 20;
/// A streamed line's bound (16 MiB; a line is one small chunk of the answer).
pub const MAX_STREAM_LINE: usize = 16 << 20;

/// Go's `BackendError`: the single error type for backend problems.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackendError {
    pub message: String,
    pub status: i64,
}

/// Why a streamed answer was not read: the answer was refused (a backend error), or reading it failed (the reader's own
/// error, which the transport words as "cannot reach backend", as Go's `readStream` returns it unwrapped).
#[derive(Clone, Debug, PartialEq)]
pub enum StreamError {
    Backend(BackendError),
    Read(String),
}

impl StreamError {
    pub fn message(&self) -> &str {
        match self {
            StreamError::Backend(b) => &b.message,
            StreamError::Read(s) => s,
        }
    }
}

impl From<BackendError> for StreamError {
    fn from(b: BackendError) -> StreamError {
        StreamError::Backend(b)
    }
}

fn be(message: impl Into<String>) -> BackendError {
    BackendError { message: message.into(), status: 0 }
}

/// One model call (Go's `Request`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub schema: Option<OutputSchema>,
    /// 0 = not sent
    pub max_tokens: i64,
    pub stream: bool,
    /// a reasoning model's hidden thinking off for this request
    pub no_think: bool,
}

/// One event of a streamed answer (Go's `StreamEvent`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamEvent {
    /// message_start, content_delta, tool_call_delta, usage, message_stop
    pub kind: String,
    pub delta: String,
    pub tool_call_index: i64,
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments_delta: String,
    pub finish_reason: String,
    pub usage: Option<Usage>,
}

/// Go's `noThink`: the latest user message ends with Qwen3's "/no_think".
fn no_think(msgs: &[Message]) -> bool {
    msgs.iter().rev().find(|m| m.role == "user").is_some_and(|m| crate::gotext::trim_space(&m.content).ends_with("/no_think"))
}

/// Go's `BuildBody`: the request body (an Encoder without HTML escaping, its newline kept); tools and tool_choice only
/// with tools.
pub fn build_body(model: &str, temperature: f64, req: &Request) -> Result<String, String> {
    let mut out = String::from("{\"model\":");
    gojson::write_str(&mut out, model, false);
    out.push_str(",\"messages\":[");
    for (i, m) in req.messages.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&m.to_json(false));
    }
    out.push_str("],\"temperature\":");
    gojson::write_f64(&mut out, temperature).map_err(|e| e.0)?;
    out.push_str(if req.stream { ",\"stream\":true" } else { ",\"stream\":false" });
    if req.stream {
        out.push_str(",\"stream_options\":{\"include_usage\":true}");
    }
    if !req.tools.is_empty() {
        out.push_str(",\"tools\":[");
        for (i, t) in req.tools.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // the tool's own MarshalJSON (json.Marshal: HTML escaped), kept as it is by the encoder
            out.push_str(&t.to_json().map_err(|e| e.0)?);
        }
        out.push_str("],\"tool_choice\":\"auto\"");
    }
    if let Some(s) = &req.schema {
        let rf = Any::Object(BTreeMap::from([
            (
                "json_schema".to_string(),
                Any::Object(BTreeMap::from([
                    ("name".to_string(), Any::String(s.name.clone())),
                    ("schema".to_string(), Any::Object(s.schema.clone())),
                    ("strict".to_string(), Any::Bool(s.strict)),
                ])),
            ),
            ("type".to_string(), Any::String("json_schema".into())),
        ]));
        out.push_str(",\"response_format\":");
        out.push_str(&gojson::any_json(&rf, false).map_err(|e| e.0)?);
    }
    if req.max_tokens != 0 {
        out.push_str(&format!(",\"max_tokens\":{}", req.max_tokens));
    }
    if req.no_think || no_think(&req.messages) {
        out.push_str(",\"reasoning_effort\":\"none\"");
    }
    out.push_str("}\n");
    Ok(out)
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

/// Go's `flattenContent`.
fn flatten_content(v: Option<&Any>) -> String {
    match v {
        None | Some(Any::Null) => String::new(),
        Some(Any::String(s)) => s.clone(),
        Some(Any::Array(items)) => {
            let mut b = String::new();
            for it in items {
                match it {
                    Any::String(s) => b.push_str(s),
                    Any::Object(m) => match m.get("text") {
                        Some(Any::String(s)) => b.push_str(s),
                        _ => b.push_str(&super::types::compact_ascii(it)),
                    },
                    other => b.push_str(&super::types::compact_ascii(other)),
                }
            }
            b
        }
        Some(other) => crate::session::sprint(other),
    }
}

/// Go's `approxReasoningTokens`: about a token per four bytes of reasoning a server sent without counting it.
fn approx_reasoning_tokens(t: &mut Turn) {
    if t.usage.reasoning_tokens == 0 && !t.reasoning.is_empty() {
        t.usage.reasoning_tokens = (t.reasoning.len() as i64 / 4).max(1);
    }
}

/// Go's `makeCall`.
fn make_call(id: &str, name: &str, args: Option<&Any>) -> ToolCall {
    let mut c = ToolCall { id: id.into(), name: name.into(), arguments: Some(BTreeMap::new()), ..ToolCall::default() };
    match args {
        None | Some(Any::Null) => {}
        Some(Any::Object(m)) => c.arguments = Some(m.clone()),
        Some(Any::String(a)) => {
            c.raw_arguments.clone_from(a);
            let (parsed, trunc, err) = parse_arguments(a);
            if !err.is_empty() {
                c.parse_error = err.into();
            } else {
                c.arguments = parsed;
                c.truncated = trunc;
            }
        }
        Some(_) => c.parse_error = "arguments must be a JSON object".into(),
    }
    c
}

fn finish_text(f: &Any) -> Option<String> {
    match f {
        Any::Null => None,
        Any::String(s) => Some(s.clone()),
        other => Some(crate::session::sprint(other)),
    }
}

/// Go's `parseResponse`: a non-streamed answer.
pub fn parse_response(b: &[u8]) -> Result<Turn, BackendError> {
    let root = match gojson::unmarshal_any(b) {
        (v, None) => v,
        (_, Some(e)) => return Err(be(format!("backend returned invalid JSON: {}", e.0))),
    };
    let Any::Object(obj) = &root else { return Err(be("backend returned a malformed JSON payload (not an object)")) };
    let choices: &[Any] = match obj.get("choices") {
        Some(Any::Array(c)) => c,
        _ => &[],
    };
    if choices.is_empty() {
        if let Some(e) = obj.get("error") {
            return Err(be(format!("backend error: {}", error_text(e))));
        }
        return Err(be("backend response contained no choices"));
    }
    let Any::Object(choice) = &choices[0] else { return Err(be("backend response had a malformed choice")) };
    let Some(Any::Object(msg)) = choice.get("message") else { return Err(be("backend response had no assistant message")) };
    let mut t = Turn { content: flatten_content(msg.get("content")), raw: Some(msg.clone()), ..Turn::default() };
    for key in ["reasoning_content", "reasoning"] {
        if let Some(Any::String(s)) = msg.get(key) {
            if !s.is_empty() {
                t.reasoning.clone_from(s);
                break;
            }
        }
    }
    if let Some(f) = choice.get("finish_reason").and_then(finish_text) {
        t.finish_reason = f;
        t.has_finish_reason = true;
    }
    t.usage = usage_from_provider(obj.get("usage"));
    t.usage_reported = !t.usage.is_zero();
    approx_reasoning_tokens(&mut t);
    if let Some(Any::Array(list)) = msg.get("tool_calls") {
        for item in list {
            let Any::Object(m) = item else { return Err(be("backend returned a malformed tool call")) };
            let empty = BTreeMap::new();
            let func = match m.get("function") {
                Some(Any::Object(f)) => f,
                _ => &empty,
            };
            let name = match func.get("name") {
                Some(Any::String(s)) => s.as_str(),
                _ => "",
            };
            if name.is_empty() {
                return Err(be("tool call missing function name"));
            }
            let id = match m.get("id") {
                Some(Any::String(s)) => s.as_str(),
                _ => "",
            };
            t.tool_calls.push(make_call(id, name, func.get("arguments")));
        }
    } else if let Some(Any::Object(fc)) = msg.get("function_call") {
        let name = match fc.get("name") {
            Some(Any::String(s)) => s.as_str(),
            _ => "",
        };
        if name.is_empty() {
            return Err(be("tool call missing function name"));
        }
        t.tool_calls.push(make_call("call_0", name, fc.get("arguments")));
    }
    Ok(t)
}

#[derive(Default)]
struct FragCall {
    id: String,
    name: String,
    args: String,
}

#[derive(Default)]
struct StreamAcc {
    content: String,
    reasoning: String,
    slots: Vec<FragCall>,
    by_index: HashMap<i64, usize>,
    finish: String,
    has_finish: bool,
    usage: Usage,
    payloads: usize,
}

impl StreamAcc {
    fn merge(&mut self, index: i64, id: &str, name: &str, args: &str) {
        let mut pos = self.by_index.get(&index).copied();
        if let Some(p) = pos {
            // some servers reuse index 0 for every parallel call
            if !id.is_empty() && !self.slots[p].id.is_empty() && self.slots[p].id != id {
                pos = None;
            }
        }
        let p = match pos {
            Some(p) => p,
            None => {
                self.slots.push(FragCall::default());
                self.by_index.insert(index, self.slots.len() - 1);
                self.slots.len() - 1
            }
        };
        let s = &mut self.slots[p];
        if !id.is_empty() {
            s.id = id.into();
        }
        if name.is_empty() || name == s.name {
        } else if s.name.is_empty() || name.starts_with(&s.name) {
            s.name = name.into();
        } else {
            s.name.push_str(name);
        }
        s.args.push_str(args);
    }

    fn process(&mut self, m: &BTreeMap<String, Any>, emit: &mut dyn FnMut(StreamEvent)) -> Result<(), BackendError> {
        self.payloads += 1;
        if !m.contains_key("choices") {
            if let Some(e) = m.get("error") {
                return Err(be(format!("backend error: {}", error_text(e))));
            }
        }
        let u = usage_from_provider(m.get("usage"));
        if !u.is_zero() {
            // servers repeat cumulative usage: keep the last, do not sum
            self.usage = u;
            emit(StreamEvent { kind: "usage".into(), usage: Some(u), ..StreamEvent::default() });
        }
        let Some(Any::Array(choices)) = m.get("choices") else { return Ok(()) };
        for ch in choices {
            let Any::Object(cm) = ch else { continue };
            if let Some(Any::Object(d)) = cm.get("delta") {
                if let Some(Any::String(s)) = d.get("content") {
                    if !s.is_empty() {
                        self.content.push_str(s);
                        emit(StreamEvent { kind: "content_delta".into(), delta: s.clone(), ..StreamEvent::default() });
                    }
                }
                for key in ["reasoning_content", "reasoning"] {
                    if let Some(Any::String(s)) = d.get(key) {
                        if !s.is_empty() {
                            self.reasoning.push_str(s);
                            break;
                        }
                    }
                }
                if let Some(Any::Array(tcs)) = d.get("tool_calls") {
                    for tc in tcs {
                        let Any::Object(tm) = tc else { continue };
                        let idx = match tm.get("index") {
                            Some(Any::Bool(_)) => 0,
                            other => crate::config::as_int(other).unwrap_or(0),
                        };
                        let s = |v: Option<&Any>| match v {
                            Some(Any::String(x)) => x.clone(),
                            _ => String::new(),
                        };
                        let id = s(tm.get("id"));
                        let (name, args) = match tm.get("function") {
                            Some(Any::Object(f)) => (s(f.get("name")), s(f.get("arguments"))),
                            _ => (String::new(), String::new()),
                        };
                        self.merge(idx, &id, &name, &args);
                        emit(StreamEvent {
                            kind: "tool_call_delta".into(),
                            tool_call_index: idx,
                            tool_call_id: id,
                            tool_name: name,
                            arguments_delta: args,
                            ..StreamEvent::default()
                        });
                    }
                }
            }
            if let Some(f) = cm.get("finish_reason").and_then(finish_text) {
                self.finish = f.clone();
                self.has_finish = true;
                emit(StreamEvent { kind: "message_stop".into(), finish_reason: f, ..StreamEvent::default() });
            }
        }
        Ok(())
    }
}

enum LineEnd {
    Line,
    Eof,
    TooLong,
    Failed(String),
}

/// Go's `readLine` on its bufio.Reader: up to and including the next newline, refusing more than `max` bytes.
fn read_line(r: &mut impl BufRead, max: usize, line: &mut Vec<u8>) -> LineEnd {
    line.clear();
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) => return LineEnd::Failed(e.to_string()),
        };
        if buf.is_empty() {
            return LineEnd::Eof;
        }
        let (take, end) = match buf.iter().position(|&c| c == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        if line.len() + take > max {
            return LineEnd::TooLong;
        }
        line.extend_from_slice(&buf[..take]);
        r.consume(take);
        if end {
            return LineEnd::Line;
        }
    }
}

/// Go's `readStream` with its bounds: the turn a stream of server-sent events makes, each event handed to `on` as it
/// comes; `touch` on every line (the caller's idle timer).
pub fn read_stream(
    r: impl Read,
    max_response: usize,
    max_line: usize,
    touch: &mut dyn FnMut(),
    on: &mut dyn FnMut(StreamEvent),
) -> Result<Turn, StreamError> {
    let mut acc = StreamAcc::default();
    on(StreamEvent { kind: "message_start".into(), ..StreamEvent::default() });
    // Go's bufio.Reader (64 KiB) over an io.LimitedReader of one byte past the bound
    let mut br = BufReader::with_capacity(64 << 10, r.take(max_response as u64 + 1));
    let mut data: Vec<String> = Vec::new();
    let mut done = false;
    let mut flush = |data: &mut Vec<String>, acc: &mut StreamAcc, done: &mut bool| -> Result<(), BackendError> {
        if data.is_empty() {
            return Ok(());
        }
        let payload = crate::gotext::trim_space(&data.join("\n")).to_string();
        data.clear();
        if payload == "[DONE]" {
            *done = true;
            return Ok(());
        }
        let v = match gojson::unmarshal_any(payload.as_bytes()) {
            (v, None) => v,
            _ => return Err(be("invalid JSON in streaming response")),
        };
        let Any::Object(m) = &v else { return Err(be("invalid JSON in streaming response (not an object)")) };
        acc.process(m, on)
    };
    let mut line = Vec::new();
    while !done {
        let end = read_line(&mut br, max_line, &mut line);
        if br.get_ref().limit() == 0 {
            return Err(be(format!("the model's streamed answer is larger than {max_response} bytes; not read")).into());
        }
        if let LineEnd::TooLong = end {
            return Err(be(format!("a line of the model's streamed answer is longer than {max_line} bytes; not read")).into());
        }
        if !line.is_empty() {
            touch();
            let text = String::from_utf8_lossy(&line).into_owned();
            let text = text.trim_end_matches(['\r', '\n']);
            if text.is_empty() {
                flush(&mut data, &mut acc, &mut done)?;
            } else if let Some(rest) = text.strip_prefix("data:") {
                data.push(rest.to_string());
            }
        }
        match end {
            LineEnd::Line => {}
            LineEnd::Eof => break,
            LineEnd::Failed(e) => return Err(StreamError::Read(e)),
            LineEnd::TooLong => unreachable!("handled above"),
        }
    }
    flush(&mut data, &mut acc, &mut done)?;
    if acc.payloads == 0 {
        return Err(be("backend returned an empty stream").into());
    }
    let mut t = Turn {
        content: acc.content,
        reasoning: acc.reasoning,
        finish_reason: acc.finish,
        has_finish_reason: acc.has_finish,
        usage_reported: !acc.usage.is_zero(),
        usage: acc.usage,
        ..Turn::default()
    };
    approx_reasoning_tokens(&mut t);
    for s in acc.slots {
        if s.name.is_empty() {
            return Err(be("tool call missing function name").into());
        }
        t.tool_calls.push(make_call(&s.id, &s.name, Some(&Any::String(s.args))));
    }
    Ok(t)
}
