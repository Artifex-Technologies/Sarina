//! A small HTTP/1.1 client for the two local services the programs talk to (Ollama's embeddings, Heimdall's API), as
//! urllib.request.urlopen was used: http:// only, a time limit on connecting and on each read, any status outside 2xx is an
//! error, and the body is read up to a limit (r.read(n)).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The request failed (no connection, a status outside 2xx, a broken answer): urlopen's exception, without the detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestFailed;

pub struct Url {
    host: String,
    port: u16,
    path: String,
}

fn parse(url: &str) -> Option<Url> {
    let rest = url.strip_prefix("http://")?;
    let (hostport, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = path.split('#').next().unwrap_or("/");
    let path = if path.starts_with('?') { format!("/{}", path) } else { path.to_string() };
    let hostport = hostport.rsplit('@').next()?;
    let (host, port) = if let Some(h) = hostport.strip_prefix('[') {
        let (h, p) = h.split_once(']')?;
        (h.to_string(), p.strip_prefix(':').map(|p| p.parse().ok()).unwrap_or(Some(80))?)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() => (h.to_string(), p.parse().ok()?),
            Some((h, _)) => (h.to_string(), 80),
            None => (hostport.to_string(), 80),
        }
    };
    if host.is_empty() {
        return None;
    }
    Some(Url { host, port, path: if path.is_empty() { "/".into() } else { path } })
}

/// GET or POST; the body (at most `limit` bytes) when the status is 2xx.
pub fn request(url: &str, body: Option<(&[u8], &str)>, timeout: f64, limit: usize) -> Result<Vec<u8>, RequestFailed> {
    let u = parse(url).ok_or(RequestFailed)?;
    let t = Duration::from_secs_f64(timeout);
    let addrs: Vec<_> = (u.host.as_str(), u.port).to_socket_addrs().map_err(|_| RequestFailed)?.collect();
    let mut stream = None;
    for a in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&a, t) {
            stream = Some(s);
            break;
        }
    }
    let mut s = stream.ok_or(RequestFailed)?;
    s.set_read_timeout(Some(t)).map_err(|_| RequestFailed)?;
    s.set_write_timeout(Some(t)).map_err(|_| RequestFailed)?;
    let host = if u.port == 80 { u.host.clone() } else { format!("{}:{}", u.host, u.port) };
    let mut req = format!(
        "{} {} HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: {}\r\nUser-Agent: sarina-voice\r\nConnection: close\r\n",
        if body.is_some() { "POST" } else { "GET" },
        u.path,
        host
    );
    if let Some((b, ctype)) = body {
        req.push_str(&format!("Content-Type: {}\r\nContent-Length: {}\r\n", ctype, b.len()));
    }
    req.push_str("\r\n");
    let mut wire = req.into_bytes();
    if let Some((b, _)) = body {
        wire.extend_from_slice(b);
    }
    s.write_all(&wire).map_err(|_| RequestFailed)?;
    let mut r = Reader { s, buf: Vec::new(), pos: 0 };
    let status = r.line()?;
    let mut parts = status.splitn(3, ' ');
    let proto = parts.next().unwrap_or("");
    let code: u16 = parts.next().and_then(|c| c.parse().ok()).ok_or(RequestFailed)?;
    if !proto.starts_with("HTTP/") {
        return Err(RequestFailed);
    }
    let mut length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let h = r.line()?;
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim();
            if k == "content-length" {
                length = v.parse().ok();
            } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    if !(200..300).contains(&code) {
        return Err(RequestFailed);
    }
    let mut out = Vec::new();
    if chunked {
        loop {
            let l = r.line()?;
            let n = usize::from_str_radix(l.split(';').next().unwrap_or("").trim(), 16).map_err(|_| RequestFailed)?;
            if n == 0 {
                break;
            }
            out.extend(r.take(n)?);
            r.line()?;
            if out.len() >= limit {
                break;
            }
        }
    } else if let Some(n) = length {
        out = r.take(n.min(limit))?;
    } else {
        out = r.rest(limit)?;
    }
    out.truncate(limit);
    Ok(out)
}

struct Reader {
    s: TcpStream,
    buf: Vec<u8>,
    pos: usize,
}

impl Reader {
    fn fill(&mut self) -> Result<bool, RequestFailed> {
        let mut b = [0u8; 8192];
        let n = self.s.read(&mut b).map_err(|_| RequestFailed)?;
        self.buf.extend_from_slice(&b[..n]);
        Ok(n > 0)
    }

    fn line(&mut self) -> Result<String, RequestFailed> {
        loop {
            if let Some(i) = self.buf[self.pos..].iter().position(|&c| c == b'\n') {
                let l = &self.buf[self.pos..self.pos + i];
                let l = l.strip_suffix(b"\r").unwrap_or(l);
                let s = String::from_utf8_lossy(l).into_owned();
                self.pos += i + 1;
                return Ok(s);
            }
            if !self.fill()? {
                return Err(RequestFailed);
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<Vec<u8>, RequestFailed> {
        while self.buf.len() - self.pos < n {
            if !self.fill()? {
                return Err(RequestFailed);
            }
        }
        let v = self.buf[self.pos..self.pos + n].to_vec();
        self.pos += n;
        Ok(v)
    }

    fn rest(&mut self, limit: usize) -> Result<Vec<u8>, RequestFailed> {
        while self.buf.len() - self.pos < limit && self.fill()? {}
        Ok(self.buf[self.pos..].to_vec())
    }
}
