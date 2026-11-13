//! Go 1.27's `net/http` server, as far as Sarina's service uses it (`http.Server` with `ReadHeaderTimeout` and
//! `IdleTimeout`, `DisableGeneralOptionsHandler`, its handlers answering whole bodies): reading a request the way Go does
//! (the request line, the MIME header with its continuation lines and checks, the Host rules, Content-Length and chunked
//! bodies, `Expect: 100-continue`), the errors Go answers before any handler runs (400, 431, 501, 505 with Go's texts),
//! and an answer written as Go writes it (the header in sorted order, then Date, Content-Length, Connection and
//! Transfer-Encoding; a body over 2 KiB chunked; the connection kept alive or closed by Go's rules, the unread part of a
//! body drained up to 256 KiB). One thread per connection, at most [`MAX_CONNS`] at once.
//!
//! What is not Go's (docs/RUST_PORT.md "Differences"): the head's 1 MiB limit counts the head's own bytes where Go counts
//! what its 4 KiB reads took from the socket (the same answer but within 4 KiB of the limit); a trailer after a chunked
//! body must arrive with the last chunk; past [`MAX_CONNS`] connections a new one is closed at once (Go has no limit).

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::gourl::{self, Url};

/// Go's `DefaultMaxHeaderBytes`; the head may be this plus 4 KiB (`initialReadLimitSize`).
pub const MAX_HEADER_BYTES: i64 = 1 << 20;
const INITIAL_READ_LIMIT: i64 = MAX_HEADER_BYTES + 4096;
/// Go's bufio size for a connection's reader.
const READ_CHUNK: usize = 4096;
/// Go's `maxPostHandlerReadBytes`: how much of an unread body is drained to keep a connection.
const MAX_POST_HANDLER_READ: i64 = 256 << 10;
/// Go's `bufferBeforeChunkingSize`: an answer longer than this was written before the handler ended, so it is chunked.
const BUFFER_BEFORE_CHUNKING: usize = 2048;
/// Go's `rstAvoidanceDelay`.
const RST_AVOIDANCE: Duration = Duration::from_millis(500);
/// The most connections served at once (each is a thread); Go has no limit.
pub const MAX_CONNS: usize = 256;

/// Go's `StatusText` for the codes the service answers with.
pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue",
        200 => "OK",
        307 => "Temporary Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Request Entity Too Large",
        417 => "Expectation Failed",
        422 => "Unprocessable Entity",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

// ---- the connection's reader ----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum RErr {
    Eof,
    /// a read deadline passed
    Timeout,
    /// any other error of the socket
    Other,
}

/// Go's `connReader` (a byte limit while the head is read, a read deadline) under a `bufio.Reader`.
struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    pos: usize,
    /// bytes the reader may still take from the socket (Go's `remain`)
    remain: i64,
    deadline: Option<Instant>,
    eof: bool,
}

impl Conn {
    fn buffered(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    fn hit_read_limit(&self) -> bool {
        self.remain <= 0
    }

    /// One read from the socket into the buffer; Ok(false) at the end of input (or the read limit).
    fn fill(&mut self) -> Result<bool, RErr> {
        if self.eof {
            return Ok(false);
        }
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        if self.remain <= 0 {
            return Ok(false);
        }
        let timeout = match self.deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    return Err(RErr::Timeout);
                }
                Some(d - now)
            }
            None => None,
        };
        let _ = self.stream.set_read_timeout(timeout);
        let want = READ_CHUNK.min(self.remain.max(0) as usize);
        let mut tmp = [0u8; READ_CHUNK];
        loop {
            match self.stream.read(&mut tmp[..want]) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(n) => {
                    self.remain -= n as i64;
                    if self.pos > 0 {
                        self.buf.drain(..self.pos);
                        self.pos = 0;
                    }
                    self.buf.extend_from_slice(&tmp[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Err(RErr::Timeout),
                Err(_) => return Err(RErr::Other),
            }
        }
    }

    /// Go's `bufio.Reader.ReadLine` joined: a line without its `\n` (or `\r\n`); what is left at the end of input as a
    /// line of its own; `Eof` when nothing is left.
    fn read_line(&mut self) -> Result<Vec<u8>, RErr> {
        let mut from = 0;
        loop {
            if let Some(i) = self.buffered()[from..].iter().position(|&c| c == b'\n') {
                let end = self.pos + from + i;
                let mut line = self.buf[self.pos..end].to_vec();
                self.pos = end + 1;
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(line);
            }
            from = self.buffered().len();
            if !self.fill()? {
                if self.buffered().is_empty() {
                    return Err(RErr::Eof);
                }
                let line = self.buffered().to_vec();
                self.pos = self.buf.len();
                return Ok(line);
            }
        }
    }

    /// Go's `Peek(n)`: at least `n` bytes buffered, or fewer at the end of input (with the reason).
    fn peek(&mut self, n: usize) -> Result<&[u8], (RErr, usize)> {
        while self.buffered().len() < n {
            match self.fill() {
                Ok(true) => {}
                Ok(false) => return Err((RErr::Eof, self.buffered().len())),
                Err(e) => return Err((e, self.buffered().len())),
            }
        }
        Ok(&self.buffered()[..n])
    }

    fn read_byte(&mut self) -> Result<u8, RErr> {
        if self.buffered().is_empty() && !self.fill()? {
            return Err(RErr::Eof);
        }
        let c = self.buf[self.pos];
        self.pos += 1;
        Ok(c)
    }

    /// Up to `max` bytes (fewer when that much is buffered); empty at the end of input.
    fn read_some(&mut self, max: usize) -> Result<Vec<u8>, RErr> {
        if self.buffered().is_empty() && !self.fill()? {
            return Ok(Vec::new());
        }
        let n = self.buffered().len().min(max);
        let out = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(out)
    }

    fn write_all(&mut self, b: &[u8]) -> bool {
        self.stream.write_all(b).is_ok()
    }
}

