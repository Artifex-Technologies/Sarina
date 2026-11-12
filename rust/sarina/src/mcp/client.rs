//! Go: `mcp/client.go`: one connection to one Model Context Protocol server over stdio, JSON-RPC 2.0 one message a line.
//! Every read from the server is bounded: a line of at most `MAX_LINE_BYTES` (a longer one is dropped, never buffered),
//! the last `STDERR_KEEP` bytes of its error output, at most `MAX_REPLIES` answers to its own requests being written at
//! once (more are dropped; Go and Rust since 2026-11-12, before which Go started a writer per request without bound), at
//! most `MAX_PAGES` pages of a listing. A call times out or follows its context and leaves the connection usable; a server
//! that dies fails pending calls at once, naming its last words on stderr (waited for, at most 2 s).

use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gojson;
use crate::gostruct::{self, Field, Ty, Val};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What the client announces in initialize.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// One JSON-RPC line from a server at most; a longer one is dropped.
pub const MAX_LINE_BYTES: usize = 8 << 20;
/// How much of a server's stderr is kept for errors.
pub const STDERR_KEEP: usize = 2048;
/// Answers to a server's own requests being written at once.
pub const MAX_REPLIES: usize = 8;
const MAX_PAGES: usize = 10;

/// One configured stdio server (Go's `ServerDef`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerDef {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// the operator's overlay, applied after the filtered parent environment
    pub env: BTreeMap<String, String>,
    /// resolved; "" = the process's folder
    pub cwd: String,
    pub description: String,
    pub read_only: bool,
    pub read_only_tools: BTreeMap<String, bool>,
    pub timeout_seconds: f64,
    pub source: String,
    pub file: String,
}

/// A flag threads wait for.
#[derive(Default)]
struct Flag {
    set: Mutex<bool>,
    cv: Condvar,
}

