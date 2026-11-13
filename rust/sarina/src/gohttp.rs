//! A small HTTP/1.1 client answering as Go's `net/http` client does where Sarina's code can see it: `http://` and
//! `https://` URLs (`host[:port]`, path and query), one request with its headers and body, the status and body back
//! (Content-Length, chunked or until the connection closes), Go's error texts (`Post "URL": dial tcp ADDR: connect: ...`).
//! Two ways to call it: `request`/`post`, one deadline over the whole exchange like `http.Client.Timeout`, the body read
//! up to a limit; and `open`, the body read as it comes under a deadline the caller can move (the model clients' idle
//! timer) and a context whose cancel shuts the connection (`goctx`).
//!
//! TLS is rustls with ring, TLS 1.3 and 1.2 as Go's client offers them, the server's certificate checked against the
//! system's authorities read from the files Go's crypto/x509 reads on Linux (`roots`), or against the caller's own
//! (tests). The words of the common refusals are Go's (`tls_text`).
//!
//! A URL's user information is left out of the Host and sent as Basic credentials when the request has no Authorization
//! header of its own, as Go's client does.
//!
//! Not as Go, written down in docs/RUST_PORT.md: redirects are not followed (an answer is taken only from the address
//! configured, and an API key never goes elsewhere); no proxy from the environment; no `Accept-Encoding: gzip` is sent, so
//! a body with a `Content-Encoding` is not decoded and reads as an error; one request per connection.

use crate::goctx::{self, Ctx, Sock};
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Where a request goes: the URL's host over TCP, or a unix socket (the URL then only names the path, as Go's clients
/// with a socket dialer do).
pub enum Dial<'a> {
    Tcp,
    Unix(&'a str),
}

enum Conn {
    Tcp(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    Unix(UnixStream),
}

impl Conn {
    fn timeouts(&self, d: Duration) {
        let d = Some(d.max(Duration::from_millis(1)));
        match self {
            Conn::Tcp(s) => {
                let _ = s.set_read_timeout(d);
                let _ = s.set_write_timeout(d);
            }
            Conn::Tls(s) => {
                let _ = s.sock.set_read_timeout(d);
                let _ = s.sock.set_write_timeout(d);
            }
            Conn::Unix(s) => {
                let _ = s.set_read_timeout(d);
                let _ = s.set_write_timeout(d);
            }
        }
    }
}

impl Read for Conn {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.read(b),
            // a peer that closes without TLS's close_notify ends the stream, as in Go's crypto/tls
            Conn::Tls(s) => match s.read(b) {
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
                r => r,
            },
            Conn::Unix(s) => s.read(b),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.write(b),
            Conn::Tls(s) => s.write(b),
            Conn::Unix(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Tcp(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
            Conn::Unix(s) => s.flush(),
        }
    }
}

/// A response read whole (the parts Sarina's code reads).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub status: u16,
    /// Go's `Response.Status` ("404 Not Found")
    pub status_text: String,
    /// what was read of the body (at most the limit asked for)
    pub body: Vec<u8>,
    /// the error the body ended with ("unexpected EOF", a timeout...), as Go's reader returns it after the bytes it gave
    pub body_err: Option<String>,
}

struct Target {
    https: bool,
    host: String,
    host_port: String,
    host_header: String,
    request_uri: String,
    /// the URL's user information as Go's client sends it (`Basic` and the base64 of "user:password") when the
    /// request has no Authorization header of its own
    basic: Option<String>,
}

