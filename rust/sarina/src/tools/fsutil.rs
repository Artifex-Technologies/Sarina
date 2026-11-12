//! Go: `go/tools/fsutil.go`, the path half: where a file tool's path leads, and whether that stays in the workspace
//! (`ResolveInRoot` for a path that must exist, `ResolveForWrite` for one that may not yet), and the text helpers
//! (`TruncateOutput`, `preview`, `relPath`).

use std::os::unix::ffi::OsStrExt;

use crate::gopath;
use crate::gotext::is_space;
use crate::tools::private::home_dir;

/// Go's `TruncateOutput`: the head and tail of s when it exceeds max characters.
pub fn truncate_output(s: &str, max: i64) -> String {
    let r: Vec<char> = s.chars().collect();
    if max <= 0 || r.len() as i64 <= max {
        return s.to_owned();
    }
    let max = max as usize;
    let head = max * 2 / 3;
    let tail = max - head;
    let omitted = r.len() - head - tail;
    let mut out: String = r[..head].iter().collect();
    out.push_str(&format!("\n[... {omitted} characters omitted ...]\n"));
    out.extend(&r[r.len() - tail..]);
    out
}

/// Go's `expandHome`: "~", "~/x" and "~\x" against $HOME (left as they are when $HOME is empty).
pub fn expand_home(p: &str) -> String {
    if p == "~" || p.starts_with("~/") || p.starts_with("~\\") {
        let home = home_dir();
        if !home.is_empty() {
            return String::from_utf8_lossy(&gopath::join(&[home.as_bytes(), &p.as_bytes()[1..]])).into_owned();
        }
    }
    p.to_owned()
}

/// Go's `realRoot`: root with its links followed, or root when that fails.
pub fn real_root(root: &[u8]) -> Vec<u8> {
    gopath::eval_symlinks(root).unwrap_or_else(|_| root.to_vec())
}

/// Go's `within`: p is root or below it.
pub fn within(root: &[u8], p: &[u8]) -> bool {
    gopath::rel(root, p).is_ok_and(|r| r != b".." && !r.starts_with(b"../"))
}

fn full_path(root: &str, p: &str) -> Vec<u8> {
    let mut full = expand_home(p).into_bytes();
    if !gopath::is_abs(&full) {
        full = gopath::join(&[root.as_bytes(), &full]);
    }
    gopath::clean(&full)
}

/// Go's `ResolveInRoot`: p against root for a path that must exist, following links; anything that escapes the workspace
/// is refused.
pub fn resolve_in_root(root: &str, p: &str) -> Result<Vec<u8>, String> {
    if p.is_empty() {
        return Err("path is required".into());
    }
    let resolved = gopath::eval_symlinks(&full_path(root, p)).map_err(|e| format!("cannot access {p}: {e}"))?;
    if !within(&real_root(root.as_bytes()), &resolved) {
        return Err(format!("{p} is outside the workspace"));
    }
    Ok(resolved)
}

/// Go's `ResolveForWrite`: resolve_in_root for a path that may not exist yet. The deepest existing ancestor is resolved
/// through links, so a write cannot leave the workspace through a link, and a dangling link is refused rather than followed.
pub fn resolve_for_write(root: &str, p: &str) -> Result<Vec<u8>, String> {
    if p.is_empty() {
        return Err("path is required".into());
    }
    let full = full_path(root, p);
    let mut cur = full;
    let mut tail: Vec<Vec<u8>> = Vec::new();
    while std::fs::symlink_metadata(std::ffi::OsStr::from_bytes(&cur)).is_err() {
        let parent = gopath::dir(&cur);
        if parent == cur {
            return Err(format!("cannot access {p}"));
        }
        tail.insert(0, gopath::base(&cur));
        cur = parent;
    }
    let resolved = gopath::eval_symlinks(&cur).map_err(|e| format!("cannot write {p}: {e} (a dangling symlink is not followed)"))?;
    let mut parts: Vec<&[u8]> = vec![&resolved];
    parts.extend(tail.iter().map(Vec::as_slice));
    let resolved = gopath::join(&parts);
    if !within(&real_root(root.as_bytes()), &resolved) {
        return Err(format!("{p} is outside the workspace"));
    }
    Ok(resolved)
}

/// Go's `relPath`: p relative to the workspace root (or its real path), else p.
pub fn rel_path(root: &str, p: &[u8]) -> Vec<u8> {
    for base in [root.as_bytes().to_vec(), real_root(root.as_bytes())] {
        if let Ok(rel) = gopath::rel(&base, p) {
            if rel != b".." && !rel.starts_with(b"../") {
                return rel;
            }
        }
    }
    p.to_vec()
}

/// Go's `preview`: whitespace runs collapsed to one space, cut to n characters (with "...").
pub fn preview(s: &str, n: usize) -> String {
    let joined = s.split(is_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let r: Vec<char> = joined.chars().collect();
    if r.len() > n {
        let mut out: String = r[..n].iter().collect();
        out.push_str("...");
        return out;
    }
    joined
}
