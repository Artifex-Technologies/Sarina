//! Go: `http.StripPrefix("/static/", http.FileServerFS(static))` as the console serves its embedded page (Go 1.27's
//! `net/http/fs.go`): the files of `go/gui/static` (the same files, built in), the redirects for `index.html`, a folder
//! without its slash and a file with one, the conditions (If-Match, If-None-Match; the files have no time and no ETag),
//! byte ranges (one range, or several as multipart/byteranges) and the errors with their headers dropped.

use crate::gohttpd::{Request, Response};
use crate::gopath;

pub const INDEX_HTML: &[u8] = include_bytes!("../../../../go/gui/static/index.html");
const APP_CSS: &[u8] = include_bytes!("../../../../go/gui/static/app.css");
const APP_JS: &[u8] = include_bytes!("../../../../go/gui/static/app.js");

/// The embedded folder: a name (Go's fs path, "." the folder itself) and its file, None for the folder.
fn open(name: &str) -> Option<Option<&'static [u8]>> {
    match name {
        "." => Some(None),
        "index.html" => Some(Some(INDEX_HTML)),
        "app.css" => Some(Some(APP_CSS)),
        "app.js" => Some(Some(APP_JS)),
        _ => None,
    }
}

/// Go's `mime.TypeByExtension` for the three files (its built-in table, which a system's mime files never override).
fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e) {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        _ => "text/html; charset=utf-8",
    }
}

/// Go's `http.Error`.
pub fn error(w: &mut Response, text: &str, code: u16) {
    w.header.remove("Content-Length");
    w.set("Content-Type", "text/plain; charset=utf-8");
    w.set("X-Content-Type-Options", "nosniff");
    w.status = code;
    w.body = format!("{text}\n").into_bytes();
}

/// Go's `serveError`: the caching headers dropped, then `http.Error`.
fn serve_error(w: &mut Response, text: &str, code: u16) {
    for k in ["Cache-Control", "Content-Encoding", "Etag", "Last-Modified"] {
        w.header.remove(k);
    }
    error(w, text, code);
}

/// Go's `localRedirect` (301, the query kept; a path with an escaped slash is a 404 instead).
fn local_redirect(w: &mut Response, req: &Request, to: &str) {
    let p = req.url.escaped_path();
    if p.contains("%2f") || p.contains("%2F") {
        error(w, "404 page not found", 404);
        return;
    }
    let mut to = to.to_owned();
    if !req.url.raw_query.is_empty() {
        to.push('?');
        to.push_str(&String::from_utf8_lossy(&req.url.raw_query));
    }
    w.set("Location", &to);
    w.status = 301;
}

/// The request's path with "/static/" taken off (Go's `StripPrefix`); None when it does not start with it.
fn strip(req: &Request) -> Option<String> {
    let path = &req.url.path;
    let p = path.strip_prefix(b"/static/".as_slice())?;
    if !req.url.raw_path.is_empty() && !req.url.raw_path.starts_with(b"/static/") {
        return None;
    }
    Some(String::from_utf8_lossy(p).into_owned())
}

/// The handler for `GET /static/`.
pub fn serve(w: &mut Response, req: &Request) {
    let Some(p) = strip(req) else {
        error(w, "404 page not found", 404);
        return;
    };
    let upath = if p.starts_with('/') { p } else { format!("/{p}") };
    let name = String::from_utf8_lossy(&gopath::clean(upath.as_bytes())).into_owned();
    if upath.ends_with("/index.html") {
        local_redirect(w, req, "./");
        return;
    }
    let fs_name = if name == "/" { ".".to_owned() } else { name.trim_start_matches('/').to_owned() };
    let Some(mut file) = open(&fs_name) else {
        serve_error(w, "404 page not found", 404);
        return;
    };
    let base = |u: &str| String::from_utf8_lossy(&gopath::base(u.as_bytes())).into_owned();
    match file {
        None if !upath.ends_with('/') => {
            local_redirect(w, req, &format!("{}/", base(&upath)));
            return;
        }
        Some(_) if upath.ends_with('/') => {
            let b = base(&upath);
            if b == "/" || b == "." {
                serve_error(w, "http: attempting to traverse a non-directory", 500);
            } else {
                local_redirect(w, req, &format!("../{b}"));
            }
            return;
        }
        _ => {}
    }
    let mut file_name = base(&fs_name);
    if file.is_none() {
        file = Some(INDEX_HTML);
        file_name = "index.html".into();
    }
    let content = file.unwrap_or_default();
    serve_content(w, req, &file_name, content);
}