/// Go's `unescape` in its user-information mode: %XX decoded ('+' stays); a bad escape is Go's error, naming at most
/// three bytes from the '%'.
fn unescape_userinfo(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: Option<&u8>| c.and_then(|c| (*c as char).to_digit(16));
            match (hex(b.get(i + 1)), hex(b.get(i + 2))) {
                (Some(h), Some(l)) => out.push((h * 16 + l) as u8),
                _ => {
                    let bad = String::from_utf8_lossy(&b[i..(i + 3).min(b.len())]).into_owned();
                    return Err(format!("invalid URL escape {}", crate::gotext::quote(&bad)));
                }
            }
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// Standard base64 with padding (Go's `base64.StdEncoding`), for the `Basic` credentials.
fn base64_std(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            out.push(if k <= c.len() { A[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// Go's `url.validUserinfo`: the characters user information may hold.
fn valid_userinfo(s: &str) -> bool {
    s.bytes().all(|c| c.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&c))
}

/// Go's `url.Parse` plus the transport's checks, as far as an `http://` or `https://` URL goes.
fn target(url: &str) -> Result<Target, String> {
    let (scheme, rest) = match url.find("://") {
        Some(i) if i > 0 && url[..i].bytes().all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c)) => (&url[..i], &url[i + 3..]),
        _ => return Err("unsupported protocol scheme \"\"".into()),
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("unsupported protocol scheme {}", crate::gotext::quote(&scheme)));
    }
    let https = scheme == "https";
    let rest = rest.split('#').next().unwrap_or("");
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // user information (up to the last '@'): left out of the Host, sent as Basic credentials (Go's client)
    let (basic, authority) = match authority.rfind('@') {
        Some(i) => {
            let info = &authority[..i];
            if !valid_userinfo(info) {
                return Err("net/url: invalid userinfo".into());
            }
            let (user, pass) = match info.find(':') {
                Some(j) => (&info[..j], &info[j + 1..]),
                None => (info, ""),
            };
            let mut cred = unescape_userinfo(user)?;
            cred.push(b':');
            cred.extend(unescape_userinfo(pass)?);
            (Some(format!("Basic {}", base64_std(&cred))), &authority[i + 1..])
        }
        None => (None, authority),
    };
    if authority.is_empty() {
        return Err("no Host in request URL".into());
    }
    let has_port = match authority.rfind(':') {
        Some(i) => !authority.ends_with(']') && authority[..i].rfind(']').is_none_or(|j| j < i),
        None => false,
    };
    let host = if has_port { &authority[..authority.rfind(':').unwrap_or(0)] } else { authority };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let host_port = if has_port { authority.to_string() } else { format!("{authority}:{}", if https { 443 } else { 80 }) };
    let request_uri = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    Ok(Target { https, host, host_port, host_header: authority.to_string(), request_uri, basic })
}

// ---- TLS ----

/// The most a certificate bundle file is read (Arch's bundle is about 220 KiB).
const MAX_CERT_FILE: u64 = 16 << 20;

/// The system's certificate authorities as Go's crypto/x509 finds them on Linux: the first bundle that can be read
/// (SSL_CERT_FILE, else Go's list of six), and every file in the folders (SSL_CERT_DIR, colon-separated, else
/// /etc/ssl/certs and /etc/pki/tls/certs).
pub fn roots() -> rustls::RootCertStore {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let mut store = rustls::RootCertStore::empty();
    let files: Vec<String> = match std::env::var("SSL_CERT_FILE") {
        Ok(f) if !f.is_empty() => vec![f],
        _ => [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
            "/etc/ssl/ca-bundle.pem",
            "/etc/pki/tls/cacert.pem",
            "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
            "/etc/ssl/cert.pem",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    };
    let read = |p: &std::path::Path| -> Option<Vec<u8>> {
        let f = std::fs::File::open(p).ok()?;
        let mut b = Vec::new();
        f.take(MAX_CERT_FILE).read_to_end(&mut b).ok()?;
        Some(b)
    };
    let mut add = |data: &[u8]| {
        for c in CertificateDer::pem_slice_iter(data).flatten() {
            let _ = store.add(c);
        }
    };
    for f in files {
        if let Some(b) = read(std::path::Path::new(&f)) {
            add(&b);
            break;
        }
    }
    let dirs: Vec<String> = match std::env::var("SSL_CERT_DIR") {
        Ok(d) if !d.is_empty() => d.split(':').map(str::to_string).collect(),
        _ => vec!["/etc/ssl/certs".into(), "/etc/pki/tls/certs".into()],
    };
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut names: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        names.sort();
        for p in names {
            if p.is_file() {
                if let Some(b) = read(&p) {
                    add(&b);
                }
            }
        }
    }
    store
}

/// A client configuration trusting `roots`: TLS 1.3 and 1.2, ring's crypto.
pub fn tls_config(roots: rustls::RootCertStore) -> Arc<rustls::ClientConfig> {
    let p = Arc::new(rustls::crypto::ring::default_provider());
    let c = rustls::ClientConfig::builder_with_provider(p)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .expect("TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(c)
}

/// The system's configuration, made once.
pub fn system_tls() -> Arc<rustls::ClientConfig> {
    static C: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    C.get_or_init(|| tls_config(roots())).clone()
}

/// Go's words for a TLS failure, as its transport returns them.
fn tls_text(e: &rustls::Error, host: &str) -> String {
    use rustls::CertificateError as C;
    let x509 = |s: String| format!("tls: failed to verify certificate: x509: {s}");
    match e {
        rustls::Error::InvalidCertificate(c) => match c {
            C::UnknownIssuer | C::BadSignature | C::UnhandledCriticalExtension => x509("certificate signed by unknown authority".into()),
            C::NotValidForNameContext { presented, .. } => {
                let unwrap = |p: &String, pre: &str| -> Option<String> {
                    p.strip_prefix(pre).and_then(|x| x.strip_suffix(')')).map(|x| x.trim_matches('"').to_string())
                };
                if host.parse::<std::net::IpAddr>().is_ok() {
                    let ips: Vec<String> = presented.iter().filter_map(|p| unwrap(p, "IpAddress(")).collect();
                    if ips.is_empty() {
                        x509(format!("cannot validate certificate for {host} because it doesn't contain any IP SANs"))
                    } else {
                        x509(format!("certificate is valid for {}, not {host}", ips.join(", ")))
                    }
                } else {
                    let names: Vec<String> = presented.iter().filter_map(|p| unwrap(p, "DnsName(")).collect();
                    if names.is_empty() {
                        x509(format!("certificate is not valid for any names, but wanted to match {host}"))
                    } else {
                        x509(format!("certificate is valid for {}, not {host}", names.join(", ")))
                    }
                }
            }
            C::Expired | C::ExpiredContext { .. } | C::NotValidYet | C::NotValidYetContext { .. } => {
                x509("certificate has expired or is not yet valid".into())
            }
            other => x509(format!("{other:?}")),
        },
        other => format!("tls: {other}"),
    }
}

// ---- errors ----

/// Why a request failed before its status came (the text is what follows `Post "URL": ` in Go's error).
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    /// the deadline passed
    Deadline,
    /// the context ended (its error is `ctx.err()`)
    Ctx,
    Other(String),
}

/// Go's `url.Error` text for a request that failed.
fn url_error(method: &str, url: &str, what: &str) -> String {
    let verb = format!("{}{}", &method[..1.min(method.len())], method.get(1..).unwrap_or("").to_ascii_lowercase());
    format!("{verb} {}: {what}", crate::gotext::quote(url))
}

/// The body-read error texts (`std::io::Error` carrying Go's words).
const BODY_DEADLINE: &str = "context deadline exceeded (Client.Timeout or context cancellation while reading body)";

// ---- the request ----

/// One request to send.
pub struct Opts<'a> {
    pub dial: Dial<'a>,
    /// in capitals
    pub method: &'a str,
    pub url: &'a str,
    /// sent after Host, User-Agent and Content-Length, in the order given (Content-Type among them)
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    /// None: the system's authorities
    pub tls: Option<Arc<rustls::ClientConfig>>,
}

