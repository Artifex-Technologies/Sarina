//! Go: `llm/client.go`'s transport (`Complete`): one model call to an OpenAI-compatible `/chat/completions` endpoint over
//! `gohttp` (TLS through rustls for the online tiers), with Go's one retry for a failed connection, 429 and 5xx (after
//! `retry_delay`), the fallback models for a model that still fails that way, is missing (404) or timed out, the timer
//! (the whole call when not streamed, longer by a second per 40 tokens allowed; reset by every streamed line), and the
//! caller's context (a cancel ends the call at once, also during the retry's wait). The answer's bounds are
//! `openai.rs`'s.

use super::normalize::normalize;
use super::openai::{build_body, parse_response, read_stream, BackendError, Request, StreamError, StreamEvent};
use super::openai::{MAX_RESPONSE_BYTES, MAX_STREAM_LINE};
use super::types::Turn;
use crate::config::{go_int, ModelConfig, DEFAULT_TIMEOUT_SECONDS};
use crate::goctx::{Ctx, CANCELED};
use crate::gohttp::{self, Dial, Fail, Opts};
use std::cell::Cell;
use std::io::Read;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The wait before the one retry when none is set (Go's 750 ms).
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(750);

/// What a model call ends with when it fails: a backend error, or the context's own ("context canceled", "context deadline
/// exceeded"), as Go's `Complete` returns `ctx.Err()`.
#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    Backend(BackendError),
    Ctx(&'static str),
}

impl Error {
    pub fn text(&self) -> &str {
        match self {
            Error::Backend(b) => &b.message,
            Error::Ctx(s) => s,
        }
    }
}

fn be(message: impl Into<String>) -> Error {
    Error::Backend(BackendError { message: message.into(), status: 0 })
}

/// Go's `time.Duration(seconds * float64(time.Second))` on amd64, the default when that is not positive: nanoseconds.
pub(crate) fn timeout_nanos(seconds: f64) -> i64 {
    let t = go_int(seconds * 1e9);
    if t <= 0 {
        go_int(DEFAULT_TIMEOUT_SECONDS * 1e9)
    } else {
        t
    }
}

/// A deadline `nanos` from now (now for a negative one; far ahead when it does not fit).
pub(crate) fn deadline_after(nanos: i64) -> Instant {
    let now = Instant::now();
    if nanos <= 0 {
        return now;
    }
    now.checked_add(Duration::from_nanos(nanos as u64)).unwrap_or_else(|| now + Duration::from_secs(100 * 365 * 86400))
}

/// What went wrong in one try, before `fail` words it.
pub(crate) enum Failure {
    Backend(BackendError),
    /// the transport's text (Go's `*url.Error` or a body read error)
    Transport(String),
}

/// Reads at most `max` bytes; past it, the model's answer is refused (Go's `readBounded`).
pub(crate) fn read_bounded(r: &mut impl Read, max: usize) -> Result<Vec<u8>, Failure> {
    let mut b = Vec::new();
    match r.take(max as u64 + 1).read_to_end(&mut b) {
        Err(e) => Err(Failure::Transport(e.to_string())),
        Ok(_) if b.len() > max => {
            Err(Failure::Backend(BackendError { message: format!("the model's answer is larger than {max} bytes; not read"), status: 0 }))
        }
        Ok(_) => Ok(b),
    }
}

/// At most 64 KiB of an error answer's body, errors ignored (Go's `io.ReadAll(io.LimitReader(resp.Body, 64<<10))`).
pub(crate) fn error_body(r: &mut impl Read) -> Vec<u8> {
    let mut b = Vec::new();
    let mut part = [0u8; 16384];
    let mut lr = r.take(64 << 10);
    loop {
        match lr.read(&mut part) {
            Ok(0) | Err(_) => return b,
            Ok(n) => b.extend_from_slice(&part[..n]),
        }
    }
}

/// Go's `http.Header.Set` values are checked by the transport before anything is sent.
pub(crate) fn bad_header(headers: &[(&str, &str)]) -> Option<String> {
    headers
        .iter()
        .find(|(_, v)| v.bytes().any(|c| (c < 0x20 && c != b'\t') || c == 0x7f))
        .map(|(k, _)| format!("net/http: invalid header field value for {}", crate::gotext::quote(k)))
}

/// Go's `llm.Client`.
pub struct Client {
    pub config: ModelConfig,
    /// the wait before the retry; zero = 750 ms
    pub retry_delay: Duration,
    /// the authorities for https; None = the system's (Go's `HTTPClient` nil)
    pub tls: Option<Arc<rustls::ClientConfig>>,
    /// the answer's bounds (tests set them low, as Go's tests set its variables)
    pub max_response: usize,
    pub max_line: usize,
}

impl Client {
    pub fn new(config: ModelConfig) -> Client {
        Client { config, retry_delay: Duration::ZERO, tls: None, max_response: MAX_RESPONSE_BYTES, max_line: MAX_STREAM_LINE }
    }

    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'))
    }

    /// Go's `Complete`: one model call; with `req.stream` each event of the answer goes to `on` as it comes.
    pub fn complete(&self, ctx: &Ctx, req: &Request, mut on: Option<&mut dyn FnMut(StreamEvent)>) -> Result<Turn, Error> {
        let mut none = |_: StreamEvent| {};
        let on: &mut dyn FnMut(StreamEvent) = match on.as_mut() {
            Some(f) => *f,
            None => &mut none,
        };
        let (r, fallback) = self.complete_with(ctx, &self.config.model, req, on);
        if r.is_ok() || !fallback {
            return r;
        }
        for m in &self.config.fallbacks {
            let (r2, fb) = self.complete_with(ctx, m, req, on);
            match r2 {
                Ok(mut t) => {
                    t.model.clone_from(m);
                    return Ok(t);
                }
                Err(e) if !fb => return Err(e),
                Err(_) => {}
            }
        }
        r
    }

    /// One model; the bool says whether another model might answer where this one failed.
    fn complete_with(&self, ctx: &Ctx, model: &str, req: &Request, on: &mut dyn FnMut(StreamEvent)) -> (Result<Turn, Error>, bool) {
        let body = match build_body(model, self.config.temperature, req) {
            Ok(b) => b,
            Err(e) => return (Err(be(format!("cannot encode request: {e}"))), false),
        };
        let names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        let mut last = None;
        let mut retryable = false;
        for _attempt in 0..2 {
            let (r, rt) = self.once(ctx, body.as_bytes(), req, on);
            retryable = rt;
            match r {
                Ok(mut t) => {
                    normalize(&mut t, &names);
                    return (Ok(t), false);
                }
                Err(e) => last = Some(e),
            }
            if !retryable || ctx.err().is_some() {
                break;
            }
            let delay = if self.retry_delay.is_zero() { DEFAULT_RETRY_DELAY } else { self.retry_delay };
            if !ctx.sleep(delay) {
                return (Err(Error::Ctx(ctx.err().unwrap_or(CANCELED))), false);
            }
        }
        let last = last.unwrap_or_else(|| be("no answer"));
        if ctx.err().is_some() {
            return (Err(last), false);
        }
        let (missing, timed_out) = match &last {
            Error::Backend(b) => (b.status == 404, b.message.contains("timed out")),
            Error::Ctx(_) => (false, false),
        };
        (Err(last), retryable || missing || timed_out)
    }

    fn once(&self, ctx: &Ctx, body: &[u8], req: &Request, on: &mut dyn FnMut(StreamEvent)) -> (Result<Turn, Error>, bool) {
        let mut timeout = timeout_nanos(self.config.timeout_seconds);
        if !req.stream && req.max_tokens > 0 {
            // a non-streamed reply arrives all at once: a second per 40 tokens allowed on top (Go's comment has the numbers)
            timeout = timeout.wrapping_add(req.max_tokens.wrapping_mul(1_000_000_000) / 40);
        }
        let deadline: gohttp::Deadline = Rc::new(Cell::new(deadline_after(timeout)));
        let endpoint = self.endpoint();
        let fail = |f: Failure, retryable: bool| -> (Result<Turn, Error>, bool) {
            if let Some(e) = ctx.err() {
                return (Err(Error::Ctx(e)), false);
            }
            if Instant::now() >= deadline.get() {
                let d = crate::gotime::duration_string(timeout);
                return (Err(be(format!("request to {endpoint} timed out after {d}"))), false);
            }
            match f {
                Failure::Backend(b) => (Err(Error::Backend(b)), retryable),
                Failure::Transport(t) => (Err(be(format!("cannot reach backend at {}: {t}", self.config.base_url))), retryable),
            }
        };
        let auth = format!("Bearer {}", self.config.api_key);
        let headers = [("Authorization", auth.as_str()), ("Content-Type", "application/json")];
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
        if req.stream {
            let mut touch = || deadline.set(deadline_after(timeout));
            return match read_stream(&mut resp.body, self.max_response, self.max_line, &mut touch, on) {
                Ok(t) => (Ok(t), false),
                Err(StreamError::Backend(b)) => fail(Failure::Backend(b), false),
                Err(StreamError::Read(s)) => fail(Failure::Transport(s), false),
            };
        }
        let b = match read_bounded(&mut resp.body, self.max_response) {
            Ok(b) => b,
            Err(f) => return fail(f, false),
        };
        match parse_response(&b) {
            Ok(t) => (Ok(t), false),
            Err(e) => (Err(Error::Backend(e)), false),
        }
    }
}
