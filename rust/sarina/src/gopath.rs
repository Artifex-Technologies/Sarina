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