/// An answer whose body is read as it comes.
pub struct Response {
    pub status: u16,
    /// Go's `Response.Status`: the status line after the version ("404 Not Found")
    pub status_text: String,
    /// a Content-Encoding other than identity (never asked for: such a body is not read)
    pub encoded: bool,
    pub body: Body,
}

/// The deadline every step of a request waits for at most; the caller may move it (`Rc` so a reader and its timer share it).
pub type Deadline = Rc<Cell<Instant>>;

/// Opens the request: connects (TLS for https), sends it, reads the head. Every wait ends at `deadline` (as it is when the
/// wait starts) or when `ctx` ends; a cancel shuts the connection.
pub fn open(o: &Opts, ctx: &Ctx, deadline: &Deadline) -> Result<Response, Fail> {
    let t = target(o.url).map_err(Fail::Other)?;
    let left = || -> Result<Duration, Fail> {
        if ctx.err().is_some() {
            return Err(Fail::Ctx);
        }
        let mut by = deadline.get();
        if let Some(d) = ctx.deadline() {
            by = by.min(d);
        }
        let l = by.saturating_duration_since(Instant::now());
        if l.is_zero() {
            return Err(if ctx.err().is_some() { Fail::Ctx } else { Fail::Deadline });
        }
        Ok(l)
    };
    let (mut conn, watch) = match o.dial {
        Dial::Unix(path) => match UnixStream::connect(path) {
            Ok(c) => {
                let w = c.try_clone().ok().map(|s| ctx.watch(Sock::Unix(s)));
                (Conn::Unix(c), w)
            }
            Err(e) => return Err(Fail::Other(format!("dial unix {path}: connect: {}", crate::goerr::text(&e)))),
        },
        Dial::Tcp => {
            let tcp = dial_tcp(&t.host_port, ctx, deadline)?;
            let w = tcp.try_clone().ok().map(|s| ctx.watch(Sock::Tcp(s)));
            if t.https {
                let cfg = o.tls.clone().unwrap_or_else(system_tls);
                let name = rustls::pki_types::ServerName::try_from(t.host.clone())
                    .map_err(|_| Fail::Other(format!("tls: invalid server name {}", crate::gotext::quote(&t.host))))?;
                let c = rustls::ClientConnection::new(cfg, name).map_err(|e| Fail::Other(tls_text(&e, &t.host)))?;
                let mut s = rustls::StreamOwned::new(c, tcp);
                while s.conn.is_handshaking() {
                    let l = left()?;
                    let _ = s.sock.set_read_timeout(Some(l));
                    let _ = s.sock.set_write_timeout(Some(l));
                    if let Err(e) = s.conn.complete_io(&mut s.sock) {
                        return Err(io_fail(&e, ctx, &t.host, true));
                    }
                }
                (Conn::Tls(Box::new(s)), w)
            } else {
                (Conn::Tcp(tcp), w)
            }
        }
    };
    let mut req = format!("{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: Go-http-client/1.1\r\n", o.method, t.request_uri, t.host_header);
    if let Some(b) = o.body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    for (k, v) in o.headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = &t.basic {
        if !o.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("Authorization")) {
            req.push_str(&format!("Authorization: {b}\r\n"));
        }
    }
    req.push_str("Connection: close\r\n\r\n");
    let mut req = req.into_bytes();
    if let Some(b) = o.body {
        req.extend_from_slice(b);
    }
    conn.timeouts(left()?);
    conn.write_all(&req).and_then(|_| conn.flush()).map_err(|e| io_fail(&e, ctx, &t.host, false))?;
    // the head: lines end in \n (a \r before it is dropped), and an empty line ends it (Go's textproto); informational
    // answers (1xx but 101) are skipped as Go's transport skips them, their heads counting toward its 10 MiB
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16384];
    let mut skipped = 0usize;
    let (head, head_end, status, status_text) = loop {
        let head_end = loop {
            if let Some(end) = head_end(&raw) {
                break end;
            }
            conn.timeouts(left()?);
            match conn.read(&mut buf) {
                Ok(0) if ctx.err().is_some() => return Err(Fail::Ctx),
                Ok(0) => return Err(Fail::Other(if raw.is_empty() && skipped == 0 { "EOF".into() } else { "unexpected EOF".into() })),
                Ok(n) => raw.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset && raw.is_empty() && ctx.err().is_none() => {
                    return Err(Fail::Other("EOF".into()))
                }
                Err(e) => return Err(io_fail(&e, ctx, &t.host, false)),
            }
            // Go's Transport: 10 MiB of head at most
            if skipped + raw.len() > 10 << 20 {
                return Err(Fail::Other("net/http: server response headers exceeded 10485760 bytes; aborted".into()));
            }
        };
        let head = String::from_utf8_lossy(&raw[..head_end.0]).into_owned();
        let first = head.split('\n').next().unwrap_or("");
        let first = first.strip_suffix('\r').unwrap_or(first);
        let broken = |what: String| Fail::Other(format!("net/http: HTTP/1.x transport connection broken: {what}"));
        let (proto, rest) =
            first.split_once(' ').ok_or_else(|| broken(format!("malformed HTTP response {}", crate::gotext::quote(first))))?;
        let pb = proto.as_bytes();
        if !(pb.len() == 8 && proto.starts_with("HTTP/") && pb[5].is_ascii_digit() && pb[6] == b'.' && pb[7].is_ascii_digit()) {
            return Err(broken(format!("malformed HTTP version {}", crate::gotext::quote(proto))));
        }
        let code = rest.split(' ').next().unwrap_or("");
        if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(broken(format!("malformed HTTP status code {}", crate::gotext::quote(code))));
        }
        let status: u16 = code.parse().unwrap_or(0);
        if (100..200).contains(&status) && status != 101 {
            skipped += head_end.1;
            raw.drain(..head_end.1);
            continue;
        }
        let text = rest.trim_start_matches(' ').to_string();
        break (head, head_end, status, text);
    };
    let broken = |what: String| Fail::Other(format!("net/http: HTTP/1.x transport connection broken: {what}"));
    let lines = head.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).skip(1);
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
    let no_body = status == 204 || status == 304 || (100..200).contains(&status) || o.method == "HEAD";
    let framing = if no_body {
        Framing::Length(0)
    } else if chunked {
        Framing::Chunked(Chunk::default())
    } else {
        match length {
            Some(n) => Framing::Length(n),
            None => Framing::Close,
        }
    };
    let body = Body {
        conn,
        _watch: watch,
        ctx: ctx.clone(),
        deadline: deadline.clone(),
        pending: raw[head_end.1..].to_vec(),
        at: 0,
        framing,
        failed: None,
        stashed: None,
    };
    Ok(Response { status, status_text, encoded, body })
}