// ---- the request ----------------------------------------------------------------------------------------------------

/// A request as Go's `readRequest` makes it.
pub struct Request {
    pub method: String,
    /// the target as written (Go's `RequestURI`)
    pub request_uri: Vec<u8>,
    pub proto_major: u32,
    pub proto_minor: u32,
    pub proto: String,
    pub url: Url,
    /// Go's `Header` (a map of lists)
    pub header: Header,
    /// Go's `Host`: the target's host, else the Host header
    pub host: Vec<u8>,
    /// -1: chunked
    pub content_length: i64,
    pub close: bool,
    /// the wildcard of the matched route (Go's `PathValue("id")`)
    pub path_id: String,
}

impl Request {
    /// Go's `Header.Get`: the first value of a canonical name.
    pub fn header(&self, name: &str) -> &[u8] {
        self.values(name).first().map(|v| v.as_slice()).unwrap_or_default()
    }

    fn values(&self, name: &str) -> &[Vec<u8>] {
        self.header.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_slice()).unwrap_or_default()
    }

    pub fn proto_at_least(&self, major: u32, minor: u32) -> bool {
        self.proto_major > major || self.proto_major == major && self.proto_minor >= minor
    }

    fn is_h2_upgrade(&self) -> bool {
        self.method == "PRI" && self.header.is_empty() && self.url.path == b"*" && self.proto == "HTTP/2.0"
    }
}