/// textproto's `TrimString`.
fn trim(s: &str) -> &str {
    s.trim_matches(|c| c == ' ' || c == '\t' || c == '\n' || c == '\r')
}

/// Go's `scanETag`: a valid ETag at the start of `s`, and the rest.
fn scan_etag(s: &str) -> Option<(&str, &str)> {
    let s = trim(s);
    let start = if s.starts_with("W/") { 2 } else { 0 };
    let b = s.as_bytes();
    if b.len() < start + 2 || b[start] != b'"' {
        return None;
    }
    for i in start + 1..b.len() {
        let c = b[i];
        if c == b'"' {
            return Some((&s[..i + 1], &s[i + 1..]));
        }
        if !(c == 0x21 || (0x23..=0x7e).contains(&c) || c >= 0x80) {
            return None;
        }
    }
    None
}

#[derive(PartialEq)]
enum Cond {
    None,
    True,
    False,
}

fn header(req: &Request, k: &str) -> String {
    String::from_utf8_lossy(req.header(k)).into_owned()
}

/// Go's `checkIfMatch` with no ETag of ours: only `*` holds.
fn if_match(req: &Request) -> Cond {
    let im = header(req, "If-Match");
    if im.is_empty() {
        return Cond::None;
    }
    let mut rest = im.as_str();
    loop {
        rest = trim(rest);
        if rest.is_empty() {
            break;
        }
        if let Some(r) = rest.strip_prefix(',') {
            rest = r;
            continue;
        }
        if rest.starts_with('*') {
            return Cond::True;
        }
        match scan_etag(rest) {
            Some((_, r)) => rest = r,
            None => break,
        }
    }
    Cond::False
}

/// Go's `checkIfNoneMatch` with no ETag of ours: only `*` fails.
fn if_none_match(req: &Request) -> Cond {
    let inm = header(req, "If-None-Match");
    if inm.is_empty() {
        return Cond::None;
    }
    let mut rest = inm.as_str();
    loop {
        rest = trim(rest);
        if rest.is_empty() {
            break;
        }
        if let Some(r) = rest.strip_prefix(',') {
            rest = r;
            continue;
        }
        if rest.starts_with('*') {
            return Cond::False;
        }
        match scan_etag(rest) {
            Some((_, r)) => rest = r,
            None => break,
        }
    }
    Cond::True
}

fn not_modified(w: &mut Response) {
    for k in ["Content-Type", "Content-Length", "Content-Encoding"] {
        w.header.remove(k);
    }
    w.status = 304;
    w.body.clear();
}

/// Go's `parseRange`: Ok(None) for no (or an ignored) Range, Err for a bad one (with "no overlap" told apart).
fn parse_range(s: &str, size: i64) -> Result<Vec<(i64, i64)>, &'static str> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let Some(spec) = s.strip_prefix("bytes=") else { return Err("invalid range") };
    if spec.matches(',').count() + 1 > 200 {
        return Ok(Vec::new());
    }
    let mut ranges = Vec::new();
    let mut no_overlap = false;
    let int = |t: &str| crate::gotext::atoi(t.as_bytes());
    for ra in spec.split(',') {
        let ra = trim(ra);
        if ra.is_empty() {
            continue;
        }
        let Some((start, end)) = ra.split_once('-') else { return Err("invalid range") };
        let (start, end) = (trim(start), trim(end));
        if start.is_empty() {
            if end.is_empty() || end.starts_with('-') {
                return Err("invalid range");
            }
            let Some(mut i) = int(end).filter(|i| *i >= 0) else { return Err("invalid range") };
            if i > size {
                i = size;
            }
            ranges.push((size - i, i));
        } else {
            let Some(i) = int(start).filter(|i| *i >= 0) else { return Err("invalid range") };
            if i >= size {
                no_overlap = true;
                continue;
            }
            let len = if end.is_empty() {
                size - i
            } else {
                let Some(mut e) = int(end) else { return Err("invalid range") };
                if i > e {
                    return Err("invalid range");
                }
                if e >= size {
                    e = size - 1;
                }
                e - i + 1
            };
            ranges.push((i, len));
        }
    }
    if no_overlap && ranges.is_empty() {
        return Err("invalid range: failed to overlap");
    }
    Ok(ranges)
}

