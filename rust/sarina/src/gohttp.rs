//! A small HTTP/1.1 client for the loopback services Sarina asks (Heimdall's review, Ollama's embeddings), answering as Go's
//! `net/http` client does where Sarina's code can see it: `http://` URLs (`host[:port]`, path and query), one POST with a
//! body and `Content-Type`, the status and body back (Content-Length, chunked or until the connection closes), one deadline
//! over the whole exchange like `http.Client.Timeout`, and Go's error texts (`Post "URL": dial tcp ADDR: connect: ...`).
//!
//! Not as Go, written down in docs/RUST_PORT.md: `https://` is refused (TLS comes with the model clients); redirects are
//! not followed (an answer is taken only from the address configured); a body with a `Content-Encoding` is not decoded
//! and reads as an error (Go asks for gzip and unpacks it; Heimdall and Ollama never send one).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// A response (the parts Sarina's code reads).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub status: u16,
    /// what was read of the body (at most the limit asked for)
    pub body: Vec<u8>,
    /// the error the body ended with ("unexpected EOF", a timeout...), as Go's reader returns it after the bytes it gave
    pub body_err: Option<String>,
}

struct Target {
    host_port: String,
    host_header: String,
    request_uri: String,
}

/// Go's `url.Parse` plus the transport's checks, as far as an `http://` URL goes.
fn target(url: &str) -> Result<Target, String> {
    let (scheme, rest) = match url.find("://") {
        Some(i) if i > 0 && url[..i].bytes().all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c)) => (&url[..i], &url[i + 3..]),
        _ => return Err("unsupported protocol scheme \"\"".into()),
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" {
        return Err(format!("unsupported protocol scheme {}", crate::gotext::quote(&scheme)));
    }
    let rest = rest.split('#').next().unwrap_or("");
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err("user information in the URL is not supported".into());
    }
    if authority.is_empty() {
        return Err("no Host in request URL".into());
    }
    let has_port = match authority.rfind(':') {
        Some(i) => !authority.ends_with(']') && authority[..i].rfind(']').is_none_or(|j| j < i),
        None => false,
    };
    let host_port = if has_port { authority.to_string() } else { format!("{authority}:80") };
    let request_uri = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    Ok(Target { host_port, host_header: authority.to_string(), request_uri })
}

/// Go's `http.Client{Timeout}.Post(url, contentType, body)`, reading at most `limit` bytes of the body. Err is Go's
/// `*url.Error` text.
pub fn post(url: &str, content_type: &str, body: &[u8], timeout: Duration, limit: usize) -> Result<Reply, String> {
    let wrap = |what: String| format!("Post {}: {what}", go_quote_url(url));
    let t = target(url).map_err(wrap)?;
    let by = Instant::now() + timeout;
    let deadline = || wrap("context deadline exceeded (Client.Timeout exceeded while awaiting headers)".into());
    let addrs: Vec<_> = t.host_port.to_socket_addrs().map_err(|e| wrap(format!("dial tcp: lookup {}: {e}", t.host_port)))?.collect();
    let mut stream = None;
    let mut last = String::from("dial tcp: no address");
    for a in &addrs {
        let left = by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(deadline());
        }
        match TcpStream::connect_timeout(a, left) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return Err(deadline()),
            Err(e) => last = format!("dial tcp {a}: connect: {}", crate::goerr::text(&e)),
        }
    }
    let Some(mut s) = stream else { return Err(wrap(last)) };
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Go-http-client/1.1\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nConnection: close\r\n\r\n",
        t.request_uri,
        t.host_header,
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(body);
    let io_err = |e: std::io::Error| {
        if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
            deadline()
        } else {
            wrap(crate::goerr::text(&e))
        }
    };
    let _ = s.set_write_timeout(Some(by.saturating_duration_since(Instant::now()).max(Duration::from_millis(1))));
    s.write_all(&req).map_err(io_err)?;
    // the head: lines end in \n (a \r before it is dropped), and an empty line ends it (Go's textproto)
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16384];
    let head_end = loop {
        if let Some(end) = head_end(&raw) {
            break end;
        }
        let left = by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(deadline());
        }
        let _ = s.set_read_timeout(Some(left));
        match s.read(&mut buf) {
            Ok(0) => return Err(wrap(if raw.is_empty() { "EOF".into() } else { "unexpected EOF".into() })),
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset && raw.is_empty() => return Err(wrap("EOF".into())),
            Err(e) => return Err(io_err(e)),
        }
        // Go's Transport: 10 MiB of head at most
        if raw.len() > 10 << 20 {
            return Err(wrap("net/http: server response headers exceeded 10485760 bytes; aborted".into()));
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end.0]).into_owned();
    let mut lines = head.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let first = lines.next().unwrap_or("");
    let broken = |what: String| wrap(format!("net/http: HTTP/1.x transport connection broken: {what}"));
    let (proto, rest) = first.split_once(' ').ok_or_else(|| broken(format!("malformed HTTP response {}", crate::gotext::quote(first))))?;
    let pb = proto.as_bytes();
    if !(pb.len() == 8 && proto.starts_with("HTTP/") && pb[5].is_ascii_digit() && pb[6] == b'.' && pb[7].is_ascii_digit()) {
        return Err(broken(format!("malformed HTTP version {}", crate::gotext::quote(proto))));
    }
    let code = rest.split(' ').next().unwrap_or("");
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(broken(format!("malformed HTTP status code {}", crate::gotext::quote(code))));
    }
    let status: u16 = code.parse().unwrap_or(0);
    let (mut chunked, mut length, mut encoded) = (false, None::<usize>, false);
    let mut te_count = 0;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match k.as_str() {
                "transfer-encoding" => {
                    te_count += 1;
                    if te_count > 1 || !v.eq_ignore_ascii_case("chunked") {
                        return Err(broken(format!("unsupported transfer encoding: {}", crate::gotext::quote(v))));
                    }
                    chunked = true;
                }
                "content-length" => {
                    let n: usize = v.parse().map_err(|_| broken(format!("bad Content-Length {}", crate::gotext::quote(v))))?;
                    if length.is_some_and(|m| m != n) {
                        return Err(broken("http: message cannot contain multiple Content-Length headers".into()));
                    }
                    length = Some(n);
                }
                "content-encoding" if !v.is_empty() && !v.eq_ignore_ascii_case("identity") => encoded = true,
                _ => {}
            }
        }
    }
    let no_body = status == 204 || status == 304 || (100..200).contains(&status);
    let mut reader = BodyReader { s, by, pending: raw[head_end.1..].to_vec() };
    let mut reply = Reply { status, ..Reply::default() };
    if no_body {
        return Ok(reply);
    }
    let r = if chunked { reader.chunked(limit) } else { reader.plain(length, limit) };
    match r {
        Ok(b) => reply.body = b,
        Err((b, e)) => {
            reply.body = b;
            reply.body_err = Some(e);
        }
    }
    if encoded {
        reply.body.clear();
        reply.body_err = Some("the answer is compressed (Content-Encoding), which this client does not read".into());
    }
    Ok(reply)
}