/// Why a request could not be read (Go's `c.serve` branches).
#[derive(Debug, PartialEq)]
enum ReqErr {
    /// nothing to say: the connection just ends (end of input before a request, a timeout, a broken socket)
    Quiet,
    /// 431
    TooLarge,
    /// 501
    UnsupportedTe,
    /// Go's statusError: "CODE TEXT: msg"
    Status(u16, &'static str),
    /// "400 Bad Request"
    Bad,
}

fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

/// Go's `textproto.validHeaderFieldByte`.
fn valid_field_byte(c: u8) -> bool {
    is_tchar(c)
}

/// Go's `textproto.validHeaderValueByte`: not a control character but tab; bytes from 0x80 allowed.
fn valid_value_byte(c: u8) -> bool {
    c >= 0x80 || c == b'\t' || (0x20..0x7f).contains(&c)
}

/// Go's `canonicalMIMEHeaderKey`.
fn canonical_key(a: &[u8]) -> Option<String> {
    if a.is_empty() {
        return None;
    }
    let mut no_canon = false;
    for &c in a {
        if valid_field_byte(c) {
            continue;
        }
        if c == b' ' {
            no_canon = true;
            continue;
        }
        return None;
    }
    if no_canon {
        return Some(String::from_utf8_lossy(a).into_owned());
    }
    let mut out = Vec::with_capacity(a.len());
    let mut upper = true;
    for &c in a {
        let c = if upper && c.is_ascii_lowercase() {
            c.to_ascii_uppercase()
        } else if !upper && c.is_ascii_uppercase() {
            c.to_ascii_lowercase()
        } else {
            c
        };
        out.push(c);
        upper = c == b'-';
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Go's `CanonicalHeaderKey` for a name already known to be a token (a trailer's names).
fn canonical_name(s: &[u8]) -> String {
    canonical_key(s).unwrap_or_else(|| String::from_utf8_lossy(s).into_owned())
}

fn trim_ows(s: &[u8]) -> &[u8] {
    let mut s = s;
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = s {
        s = rest;
    }
    s
}

/// httpguts's `headerValueContainsToken` over every value.
fn values_contain_token(values: &[Vec<u8>], token: &str) -> bool {
    values.iter().any(|v| v.split(|&c| c == b',').any(|t| trim_ows(t).eq_ignore_ascii_case(token.as_bytes())))
}

/// Go's server `hasToken`: `token` in `v` between token boundaries (space, comma, tab), case folded.
fn has_token(v: &[u8], token: &str) -> bool {
    let t = token.as_bytes();
    if t.len() > v.len() || t.is_empty() {
        return false;
    }
    if v == t {
        return true;
    }
    let boundary = |b: u8| b == b' ' || b == b',' || b == b'\t';
    for sp in 0..=v.len() - t.len() {
        let b = v[sp];
        if b != t[0] && b | 0x20 != t[0] {
            continue;
        }
        if sp > 0 && !boundary(v[sp - 1]) {
            continue;
        }
        let end = sp + t.len();
        if end != v.len() && !boundary(v[end]) {
            continue;
        }
        if v[sp..end].eq_ignore_ascii_case(t) {
            return true;
        }
    }
    false
}

/// httpguts's `ValidHostHeader`.
fn valid_host_header(h: &[u8]) -> bool {
    h.iter().all(|&c| c.is_ascii_alphanumeric() || b"!$%&()*+,-.:;=[]'_~".contains(&c))
}

/// Go's `ParseHTTPVersion`.
fn parse_version(v: &str) -> Option<(u32, u32)> {
    match v {
        "HTTP/1.1" => return Some((1, 1)),
        "HTTP/1.0" => return Some((1, 0)),
        _ => {}
    }
    let b = v.as_bytes();
    if !v.starts_with("HTTP/") || b.len() != 8 || b[6] != b'.' || !b[5].is_ascii_digit() || !b[7].is_ascii_digit() {
        return None;
    }
    Some(((b[5] - b'0') as u32, (b[7] - b'0') as u32))
}

/// Go's `textproto.TrimString`.
fn trim_ascii_space(s: &[u8]) -> &[u8] {
    let sp = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r');
    let start = s.iter().position(|c| !sp(c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !sp(c)).map_or(start, |i| i + 1);
    &s[start..end]
}

/// A request's header: the canonical names and their values, in the order first seen.
type Header = Vec<(String, Vec<Vec<u8>>)>;

/// Go's `readMIMEHeader` (no limits but the connection's read limit, as the server uses it).
fn read_mime_header(c: &mut Conn) -> Result<Header, RErr2> {
    let mut m: Header = Vec::new();
    // the first line cannot start with a space
    if let Ok(b) = c.peek(1) {
        if b[0] == b' ' || b[0] == b'\t' {
            return Err(RErr2::Protocol);
        }
    }
    loop {
        let mut kv = c.read_line().map_err(RErr2::Read)?;
        if kv.is_empty() {
            return Ok(m);
        }
        if !kv.contains(&b':') {
            return Err(RErr2::Protocol);
        }
        kv = trim_ows(&kv).to_vec();
        // continuation lines
        loop {
            match c.peek(1) {
                Ok(b) if b[0] == b' ' || b[0] == b'\t' => {}
                _ => break,
            }
            while let Ok(b) = c.peek(1) {
                if b[0] != b' ' && b[0] != b'\t' {
                    break;
                }
                let _ = c.read_byte();
            }
            kv.push(b' ');
            let line = c.read_line().map_err(RErr2::Read)?;
            kv.extend_from_slice(trim_ows(&line));
        }
        let i = kv.iter().position(|&b| b == b':').unwrap_or(kv.len());
        let (k, v) = (&kv[..i], &kv[(i + 1).min(kv.len())..]);
        let Some(key) = canonical_key(k) else { return Err(RErr2::Protocol) };
        if !v.iter().all(|&b| valid_value_byte(b)) {
            return Err(RErr2::Protocol);
        }
        let mut s = v;
        while let [b' ' | b'\t', rest @ ..] = s {
            s = rest;
        }
        match m.iter_mut().find(|(k, _)| *k == key) {
            Some((_, vv)) => vv.push(s.to_vec()),
            None => m.push((key, vec![s.to_vec()])),
        }
    }
}

enum RErr2 {
    Read(RErr),
    Protocol,
}

/// Go's `readRequest` with the server's checks after it (`conn.readRequest`).
fn read_request(c: &mut Conn) -> Result<Request, ReqErr> {
    let r = read_request_inner(c);
    if r.is_err() && c.hit_read_limit() {
        return Err(ReqErr::TooLarge);
    }
    let req = r?;
    if !(req.proto_major == 1 || req.proto_major == 2 && req.proto_minor == 0 && req.method == "PRI" && req.request_uri == b"*") {
        return Err(ReqErr::Status(505, "unsupported protocol version"));
    }
    let hosts = req.values("Host");
    let h2 = req.is_h2_upgrade();
    if req.proto_at_least(1, 1) && hosts.is_empty() && !h2 && req.method != "CONNECT" {
        return Err(ReqErr::Status(400, "missing required Host header"));
    }
    if hosts.len() == 1 && !valid_host_header(&hosts[0]) {
        return Err(ReqErr::Status(400, "malformed Host header"));
    }
    for (k, vv) in &req.header {
        if k.is_empty() || !k.bytes().all(valid_field_byte) {
            return Err(ReqErr::Status(400, "invalid header name"));
        }
        for v in vv {
            if v.iter().any(|&b| b < 0x20 && b != b'\t' || b == 0x7f) {
                return Err(ReqErr::Status(400, "invalid header value"));
            }
        }
    }
    Ok(req)
}

fn read_request_inner(c: &mut Conn) -> Result<Request, ReqErr> {
    // the first line: a read error or the end of input is nothing to answer
    let line = c.read_line().map_err(|_| ReqErr::Quiet)?;
    // after it, the end of input is an unexpected one (400); a timeout or a broken socket still nothing
    let after = |e: RErr| match e {
        RErr::Eof => ReqErr::Bad,
        _ => ReqErr::Quiet,
    };
    let mut parts = line.splitn(2, |&b| b == b' ');
    let method = parts.next().unwrap_or_default();
    let Some(rest) = parts.next() else { return Err(ReqErr::Bad) };
    let Some(sp) = rest.iter().position(|&b| b == b' ') else { return Err(ReqErr::Bad) };
    let (uri, proto) = (&rest[..sp], &rest[sp + 1..]);
    if method.is_empty() || !method.iter().all(|&b| is_tchar(b)) {
        return Err(ReqErr::Bad);
    }
    let method = String::from_utf8_lossy(method).into_owned();
    let proto = String::from_utf8_lossy(proto).into_owned();
    let Some((major, minor)) = parse_version(&proto) else { return Err(ReqErr::Bad) };
    let just_authority = method == "CONNECT" && !uri.starts_with(b"/");
    let url = if just_authority {
        let mut u = gourl::parse_request_uri(&[b"http://", uri].concat()).ok_or(ReqErr::Bad)?;
        u.scheme.clear();
        u
    } else {
        gourl::parse_request_uri(uri).ok_or(ReqErr::Bad)?
    };
    let header = match read_mime_header(c) {
        Ok(h) => h,
        Err(RErr2::Read(e)) => return Err(after(e)),
        Err(RErr2::Protocol) => return Err(ReqErr::Bad),
    };
    let mut req = Request {
        method,
        request_uri: uri.to_vec(),
        proto_major: major,
        proto_minor: minor,
        proto,
        url,
        header,
        host: Vec::new(),
        content_length: 0,
        close: false,
        path_id: String::new(),
    };
    if req.values("Host").len() > 1 {
        return Err(ReqErr::Bad);
    }
    req.host = if !req.url.host.is_empty() { req.url.host.clone() } else { req.header("Host").to_vec() };
    // shouldClose
    let conn = req.values("Connection").to_vec();
    let has_close = values_contain_token(&conn, "close");
    req.close = if major < 1 {
        true
    } else if major == 1 && minor == 0 {
        has_close || !values_contain_token(&conn, "keep-alive")
    } else {
        has_close
    };
    // readTransfer
    let mut chunked = false;
    if let Some(i) = req.header.iter().position(|(k, _)| k == "Transfer-Encoding") {
        let (_, raw) = req.header.remove(i);
        if req.proto_at_least(1, 1) {
            if raw.len() != 1 || !raw[0].eq_ignore_ascii_case(b"chunked") {
                return Err(ReqErr::UnsupportedTe);
            }
            chunked = true;
        }
    }
    let cls = req.values("Content-Length").to_vec();
    if cls.len() > 1 {
        let first = trim_ascii_space(&cls[0]);
        if cls[1..].iter().any(|v| trim_ascii_space(v) != first) {
            return Err(ReqErr::Bad);
        }
    }
    let mut n: i64 = 0;
    if let Some(cl) = cls.first() {
        let cl = trim_ascii_space(cl);
        if cl.is_empty() || !cl.iter().all(u8::is_ascii_digit) {
            return Err(ReqErr::Bad);
        }
        n = std::str::from_utf8(cl).ok().and_then(|s| s.parse::<u64>().ok()).filter(|&v| v < 1 << 63).ok_or(ReqErr::Bad)? as i64;
    }
    if chunked {
        req.header.retain(|(k, _)| k != "Content-Length");
        req.content_length = -1;
    } else {
        req.content_length = n;
    }
    // fixTrailer
    if chunked {
        if let Some(i) = req.header.iter().position(|(k, _)| k == "Trailer") {
            let (_, vv) = req.header.remove(i);
            for v in vv {
                for name in v.split(|&b| b == b',') {
                    let name = trim_ascii_space(name);
                    if name.is_empty() {
                        continue;
                    }
                    if matches!(canonical_name(name).as_str(), "Transfer-Encoding" | "Trailer" | "Content-Length") {
                        return Err(ReqErr::Bad);
                    }
                }
            }
        }
    }
    if req.is_h2_upgrade() {
        req.close = true;
    }
    Ok(req)
}

// ---- the body ---------------------------------------------------------------------------------------------------------

enum Kind {
    None,
    Length(i64),
    /// bytes left in the current chunk (0: a chunk line is next); done when the last chunk and trailer are read
    Chunked {
        left: u64,
        started: bool,
    },
}

/// A request's body (Go's `body`, with `doEarlyClose`).
struct Body {
    kind: Kind,
    saw_eof: bool,
    closed: bool,
    early_close: bool,
    /// the request asked for the connection to close (Go's `closing`)
    closing: bool,
    failed: bool,
}

#[derive(Debug, PartialEq)]
pub enum BodyErr {
    /// more than the handler's limit (Go's `MaxBytesError`)
    TooLarge,
    /// cut short, too slow, badly chunked
    Broken,
}

impl Body {
    fn remains(&self) -> bool {
        !matches!(self.kind, Kind::None) && !self.saw_eof
    }

    /// Up to `max` bytes; empty at the end of the body.
    fn read(&mut self, c: &mut Conn, max: usize) -> Result<Vec<u8>, BodyErr> {
        if self.saw_eof || self.failed {
            return if self.failed { Err(BodyErr::Broken) } else { Ok(Vec::new()) };
        }
        let r = self.read_inner(c, max);
        if r.is_err() {
            self.failed = true;
        }
        r
    }

    fn read_inner(&mut self, c: &mut Conn, max: usize) -> Result<Vec<u8>, BodyErr> {
        match &mut self.kind {
            Kind::None => {
                self.saw_eof = true;
                Ok(Vec::new())
            }
            Kind::Length(left) => {
                if *left == 0 {
                    self.saw_eof = true;
                    return Ok(Vec::new());
                }
                let b = c.read_some(max.min(*left as usize)).map_err(|_| BodyErr::Broken)?;
                if b.is_empty() {
                    return Err(BodyErr::Broken);
                }
                *left -= b.len() as i64;
                if *left == 0 {
                    self.saw_eof = true;
                }
                Ok(b)
            }
            Kind::Chunked { left, started } => {
                if *left == 0 {
                    if *started {
                        // the CRLF after a chunk's data
                        let mut two = [0u8; 2];
                        for b in &mut two {
                            *b = c.read_byte().map_err(|_| BodyErr::Broken)?;
                        }
                        if &two != b"\r\n" {
                            return Err(BodyErr::Broken);
                        }
                    }
                    *started = true;
                    let line = c.read_line().map_err(|_| BodyErr::Broken)?;
                    if line.len() >= 4096 {
                        return Err(BodyErr::Broken);
                    }
                    let line = line.split(|&b| b == b';').next().unwrap_or_default();
                    let line = trim_ascii_space(line);
                    if line.is_empty() || line.len() > 16 || !line.iter().all(u8::is_ascii_hexdigit) {
                        return Err(BodyErr::Broken);
                    }
                    let n = u64::from_str_radix(std::str::from_utf8(line).unwrap_or("x"), 16).map_err(|_| BodyErr::Broken)?;
                    if n == 0 {
                        read_trailer(c).map_err(|_| BodyErr::Broken)?;
                        self.saw_eof = true;
                        return Ok(Vec::new());
                    }
                    *left = n;
                }
                let b = c.read_some(max.min((*left).min(usize::MAX as u64) as usize)).map_err(|_| BodyErr::Broken)?;
                if b.is_empty() {
                    return Err(BodyErr::Broken);
                }
                *left -= b.len() as u64;
                Ok(b)
            }
        }
    }

    /// Go's `MaxBytesReader` and `io.ReadAll`: the whole body, or `TooLarge` once it holds more than `max` bytes.
    fn read_all(&mut self, c: &mut Conn, max: usize) -> Result<Vec<u8>, BodyErr> {
        let mut out = Vec::new();
        loop {
            let room = max + 1 - out.len();
            let b = self.read(c, room.min(64 << 10))?;
            if b.is_empty() {
                return Ok(out);
            }
            out.extend_from_slice(&b);
            if out.len() > max {
                return Err(BodyErr::TooLarge);
            }
        }
    }

    /// The unread part Go weighs (a Length body's bytes left; -1 for chunked).
    fn unread(&self) -> i64 {
        match self.kind {
            Kind::Length(n) => n,
            _ => -1,
        }
    }

    /// Go's `body.Close` with `doEarlyClose`.
    fn close(&mut self, c: &mut Conn) {
        if self.closed {
            return;
        }
        self.closed = true;
        let chunked = matches!(self.kind, Kind::Chunked { .. });
        if self.saw_eof || matches!(self.kind, Kind::None) || !chunked && self.closing || self.failed {
            return;
        }
        if let Kind::Length(n) = self.kind {
            if n > MAX_POST_HANDLER_READ {
                self.early_close = true;
                return;
            }
        }
        let mut n: i64 = 0;
        while n < MAX_POST_HANDLER_READ {
            match self.read(c, (MAX_POST_HANDLER_READ - n) as usize) {
                Ok(b) if b.is_empty() => return,
                Ok(b) => n += b.len() as i64,
                Err(_) => return,
            }
        }
        self.early_close = true;
    }
}

/// Go's `body.readTrailer`: the blank line after the last chunk, or trailer lines that are already buffered.
fn read_trailer(c: &mut Conn) -> Result<(), ()> {
    match c.peek(2) {
        Ok(b) if b == b"\r\n" => {
            c.pos += 2;
            return Ok(());
        }
        Ok(_) => {}
        Err(_) => return Err(()),
    }
    if !c.buffered().windows(4).any(|w| w == b"\r\n\r\n") {
        return Err(());
    }
    loop {
        let line = c.read_line().map_err(|_| ())?;
        if line.is_empty() {
            return Ok(());
        }
        if !line.contains(&b':') {
            return Err(());
        }
    }
}

// ---- the exchange a handler sees --------------------------------------------------------------------------------------

/// An answer: status, the header (canonical names, Go's map), the body written in one piece.
pub struct Response {
    pub status: u16,
    pub header: BTreeMap<String, Vec<String>>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16) -> Response {
        Response { status, header: BTreeMap::new(), body: Vec::new() }
    }

    pub fn set(&mut self, k: &str, v: &str) {
        self.header.insert(k.to_owned(), vec![v.to_owned()]);
    }

    fn get(&self, k: &str) -> &str {
        self.header.get(k).and_then(|v| v.first()).map(String::as_str).unwrap_or("")
    }
}

/// One request being served: what the handler reads, and what Go's response writer keeps.
pub struct Exchange<'a> {
    pub req: Request,
    conn: &'a mut Conn,
    body: Body,
    /// `Expect: 100-continue` honoured (Go's ecReader): the 100 is written at the first read of the body
    expect_continue: bool,
    wrote_continue: bool,
    close_after: bool,
    limit_hit: bool,
    /// the connection's state, shared with the server (for a shutdown)
    peer: TcpStream,
}

impl Exchange<'_> {
    /// The body, at most `max` bytes (Go's `io.ReadAll(http.MaxBytesReader(w, r.Body, max))`). Past the limit the
    /// connection closes after the answer, which says `Connection: close` after its length (Go 1.27.4's
    /// `requestTooLarge`; 1.27.3 put the header first, into the handler's own).
    pub fn read_body(&mut self, max: usize) -> Result<Vec<u8>, BodyErr> {
        if self.expect_continue && !self.wrote_continue && self.body.remains() {
            self.wrote_continue = true;
            let _ = self.conn.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        let r = self.body.read_all(self.conn, max);
        if r == Err(BodyErr::TooLarge) {
            self.close_after = true;
            self.limit_hit = true;
        }
        r
    }

    /// Go's `ResponseController.SetReadDeadline`.
    pub fn set_read_deadline(&mut self, d: Option<Instant>) {
        self.conn.deadline = d;
    }

    /// While a turn runs: whether the client hung up (Go's background read after the body: the end of input, or an
    /// error, cancels the request's context; a byte of a next request ends the watch). Some(true): hung up;
    /// Some(false): data came, stop asking; None: nothing yet.
    pub fn peer_gone(&self) -> Option<bool> {
        let fd = std::os::fd::AsRawFd::as_raw_fd(&self.peer);
        let mut b = [0u8; 1];
        // SAFETY: a non-blocking peek of one byte into a local buffer on our own socket.
        let n = unsafe { libc::recv(fd, b.as_mut_ptr().cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
        if n == 0 {
            return Some(true);
        }
        if n > 0 {
            return Some(false);
        }
        let e = std::io::Error::last_os_error();
        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) {
            None
        } else {
            Some(true)
        }
    }
}

/// What the server's handler is.
pub type Handler = dyn Fn(&mut Exchange) -> Response + Send + Sync;

// ---- the server -------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum State {
    New,
    Active,
    Idle,
}

/// The connections being served, for a shutdown (Go's `activeConn`).
#[derive(Default)]
pub struct Tracker {
    next: AtomicUsize,
    conns: Mutex<BTreeMap<usize, (TcpStream, State, Instant)>>,
    pub shutting_down: AtomicBool,
}

impl Tracker {
    fn set(&self, id: usize, st: State) {
        if let Some(e) = self.conns.lock().unwrap_or_else(|p| p.into_inner()).get_mut(&id) {
            e.1 = st;
            e.2 = Instant::now();
        }
    }

