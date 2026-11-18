//! Go: `chymaera/console`: the client half of Argus App's Sarina control API. The protocol is Argus_App's
//! (docs/SARINA_API.md): newline-delimited JSON over loopback TCP, one request object per line, one reply object per
//! line; implemented as specified, never extended. Only what the commands use is here (`Status`, `CaptureList`); Go's
//! `Ping`, `Help`, `LogTail` and `SessionStart` are used by nothing but Go's tests.

use crate::goany::Any;
use crate::pyfmt;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: i64 = 44700;
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// One reply line at most (Go's `MaxReplyBytes`): longer is not the protocol and is not read into memory.
pub const MAX_REPLY_BYTES: usize = 16 << 20;

/// Go's `TokenFile`: $XDG_RUNTIME_DIR/argus/token, or "" without a runtime folder.
fn token_file(getenv: &dyn Fn(&str) -> String) -> Vec<u8> {
    let d = getenv("XDG_RUNTIME_DIR");
    if d.is_empty() {
        return Vec::new();
    }
    crate::gopath::join(&[d.as_bytes(), b"argus", b"token"])
}

/// Go's `DefaultToken`: the token Argus App wrote (a regular file only, bounded), or "".
pub fn default_token(getenv: &dyn Fn(&str) -> String) -> String {
    let f = token_file(getenv);
    if f.is_empty() {
        return String::new();
    }
    match crate::session::readfile::read_regular(&f, 64 << 10) {
        Ok(b) => crate::gotext::trim_space(&crate::gotext::lossy(&b)).to_string(),
        Err(_) => String::new(),
    }
}

/// Go's `net.JoinHostPort`.
fn join_host_port(host: &str, port: i64) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// One connection to the console (Go's `Client`, connected by `connect` and reused).
pub struct Client {
    pub host: String,
    pub port: i64,
    pub token: String,
    pub timeout: Duration,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: i64,
}

impl Client {
    pub fn new() -> Client {
        Client { host: DEFAULT_HOST.into(), port: DEFAULT_PORT, token: String::new(), timeout: DEFAULT_TIMEOUT, conn: None, next_id: 0 }
    }

    /// Go's `net.DialTimeout("tcp", ...)`, with Go's error texts for what can happen on loopback.
    fn dial(&self) -> Result<TcpStream, String> {
        let hp = join_host_port(&self.host, self.port);
        if !(0..=65535).contains(&self.port) {
            return Err(format!("dial tcp: address {}: invalid port", self.port));
        }
        // an empty host is this machine (Go's dialer); the error names it as given
        let target = if self.host.is_empty() { join_host_port("127.0.0.1", self.port) } else { hp.clone() };
        let addrs: Vec<SocketAddr> = match target.to_socket_addrs() {
            Ok(a) => a.collect(),
            Err(_) => return Err(format!("dial tcp: lookup {}: no such host", self.host)),
        };
        let by = Instant::now() + self.timeout;
        let mut last = format!("dial tcp {hp}: connect: no address");
        for a in addrs {
            let shown = if self.host.is_empty() { hp.clone() } else { a.to_string() };
            let left = by.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("dial tcp {shown}: i/o timeout"));
            }
            match TcpStream::connect_timeout(&a, left) {
                Ok(s) => return Ok(s),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Err(format!("dial tcp {shown}: i/o timeout")),
                Err(e) => last = format!("dial tcp {shown}: connect: {}", crate::goerr::text(&e)),
            }
        }
        Err(last)
    }

    /// Go's `Connect`: the connection opened unless it is open.
    pub fn connect(&mut self) -> Result<(), String> {
        if self.conn.is_some() {
            return Ok(());
        }
        match self.dial() {
            Ok(s) => {
                let r = s.try_clone().map_err(|e| e.to_string())?;
                self.conn = Some((BufReader::new(r), s));
                Ok(())
            }
            Err(e) => Err(format!(
                "no Argus App console at {}:{} ({e}) - the app must be running with its Sarina server started",
                self.host, self.port
            )),
        }
    }

    /// Go's `Close`.
    pub fn close(&mut self) {
        self.conn = None;
    }

    /// Go's `Request` on an open connection: the reply's payload; Err is Go's error text (`cmd: message` when the console
    /// answered ok false).
    fn request(&mut self, cmd: &str, params: &[(&str, i64)]) -> Result<BTreeMap<String, Any>, String> {
        let owns = self.conn.is_none();
        self.connect()?;
        if self.next_id == 0 {
            self.next_id = 1;
        }
        let id = self.next_id;
        self.next_id += 1;
        // Go's json.Marshal of the map: keys sorted, HTML escaping on
        let mut members: Vec<(String, String)> = vec![("cmd".into(), jstr(cmd)), ("id".into(), id.to_string())];
        for (k, v) in params {
            members.push((k.to_string(), v.to_string()));
        }
        if !self.token.is_empty() {
            members.push(("token".into(), jstr(&self.token)));
        }
        members.sort();
        let data = format!("{{{}}}\n", members.iter().map(|(k, v)| format!("{}:{v}", jstr(k))).collect::<Vec<_>>().join(","));
        let (reader, stream) = self.conn.as_mut().expect("connected");
        let deadline = Instant::now() + self.timeout;
        let local = stream.local_addr().map(|a| a.to_string()).unwrap_or_default();
        let remote = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        let r = stream
            .set_write_timeout(Some(self.timeout))
            .and_then(|()| stream.write_all(data.as_bytes()))
            .map_err(|e| ("write", e))
            .and_then(|()| read_line(reader, deadline).map_err(|e| ("read", e)));
        let line = match r {
            Ok(Line::Text(l)) => l,
            Ok(Line::TooLong) => {
                self.close();
                return Err(format!("{cmd}: console sent a reply longer than {MAX_REPLY_BYTES} bytes"));
            }
            Err((op, e)) => {
                self.close();
                let why = if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                    "i/o timeout".to_string()
                } else {
                    crate::goerr::text(&e)
                };
                return Err(format!("{cmd}: connection failed ({op} tcp {local}->{remote}: {why})"));
            }
        };
        if line.is_empty() {
            self.close();
            return Err(format!("{cmd}: console closed the connection without replying"));
        }
        let v = match decode_one(&line) {
            Ok(v) => v,
            Err(e) => {
                self.close();
                return Err(format!("{cmd}: console sent malformed JSON ({e})"));
            }
        };
        let resp = match v.into_object() {
            Ok(m) => m,
            Err(v) => {
                self.close();
                return Err(format!("{cmd}: expected a JSON object, got {}", pyfmt::type_name(&v)));
            }
        };
        if owns {
            self.close();
        }
        if !resp.get("ok").is_some_and(pyfmt::truthy) {
            let msg = match resp.get("error") {
                Some(e) => pyfmt::str(e),
                None => "unknown error".into(),
            };
            return Err(format!("{cmd}: {msg}"));
        }
        Ok(resp)
    }

    /// Go's `Status`.
    pub fn status(&mut self) -> Result<BTreeMap<String, Any>, String> {
        self.request("status", &[])
    }

    /// Go's `CaptureList`: the records that are objects.
    pub fn capture_list(&mut self, limit: i64) -> Result<Vec<BTreeMap<String, Any>>, String> {
        let r = self.request("capture.list", &[("limit", limit)])?;
        Ok(objects(r.get("captures")))
    }
}

