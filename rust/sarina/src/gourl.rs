//! Go 1.27's `net/url`, as far as `serverRoot` (llm/discover.go) uses it: `url.Parse` (scheme, authority with user
//! information, host and port checks with the strict colons for http and https, the path unescaped and its raw form
//! kept, the query and fragment), and `URL.String` for a URL whose query and fragment were dropped. Error texts are not
//! kept: a URL Go refuses is `None`.

/// The characters each part leaves as they are (Go's encoding table; letters and digits are always left).
const PATH_OK: &[u8] = b"$&+,-./:;=@_~";
const HOST_OK: &[u8] = b"!\"$&'()*+,-.:;<=>[]_~";
const USER_OK: &[u8] = b"$&+,-.;=_~";

fn should_escape(c: u8, ok: &[u8]) -> bool {
    !(c.is_ascii_alphanumeric() || ok.contains(&c))
}

/// Go's `escape`: each byte not left as it is becomes `%XX`.
fn escape(s: &[u8], ok: &[u8]) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s {
        if should_escape(c, ok) {
            out.push_str(&format!("%{c:02X}"));
        } else {
            out.push(c as char);
        }
    }
    out
}

fn unhex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

#[derive(PartialEq)]
enum Mode {
    Path,
    Host,
    User,
}

/// Go's `unescape` for a path, a host or user information: None for a bad `%` (or, in a host, a character a host cannot
/// hold, or an escaped ASCII byte other than `%25`).
fn unescape(s: &[u8], mode: Mode) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'%' => {
                if i + 2 >= s.len() || !s[i + 1].is_ascii_hexdigit() || !s[i + 2].is_ascii_hexdigit() {
                    return None;
                }
                if mode == Mode::Host && unhex(s[i + 1]) < 8 && &s[i..i + 3] != b"%25" {
                    return None;
                }
                out.push(unhex(s[i + 1]) << 4 | unhex(s[i + 2]));
                i += 3;
            }
            c => {
                if mode == Mode::Host && c < 0x80 && should_escape(c, HOST_OK) {
                    return None;
                }
                out.push(c);
                i += 1;
            }
        }
    }
    Some(out)
}

fn valid_optional_port(p: &[u8]) -> bool {
    p.is_empty() || (p[0] == b':' && p[1..].iter().all(u8::is_ascii_digit))
}

/// A parsed URL (the parts `serverRoot` writes back).
pub struct Url {
    pub scheme: String,
    pub opaque: bool,
    /// user information as written back: (name, password)
    pub user: Option<(Vec<u8>, Option<Vec<u8>>)>,
    pub host: Vec<u8>,
    pub omit_host: bool,
    pub path: Vec<u8>,
    pub raw_path: Vec<u8>,
}

fn parse_host(scheme: &str, host: &[u8]) -> Option<Vec<u8>> {
    if let Some(open) = host.iter().rposition(|&c| c == b'[') {
        if open > 0 {
            return None;
        }
        let close = host.iter().rposition(|&c| c == b']')?;
        let port = &host[close + 1..];
        if !valid_optional_port(port) {
            return None;
        }
        let name = &host[1..close];
        let unescaped = match name.windows(3).position(|w| w == b"%25") {
            // a zone: kept as it is but checked, as Go's encodeZone allows almost anything escaped
            Some(z) => {
                let mut h = unescape(&name[..z], Mode::Host)?;
                h.extend(unescape(&name[z..], Mode::Path)?);
                h
            }
            None => unescape(name, Mode::Host)?,
        };
        let text = std::str::from_utf8(&unescaped).ok()?;
        let addr_part = text.split('%').next().unwrap_or("");
        if addr_part.parse::<std::net::Ipv6Addr>().is_err() {
            return None;
        }
        let mut out = b"[".to_vec();
        out.extend(&unescaped);
        out.push(b']');
        out.extend(port);
        return Some(out);
    }
    if let Some(first) = host.iter().position(|&c| c == b':') {
        let last = host.iter().rposition(|&c| c == b':').unwrap_or(first);
        // Go 1.27: more than one colon is refused for http and https (the strict colons); other schemes take the last
        let i = if last != first {
            if scheme == "http" || scheme == "https" {
                return None;
            }
            last
        } else {
            first
        };
        if !valid_optional_port(&host[i..]) {
            return None;
        }
    }
    unescape(host, Mode::Host)
}