/// The TCP connection: the name looked up and each address tried in a helper thread, so a cancel or the deadline ends
/// the wait at once (Go's dialer is cancellable the same way); a connection made after that is dropped.
fn dial_tcp(host_port: &str, ctx: &Ctx, deadline: &Deadline) -> Result<TcpStream, Fail> {
    let (tx, rx) = std::sync::mpsc::channel();
    let hp = host_port.to_string();
    let mut by = deadline.get();
    if let Some(d) = ctx.deadline() {
        by = by.min(d);
    }
    std::thread::spawn(move || {
        let addrs: Vec<_> = match hp.to_socket_addrs() {
            Ok(a) => a.collect(),
            Err(e) => {
                let _ = tx.send(Err(Fail::Other(format!("dial tcp: lookup {hp}: {e}"))));
                return;
            }
        };
        let mut last = Fail::Other("dial tcp: no address".into());
        // Go dials an IPv4-mapped IPv6 address as the IPv4 address it holds (and names it so in an error)
        let addrs: Vec<std::net::SocketAddr> = addrs
            .into_iter()
            .map(|a| match a {
                std::net::SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
                    Some(v4) => std::net::SocketAddr::new(v4.into(), v6.port()),
                    None => a,
                },
                _ => a,
            })
            .collect();
        for a in &addrs {
            let left = by.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let _ = tx.send(Err(Fail::Deadline));
                return;
            }
            match TcpStream::connect_timeout(a, left) {
                Ok(s) => {
                    let _ = tx.send(Ok(s));
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    let _ = tx.send(Err(Fail::Deadline));
                    return;
                }
                Err(e) => last = Fail::Other(format!("dial tcp {a}: connect: {}", crate::goerr::text(&e))),
            }
        }
        let _ = tx.send(Err(last));
    });
    loop {
        if ctx.err().is_some() {
            return Err(Fail::Ctx);
        }
        let left = by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Fail::Deadline);
        }
        match rx.recv_timeout(left.min(Duration::from_millis(20))) {
            Ok(r) => return r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(Fail::Other("dial tcp: failed".into())),
        }
    }
}