impl Flag {
    fn set(&self) {
        *self.set.lock().unwrap_or_else(|p| p.into_inner()) = true;
        self.cv.notify_all();
    }
    fn is_set(&self) -> bool {
        *self.set.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// true when set within `d`
    fn wait(&self, d: Duration) -> bool {
        let g = self.set.lock().unwrap_or_else(|p| p.into_inner());
        let (g, _) = self.cv.wait_timeout_while(g, d, |s| !*s).unwrap_or_else(|p| p.into_inner());
        *g
    }
}

/// An answer to one of our calls.
struct Answer {
    result: Option<Vec<u8>>,
    error: Option<(i64, String)>,
}

struct Shared {
    name: String,
    w: Mutex<Option<Box<dyn Write + Send>>>,
    pending: Mutex<HashMap<i64, Sender<Answer>>>,
    next: AtomicI64,
    closed: Flag,
    err_done: Flag,
    reaped: Flag,
    tail: Mutex<Vec<u8>>,
    inflight: AtomicUsize,
    dropped: AtomicI64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A connection to a server; safe to share between threads.
pub struct Client {
    sh: Arc<Shared>,
    pid: Option<u32>,
    closing: AtomicBool,
}

static RPC_ERROR: Ty = Ty::Struct("mcp.rpcError", "", &[Field("code", &Ty::Int), Field("message", &Ty::Str)]);
static RPC_ERROR_PTR: Ty = Ty::Ptr(&RPC_ERROR);
// id, params and result are json.RawMessage in Go: any JSON, kept as written (read separately, `raw_members`)
static RPC: Ty = Ty::Struct("mcp.rpcMessage", "", &[Field("jsonrpc", &Ty::Str), Field("method", &Ty::Str), Field("error", &RPC_ERROR_PTR)]);

/// The raw text of the top-level members of a JSON object (already checked to parse), in order.
fn raw_members(b: &[u8]) -> Vec<(String, &[u8])> {
    fn ws(b: &[u8], mut i: usize) -> usize {
        while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        i
    }
    fn string_end(b: &[u8], mut i: usize) -> usize {
        i += 1;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'"' => return i + 1,
                _ => i += 1,
            }
        }
        i
    }
    fn value_end(b: &[u8], mut i: usize) -> usize {
        match b.get(i) {
            Some(b'"') => string_end(b, i),
            Some(b'{') | Some(b'[') => {
                let mut depth = 0usize;
                while i < b.len() {
                    match b[i] {
                        b'"' => {
                            i = string_end(b, i);
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                return i + 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i
            }
            _ => {
                while i < b.len() && !matches!(b[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                i
            }
        }
    }
    let mut out = Vec::new();
    let mut i = ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return out;
    }
    i = ws(b, i + 1);
    while i < b.len() && b[i] == b'"' {
        let ke = string_end(b, i).min(b.len());
        let key = match &gojson::parse(&b[i..ke]) {
            Ok(gojson::Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        i = ws(b, ke);
        i = ws(b, i + 1); // the colon
        let ve = value_end(b, i).min(b.len());
        out.push((key, &b[i.min(ve)..ve]));
        i = ws(b, ve);
        if b.get(i) == Some(&b',') {
            i = ws(b, i + 1);
        } else {
            break;
        }
    }
    out
}

/// Go's json.Marshal of a RawMessage: compacted, with `<`, `>`, `&`, U+2028 and U+2029 in strings escaped.
fn compact_html(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    let mut in_str = false;
    while i < raw.len() {
        let c = raw[i];
        if in_str {
            match c {
                b'\\' => {
                    out.extend_from_slice(&raw[i..(i + 2).min(raw.len())]);
                    i += 2;
                    continue;
                }
                b'"' => in_str = false,
                b'<' | b'>' | b'&' => {
                    out.extend_from_slice(format!("\\u00{c:02x}").as_bytes());
                    i += 1;
                    continue;
                }
                0xE2 if raw.get(i + 1) == Some(&0x80) && matches!(raw.get(i + 2), Some(0xA8) | Some(0xA9)) => {
                    out.extend_from_slice(if raw[i + 2] == 0xA8 { b"\\u2028" } else { b"\\u2029" });
                    i += 3;
                    continue;
                }
                _ => {}
            }
            out.push(c);
        } else {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => {}
                b'"' => {
                    in_str = true;
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Go's `parseID`: the id without surrounding quotes and spaces, as `strconv.ParseInt(s, 10, 64)` reads it.
fn parse_id(raw: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(raw).ok()?.trim_matches(|c| c == '"' || c == ' ');
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut n: i64 = 0;
    for c in digits.bytes() {
        n = n.checked_mul(10)?;
        n = if neg { n.checked_sub(i64::from(c - b'0'))? } else { n.checked_add(i64::from(c - b'0'))? };
    }
    Some(n)
}

/// Go's `overlayEnv`: the base environment without the names the overlay sets, then the overlay.
pub fn overlay_env(base: &[String], extra: &BTreeMap<String, String>) -> Vec<String> {
    if extra.is_empty() {
        return base.to_vec();
    }
    let mut out: Vec<String> = base
        .iter()
        .filter(|kv| {
            let k = match kv.find('=') {
                Some(i) if i > 0 => &kv[..i],
                _ => kv.as_str(),
            };
            !extra.contains_key(k)
        })
        .cloned()
        .collect();
    out.extend(extra.iter().map(|(k, v)| format!("{k}={v}")));
    out
}

/// Go's `exec.LookPath` with the parent's PATH: a name with a slash as it is, else the first executable file on PATH.
fn look_path(name: &str) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    if name.contains('/') {
        return Ok(name.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = format!("{dir}/{name}");
        if let Ok(m) = std::fs::metadata(&p) {
            if !m.is_dir() && m.permissions().mode() & 0o111 != 0 {
                if !p.starts_with('/') {
                    return Err(format!("exec: {}: cannot run executable found relative to current directory", crate::gotext::quote(name)));
                }
                return Ok(p);
            }
        }
    }
    Err(format!("exec: {}: executable file not found in $PATH", crate::gotext::quote(name)))
}

/// Go's `lastLines`.
fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = crate::gotext::trim_space(s).split('\n').collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

fn write_err(e: &std::io::Error) -> String {
    format!("write |1: {}", crate::goerr::text(e))
}

impl Shared {
    fn add_tail(&self, b: &[u8]) {
        let mut t = lock(&self.tail);
        t.extend_from_slice(b);
        if t.len() > STDERR_KEEP {
            let cut = t.len() - STDERR_KEEP;
            t.drain(..cut);
        }
    }

    fn tail(&self) -> String {
        crate::gotext::trim_space(&String::from_utf8_lossy(&lock(&self.tail))).to_string()
    }

    fn send(&self, line: &str) -> Result<(), String> {
        let mut w = lock(&self.w);
        let Some(w) = w.as_mut() else { return Err("write |1: file already closed".into()) };
        let mut b = line.as_bytes().to_vec();
        b.push(b'\n');
        w.write_all(&b).and_then(|_| w.flush()).map_err(|e| write_err(&e))
    }

    fn failure(&self, msg: &str) -> String {
        let mut msg = msg.to_string();
        let t = self.tail();
        if !t.is_empty() {
            msg.push_str(&format!(" (server stderr: {})", last_lines(&t, 5)));
        }
        format!("MCP server {}: {msg}", crate::gotext::quote(&self.name))
    }

    fn notify(&self, method: &str, params: Option<&Any>) -> Result<(), String> {
        let mut line = String::from("{\"jsonrpc\":\"2.0\",\"method\":");
        gojson::write_str(&mut line, method, true);
        if let Some(p) = params {
            line.push_str(",\"params\":");
            line.push_str(&gojson::marshal_any(p).map_err(|e| e.0)?);
        }
        line.push('}');
        self.send(&line)
    }

    fn handle(self: &Arc<Self>, line: &[u8]) {
        let (v, err) = gostruct::unmarshal(&RPC, line);
        if err.is_some() {
            return; // not a JSON-RPC object: ignored, as servers may log to stdout
        }
        let Val::Struct(f) = v else { return };
        let method = match &f[1] {
            Val::Str(s) => s.clone(),
            _ => String::new(),
        };
        let (mut id, mut result) = (None, None);
        for (k, raw) in raw_members(line) {
            if gostruct::key_matches(&k, "id") {
                id = Some(raw);
            } else if gostruct::key_matches(&k, "result") {
                result = Some(raw);
            }
        }
        match (method.is_empty(), id) {
            (false, Some(raw)) => {
                // a request from the server: only ping is answered; the answer is written on its own thread (the read loop
                // never blocks on a write) and at most MAX_REPLIES at once
                let id = compact_html(raw);
                let reply = if method == "ping" {
                    format!("{{\"id\":{id},\"jsonrpc\":\"2.0\",\"result\":{{}}}}")
                } else {
                    format!(
                        "{{\"error\":{{\"code\":-32601,\"message\":\"method not supported by this client\"}},\"id\":{id},\"jsonrpc\":\"2.0\"}}"
                    )
                };
                if self.inflight.fetch_add(1, Ordering::SeqCst) >= MAX_REPLIES {
                    self.inflight.fetch_sub(1, Ordering::SeqCst);
                    self.dropped.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                let sh = self.clone();
                std::thread::spawn(move || {
                    let _ = sh.send(&reply);
                    sh.inflight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            (false, None) => {} // a notification
            (true, Some(raw)) => {
                let Some(id) = parse_id(raw) else { return };
                let Some(tx) = lock(&self.pending).remove(&id) else { return };
                let error = match &f[2] {
                    Val::Ptr(Some(e)) => match e.as_ref() {
                        Val::Struct(ef) => Some((
                            match &ef[0] {
                                Val::Int(n) => *n,
                                _ => 0,
                            },
                            match &ef[1] {
                                Val::Str(s) => s.clone(),
                                _ => String::new(),
                            },
                        )),
                        _ => None,
                    },
                    _ => None,
                };
                let _ = tx.send(Answer { result: result.map(<[u8]>::to_vec), error });
            }
            (true, None) => {}
        }
    }

    /// The read loop: lines of at most MAX_LINE_BYTES (a longer one is dropped as it comes); at the end every pending
    /// call is failed.
    fn read_loop(self: Arc<Self>, r: impl Read) {
        let mut br = BufReader::with_capacity(64 << 10, r);
        let mut line: Vec<u8> = Vec::new();
        let mut dropping = false;
        loop {
            let buf = match br.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            if buf.is_empty() {
                // Go's ReadLine gives a last line without its newline too
                if !dropping && !line.is_empty() {
                    let l = std::mem::take(&mut line);
                    self.handle(&l);
                }
                break;
            }
            let (take, end) = match buf.iter().position(|&c| c == b'\n') {
                Some(i) => (i + 1, true),
                None => (buf.len(), false),
            };
            if !dropping {
                line.extend_from_slice(&buf[..take]);
                let content = line.len() - usize::from(end) - usize::from(end && line.len() >= 2 && line[line.len() - 2] == b'\r');
                if content > MAX_LINE_BYTES {
                    dropping = true;
                    line = Vec::new();
                }
            }
            br.consume(take);
            if end {
                if !dropping {
                    let mut l = std::mem::take(&mut line);
                    l.pop();
                    if l.last() == Some(&b'\r') {
                        l.pop();
                    }
                    if !l.is_empty() {
                        self.handle(&l);
                    }
                }
                line.clear();
                dropping = false;
            }
        }
        self.closed.set();
        lock(&self.pending).clear();
    }
}

/// A tool a server offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// None: Go's nil map
    pub input_schema: Option<BTreeMap<String, Any>>,
}

/// A resource a server offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceInfo {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

/// A tools/call answer flattened to text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

static TOOL: Ty = Ty::Struct("", "", &[Field("name", &Ty::Str), Field("description", &Ty::Str), Field("inputSchema", &MAP_ANY)]);
static TOOLS: Ty = Ty::Slice(&TOOL);
static TOOL_LIST: Ty = Ty::Struct("", "", &[Field("tools", &TOOLS), Field("nextCursor", &Ty::Str)]);
static RESOURCE: Ty =
    Ty::Struct("", "", &[Field("uri", &Ty::Str), Field("name", &Ty::Str), Field("description", &Ty::Str), Field("mimeType", &Ty::Str)]);
static RESOURCES: Ty = Ty::Slice(&RESOURCE);
static RESOURCE_LIST: Ty = Ty::Struct("", "", &[Field("resources", &RESOURCES), Field("nextCursor", &Ty::Str)]);
static MAP_ANY: Ty = Ty::Map(&Ty::Any);
static MAPS: Ty = Ty::Slice(&MAP_ANY);
static CALL: Ty = Ty::Struct("", "", &[Field("content", &MAPS), Field("structuredContent", &Ty::Any), Field("isError", &Ty::Bool)]);
static READ: Ty = Ty::Struct("", "", &[Field("contents", &MAPS)]);

fn s(v: &Val) -> String {
    match v {
        Val::Str(x) => x.clone(),
        _ => String::new(),
    }
}

fn map_of(v: &Val) -> Option<BTreeMap<String, Any>> {
    match v {
        Val::Map(Some(m)) => Some(
            m.iter()
                .map(|(k, x)| {
                    (
                        k.clone(),
                        match x {
                            Val::Any(a) => a.clone(),
                            _ => Any::Null,
                        },
                    )
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Go's `json.Unmarshal(raw, &v) == nil` for a result (none reads as "unexpected end of JSON input").
fn read_result(t: &Ty, raw: Option<&[u8]>) -> Option<Vec<Val>> {
    match gostruct::unmarshal(t, raw.unwrap_or(b"")) {
        (Val::Struct(f), None) => Some(f),
        _ => None,
    }
}

/// Go's `renderContent`: one content item as text; binary payloads are named, not inlined.
fn render_content(item: Option<&BTreeMap<String, Any>>) -> String {
    let Some(m) = item else { return "null".into() };
    let st = |k: &str| match m.get(k) {
        Some(Any::String(x)) => Some(x.clone()),
        _ => None,
    };
    if let Some(t) = st("text") {
        return t;
    }
    let or = |x: Option<String>, d: &str| x.filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
    let (typ, mime) = (st("type"), st("mimeType"));
    if let Some(blob) = st("blob") {
        return format!("[binary content omitted: {}, {} base64 characters]", or(mime, "unknown type"), blob.len());
    }
    if let Some(data) = st("data") {
        return format!("[{} content omitted: {}, {} base64 characters]", or(typ, "binary"), or(mime, "unknown type"), data.len());
    }
    if let Some(Any::Object(r)) = m.get("resource") {
        return render_content(Some(r));
    }
    if let Some(uri) = st("uri") {
        return format!("[resource {uri}]");
    }
    gojson::marshal_any(&Any::Object(m.clone())).unwrap_or_default()
}

impl Client {
    /// Go's `Spawn`: starts the server's command (its own process group) with `env` (already filtered) and the
    /// definition's overlay, and connects to it.
    pub fn spawn(def: &ServerDef, env: &[String]) -> Result<Client, String> {
        use std::os::unix::process::CommandExt;
        let start_err = |e: String| format!("cannot start MCP server {} ({}): {e}", crate::gotext::quote(&def.name), def.command);
        let path = look_path(&def.command).map_err(start_err)?;
        let mut cmd = Command::new(&path);
        cmd.args(&def.args).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
        for kv in overlay_env(env, &def.env) {
            if let Some((k, v)) = kv.split_once('=') {
                if !k.is_empty() {
                    cmd.env(k, v);
                }
            }
        }
        if !def.cwd.is_empty() {
            cmd.current_dir(&def.cwd);
        }
        let mut child: Child = cmd.spawn().map_err(|e| start_err(format!("fork/exec {path}: {}", crate::goerr::text(&e))))?;
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(mut stderr)) = (stdin, stdout, stderr) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(start_err("no pipes".into()));
        };
        let c = Client::with(&def.name, stdout, Box::new(stdin));
        let pid = child.id();
        let sh = c.sh.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => sh.add_tail(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            sh.err_done.set();
        });
        let sh = c.sh.clone();
        std::thread::spawn(move || {
            // Go's reaper: once the output and the error output have ended, wait for the process; its stdin is closed
            // then (exec.Cmd.Wait closes the pipes it made)
            while !sh.closed.wait(Duration::from_secs(3600)) {}
            while !sh.err_done.wait(Duration::from_secs(3600)) {}
            let _ = child.wait();
            if let Ok(mut w) = sh.w.try_lock() {
                *w = None;
            }
            sh.reaped.set();
        });
        let mut c = c;
        c.pid = Some(pid);
        Ok(c)
    }

    fn with(name: &str, r: impl Read + Send + 'static, w: Box<dyn Write + Send>) -> Client {
        let sh = Arc::new(Shared {
            name: name.to_string(),
            w: Mutex::new(Some(w)),
            pending: Mutex::new(HashMap::new()),
            next: AtomicI64::new(0),
            closed: Flag::default(),
            err_done: Flag::default(),
            reaped: Flag::default(),
            tail: Mutex::new(Vec::new()),
            inflight: AtomicUsize::new(0),
            dropped: AtomicI64::new(0),
        });
        let s2 = sh.clone();
        std::thread::spawn(move || s2.read_loop(r));
        Client { sh, pid: None, closing: AtomicBool::new(false) }
    }

    /// Go's `NewPipeClient`: a server over an existing pipe pair (no process).
    pub fn pipe(name: &str, r: impl Read + Send + 'static, w: Box<dyn Write + Send>) -> Client {
        let c = Client::with(name, r, w);
        c.sh.err_done.set();
        c.sh.reaped.set();
        c
    }

    /// The end of the server's stderr.
    pub fn stderr_tail(&self) -> String {
        self.sh.tail()
    }

    /// Whether the server's output is still open.
    pub fn alive(&self) -> bool {
        !self.sh.closed.is_set()
    }

    /// Answers to the server's own requests not written because MAX_REPLIES were in flight.
    pub fn dropped(&self) -> i64 {
        self.sh.dropped.load(Ordering::SeqCst)
    }

    /// Go's `Notify`: a notification (no answer expected).
    pub fn notify(&self, method: &str, params: Option<&Any>) -> Result<(), String> {
        self.sh.notify(method, params)
    }

    /// Go's `Call`: one request and its answer (the result's JSON as the server wrote it; None when it sent none). A
    /// JSON-RPC error is an error; a timeout or an ended context sends notifications/cancelled and leaves the connection
    /// usable.
    pub fn call(&self, ctx: &Ctx, method: &str, params: Option<&Any>, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        let sh = &self.sh;
        let id = sh.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx): (Sender<Answer>, Receiver<Answer>) = mpsc::channel();
        // after the read loop has ended nothing answers (as in Go, the send fails first)
        lock(&sh.pending).insert(id, tx);
        let forget = || {
            lock(&sh.pending).remove(&id);
        };
        let mut line = format!("{{\"id\":{id},\"jsonrpc\":\"2.0\",\"method\":");
        gojson::write_str(&mut line, method, true);
        if let Some(p) = params {
            line.push_str(",\"params\":");
            line.push_str(&gojson::marshal_any(p).map_err(|e| e.0)?);
        }
        line.push('}');
        // written on its own thread, so a server that stopped reading cannot make this call outlive its timeout
        let (stx, srx) = mpsc::channel();
        let s2 = sh.clone();
        std::thread::spawn(move || {
            let _ = stx.send(s2.send(&line));
        });
        let mut sent = Some(srx);
        let by = (!timeout.is_zero()).then(|| Instant::now() + timeout);
        loop {
            if let Some(r) = &sent {
                match r.try_recv() {
                    Ok(Err(e)) => {
                        forget();
                        return Err(sh.failure(&format!("could not send {method}: {e}")));
                    }
                    Ok(Ok(())) | Err(TryRecvError::Disconnected) => sent = None,
                    Err(TryRecvError::Empty) => {}
                }
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(a) => {
                    if let Some((code, msg)) = a.error {
                        return Err(format!("MCP {method} failed on server {}: {msg} (code {code})", crate::gotext::quote(&sh.name)));
                    }
                    return Ok(a.result);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // the server's last words on stderr may still be on their way (at most 2 s)
                    sh.err_done.wait(Duration::from_secs(2));
                    return Err(sh.failure(&format!("server exited during {method}")));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if by.is_some_and(|b| Instant::now() >= b) {
                forget();
                self.cancel(id);
                let d = crate::gotime::duration_string(timeout.as_nanos().min(i64::MAX as u128) as i64);
                return Err(sh.failure(&format!("no answer to {method} within {d}")));
            }
            if let Some(e) = ctx.err() {
                forget();
                self.cancel(id);
                return Err(e.to_string());
            }
        }
    }

    fn cancel(&self, id: i64) {
        let sh = self.sh.clone();
        std::thread::spawn(move || {
            let p = Any::Object(BTreeMap::from([
                ("requestId".to_string(), Any::Int(id)),
                ("reason".to_string(), Any::String("client gave up".into())),
            ]));
            let _ = sh.notify("notifications/cancelled", Some(&p));
        });
    }

    /// Go's `Initialize`: the handshake.
    pub fn initialize(&self, ctx: &Ctx, timeout: Duration) -> Result<(), String> {
        let p = Any::Object(BTreeMap::from([
            ("protocolVersion".to_string(), Any::String(PROTOCOL_VERSION.into())),
            ("capabilities".to_string(), Any::Object(BTreeMap::new())),
            (
                "clientInfo".to_string(),
                Any::Object(BTreeMap::from([
                    ("name".to_string(), Any::String("sarina".into())),
                    ("version".to_string(), Any::String(crate::version::VERSION.into())),
                ])),
            ),
        ]));
        self.call(ctx, "initialize", Some(&p), timeout)?;
        self.notify("notifications/initialized", None)
    }

    /// Go's `Close`: the server's stdin closed, a second for it to leave by itself, then its process group killed.
    pub fn close(&self) {
        if self.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        // a write blocked on a server that does not read holds the lock: the kill below ends it
        if let Ok(mut w) = self.sh.w.try_lock() {
            *w = None;
        }
        if !self.sh.closed.wait(Duration::from_secs(1)) {
            if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()).filter(|&p| p > 0) {
                // SAFETY: kill(2) on our own child's process group (made with process_group(0)); the child is not reaped
                // yet (the reaper waits for its output to end), so the id is still ours
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        self.sh.reaped.wait(Duration::from_secs(3));
    }

    fn page(&self, ctx: &Ctx, method: &str, cursor: &str, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        let params = (!cursor.is_empty()).then(|| Any::Object(BTreeMap::from([("cursor".to_string(), Any::String(cursor.into()))])));
        self.call(ctx, method, params.as_ref(), timeout)
    }

    /// Go's `ListTools`: the pages followed (at most ten). On an error, the tools read so far come with it.
    pub fn list_tools(&self, ctx: &Ctx, timeout: Duration) -> (Vec<ToolInfo>, Option<String>) {
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..MAX_PAGES {
            let raw = match self.page(ctx, "tools/list", &cursor, timeout) {
                Ok(r) => r,
                Err(e) => return (out, Some(e)),
            };
            let Some(f) = read_result(&TOOL_LIST, raw.as_deref()) else {
                let e = format!("MCP server {} returned a tools/list result that is not an object", crate::gotext::quote(&self.sh.name));
                return (out, Some(e));
            };
            if let Val::Slice(Some(tools)) = &f[0] {
                for t in tools {
                    let Val::Struct(tf) = t else { continue };
                    let name = s(&tf[0]);
                    if !name.is_empty() {
                        out.push(ToolInfo { name, description: s(&tf[1]), input_schema: map_of(&tf[2]) });
                    }
                }
            }
            cursor = s(&f[1]);
            if cursor.is_empty() {
                break;
            }
        }
        (out, None)
    }

    /// Go's `ListResources`.
    pub fn list_resources(&self, ctx: &Ctx, timeout: Duration) -> (Vec<ResourceInfo>, Option<String>) {
        let mut out = Vec::new();
        let mut cursor = String::new();
        for _ in 0..MAX_PAGES {
            let raw = match self.page(ctx, "resources/list", &cursor, timeout) {
                Ok(r) => r,
                Err(e) => return (out, Some(e)),
            };
            let Some(f) = read_result(&RESOURCE_LIST, raw.as_deref()) else {
                let e =
                    format!("MCP server {} returned a resources/list result that is not an object", crate::gotext::quote(&self.sh.name));
                return (out, Some(e));
            };
            if let Val::Slice(Some(list)) = &f[0] {
                for r in list {
                    let Val::Struct(rf) = r else { continue };
                    let uri = s(&rf[0]);
                    if !uri.is_empty() {
                        out.push(ResourceInfo { uri, name: s(&rf[1]), description: s(&rf[2]), mime_type: s(&rf[3]) });
                    }
                }
            }
            cursor = s(&f[1]);
            if cursor.is_empty() {
                break;
            }
        }
        (out, None)
    }

    /// Go's `CallTool`: a tool run, its content rendered as text.
    pub fn call_tool(&self, ctx: &Ctx, name: &str, args: Option<&BTreeMap<String, Any>>, timeout: Duration) -> Result<CallResult, String> {
        let p = Any::Object(BTreeMap::from([
            ("name".to_string(), Any::String(name.into())),
            ("arguments".to_string(), Any::Object(args.cloned().unwrap_or_default())),
        ]));
        let raw = self.call(ctx, "tools/call", Some(&p), timeout)?;
        let Some(f) = read_result(&CALL, raw.as_deref()) else {
            return Err(format!("MCP server {} returned a tools/call result that is not an object", crate::gotext::quote(&self.sh.name)));
        };
        let mut parts = Vec::new();
        if let Val::Slice(Some(items)) = &f[0] {
            for it in items {
                parts.push(render_content(map_of(it).as_ref()));
            }
        }
        if let Val::Any(sc) = &f[1] {
            if *sc != Any::Null {
                if let Ok(b) = gojson::marshal_any(sc) {
                    parts.push(gojson::indent(&b, "  "));
                }
            }
        }
        let mut text = parts.join("\n\n");
        if crate::gotext::trim_space(&text).is_empty() {
            text = "(the tool returned no content)".into();
        }
        Ok(CallResult { text, is_error: matches!(f[2], Val::Bool(true)) })
    }

    /// Go's `ReadResource`: a resource's contents as text.
    pub fn read_resource(&self, ctx: &Ctx, uri: &str, timeout: Duration) -> Result<String, String> {
        let p = Any::Object(BTreeMap::from([("uri".to_string(), Any::String(uri.into()))]));
        let raw = self.call(ctx, "resources/read", Some(&p), timeout)?;
        let Some(f) = read_result(&READ, raw.as_deref()) else { return Err("empty result".into()) };
        let mut parts = Vec::new();
        if let Val::Slice(Some(items)) = &f[0] {
            for it in items {
                parts.push(render_content(map_of(it).as_ref()));
            }
        }
        Ok(parts.join("\n\n"))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.pid.is_some() {
            self.close();
        }
    }
}

/// Any bytes as a line from a server, as the read loop handles it (for the fuzz target): no panic, nothing answered.
pub fn fuzz_line(data: &[u8]) {
    let _ = raw_members(data);
    let _ = compact_html(data);
    let _ = parse_id(data);
    let c = Client::with("fuzz", std::io::empty(), Box::new(std::io::sink()));
    c.sh.handle(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_raw_members() {
        assert_eq!(parse_id(b"\"+4\""), Some(4));
        assert_eq!(parse_id(b"\" 7\""), Some(7));
        assert_eq!(parse_id(b"null"), None);
        assert_eq!(parse_id(b"9223372036854775808"), None);
        assert_eq!(parse_id(b"-9223372036854775808"), Some(i64::MIN));
        let m = raw_members(br#" { "a" : [1, "]", {"b":"}"}] , "c":"x\"y" ,"d": 5 } "#);
        let got: Vec<(String, String)> = m.iter().map(|(k, v)| (k.clone(), String::from_utf8_lossy(v).into_owned())).collect();
        assert_eq!(got, vec![("a".into(), r#"[1, "]", {"b":"}"}]"#.into()), ("c".into(), r#""x\"y""#.into()), ("d".into(), "5".into())]);
        let bs = '\\';
        assert_eq!(compact_html(br#"{ "x" : "<a> & \"" }"#), format!(r#"{{"x":"{bs}u003ca{bs}u003e {bs}u0026 \""}}"#));
    }
}