/// Go's `url.Parse`; None where Go returns an error.
pub fn parse(raw: &str) -> Option<Url> {
    let raw = raw.as_bytes();
    let (u, frag) = match raw.iter().position(|&c| c == b'#') {
        Some(i) => (&raw[..i], Some(&raw[i + 1..])),
        None => (raw, None),
    };
    if u.iter().any(|&c| c < 0x20 || c == 0x7f) {
        return None;
    }
    if let Some(f) = frag {
        // the fragment's escapes must be well formed (its text is not kept)
        unescape(f, Mode::Path)?;
    }
    let mut url = Url {
        scheme: String::new(),
        opaque: false,
        user: None,
        host: Vec::new(),
        omit_host: false,
        path: Vec::new(),
        raw_path: Vec::new(),
    };
    if u == b"*" {
        url.path = b"*".to_vec();
        return Some(url);
    }
    // the scheme
    let mut rest = u;
    for (i, &c) in u.iter().enumerate() {
        if c.is_ascii_alphabetic() {
            continue;
        }
        if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
            if i == 0 {
                break;
            }
            continue;
        }
        if c == b':' {
            if i == 0 {
                return None;
            }
            url.scheme = String::from_utf8_lossy(&u[..i]).to_ascii_lowercase();
            rest = &u[i + 1..];
        }
        break;
    }
    // the query is dropped
    if let Some(q) = rest.iter().position(|&c| c == b'?') {
        rest = &rest[..q];
    }
    if !rest.starts_with(b"/") {
        if !url.scheme.is_empty() {
            url.opaque = true;
            return Some(url);
        }
        let seg = rest.split(|&c| c == b'/').next().unwrap_or(&[]);
        if seg.contains(&b':') {
            return None;
        }
    }
    if (!url.scheme.is_empty() || !rest.starts_with(b"///")) && rest.starts_with(b"//") {
        let a = &rest[2..];
        let (authority, r) = match a.iter().position(|&c| c == b'/') {
            Some(i) => (&a[..i], &a[i..]),
            None => (a, &[][..]),
        };
        rest = r;
        let at = authority.iter().rposition(|&c| c == b'@');
        url.host = parse_host(&url.scheme, at.map_or(authority, |i| &authority[i + 1..]))?;
        if let Some(i) = at {
            let info = &authority[..i];
            let valid = info.iter().all(|&c| c.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&c));
            if !valid {
                return None;
            }
            url.user = Some(match info.iter().position(|&c| c == b':') {
                None => (unescape(info, Mode::User)?, None),
                Some(c) => (unescape(&info[..c], Mode::User)?, Some(unescape(&info[c + 1..], Mode::User)?)),
            });
        }
    } else if !url.scheme.is_empty() && rest.starts_with(b"/") {
        url.omit_host = true;
    }
    url.path = unescape(rest, Mode::Path)?;
    url.raw_path = if escape(&url.path, PATH_OK).as_bytes() == rest { Vec::new() } else { rest.to_vec() };
    Some(url)
}

/// Go's `validEncoded` for a path.
fn valid_encoded(s: &[u8]) -> bool {
    s.iter().all(|&c| b"!$&'()*+,;=:@[]%".contains(&c) || !should_escape(c, PATH_OK))
}

impl Url {
    /// Go's `EscapedPath`.
    fn escaped_path(&self) -> String {
        if !self.raw_path.is_empty() && valid_encoded(&self.raw_path) && unescape(&self.raw_path, Mode::Path).as_deref() == Some(&self.path)
        {
            return String::from_utf8_lossy(&self.raw_path).into_owned();
        }
        if self.path == b"*" {
            return "*".into();
        }
        escape(&self.path, PATH_OK)
    }

    /// Go's `URL.String` without query and fragment (opaque URLs never reach it here).
    pub fn string(&self) -> String {
        let mut out = String::new();
        if !self.scheme.is_empty() {
            out.push_str(&self.scheme);
            out.push(':');
        }
        let empty_host = self.host.is_empty() && self.user.is_none();
        if (!self.scheme.is_empty() || !self.host.is_empty() || self.user.is_some()) && !(self.omit_host && empty_host) {
            if !self.host.is_empty() || !self.path.is_empty() || self.user.is_some() {
                out.push_str("//");
            }
            if let Some((name, pass)) = &self.user {
                out.push_str(&escape(name, USER_OK));
                if let Some(p) = pass {
                    out.push(':');
                    out.push_str(&escape(p, USER_OK));
                }
                out.push('@');
            }
            out.push_str(&escape(&self.host, HOST_OK));
        }
        let mut path = self.escaped_path();
        if self.omit_host && empty_host && path.starts_with("//") {
            out.push_str("%2F");
            path.remove(0);
        }
        if !path.is_empty() && !path.starts_with('/') && !self.host.is_empty() {
            out.push('/');
        }
        if out.is_empty() && path.split('/').next().unwrap_or("").contains(':') {
            out.push_str("./");
        }
        out.push_str(&path);
        out
    }
}