fn io_fail(e: &std::io::Error, ctx: &Ctx, host: &str, handshake: bool) -> Fail {
    if ctx.err().is_some() {
        return Fail::Ctx;
    }
    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
        return Fail::Deadline;
    }
    if let Some(t) = e.get_ref().and_then(|x| x.downcast_ref::<rustls::Error>()) {
        return Fail::Other(tls_text(t, host));
    }
    if handshake && matches!(e.kind(), std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset) {
        return Fail::Other("EOF".into());
    }
    Fail::Other(crate::goerr::text(e))
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

#[derive(Clone, Copy)]
enum Framing {
    /// bytes still to come
    Length(usize),
    /// until the connection closes
    Close,
    Chunked(Chunk),
}

/// Where a chunked body stands (Go's `chunkedReader`): `left` bytes of the current chunk to come, its CRLF still to
/// check, the overhead counted against the data, the last chunk seen.
#[derive(Clone, Copy, Default)]
struct Chunk {
    left: u64,
    check_end: bool,
    excess: i64,
    done: bool,
}

/// A response body read as it comes. Its errors carry Go's words: "unexpected EOF" (`ErrorKind::UnexpectedEof`), the
/// deadline (`TimedOut`), "context canceled", "invalid byte in chunk length", or the socket's own error.
pub struct Body {
    conn: Conn,
    _watch: Option<goctx::Watch>,
    ctx: Ctx,
    deadline: Deadline,
    pending: Vec<u8>,
    at: usize,
    framing: Framing,
    /// the error the body ended with, returned by every later read (Go's readers keep theirs)
    failed: Option<(std::io::ErrorKind, String)>,
    /// an error that came with bytes: those are returned first, the error by the next read
    stashed: Option<std::io::Error>,
}