    /// Go's `closeIdleConns`: idle connections closed, and new ones older than 5 s; true when none is left.
    pub fn close_idle(&self) -> bool {
        let mut m = self.conns.lock().unwrap_or_else(|p| p.into_inner());
        let mut quiescent = true;
        for (s, st, since) in m.values() {
            match st {
                State::Idle => {
                    let _ = s.shutdown(Shutdown::Both);
                }
                State::New if since.elapsed() > Duration::from_secs(5) => {
                    let _ = s.shutdown(Shutdown::Both);
                }
                _ => quiescent = false,
            }
        }
        m.retain(|_, (_, st, _)| *st == State::Active || *st == State::New);
        quiescent && m.is_empty()
    }
}

/// The server's settings (Go's `http.Server` fields the service sets).
pub struct Settings {
    pub read_header_timeout: Duration,
    pub idle_timeout: Duration,
}

/// Serves one connection until it ends (Go's `conn.serve`).
pub fn serve_conn(stream: TcpStream, settings: &Settings, handler: &Handler, tracker: &Tracker) {
    let id = tracker.next.fetch_add(1, Ordering::SeqCst);
    let Ok(peer) = stream.try_clone() else { return };
    if let Ok(s) = stream.try_clone() {
        tracker.conns.lock().unwrap_or_else(|p| p.into_inner()).insert(id, (s, State::New, Instant::now()));
    }
    let mut c = Conn { stream, buf: Vec::new(), pos: 0, remain: INITIAL_READ_LIMIT, deadline: None, eof: false };
    serve_loop(&mut c, peer, settings, handler, tracker, id);
    tracker.conns.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
}

fn close_write_and_wait(c: &mut Conn) {
    let _ = c.stream.flush();
    let _ = c.stream.shutdown(Shutdown::Write);
    std::thread::sleep(RST_AVOIDANCE);
}

fn serve_loop(c: &mut Conn, peer: TcpStream, settings: &Settings, handler: &Handler, tracker: &Tracker, id: usize) {
    const ERR_HEADERS: &str = "\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n";
    let mut last_method = String::new();
    let mut first = true;
    loop {
        // readRequest: the header's deadline, the read limit, an old client's CR/LF after a POST
        c.deadline = Some(Instant::now() + settings.read_header_timeout);
        c.remain = INITIAL_READ_LIMIT;
        if last_method == "POST" {
            let skip = match c.peek(4) {
                Ok(b) => b.iter().take_while(|&&x| x == b'\r' || x == b'\n').count(),
                Err((_, n)) => c.buffered()[..n].iter().take_while(|&&x| x == b'\r' || x == b'\n').count(),
            };
            c.pos += skip;
        }
        let r = read_request(c);
        if !first || !c.buffered().is_empty() || c.remain != INITIAL_READ_LIMIT {
            tracker.set(id, State::Active);
        }
        first = false;
        if tracker.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let req = match r {
            Ok(r) => r,
            Err(ReqErr::Quiet) => return,
            Err(ReqErr::TooLarge) => {
                const E: &str = "431 Request Header Fields Too Large";
                c.write_all(format!("HTTP/1.1 {E}{ERR_HEADERS}{E}").as_bytes());
                close_write_and_wait(c);
                return;
            }
            Err(ReqErr::UnsupportedTe) => {
                c.write_all(format!("HTTP/1.1 501 Not Implemented{ERR_HEADERS}Unsupported transfer encoding").as_bytes());
                return;
            }
            Err(ReqErr::Status(code, text)) => {
                let s = format!("{code} {}: {text}", status_text(code));
                c.write_all(format!("HTTP/1.1 {s}{ERR_HEADERS}{s}").as_bytes());
                return;
            }
            Err(ReqErr::Bad) => {
                c.write_all(format!("HTTP/1.1 400 Bad Request{ERR_HEADERS}400 Bad Request").as_bytes());
                return;
            }
        };
        last_method = req.method.clone();
        c.remain = i64::MAX;
        c.deadline = None;
        let kind = if req.content_length == -1 {
            Kind::Chunked { left: 0, started: false }
        } else if req.content_length > 0 {
            Kind::Length(req.content_length)
        } else {
            Kind::None
        };
        let body = Body { kind, saw_eof: false, closed: false, early_close: false, closing: req.close, failed: false };
        let expects = has_token(req.header("Expect"), "100-continue");
        let mut ex = Exchange {
            expect_continue: expects && req.proto_at_least(1, 1) && req.content_length != 0,
            close_after: req.is_h2_upgrade(),
            req,
            conn: c,
            body,
            wrote_continue: false,
            limit_hit: false,
            peer: match peer.try_clone() {
                Ok(p) => p,
                Err(_) => return,
            },
        };
        if !expects && !ex.req.header("Expect").is_empty() {
            // Go's sendExpectationFailed
            let mut resp = Response::new(417);
            resp.set("Connection", "close");
            finish(&mut ex, resp, tracker);
            return;
        }
        let resp = handler(&mut ex);
        let reuse = finish(&mut ex, resp, tracker);
        if !reuse {
            if ex.limit_hit || ex.body.early_close {
                close_write_and_wait(ex.conn);
            }
            return;
        }
        tracker.set(id, State::Idle);
        if tracker.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        // wait for the next request, at most the idle timeout
        c.deadline = Some(Instant::now() + settings.idle_timeout);
        if c.peek(4).is_err() {
            return;
        }
    }
}

/// Writes the answer as Go's `chunkWriter.writeHeader`, `Write` and `finishRequest` do; whether the connection may serve
/// another request (Go's `shouldReuseConnection`).
fn finish(ex: &mut Exchange, resp: Response, tracker: &Tracker) -> bool {
    let keep_alives = !tracker.shutting_down.load(Ordering::SeqCst);
    let req = &ex.req;
    let is_head = req.method == "HEAD";
    let is_connect = req.method == "CONNECT";
    let status = resp.status;
    let body_allowed = !(100..200).contains(&status) && status != 204 && status != 304;
    let p = &resp.body;
    let handler_done = p.len() <= BUFFER_BEFORE_CHUNKING;
    let mut header = resp.header.clone();
    let get = |h: &BTreeMap<String, Vec<String>>, k: &str| h.get(k).and_then(|v| v.first()).cloned().unwrap_or_default();
    // WriteHeader: an explicit Content-Length
    let mut content_length: i64 = -1;
    let cl = get(&header, "Content-Length");
    if !cl.is_empty() {
        match cl.parse::<i64>() {
            Ok(v) if v >= 0 => content_length = v,
            _ => {
                header.remove("Content-Length");
            }
        }
    }
    let wants10 = req.proto_major == 1 && req.proto_minor == 0 && has_token(req.header("Connection"), "keep-alive");
    let wants_close = req.close || has_token(req.header("Connection"), "close");
    let mut close_after = ex.close_after;
    let mut x_cl: Option<usize> = None;
    let mut x_conn = String::new();
    let mut x_te = String::new();
    if handler_done && body_allowed && !header.contains_key("Content-Length") && (!is_head || !p.is_empty()) {
        content_length = p.len() as i64;
        x_cl = Some(p.len());
    }
    if wants10 && keep_alives {
        let sent = !get(&header, "Content-Length").is_empty();
        if sent && get(&header, "Connection") == "keep-alive" {
            close_after = false;
        }
    }
    let has_cl = content_length != -1;
    if wants10 && (is_head || has_cl || !body_allowed) {
        if !header.contains_key("Connection") {
            x_conn = "keep-alive".into();
        }
    } else if !req.proto_at_least(1, 1) || wants_close {
        close_after = true;
    }
    if get(&header, "Connection") == "close" || !keep_alives {
        close_after = true;
    }
    if ex.expect_continue && ex.body.remains() {
        close_after = true;
    }
    if is_connect {
        close_after = true;
    }
    if req.content_length != 0 && !matches!(ex.body.kind, Kind::None) && !close_after {
        let mut too_big = false;
        if ex.body.closed {
            if !ex.body.saw_eof {
                close_after = true;
            }
        } else if ex.body.unread() >= MAX_POST_HANDLER_READ {
            too_big = true;
        } else {
            ex.body.close(ex.conn);
            if ex.body.early_close {
                close_after = true;
            }
        }
        if too_big {
            close_after = true;
            ex.limit_hit = true;
            header.remove("Connection");
            x_conn = "close".into();
        }
    }
    let mut chunking = false;
    if is_head || !body_allowed || status == 204 || has_cl {
        header.remove("Transfer-Encoding");
    } else if req.proto_at_least(1, 1) {
        chunking = true;
        x_te = "chunked".into();
    } else {
        close_after = true;
        header.remove("Transfer-Encoding");
    }
    if chunking {
        header.remove("Content-Length");
    }
    let conn_close_given = has_token(get(&header, "Connection").as_bytes(), "close");
    if close_after && (!keep_alives || !conn_close_given) {
        header.remove("Connection");
        x_conn = if req.proto_at_least(1, 1) { "close".into() } else { String::new() };
    }
    let mut out = Vec::with_capacity(256 + p.len());
    out.extend_from_slice(if req.proto_at_least(1, 1) { b"HTTP/1.1 " } else { b"HTTP/1.0 " });
    match status_text(status) {
        "" => out.extend_from_slice(format!("{status:03} status code {status}\r\n").as_bytes()),
        t => out.extend_from_slice(format!("{status} {t}\r\n").as_bytes()),
    }
    for (k, vv) in &header {
        for v in vv {
            out.extend_from_slice(format!("{k}: {}\r\n", v.replace(['\r', '\n'], " ").trim()).as_bytes());
        }
    }
    if !header.contains_key("Date") {
        out.extend_from_slice(format!("Date: {}\r\n", http_date(std::time::SystemTime::now())).as_bytes());
    }
    if let Some(n) = x_cl {
        out.extend_from_slice(format!("Content-Length: {n}\r\n").as_bytes());
    }
    for (k, v) in [("Connection", &x_conn), ("Transfer-Encoding", &x_te)] {
        if !v.is_empty() {
            out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
        }
    }
    out.extend_from_slice(b"\r\n");
    let mut written = 0usize;
    if !is_head && body_allowed && !p.is_empty() {
        written = p.len();
        if chunking {
            out.extend_from_slice(format!("{:x}\r\n", p.len()).as_bytes());
            out.extend_from_slice(p);
            out.extend_from_slice(b"\r\n");
        } else {
            out.extend_from_slice(p);
        }
    }
    if chunking {
        out.extend_from_slice(b"0\r\n\r\n");
    }
    let wrote = ex.conn.write_all(&out);
    let _ = resp.get("Content-Type");
    // finishRequest: the body closed (drained up to 256 KiB, with the connection's read deadline)
    ex.body.close(ex.conn);
    ex.close_after = close_after;
    if close_after || !wrote {
        return false;
    }
    if !is_head && content_length != -1 && body_allowed && content_length != written as i64 {
        return false;
    }
    !ex.body.early_close
}

/// Go's `http.TimeFormat` of a time: "Mon, 02 Jan 2006 15:04:05 GMT".
pub fn http_date(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(m - 1) as usize];
    format!("{wd}, {d:02} {mon} {y:04} {h:02}:{mi:02}:{s:02} GMT")
}

