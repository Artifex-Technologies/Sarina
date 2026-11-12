//! Go's `path/filepath` on Linux, on bytes (a path from a shell word or a file may be any bytes): `Clean`, `Join`, `IsAbs`,
//! `Rel`, `Dir`, `Base`, `Abs` and `EvalSymlinks`, written from Go's algorithms so every edge (`..` above the root, a trailing
//! slash, a link to a relative target) comes out as Go's does.

use std::ffi::OsStr;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

use crate::goerr;

/// ENOTDIR on Linux: a file stands where a folder must be.
const ENOTDIR: i32 = 20;

pub fn os_path(b: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(b))
}

/// `filepath.IsAbs`.
pub fn is_abs(p: &[u8]) -> bool {
    p.first() == Some(&b'/')
}

/// `filepath.Clean`: the shortest equivalent path by lexical processing.
pub fn clean(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let rooted = path[0] == b'/';
    let n = path.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0, 0);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if path[r] == b'/' || path[r] == b'.' && (r + 1 == n || path[r + 1] == b'/') {
            // a separator, or a "." element
            r += 1;
        } else if path[r] == b'.' && path[r + 1] == b'.' && (r + 2 == n || path[r + 2] == b'/') {
            r += 2;
            if out.len() > dotdot {
                out.pop();
                while out.len() > dotdot && out[out.len() - 1] != b'/' {
                    out.pop();
                }
                // Go's lazybuf moves its write index back to the separator, which the next element overwrites
                if out.len() > dotdot && out[out.len() - 1] == b'/' {
                    out.pop();
                }
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if rooted && out.len() != 1 || !rooted && !out.is_empty() {
                out.push(b'/');
            }
            while r < n && path[r] != b'/' {
                out.push(path[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `filepath.Join`: the non-empty elements joined by `/` and cleaned; "" when all are empty.
pub fn join(elems: &[&[u8]]) -> Vec<u8> {
    let parts: Vec<&[u8]> = elems.iter().copied().filter(|e| !e.is_empty()).collect();
    if parts.is_empty() {
        return Vec::new();
    }
    clean(&parts.join(&b'/'))
}

/// `filepath.Base`.
pub fn base(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        return b".".to_vec();
    }
    let mut p = path;
    while p.len() > 1 && p[p.len() - 1] == b'/' {
        p = &p[..p.len() - 1];
    }
    if p == b"/" {
        return b"/".to_vec();
    }
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) => p[i + 1..].to_vec(),
        None => p.to_vec(),
    }
}

/// `filepath.Dir`.
pub fn dir(path: &[u8]) -> Vec<u8> {
    let i = path.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    clean(&path[..i])
}

/// `filepath.Rel`, with Go's error text.
pub fn rel(basepath: &[u8], targpath: &[u8]) -> Result<Vec<u8>, String> {
    let base0 = clean(basepath);
    let targ = clean(targpath);
    if targ == base0 {
        return Ok(b".".to_vec());
    }
    let base: &[u8] = if base0 == b"." { b"" } else { &base0 };
    let base_slashed = base.first() == Some(&b'/');
    let targ_slashed = targ.first() == Some(&b'/');
    let cannot = || Err(format!("Rel: can't make {} relative to {}", crate::gotext::lossy(targpath), crate::gotext::lossy(basepath)));
    if base_slashed != targ_slashed {
        return cannot();
    }
    let (bl, tl) = (base.len(), targ.len());
    let (mut b0, mut bi, mut t0, mut ti) = (0, 0, 0, 0);
    loop {
        while bi < bl && base[bi] != b'/' {
            bi += 1;
        }
        while ti < tl && targ[ti] != b'/' {
            ti += 1;
        }
        if targ[t0..ti] != base[b0..bi] {
            break;
        }
        if bi < bl {
            bi += 1;
        }
        if ti < tl {
            ti += 1;
        }
        b0 = bi;
        t0 = ti;
    }
    if &base[b0..bi] == b".." {
        return cannot();
    }
    if b0 != bl {
        let seps = base[b0..bl].iter().filter(|&&c| c == b'/').count();
        let mut out = b"..".to_vec();
        for _ in 0..seps {
            out.extend_from_slice(b"/..");
        }
        if t0 != tl {
            out.push(b'/');
            out.extend_from_slice(&targ[t0..]);
        }
        return Ok(clean(&out));
    }
    Ok(targ[t0..].to_vec())
}

/// `os.Getwd`: `$PWD` when it is absolute and names the same folder as ".", else the kernel's answer.
pub fn getwd() -> std::io::Result<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    let dot = std::fs::metadata(".")?;
    if let Some(pwd) = std::env::var_os("PWD") {
        let b = pwd.as_bytes();
        if is_abs(b) {
            if let Ok(m) = std::fs::metadata(os_path(b)) {
                if m.dev() == dot.dev() && m.ino() == dot.ino() {
                    return Ok(b.to_vec());
                }
            }
        }
    }
    Ok(std::env::current_dir()?.as_os_str().as_bytes().to_vec())
}

/// `filepath.Abs`.
pub fn abs(path: &[u8]) -> std::io::Result<Vec<u8>> {
    if is_abs(path) {
        return Ok(clean(path));
    }
    let wd = getwd()?;
    Ok(join(&[&wd, path]))
}

/// `filepath.EvalSymlinks`: the path with every link followed, cleaned; an error, with Go's text, when a part is missing, a
/// file stands where a folder must be, or there are more than 255 links.
pub fn eval_symlinks(path: &[u8]) -> Result<Vec<u8>, String> {
    let mut path = path.to_vec();
    let mut vol_len = usize::from(path.first() == Some(&b'/'));
    let mut vol: Vec<u8> = path[..vol_len].to_vec();
    let mut dest = vol.clone();
    let mut links = 0;
    let mut start = vol_len;
    while start < path.len() {
        while start < path.len() && path[start] == b'/' {
            start += 1;
        }
        let mut end = start;
        while end < path.len() && path[end] != b'/' {
            end += 1;
        }
        if end == start {
            break;
        }
        let elem = &path[start..end];
        if elem == b"." {
            start = end;
            continue;
        }
        if elem == b".." {
            let r = dest.iter().rposition(|&c| c == b'/').filter(|&r| r >= vol_len);
            match r {
                Some(r) if &dest[r + 1..] != b".." => dest.truncate(r),
                _ => {
                    if dest.len() > vol_len {
                        dest.push(b'/');
                    }
                    dest.extend_from_slice(b"..");
                }
            }
            start = end;
            continue;
        }
        if !dest.is_empty() && dest[dest.len() - 1] != b'/' {
            dest.push(b'/');
        }
        dest.extend_from_slice(elem);
        let md = std::fs::symlink_metadata(os_path(&dest)).map_err(|e| goerr::path("lstat", &dest, &e))?;
        if !md.file_type().is_symlink() {
            if !md.is_dir() && end < path.len() {
                return Err(goerr::errno(ENOTDIR));
            }
            start = end;
            continue;
        }
        links += 1;
        if links > 255 {
            return Err("EvalSymlinks: too many links".into());
        }
        let link = std::fs::read_link(os_path(&dest)).map_err(|e| goerr::path("readlink", &dest, &e))?.into_os_string().into_vec();
        let mut np = link.clone();
        np.extend_from_slice(&path[end..]);
        path = np;
        if link.first() == Some(&b'/') {
            dest = b"/".to_vec();
            end = 1;
            vol = b"/".to_vec();
            vol_len = 1;
        } else {
            match dest.iter().rposition(|&c| c == b'/').filter(|&r| r >= vol_len) {
                Some(r) => dest.truncate(r),
                None => dest = vol.clone(),
            }
            end = 0;
        }
        start = end;
    }
    Ok(clean(&dest))
}

/// What kind of file a folder entry is, as the folder itself says (Go's `fs.DirEntry.Type`: a link is a link).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    Symlink,
    Regular,
    Other,
}