impl Body {
    /// More bytes into `pending`; false at the end of the connection.
    fn fill(&mut self) -> std::io::Result<bool> {
        if self.at > 0 && self.at == self.pending.len() {
            self.pending.clear();
            self.at = 0;
        }
        let ctx_err = |c: &Ctx| std::io::Error::other(c.err().unwrap_or(goctx::CANCELED));
        if self.ctx.err().is_some() {
            return Err(ctx_err(&self.ctx));
        }
        let mut by = self.deadline.get();
        if let Some(d) = self.ctx.deadline() {
            by = by.min(d);
        }
        let left = by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, BODY_DEADLINE));
        }
        self.conn.timeouts(left);
        let mut buf = [0u8; 16384];
        match self.conn.read(&mut buf) {
            Ok(0) if self.ctx.err().is_some() => Err(ctx_err(&self.ctx)),
            Ok(0) => Ok(false),
            Ok(n) => {
                self.pending.extend_from_slice(&buf[..n]);
                Ok(true)
            }
            Err(_) if self.ctx.err().is_some() => Err(ctx_err(&self.ctx)),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, BODY_DEADLINE))
            }
            Err(e) => Err(std::io::Error::other(crate::goerr::text(&e))),
        }
    }

    fn avail(&self) -> usize {
        self.pending.len() - self.at
    }

    /// Up to `max` bytes of what is pending into `out`.
    fn take(&mut self, out: &mut [u8], max: usize) -> usize {
        let k = out.len().min(max).min(self.avail());
        out[..k].copy_from_slice(&self.pending[self.at..self.at + k]);
        self.at += k;
        k
    }

    fn unexpected_eof() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "unexpected EOF")
    }

    fn read_inner(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            match self.framing {
                Framing::Length(0) => return Ok(0),
                Framing::Length(n) => {
                    if self.avail() > 0 {
                        let k = self.take(out, n);
                        self.framing = Framing::Length(n - k);
                        return Ok(k);
                    }
                    if !self.fill()? {
                        return Err(Self::unexpected_eof());
                    }
                }
                Framing::Close => {
                    if self.avail() > 0 {
                        return Ok(self.take(out, usize::MAX));
                    }
                    if !self.fill()? {
                        return Ok(0);
                    }
                }
                Framing::Chunked(c) if c.done => return Ok(0),
                Framing::Chunked(c) => {
                    let (n, err) = self.chunked(out, c);
                    return match err {
                        None => Ok(n),
                        Some(e) if n == 0 => Err(e),
                        Some(e) => {
                            self.stashed = Some(e);
                            Ok(n)
                        }
                    };
                }
            }
        }
    }

    /// Go's `chunkedReader.Read` (and the transport's trailer read at its end): as much as is there, crossing chunks
    /// while their heads are already read, never waiting for more once it has bytes; the bytes and the error they ended
    /// with.
    fn chunked(&mut self, out: &mut [u8], mut c: Chunk) -> (usize, Option<std::io::Error>) {
        let other = |s: &str| std::io::Error::other(s.to_string());
        let mut n = 0;
        let err = loop {
            if c.check_end {
                if n > 0 && self.avail() < 2 {
                    break None;
                }
                let mut fill_err = None;
                while self.avail() < 2 {
                    match self.fill() {
                        Ok(true) => {}
                        Ok(false) => {
                            fill_err = Some(Self::unexpected_eof());
                            break;
                        }
                        Err(e) => {
                            fill_err = Some(e);
                            break;
                        }
                    }
                }
                if let Some(e) = fill_err {
                    break Some(e);
                }
                if &self.pending[self.at..self.at + 2] != b"\r\n" {
                    break Some(other("malformed chunked encoding"));
                }
                self.at += 2;
                c.check_end = false;
            }
            if c.left == 0 {
                if n > 0 && !self.pending[self.at..].contains(&b'\n') {
                    break None;
                }
                // the size line: CRLF-ended, at most 4096 bytes (the transport's buffer)
                let line_end = loop {
                    if let Some(i) = self.pending[self.at..].iter().position(|&b| b == b'\n') {
                        break Ok(self.at + i);
                    }
                    if self.avail() >= 4096 {
                        break Err(other("header line too long"));
                    }
                    match self.fill() {
                        Ok(true) => {}
                        Ok(false) => break Err(Self::unexpected_eof()),
                        Err(e) => break Err(e),
                    }
                };
                let line_end = match line_end {
                    Ok(i) => i,
                    Err(e) => break Some(e),
                };
                let raw = self.pending[self.at..=line_end].to_vec();
                self.at = line_end + 1;
                match raw.iter().position(|&b| b == b'\r') {
                    None => break Some(other("chunked line ends with bare LF")),
                    Some(i) if i != raw.len() - 2 => break Some(other("invalid CR in chunked line")),
                    _ => {}
                }
                let mut line = &raw[..raw.len() - 2];
                if line.len() >= 4096 {
                    break Some(other("header line too long"));
                }
                c.excess += line.len() as i64 + 2;
                while let Some((&last, rest)) = line.split_last() {
                    if last != b' ' && last != b'\t' {
                        break;
                    }
                    line = rest;
                }
                if let Some(i) = line.iter().position(|&b| b == b';') {
                    line = &line[..i];
                }
                if line.is_empty() {
                    break Some(other("empty hex number for chunk length"));
                }
                let mut size: u64 = 0;
                let mut bad = None;
                for (i, &b) in line.iter().enumerate() {
                    let d = match b {
                        b'0'..=b'9' => b - b'0',
                        b'a'..=b'f' => b - b'a' + 10,
                        b'A'..=b'F' => b - b'A' + 10,
                        _ => {
                            bad = Some("invalid byte in chunk length");
                            break;
                        }
                    };
                    if i == 16 {
                        bad = Some("http chunk length too large");
                        break;
                    }
                    size = (size << 4) | u64::from(d);
                }
                if let Some(b) = bad {
                    break Some(other(b));
                }
                c.left = size;
                c.excess = (c.excess - 16 - (2 * size.min(i64::MAX as u64 / 4) as i64)).max(0);
                if c.excess > 16 * 1024 {
                    break Some(other("chunked encoding contains too much non-data"));
                }
                if size == 0 {
                    c.done = true;
                    self.framing = Framing::Chunked(c);
                    return (n, self.trailer().err());
                }
                continue;
            }
            if n == out.len() {
                break None;
            }
            if self.avail() == 0 {
                match self.fill() {
                    Ok(true) => {}
                    Ok(false) => break Some(Self::unexpected_eof()),
                    Err(e) => break Some(e),
                }
            }
            let max = usize::try_from(c.left).unwrap_or(usize::MAX);
            let k = {
                let (_, rest) = out.split_at_mut(n);
                self.take(rest, max)
            };
            n += k;
            c.left -= k as u64;
            if c.left == 0 {
                c.check_end = true;
            }
        };
        self.framing = Framing::Chunked(c);
        (n, err)
    }

    /// The transport's trailer read after the last chunk: an empty line, or header lines ending in an empty line within
    /// 4096 bytes.
    fn trailer(&mut self) -> std::io::Result<()> {
        let eof = || std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "http: unexpected EOF reading trailer");
        while self.avail() < 2 {
            if !self.fill()? {
                return Err(eof());
            }
        }
        if &self.pending[self.at..self.at + 2] == b"\r\n" {
            self.at += 2;
            return Ok(());
        }
        loop {
            let window = &self.pending[self.at..self.pending.len().min(self.at + 4096)];
            if let Some(i) = window.windows(4).position(|w| w == b"\r\n\r\n") {
                self.at += i + 4;
                return Ok(());
            }
            if self.avail() >= 4096 {
                return Err(std::io::Error::other("http: suspiciously long trailer after chunked body"));
            }
            if !self.fill()? {
                return Err(eof());
            }
        }
    }
}