/// Accepts connections on `listener` until `stop` is set, each served on a thread of its own (at most [`MAX_CONNS`]).
pub fn accept_loop(listener: std::net::TcpListener, settings: Arc<Settings>, handler: Arc<Handler>, tracker: Arc<Tracker>) {
    let live = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        if tracker.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = conn else { continue };
        if live.load(Ordering::SeqCst) >= MAX_CONNS {
            drop(stream);
            continue;
        }
        live.fetch_add(1, Ordering::SeqCst);
        let (s, h, t, l) = (settings.clone(), handler.clone(), tracker.clone(), live.clone());
        let spawned = std::thread::Builder::new().name("conn".into()).stack_size(1 << 20).spawn(move || {
            serve_conn(stream, &s, &*h, &t);
            l.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            live.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

// ---- the mux ------------------------------------------------------------------------------------------------------------

/// Go's `cleanPath`: path.Clean with the trailing slash kept.
pub fn clean_path(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b"/".to_vec();
    }
    let p2 = if p[0] != b'/' { [b"/", p].concat() } else { p.to_vec() };
    let mut np = crate::gopath::clean(&p2);
    if p2.last() == Some(&b'/') && np != b"/" {
        np.push(b'/');
    }
    np
}

/// One segment unescaped (Go's `pathUnescape`: the segment as it is when it does not unescape).
fn path_unescape(s: &[u8]) -> Vec<u8> {
    gourl::unescape(s, gourl::Mode::Path).unwrap_or_else(|| s.to_vec())
}

/// Go's routing-tree segments of an escaped path: each unescaped, a trailing slash as a segment "/" of its own.
fn segments(path: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = path;
    while !rest.is_empty() {
        if rest == b"/" {
            out.push(b"/".to_vec());
            break;
        }
        let r = &rest[1..];
        let i = r.iter().position(|&b| b == b'/').unwrap_or(r.len());
        out.push(path_unescape(&r[..i]));
        rest = &r[i..];
    }
    out
}

/// What the service's mux picks (Go's `ServeMux.findHandler` for its five patterns).
#[derive(Debug, PartialEq)]
pub enum Route {
    Healthz,
    Create,
    Chat(String),
    Evict(String),
    /// the "/" pattern
    CatchAll,
    /// a redirect to a cleaned path (307), the Location before Redirect's own cleaning
    Redirect(String),
    /// Go's NotFoundHandler: no pattern matched (only a CONNECT without a path reaches it)
    NotFound,
    /// RequestURI "*": 400 with Connection: close
    Star,
}

/// The route for a request.
pub fn route(req: &Request) -> Route {
    if req.request_uri == b"*" {
        return Route::Star;
    }
    let escaped = req.url.escaped_path().into_bytes();
    let path = if req.method == "CONNECT" {
        escaped.clone()
    } else {
        let clean = clean_path(&escaped);
        if clean != escaped {
            let mut to = String::from_utf8_lossy(&clean).into_owned();
            if !req.url.raw_query.is_empty() {
                to.push('?');
                to.push_str(&String::from_utf8_lossy(&req.url.raw_query));
            }
            return Route::Redirect(to);
        }
        clean
    };
    if path.is_empty() {
        return Route::NotFound;
    }
    let segs = segments(&path);
    let s: Vec<&[u8]> = segs.iter().map(|x| x.as_slice()).collect();
    let m = req.method.as_str();
    let wild = |x: &[u8]| x != b"/";
    match s.as_slice() {
        [b"healthz"] if m == "GET" || m == "HEAD" => Route::Healthz,
        [b"v1", b"sessions"] if m == "POST" => Route::Create,
        [b"v1", b"sessions", id, b"chat"] if m == "POST" && wild(id) => Route::Chat(String::from_utf8_lossy(id).into_owned()),
        [b"v1", b"sessions", id] if m == "DELETE" && wild(id) => Route::Evict(String::from_utf8_lossy(id).into_owned()),
        _ => Route::CatchAll,
    }
}

/// Go's `http.Redirect(w, r, to, 307)` for a path the mux cleaned.
pub fn redirect(req: &Request, to: &str) -> Response {
    let (path, query) = match to.find('?') {
        Some(i) => (&to[..i], &to[i..]),
        None => (to, ""),
    };
    let mut url = String::from_utf8_lossy(&crate::gopath::clean(path.as_bytes())).into_owned();
    if path.ends_with('/') && !url.ends_with('/') {
        url.push('/');
    }
    url.push_str(query);
    let mut r = Response::new(307);
    let mut loc = String::new();
    for c in url.bytes() {
        if c >= 0x80 {
            loc.push_str(&format!("%{c:02x}"));
        } else {
            loc.push(c as char);
        }
    }
    r.set("Location", &loc);
    if req.method == "GET" || req.method == "HEAD" {
        r.set("Content-Type", "text/html; charset=utf-8");
    }
    if req.method == "GET" {
        let esc = url.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&#34;").replace('\'', "&#39;");
        r.body = format!("<a href=\"{esc}\">Temporary Redirect</a>.\n\n").into_bytes();
    }
    r
}

/// Go's `http.Error(w, "404 page not found", 404)` (NotFoundHandler).
pub fn not_found_text() -> Response {
    let mut r = Response::new(404);
    r.set("Content-Type", "text/plain; charset=utf-8");
    r.set("X-Content-Type-Options", "nosniff");
    r.body = b"404 page not found\n".to_vec();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_and_paths() {
        let t = std::time::UNIX_EPOCH + Duration::from_secs(1_762_000_000);
        assert_eq!(http_date(t), "Sat, 01 Nov 2025 12:26:40 GMT");
        assert_eq!(clean_path(b"/v1/../healthz"), b"/healthz");
        assert_eq!(clean_path(b"/a/b/"), b"/a/b/");
        assert_eq!(clean_path(b""), b"/");
        assert!(has_token(b"Keep-Alive, Close", "close"));
        assert!(!has_token(b"closed", "close"));
        assert_eq!(canonical_key(b"x-api-KEY").as_deref(), Some("X-Api-Key"));
        assert_eq!(canonical_key(b"X A").as_deref(), Some("X A"));
        assert_eq!(canonical_key(b"a\x01"), None);
    }
}