/// One folder entry (Go's `fs.DirEntry`).
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: Vec<u8>,
    pub kind: Kind,
}

fn kind_of(t: std::fs::FileType) -> Kind {
    if t.is_symlink() {
        Kind::Symlink
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_file() {
        Kind::Regular
    } else {
        Kind::Other
    }
}

/// `os.ReadDir`: the entries sorted by name, with Go's error text ("open /x: not a directory").
pub fn read_dir(path: &[u8]) -> Result<Vec<DirEntry>, String> {
    let rd = std::fs::read_dir(os_path(path)).map_err(|e| goerr::path("open", path, &e))?;
    let mut out = Vec::new();
    for e in rd {
        let e = e.map_err(|e| goerr::path("readdirent", path, &e))?;
        let kind = e.file_type().map(kind_of).unwrap_or(Kind::Other);
        out.push(DirEntry { name: e.file_name().into_vec(), kind });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// A `walk_dir` callback: the path, its entry and the error of reading it.
pub type WalkFn<'a> = dyn FnMut(&[u8], Option<&DirEntry>, Option<&str>) -> Walk + 'a;

/// What a `walk_dir` callback answers (Go's nil, `SkipDir`, `SkipAll`, or an error that ends the walk).
#[derive(Debug, PartialEq)]
pub enum Walk {
    Continue,
    SkipDir,
    SkipAll,
    Stop(String),
}

/// `filepath.WalkDir`: root and everything below it in lexical order, links not followed. The callback gets the path, the
/// entry (None only when root itself cannot be read) and the error of reading it or its folder (when a folder cannot be
/// read, it is called a second time with the error). SkipDir on a folder skips it; on a file, the rest of its folder.
pub fn walk_dir(root: &[u8], f: &mut WalkFn<'_>) -> Result<(), String> {
    let r = match std::fs::symlink_metadata(os_path(root)) {
        Err(e) => f(root, None, Some(&goerr::path("lstat", root, &e))),
        Ok(m) => walk(root, &DirEntry { name: base(root), kind: kind_of(m.file_type()) }, f),
    };
    match r {
        Walk::Stop(e) => Err(e),
        _ => Ok(()),
    }
}

fn walk(path: &[u8], d: &DirEntry, f: &mut WalkFn<'_>) -> Walk {
    let is_dir = d.kind == Kind::Dir;
    let r = f(path, Some(d), None);
    if r != Walk::Continue || !is_dir {
        return if r == Walk::SkipDir && is_dir { Walk::Continue } else { r };
    }
    let entries = match read_dir(path) {
        Ok(e) => e,
        Err(e) => match f(path, Some(d), Some(&e)) {
            Walk::Continue => Vec::new(),
            Walk::SkipDir => return Walk::Continue,
            r => return r,
        },
    };
    for e in &entries {
        match walk(&join(&[path, &e.name]), e, f) {
            Walk::Continue => {}
            Walk::SkipDir => break,
            r => return r,
        }
    }
    Walk::Continue
}

/// `path.Match`: whether name matches the shell pattern (`*`, `?`, `[...]` with ranges and `^`, `\` escapes; `*` and `?`
/// never match `/`); Err for a malformed pattern (Go's ErrBadPattern).
pub fn path_match(pattern: &[u8], name: &[u8]) -> Result<bool, BadPattern> {
    let mut pattern = pattern;
    let mut name = name;
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            return Ok(!name.contains(&b'/'));
        }
        let (t, ok, err) = match_chunk(chunk, name);
        if ok && (t.is_empty() || !pattern.is_empty()) {
            name = t;
            continue;
        }
        if err {
            return Err(BadPattern);
        }
        if star {
            let mut i = 0;
            while i < name.len() && name[i] != b'/' {
                let (t, ok, err) = match_chunk(chunk, &name[i + 1..]);
                if ok {
                    if pattern.is_empty() && !t.is_empty() {
                        i += 1;
                        continue;
                    }
                    name = t;
                    continue 'pattern;
                }
                if err {
                    return Err(BadPattern);
                }
                i += 1;
            }
        }
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            if match_chunk(chunk, b"").2 {
                return Err(BadPattern);
            }
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

/// Go's `path.ErrBadPattern`.
#[derive(Debug, PartialEq, Eq)]
pub struct BadPattern;

impl std::fmt::Display for BadPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("syntax error in pattern")
    }
}

fn scan_chunk(mut pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    while pattern.first() == Some(&b'*') {
        pattern = &pattern[1..];
        star = true;
    }
    let mut inrange = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' => {
                if i + 1 < pattern.len() {
                    i += 1;
                }
            }
            b'[' => inrange = true,
            b']' => inrange = false,
            b'*' if !inrange => return (star, &pattern[..i], &pattern[i..]),
            _ => {}
        }
        i += 1;
    }
    (star, pattern, b"")
}