impl Read for Body {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if let Some((k, s)) = &self.failed {
            return Err(std::io::Error::new(*k, s.clone()));
        }
        if let Some(e) = self.stashed.take() {
            self.failed = Some((e.kind(), e.to_string()));
            return Err(e);
        }
        match self.read_inner(out) {
            Err(e) => {
                self.failed = Some((e.kind(), e.to_string()));
                Err(e)
            }
            r => r,
        }
    }
}

/// Go's `http.Client{Timeout}.Post(url, contentType, body)`, reading at most `limit` bytes of the body. Err is Go's
/// `*url.Error` text.
pub fn post(url: &str, content_type: &str, body: &[u8], timeout: Duration, limit: usize) -> Result<Reply, String> {
    request(&Dial::Tcp, "POST", url, Some((content_type, body)), timeout, limit)
}

/// One request (`method` in capitals; a body with its Content-Type, or none) over `dial`, as Go's client sends it with
/// keep-alives off; at most `limit` bytes of the answer's body read; one deadline over all of it (`http.Client.Timeout`).
pub fn request(
    dial: &Dial,
    method: &str,
    url: &str,
    body: Option<(&str, &[u8])>,
    timeout: Duration,
    limit: usize,
) -> Result<Reply, String> {
    let headers: Vec<(&str, &str)> = body.iter().map(|(ct, _)| ("Content-Type", *ct)).collect();
    let dial = match dial {
        Dial::Tcp => Dial::Tcp,
        Dial::Unix(p) => Dial::Unix(p),
    };
    let o = Opts { dial, method, url, headers: &headers, body: body.map(|(_, b)| b), tls: None };
    let ctx = Ctx::background();
    let deadline: Deadline = Rc::new(Cell::new(Instant::now() + timeout));
    let mut resp = open(&o, &ctx, &deadline).map_err(|f| {
        url_error(
            method,
            url,
            &match f {
                Fail::Deadline | Fail::Ctx => "context deadline exceeded (Client.Timeout exceeded while awaiting headers)".into(),
                Fail::Other(s) => s,
            },
        )
    })?;
    let mut reply = Reply { status: resp.status, status_text: resp.status_text.clone(), ..Reply::default() };
    let mut buf = [0u8; 16384];
    while reply.body.len() < limit {
        let want = (limit - reply.body.len()).min(buf.len());
        match resp.body.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => reply.body.extend_from_slice(&buf[..n]),
            Err(e) => {
                reply.body_err = Some(e.to_string());
                break;
            }
        }
    }
    if resp.encoded {
        reply.body.clear();
        reply.body_err = Some("the answer is compressed (Content-Encoding), which this client does not read".into());
    }
    Ok(reply)
}

/// Go's `url.Error` text for `method` on `url` (`Post "URL": what`).
pub fn go_url_error(method: &str, url: &str, what: &str) -> String {
    url_error(method, url, what)
}
