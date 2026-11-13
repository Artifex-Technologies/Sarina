//! Go: `gui`: the single-operator web console (`sarina gui`, port 8765).
//!
//! One workspace, one agent run at a time, loopback only. Because a page the operator visits could otherwise drive an
//! agent that may have shell and write access, every request is checked: the Host must be a loopback name (DNS
//! rebinding), an Origin must be this origin, mutations must be JSON, and the API needs a random per-launch token that
//! only the page served to the operator carries. The page is built in (the files of `go/gui/static`) and loads nothing
//! from the network. The HTTP side is `gohttpd` (Go's net/http server), with Go's ServeMux patterns and FileServer.

pub mod api;
pub mod files;
pub mod run;
pub mod state;

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::cli::Getenv;
use crate::goany::Any;
use crate::gohttpd::{self, BodyErr, Exchange, Request, Response};
use crate::gojson::Value;
use crate::gostruct::{self, Ty, Val};
use crate::gotext;
use crate::service::{parse_ip, split_host_port, BODY_READ_TIMEOUT};
use crate::tiers::config::EscalationOptions;

use run::{Approval, Run};
use state::{ApiErr, State};

const TOKEN_HEADER: &str = "X-Sarina-Token";
pub const COOKIE_NAME: &str = "sarina_gui";
const MAX_BODY: usize = 8 << 20;
const TOKEN_MARKER: &[u8] = b"__SARINA_TOKEN__";
const MAX_EVENTS: usize = 4000;

/// Go's `Config`.
#[derive(Clone)]
pub struct Config {
    pub initial: State,
    /// per-launch token; generated when empty
    pub token: String,
    /// "" = the user's real home
    pub home: String,
    pub getenv: Getenv,
    pub discover: bool,
    /// unanswered approvals are denied after this; zero = 5 minutes
    pub approval_timeout: Duration,
    /// adds the claude_cli tier (operator flags only: not part of State, so the page cannot enable it)
    pub escalation: Option<EscalationOptions>,
}

/// One event of the stream, its JSON written when it was emitted.
#[derive(Clone)]
pub struct Ev {
    pub seq: i64,
    pub typ: String,
    pub run_id: String,
    pub json: String,
}

/// What the server's lock guards (Go's `Server` fields under `mu`).
pub struct Inner {
    pub st: State,
    pub run: Option<Arc<Run>>,
    pub seq: i64,
    pub events: VecDeque<Ev>,
    pub approvals: BTreeMap<String, Arc<Approval>>,
    pub next_app: i64,
}

/// Go's `Server`.
pub struct Server {
    pub cfg: Config,
    pub token: String,
    inner: Mutex<Inner>,
    /// woken on every event (Go's `notify` channel, closed and remade)
    notify: Condvar,
}