/// Go's `multipart.Writer`'s random boundary (30 random bytes in hex).
fn boundary() -> String {
    let mut b = [0u8; 30];
    // SAFETY: getrandom fills the local buffer of the length given.
    unsafe {
        libc::getrandom(b.as_mut_ptr().cast(), b.len(), 0);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Go's `serveContent` for a file with no time and no ETag.
fn serve_content(w: &mut Response, req: &Request, name: &str, content: &[u8]) {
    if if_match(req) == Cond::False {
        w.status = 412;
        return;
    }
    let get_or_head = req.method == "GET" || req.method == "HEAD";
    if if_none_match(req) == Cond::False {
        if get_or_head {
            not_modified(w);
        } else {
            w.status = 412;
        }
        return;
    }
    let mut range = header(req, "Range");
    // If-Range: an ETag never matches ours, and a date cannot match a file without one
    if !range.is_empty() && get_or_head && !req.header("If-Range").is_empty() {
        range.clear();
    }
    let ctype = content_type(name);
    w.set("Content-Type", ctype);
    let size = content.len() as i64;
    let mut ranges = match parse_range(&range, size) {
        Ok(r) => r,
        Err(e) => {
            if e == "invalid range: failed to overlap" {
                if size == 0 {
                    Vec::new()
                } else {
                    w.set("Content-Range", &format!("bytes */{size}"));
                    serve_error(w, e, 416);
                    return;
                }
            } else {
                serve_error(w, e, 416);
                return;
            }
        }
    };
    if ranges.iter().map(|r| r.1).sum::<i64>() > size {
        ranges.clear();
    }
    let mut code = 200;
    let body: Vec<u8> = match ranges.len() {
        0 => content.to_vec(),
        1 => {
            let (start, len) = ranges[0];
            code = 206;
            w.set("Content-Range", &format!("bytes {}-{}/{size}", start, start + len - 1));
            content[start as usize..(start + len) as usize].to_vec()
        }
        _ => {
            code = 206;
            let bnd = boundary();
            w.set("Content-Type", &format!("multipart/byteranges; boundary={bnd}"));
            let mut out = Vec::new();
            for (i, (start, len)) in ranges.iter().enumerate() {
                if i == 0 {
                    out.extend_from_slice(format!("--{bnd}\r\n").as_bytes());
                } else {
                    out.extend_from_slice(format!("\r\n--{bnd}\r\n").as_bytes());
                }
                out.extend_from_slice(
                    format!("Content-Range: bytes {}-{}/{size}\r\nContent-Type: {ctype}\r\n\r\n", start, start + len - 1).as_bytes(),
                );
                out.extend_from_slice(&content[*start as usize..(start + len) as usize]);
            }
            out.extend_from_slice(format!("\r\n--{bnd}--\r\n").as_bytes());
            out
        }
    };
    w.set("Accept-Ranges", "bytes");
    w.set("Content-Length", &body.len().to_string());
    w.status = code;
    if req.method != "HEAD" {
        w.body = body;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Ok(vec![(0, 10)]));
        assert_eq!(parse_range("bytes=-5", 100), Ok(vec![(95, 5)]));
        assert_eq!(parse_range("bytes=5-", 100), Ok(vec![(5, 95)]));
        assert_eq!(parse_range("bytes=200-", 100), Err("invalid range: failed to overlap"));
        assert_eq!(parse_range("bytes=5-2", 100), Err("invalid range"));
        assert_eq!(parse_range("items=0-1", 100), Err("invalid range"));
        assert_eq!(parse_range("bytes=+1-+2", 100), Ok(vec![(1, 2)]));
        assert_eq!(scan_etag(" W/\"a\" rest"), Some(("W/\"a\"", " rest")));
    }
}