fn decode(s: &[u8]) -> (char, usize) {
    match crate::gotext::decode_rune(s) {
        (Some(c), n) => (c, n),
        (None, n) => ('\u{FFFD}', n.max(1)),
    }
}

/// Go's matchChunk: (the rest of s, matched, the pattern is malformed).
fn match_chunk<'a>(mut chunk: &[u8], mut s: &'a [u8]) -> (&'a [u8], bool, bool) {
    let mut failed = false;
    while !chunk.is_empty() {
        failed = failed || s.is_empty();
        match chunk[0] {
            b'[' => {
                let mut r = '\0';
                if !failed {
                    let (c, n) = decode(s);
                    r = c;
                    s = &s[n..];
                }
                chunk = &chunk[1..];
                let mut negated = false;
                if chunk.first() == Some(&b'^') {
                    negated = true;
                    chunk = &chunk[1..];
                }
                let mut matched = false;
                let mut nrange = 0;
                loop {
                    if chunk.first() == Some(&b']') && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let Some((lo, rest)) = get_esc(chunk) else { return (b"", false, true) };
                    chunk = rest;
                    let mut hi = lo;
                    if chunk[0] == b'-' {
                        let Some((h, rest)) = get_esc(&chunk[1..]) else { return (b"", false, true) };
                        hi = h;
                        chunk = rest;
                    }
                    matched = matched || lo <= r && r <= hi;
                    nrange += 1;
                }
                failed = failed || matched == negated;
            }
            b'?' => {
                if !failed {
                    failed = s[0] == b'/';
                    let (_, n) = decode(s);
                    s = &s[n..];
                }
                chunk = &chunk[1..];
            }
            c => {
                let mut c = c;
                if c == b'\\' {
                    chunk = &chunk[1..];
                    if chunk.is_empty() {
                        return (b"", false, true);
                    }
                    c = chunk[0];
                }
                if !failed {
                    failed = c != s[0];
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    if failed {
        (b"", false, false)
    } else {
        (s, true, false)
    }
}

/// Go's getEsc: a possibly escaped character of a class and the rest; None for ErrBadPattern (which includes nothing left
/// after it).
fn get_esc(mut chunk: &[u8]) -> Option<(char, &[u8])> {
    if chunk.is_empty() || chunk[0] == b'-' || chunk[0] == b']' {
        return None;
    }
    if chunk[0] == b'\\' {
        chunk = &chunk[1..];
        if chunk.is_empty() {
            return None;
        }
    }
    let (r, n) = crate::gotext::decode_rune(chunk);
    let rest = &chunk[n.max(1)..];
    if r.is_none() || rest.is_empty() {
        return None;
    }
    Some((r.unwrap_or('\u{FFFD}'), rest))
}