impl Server {
    /// Go's `New`.
    pub fn new(mut cfg: Config) -> Server {
        if cfg.approval_timeout.is_zero() {
            cfg.approval_timeout = Duration::from_secs(300);
        }
        let token = if cfg.token.is_empty() { random_hex(24) } else { cfg.token.clone() };
        let st = cfg.initial.clone();
        Server {
            cfg,
            token,
            inner: Mutex::new(Inner { st, run: None, seq: 0, events: VecDeque::new(), approvals: BTreeMap::new(), next_app: 0 }),
            notify: Condvar::new(),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Waits at most `d` for the next event (Go's `<-wake`), the lock given back.
    pub fn wait_event<'a>(&self, g: MutexGuard<'a, Inner>, d: Duration) -> MutexGuard<'a, Inner> {
        self.notify.wait_timeout(g, d).unwrap_or_else(|p| p.into_inner()).0
    }

    /// Go's `emit`: an event appended (its JSON with the keys in order) and every listener woken.
    pub fn emit(&self, run_id: &str, typ: &str, fields: Vec<(&str, String)>) {
        let mut g = self.lock();
        g.seq += 1;
        let seq = g.seq;
        let mut m: BTreeMap<&str, String> = BTreeMap::new();
        m.insert("seq", seq.to_string());
        m.insert("type", json_str(typ));
        m.insert("run_id", json_str(run_id));
        m.insert("at", json_str(&now_rfc3339()));
        for (k, v) in fields {
            m.insert(k, v);
        }
        g.events.push_back(Ev { seq, typ: typ.into(), run_id: run_id.into(), json: raw_obj(&m) });
        while g.events.len() > MAX_EVENTS {
            g.events.pop_front();
        }
        drop(g);
        self.notify.notify_all();
    }

    /// Go's `scrubber`: the current API key and the token taken out.
    pub fn scrubber(&self) -> crate::runkit::Scrub {
        let key = self.lock().st.api_key.clone();
        crate::runkit::scrubber(&[&key, &self.token])
    }

    /// Go's `settings`.
    pub fn settings(&self) -> State {
        self.lock().st.clone()
    }

    /// Go's `snapshot`.
    fn snapshot(&self) -> Any {
        let g = self.lock();
        let mut snap = g.st.snapshot();
        snap.insert("busy".into(), Any::Bool(g.run.as_ref().is_some_and(|r| r.running())));
        Any::Object(snap)
    }

    /// Go's `homeDir`.
    pub fn home_dir(&self) -> String {
        if !self.cfg.home.is_empty() {
            return self.cfg.home.clone();
        }
        std::env::var_os("HOME").map(|v| gotext::lossy(std::os::unix::ffi::OsStrExt::as_bytes(v.as_os_str()))).unwrap_or_default()
    }

    /// The whole HTTP surface: Go's `guard` around the mux.
    pub fn handle(self: &Arc<Self>, ex: &mut Exchange) -> Response {
        if ex.req.content_length != 0 {
            ex.set_read_deadline(Some(Instant::now() + BODY_READ_TIMEOUT));
        }
        let mut w = Response::new(200);
        w.set(
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        );
        w.set("X-Content-Type-Options", "nosniff");
        w.set("X-Frame-Options", "DENY");
        w.set("Referrer-Policy", "no-referrer");
        w.set("Cache-Control", "no-store");
        if !loopback_name(&hostname(&gotext::lossy(&ex.req.host))) {
            detail(&mut w, 403, "unexpected Host header: this console only answers on a loopback address");
            return w;
        }
        let site = ex.req.header("Sec-Fetch-Site");
        if !site.is_empty() && site != b"same-origin" && site != b"none" {
            detail(&mut w, 403, "cross-site request refused");
            return w;
        }
        let origin = ex.req.header("Origin");
        if !origin.is_empty() {
            let ok = matches!(crate::gourl::parse_bytes(origin), Some(u) if u.scheme == "http" && u.host == ex.req.host);
            if !ok {
                detail(&mut w, 403, "cross-origin request refused");
                return w;
            }
        }
        if ex.req.url.path.starts_with(b"/api/") {
            if !same_token(&self.token, ex.req.header(TOKEN_HEADER)) {
                detail(&mut w, 401, "missing or invalid console token; open the console from the URL printed at startup");
                return w;
            }
            if ex.req.method != "GET" && ex.req.method != "HEAD" && media_type(ex.req.header("Content-Type")) != "application/json" {
                detail(&mut w, 415, "requests that change anything must be application/json");
                return w;
            }
        }
        self.dispatch(ex, &mut w);
        w
    }

    fn dispatch(self: &Arc<Self>, ex: &mut Exchange, w: &mut Response) {
        use Route::*;
        match route(&ex.req) {
            Star => {
                if ex.req.proto_at_least(1, 1) {
                    w.set("Connection", "close");
                }
                w.status = 400;
            }
            NotFound => {
                // Go's mux: no pattern for this method, but GET (and so HEAD) has one for "/"
                w.set("Allow", "GET, HEAD");
                files::error(w, "Method Not Allowed", 405);
            }
            Redirect(to) => {
                let r = gohttpd::redirect(&ex.req, &to);
                w.status = r.status;
                for (k, v) in r.header {
                    w.header.insert(k, v);
                }
                w.body = r.body;
            }
            Index => self.index(ex, w),
            Static => files::serve(w, &ex.req),
            GetState => write_json(w, 200, &self.snapshot()),
            PostState => self.post_state(ex, w),
            Clear => {
                self.lock().st.active_session_id.clear();
                write_json(w, 200, &self.snapshot());
            }
            Backend => self.backend(w),
            Chat => self.chat(ex, w),
            Cancel => self.cancel(w),
            RunStatus => self.run_status(w),
            Events => self.event_stream(ex, w),
            Approvals => self.list_approvals(w),
            AnswerApproval(id) => self.answer_approval(ex, w, &id),
            Sessions => self.list_sessions(&ex.req, w),
            GetSession(id) => self.get_session(w, &id),
            DeleteSession(id) => self.delete_session(w, &id),
            FileHistory => self.file_history(&ex.req, w),
            Slash => self.slash_commands(w),
            Plan => self.get_plan(w),
            Tasks => self.get_tasks(w),
            Memory => self.get_memory(w),
            ReviewMemory(id) => self.review_memory(ex, w, &id),
            Instructions => self.list_instructions(w),
            ReadInstruction => self.read_instruction(&ex.req, w),
            WriteInstruction => self.write_instruction(ex, w),
            DeleteInstruction => self.delete_instruction(&ex.req, w),
            Tiers => self.tier_status(w),
            CatchAll => detail(w, 404, "not found"),
        }
    }

    /// Go's `index`: the page with the token in it, for a request that carries the token (query, then cookie).
    fn index(&self, ex: &Exchange, w: &mut Response) {
        let mut ok = same_token(&self.token, query_get(&ex.req.url.raw_query, "t").as_bytes());
        if !ok {
            if let Some(c) = cookie(&ex.req, COOKIE_NAME) {
                ok = same_token(&self.token, c.as_bytes());
            }
        }
        if !ok {
            w.set("Content-Type", "text/plain; charset=utf-8");
            w.status = 403;
            w.body = b"This console needs the link printed when it started (it carries a one-time token).\n".to_vec();
            return;
        }
        w.header.insert("Set-Cookie".into(), vec![format!("{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Strict", self.token)]);
        w.set("Content-Type", "text/html; charset=utf-8");
        let page = files::INDEX_HTML;
        let mut body = page.to_vec();
        if let Some(i) = page.windows(TOKEN_MARKER.len()).position(|x| x == TOKEN_MARKER) {
            body = [&page[..i], self.token.as_bytes(), &page[i + TOKEN_MARKER.len()..]].concat();
        }
        w.body = body;
        w.buffered = true; // io.WriteString
    }

    fn post_state(&self, ex: &mut Exchange, w: &mut Response) {
        let Some(body) = read_json(ex, w) else { return };
        let mut patch: BTreeMap<String, Value> = BTreeMap::new();
        match &crate::gojson::parse(&body) {
            Err(_) => return fail(w, &state::wrong_type("body", "a JSON object")),
            Ok(Value::Object(members)) => {
                for (k, v) in members {
                    patch.insert(k.clone(), v.clone());
                }
            }
            Ok(Value::Null) => {}
            Ok(_) => return fail(w, &state::wrong_type("", "map[string]jsontext.Value")),
        }
        let confirmed = matches!(patch.remove("confirm_permissions"), Some(Value::Bool(true)));
        let r = self.lock().st.apply(&patch, confirmed);
        match r {
            Ok(()) => write_json(w, 200, &self.snapshot()),
            Err(e) => fail(w, &e),
        }
    }
}

// ---- the mux ------------------------------------------------------------------------------------------------------------

/// The console's routes (Go's ServeMux patterns).
#[derive(Debug, PartialEq)]
enum Route {
    Index,
    Static,
    GetState,
    PostState,
    Clear,
    Backend,
    Chat,
    Cancel,
    RunStatus,
    Events,
    Approvals,
    AnswerApproval(String),
    Sessions,
    GetSession(String),
    DeleteSession(String),
    FileHistory,
    Slash,
    Plan,
    Tasks,
    Memory,
    ReviewMemory(String),
    Instructions,
    ReadInstruction,
    WriteInstruction,
    DeleteInstruction,
    Tiers,
    /// the "/" pattern
    CatchAll,
    /// a redirect (307) to the cleaned path or the path with its slash
    Redirect(String),
    /// no pattern for the method (a CONNECT without a path): Go's 405 naming the methods "/" has
    NotFound,
    /// RequestURI "*"
    Star,
}

/// The pattern a method and an escaped path match, and whether the match is exact (Go's `exactMatch`: a multi wildcard
/// that matched nothing).
fn match_path(method: &str, path: &[u8]) -> (Route, bool) {
    use Route::*;
    let segs = gohttpd::segments(path);
    let s: Vec<&[u8]> = segs.iter().map(|x| x.as_slice()).collect();
    let get = method == "GET" || method == "HEAD";
    let one = |x: &[u8]| (x != b"/").then(|| String::from_utf8_lossy(x).into_owned());
    let hit = match (method, s.as_slice()) {
        (_, [b"/"]) if get => Some(Index),
        (_, [b"static", ..]) if get && s.len() >= 2 => {
            let exact = s.len() == 2 && s[1] == b"/";
            return (Static, exact);
        }
        (_, [b"api", b"state"]) if get => Some(GetState),
        ("POST", [b"api", b"state"]) => Some(PostState),
        ("POST", [b"api", b"clear"]) => Some(Clear),
        (_, [b"api", b"backend"]) if get => Some(Backend),
        ("POST", [b"api", b"chat"]) => Some(Chat),
        ("POST", [b"api", b"chat", b"cancel"]) => Some(Cancel),
        (_, [b"api", b"run"]) if get => Some(RunStatus),
        (_, [b"api", b"events"]) if get => Some(Events),
        (_, [b"api", b"approvals"]) if get => Some(Approvals),
        ("POST", [b"api", b"approvals", id]) => one(id).map(AnswerApproval),
        (_, [b"api", b"sessions"]) if get => Some(Sessions),
        (_, [b"api", b"sessions", id]) if get => one(id).map(GetSession),
        ("DELETE", [b"api", b"sessions", id]) => one(id).map(DeleteSession),
        (_, [b"api", b"file-history"]) if get => Some(FileHistory),
        (_, [b"api", b"slash-commands"]) if get => Some(Slash),
        (_, [b"api", b"plan"]) if get => Some(Plan),
        (_, [b"api", b"tasks"]) if get => Some(Tasks),
        (_, [b"api", b"memory"]) if get => Some(Memory),
        ("POST", [b"api", b"memory", b"review", id]) => one(id).map(ReviewMemory),
        (_, [b"api", b"instructions"]) if get => Some(Instructions),
        (_, [b"api", b"instructions", b"file"]) if get => Some(ReadInstruction),
        ("PUT", [b"api", b"instructions", b"file"]) => Some(WriteInstruction),
        ("DELETE", [b"api", b"instructions", b"file"]) => Some(DeleteInstruction),
        (_, [b"api", b"tiers"]) if get => Some(Tiers),
        _ => None,
    };
    match hit {
        Some(r) => (r, true),
        // the "/" pattern: exact only for the path "/"
        None => (CatchAll, path == b"/"),
    }
}

/// Go's `ServeMux.findHandler` for the console's patterns.
fn route(req: &Request) -> Route {
    if req.request_uri == b"*" {
        return Route::Star;
    }
    let escaped = req.url.escaped_path().into_bytes();
    if req.method == "CONNECT" {
        if escaped.is_empty() {
            return Route::NotFound;
        }
        return match_path(&req.method, &escaped).0;
    }
    let path = gohttpd::clean_path(&escaped);
    let with_query = |p: &[u8]| {
        let mut to = String::from_utf8_lossy(p).into_owned();
        if !req.url.raw_query.is_empty() {
            to.push('?');
            to.push_str(&String::from_utf8_lossy(&req.url.raw_query));
        }
        to
    };
    let (r, exact) = match_path(&req.method, &path);
    if !exact && !path.ends_with(b"/") && !path.is_empty() {
        let mut slashed = path.clone();
        slashed.push(b'/');
        if match_path(&req.method, &slashed).1 {
            return Route::Redirect(with_query(&slashed));
        }
    }
    if path != escaped {
        return Route::Redirect(with_query(&path));
    }
    r
}

// ---- request checks -------------------------------------------------------------------------------------------------------

/// Go's `hostname`.
fn hostname(hostport: &str) -> String {
    let h = split_host_port(hostport).map_or(hostport, |(h, _)| h);
    h.trim_matches(|c| c == '[' || c == ']').to_owned()
}

/// Go's `loopbackName`.
fn loopback_name(h: &str) -> bool {
    gotext::equal_fold(h, "localhost") || parse_ip(h).is_some_and(|ip| ip.is_loopback())
}

/// Go's `sameToken` (constant time).
fn same_token(want: &str, got: &[u8]) -> bool {
    if got.is_empty() || got.len() != want.len() {
        return false;
    }
    want.bytes().zip(got.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    // SAFETY: getrandom fills the local buffer of the length given.
    let got = unsafe { libc::getrandom(b.as_mut_ptr().cast(), n, 0) };
    assert_eq!(got, n as isize, "the console token needs random bytes");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn is_token_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`{|}~".contains(&c)
}

fn consume_token(v: &str) -> (&str, &str) {
    let i = v.bytes().position(|c| !is_token_char(c)).unwrap_or(v.len());
    (&v[..i], &v[i..])
}

fn is_tspecial(c: u8) -> bool {
    b"()<>@,;:\\\"/[]?=".contains(&c)
}

/// Go's `consumeValue`: a token or a quoted string; None on failure.
fn consume_value(v: &str) -> Option<(String, &str)> {
    let b = v.as_bytes();
    if b.is_empty() {
        return None;
    }
    if b[0] != b'"' {
        let (t, rest) = consume_token(v);
        return if t.is_empty() { None } else { Some((t.to_owned(), rest)) };
    }
    let mut out = Vec::new();
    let mut i = 1;
    while i < b.len() {
        let r = b[i];
        if r == b'"' {
            return Some((String::from_utf8_lossy(&out).into_owned(), &v[i + 1..]));
        }
        if r == b'\\' && i + 1 < b.len() && is_tspecial(b[i + 1]) {
            out.push(b[i + 1]);
            i += 2;
            continue;
        }
        if r == b'\r' || r == b'\n' {
            return None;
        }
        out.push(r);
        i += 1;
    }
    None
}

fn trim_left_space(s: &str) -> &str {
    s.trim_start_matches(gotext::is_space)
}

/// Go's `mime.ParseMediaType`, the media type only: "" when Go returns none (no valid type, or a parameter named twice
/// with two values); a type whose parameters do not parse is still returned, as Go does.
fn media_type(v: &[u8]) -> String {
    let v = gotext::lossy(v);
    let base = v.split(';').next().unwrap_or("");
    let mt = gotext::trim_space(&base.to_ascii_lowercase()).to_owned();
    let (typ, rest) = consume_token(&mt);
    if typ.is_empty() {
        return String::new();
    }
    if !rest.is_empty() {
        let Some(r) = rest.strip_prefix('/') else { return String::new() };
        let (sub, r) = consume_token(r);
        if sub.is_empty() || !r.is_empty() {
            return String::new();
        }
    }
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    let mut cont: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut v = &v[base.len()..];
    while !v.is_empty() {
        v = trim_left_space(v);
        if v.is_empty() {
            break;
        }
        // consumeMediaParam
        let parsed = (|| {
            let r = trim_left_space(v).strip_prefix(';')?;
            let (p, r) = consume_token(trim_left_space(r));
            if p.is_empty() {
                return None;
            }
            let r = trim_left_space(r).strip_prefix('=')?;
            let (value, r) = consume_value(trim_left_space(r))?;
            Some((p.to_ascii_lowercase(), value, r))
        })();
        let Some((key, value, rest)) = parsed else {
            if gotext::trim_space(v) == ";" {
                break;
            }
            return mt;
        };
        let pmap = match key.split_once('*') {
            Some((b, _)) => cont.entry(b.to_owned()).or_default(),
            None => &mut params,
        };
        if pmap.get(&key).is_some_and(|old| *old != value) {
            return String::new();
        }
        pmap.insert(key, value);
        v = rest;
    }
    mt
}

/// Go's `url.QueryUnescape`.
fn query_unescape(s: &[u8]) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                if i + 2 >= s.len() {
                    return None;
                }
                let h = |c: u8| (c as char).to_digit(16);
                let (a, b) = (h(s[i + 1])?, h(s[i + 2])?);
                out.push((a * 16 + b) as u8);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Some(gotext::lossy(&out))
}

/// Go's `URL.Query().Get(key)`: the first value; pairs with a semicolon or a bad escape are skipped.
pub fn query_get(raw: &[u8], key: &str) -> String {
    for pair in raw.split(|&c| c == b'&') {
        if pair.is_empty() || pair.contains(&b';') {
            continue;
        }
        let (k, v) = match pair.iter().position(|&c| c == b'=') {
            Some(i) => (&pair[..i], &pair[i + 1..]),
            None => (pair, &b""[..]),
        };
        let (Some(k), Some(v)) = (query_unescape(k), query_unescape(v)) else { continue };
        if k == key {
            return v;
        }
    }
    String::new()
}

/// Go's `Request.Cookie(name)`: the first cookie of that name whose value is valid.
fn cookie(req: &Request, name: &str) -> Option<String> {
    let lines = req.values("Cookie");
    let count: usize = lines.iter().map(|l| l.iter().filter(|&&c| c == b';').count() + 1).sum();
    if count > 3000 {
        return None;
    }
    let trim = |s: &[u8]| -> Vec<u8> {
        let s = s.iter().position(|c| !b" \t\r\n".contains(c)).map_or(&b""[..], |i| &s[i..]);
        let e = s.iter().rposition(|c| !b" \t\r\n".contains(c)).map_or(0, |i| i + 1);
        s[..e].to_vec()
    };
    for line in lines {
        for part in trim(line).split(|&c| c == b';') {
            let part = trim(part);
            if part.is_empty() {
                continue;
            }
            let (n, v) = match part.iter().position(|&c| c == b'=') {
                Some(i) => (trim(&part[..i]), part[i + 1..].to_vec()),
                None => (part.clone(), Vec::new()),
            };
            if n.is_empty() || !n.iter().all(|&c| is_token_char(c)) || n != name.as_bytes() {
                continue;
            }
            let mut v = v.as_slice();
            if v.len() > 1 && v[0] == b'"' && v[v.len() - 1] == b'"' {
                v = &v[1..v.len() - 1];
            }
            if !v.iter().all(|&b| (0x20..0x7f).contains(&b) && b != b'"' && b != b';' && b != b'\\') {
                continue;
            }
            return Some(String::from_utf8_lossy(v).into_owned());
        }
    }
    None
}

// ---- bodies and answers ---------------------------------------------------------------------------------------------------

/// Go's `readJSON` up to the decoding: the body (8 MiB at most, within its deadline), "{}" for an empty one; None when
/// the answer is already written.
pub fn read_json(ex: &mut Exchange, w: &mut Response) -> Option<Vec<u8>> {
    let body = match ex.read_body(MAX_BODY) {
        Ok(b) => b,
        Err(BodyErr::TooLarge) => {
            detail(w, 413, "request body too large");
            return None;
        }
        Err(BodyErr::Broken) => {
            w.set("Connection", "close");
            detail(w, 400, "request body could not be read");
            return None;
        }
    };
    ex.set_read_deadline(None);
    if gotext::trim_space(&String::from_utf8_lossy(&body)).is_empty() {
        return Some(b"{}".to_vec());
    }
    Some(body)
}

/// Go's `json.Unmarshal(body, &into)` with readJSON's errors: a type error names the field and Go's type, anything
/// else is "body: expected a JSON object".
pub fn decode(body: &[u8], t: &Ty, w: &mut Response) -> Option<Val> {
    let (v, err) = gostruct::unmarshal(t, body);
    let Some(e) = err else { return Some(v) };
    let top = match t {
        Ty::Struct(_, short, _) => *short,
        _ => "",
    };
    let msg = &e.0;
    let typed = (|| {
        let rest = msg.strip_prefix("json: cannot unmarshal ")?;
        let (_, rest) = rest.split_once(" into ")?;
        let (target, typ) = rest.rsplit_once(" of type ")?;
        let field = if target.starts_with("Go value") {
            String::new()
        } else {
            let t = target.strip_prefix("Go struct field ").unwrap_or(target);
            let t = t.strip_prefix(&format!("{top}.")).unwrap_or(t);
            t.to_owned()
        };
        Some((field, typ.to_owned()))
    })();
    match typed {
        Some((field, typ)) => fail(w, &state::wrong_type(&field, &typ)),
        None => fail(w, &state::wrong_type("body", "a JSON object")),
    }
    None
}

/// Go's `writeJSON`: `json.NewEncoder(w).Encode(v)` (HTML escaped, a newline after).
pub fn write_json(w: &mut Response, code: u16, v: &Any) {
    let mut out = String::new();
    let _ = crate::gojson::write_any(&mut out, v, true);
    write_raw(w, code, out);
}

/// An answer whose JSON is already written.
pub fn write_raw(w: &mut Response, code: u16, mut json: String) {
    json.push('\n');
    w.set("Content-Type", "application/json");
    w.status = code;
    w.body = json.into_bytes();
}

pub fn detail(w: &mut Response, code: u16, msg: &str) {
    write_json(w, code, &obj(vec![("detail", s(msg))]));
}

/// Go's `fail`.
pub fn fail(w: &mut Response, e: &ApiErr) {
    if e.code == 422 {
        let d = obj(vec![("msg", s(&e.msg)), ("type", s("type_error"))]);
        write_json(w, 422, &obj(vec![("detail", Any::Array(vec![d]))]));
        return;
    }
    detail(w, e.code, &e.msg);
}

pub fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

pub fn obj(kv: Vec<(&str, Any)>) -> Any {
    Any::Object(kv.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// A value's JSON as Go's `json.Marshal` writes it (HTML escaped).
pub fn json_any(v: &Any) -> String {
    crate::gojson::any_json(v, true).unwrap_or_else(|_| "null".into())
}

pub fn json_str(v: &str) -> String {
    json_any(&Any::String(v.to_owned()))
}

/// An object from JSON already written, its keys in Go's (sorted) order.
pub fn raw_obj(m: &BTreeMap<&str, String>) -> String {
    let mut out = String::from("{");
    for (i, (k, v)) in m.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_str(k));
        out.push(':');
        out.push_str(v);
    }
    out.push('}');
    out
}

/// Go's `time.Now().UTC().Format(time.RFC3339)`.
pub fn now_rfc3339() -> String {
    rfc3339(std::time::SystemTime::now())
}

pub fn rfc3339(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Go's `Serve`: the console on `listener` until `stop` fires, then Go's `Shutdown` with 3 s of grace.
pub fn serve(server: Arc<Server>, listener: std::net::TcpListener, stop: std::sync::mpsc::Receiver<()>) {
    let settings = Arc::new(gohttpd::Settings { read_header_timeout: Duration::from_secs(15), idle_timeout: Duration::from_secs(120) });
    let tracker = Arc::new(gohttpd::Tracker::default());
    let fd = std::os::fd::AsRawFd::as_raw_fd(&listener);
    let srv = server.clone();
    let handler: Arc<gohttpd::Handler> = Arc::new(move |ex: &mut Exchange| srv.handle(ex));
    let (t, s2) = (tracker.clone(), settings.clone());
    let accept = std::thread::Builder::new().name("accept".into()).spawn(move || gohttpd::accept_loop(listener, s2, handler, t));
    let _ = stop.recv();
    tracker.shutting_down.store(true, std::sync::atomic::Ordering::SeqCst);
    // SAFETY: shutting the listening socket wakes a blocked accept; the descriptor stays open (the accept thread owns it).
    unsafe {
        libc::shutdown(fd, libc::SHUT_RDWR);
    }
    let until = Instant::now() + Duration::from_secs(3);
    let mut wait = Duration::from_millis(1);
    while Instant::now() < until {
        if tracker.close_idle() {
            break;
        }
        std::thread::sleep(wait.min(until.saturating_duration_since(Instant::now())));
        wait = (wait * 2).min(Duration::from_millis(500));
    }
    drop(accept);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types() {
        assert_eq!(media_type(b"application/json"), "application/json");
        assert_eq!(media_type(b"Application/JSON; charset=utf-8"), "application/json");
        assert_eq!(media_type(b"application/json; charset"), "application/json");
        assert_eq!(media_type(b"application/json; a=1; a=2"), "");
        assert_eq!(media_type(b"application/json; a=1; a=1"), "application/json");
        assert_eq!(media_type(b"application/json, text/plain"), "");
        assert_eq!(media_type(b"text/plain"), "text/plain");
        assert_eq!(media_type(b""), "");
    }

    #[test]
    fn queries_and_times() {
        assert_eq!(query_get(b"x=1&t=a%20b&t=c", "t"), "a b");
        assert_eq!(query_get(b"t=a;b&t=c", "t"), "c");
        assert_eq!(query_get(b"t=%zz&t=ok", "t"), "ok");
        assert_eq!(rfc3339(std::time::UNIX_EPOCH + Duration::from_secs(1_762_000_000)), "2025-11-01T12:26:40Z");
    }
}