impl Default for Client {
    fn default() -> Self {
        Client::new()
    }
}

fn jstr(s: &str) -> String {
    let mut out = String::new();
    crate::gojson::write_str(&mut out, s, true);
    out
}

enum Line {
    Text(Vec<u8>),
    TooLong,
}

/// Go's `ReadString('\n')` bounded at [`MAX_REPLY_BYTES`] (Go's `readLine`), under the request's deadline; at the end of
/// the stream, what came (possibly nothing).
fn read_line(r: &mut BufReader<TcpStream>, deadline: Instant) -> std::io::Result<Line> {
    let mut b = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
        }
        r.get_ref().set_read_timeout(Some(left))?;
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(Line::Text(b));
        }
        let (take, done) = match buf.iter().position(|&c| c == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        b.extend_from_slice(&buf[..take]);
        r.consume(take);
        if b.len() > MAX_REPLY_BYTES {
            return Ok(Line::TooLong);
        }
        if done {
            return Ok(Line::Text(b));
        }
    }
}

/// Go's `json.NewDecoder` with `UseNumber`: one value, and nothing but white space after it ("extra data after JSON
/// value" otherwise: Go's second Decode must find the end of the stream).
fn decode_one(line: &[u8]) -> Result<Any, String> {
    let (first, end) = crate::gojson::parse_first_end(line);
    let v = first.map_err(|e| e.0)?;
    if !line[end..].iter().all(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n')) {
        return Err("extra data after JSON value".into());
    }
    Ok(crate::gojson::to_any_use_number(&v))
}

fn objects(v: Option<&Any>) -> Vec<BTreeMap<String, Any>> {
    match v {
        Some(Any::Array(list)) => list
            .iter()
            .filter_map(|e| match e {
                Any::Object(m) => Some(m.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Go's `RenderStatus`: the status summary and the captures, for people.
pub fn render_status(status: &BTreeMap<String, Any>, captures: &[BTreeMap<String, Any>]) -> String {
    let mut lines = vec!["# Argus App console".to_string(), String::new()];
    for key in ["version", "serverRunning", "serverPort", "sessionActive", "sessionName", "datastorePath", "eventCount"] {
        if let Some(v) = status.get(key) {
            lines.push(format!("{key}: {}", pyfmt::str(v)));
        }
    }
    lines.push(String::new());
    if captures.is_empty() {
        lines.push("No captures recorded.".into());
        return lines.join("\n");
    }
    lines.push(format!("## Captures ({})", captures.len()));
    for c in captures {
        let get = |k: &str| c.get(k).map_or("?".to_string(), pyfmt::str);
        lines.push(format!("- [{}] {} ({} bytes)", get("type"), get("name"), get("size")));
    }
    lines.join("\n")
}