/// Where the head ends: the end of its text and the start of the body (after the empty line).
fn head_end(raw: &[u8]) -> Option<(usize, usize)> {
    let mut at = 0;
    while let Some(i) = raw[at..].iter().position(|&c| c == b'\n') {
        let line = &raw[at..at + i];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() && at > 0 {
            return Some((at, at + i + 1));
        }
        at += i + 1;
    }
    None
}

/// Go's `url.Error` quotes the URL with `%q`.
fn go_quote_url(url: &str) -> String {
    crate::gotext::quote(url)
}

struct BodyReader {
    s: TcpStream,
    by: Instant,
    pending: Vec<u8>,
}

type Partial = (Vec<u8>, String);

impl BodyReader {
    /// More bytes into `pending`; false at the end of the connection.
    fn fill(&mut self) -> Result<bool, String> {
        let left = self.by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("context deadline exceeded (Client.Timeout or context cancellation while reading body)".into());
        }
        let _ = self.s.set_read_timeout(Some(left));
        let mut buf = [0u8; 16384];
        match self.s.read(&mut buf) {
            Ok(0) => Ok(false),
            Ok(n) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(true)
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                Err("context deadline exceeded (Client.Timeout or context cancellation while reading body)".into())
            }
            Err(e) => Err(crate::goerr::text(&e)),
        }
    }

    fn plain(&mut self, length: Option<usize>, limit: usize) -> Result<Vec<u8>, Partial> {
        let want = length.map_or(limit, |n| n.min(limit));
        loop {
            if self.pending.len() >= want {
                self.pending.truncate(want);
                return Ok(std::mem::take(&mut self.pending));
            }
            match self.fill() {
                Ok(true) => {}
                Ok(false) if length.is_none() => return Ok(std::mem::take(&mut self.pending)),
                Ok(false) => return Err((std::mem::take(&mut self.pending), "unexpected EOF".into())),
                Err(e) => return Err((std::mem::take(&mut self.pending), e)),
            }
        }
    }

    fn chunked(&mut self, limit: usize) -> Result<Vec<u8>, Partial> {
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            // the size line
            let line_end = loop {
                if let Some(i) = self.pending[at..].windows(2).position(|w| w == b"\r\n") {
                    break at + i;
                }
                match self.fill() {
                    Ok(true) => {}
                    Ok(false) => return Err((out, "unexpected EOF".into())),
                    Err(e) => return Err((out, e)),
                }
            };
            let line = String::from_utf8_lossy(&self.pending[at..line_end]).into_owned();
            let size_text = line.split(';').next().unwrap_or("").trim().to_string();
            let Ok(size) = usize::from_str_radix(&size_text, 16) else {
                return Err((out, "invalid byte in chunk length".into()));
            };
            at = line_end + 2;
            if size == 0 {
                return Ok(out);
            }
            while self.pending.len() < at + size + 2 {
                match self.fill() {
                    Ok(true) => {}
                    Ok(false) => {
                        let have = self.pending.len().saturating_sub(at).min(size);
                        out.extend_from_slice(&self.pending[at..at + have]);
                        out.truncate(limit);
                        return Err((out, "unexpected EOF".into()));
                    }
                    Err(e) => return Err((out, e)),
                }
            }
            out.extend_from_slice(&self.pending[at..at + size]);
            at += size + 2;
            if out.len() >= limit {
                out.truncate(limit);
                return Ok(out);
            }
        }
    }
}
