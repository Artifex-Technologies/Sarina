//! Go: `go/tools/search.go`: `glob_search` (path.Match per segment, `**` for any number of segments, links reported only
//! when they stay in the workspace) and `grep_search` (Go's regexp syntax through `goregexp`, lines read as bufio.Scanner
//! reads them: at most 1 MiB a line, files over 20 MiB, links, secret files and binary files skipped)
//! (testdata/parity/files.json).

use std::collections::BTreeMap;
use std::io::Read;

use regex::Regex;

use crate::goany::Any;
use crate::gopath::{self, Kind, Walk};
use crate::gotext::{self, to_valid_utf8};
use crate::tools::files::{arg_bool, arg_int, arg_str, fail, ok};
use crate::tools::fsutil::{real_root, rel_path, resolve_in_root, truncate_output, within};
use crate::tools::private::{is_private, private_guard};
use crate::tools::registry::{Context, Output};
use crate::tools::secret::{hard_linked_to_secret, secret_guard, secret_name};
use crate::{goerr, goregexp};

const MAX_GLOB_MATCHES: usize = 20000;
const MAX_GREP_FILE_SIZE: u64 = 20 << 20;
const MAX_GREP_LINE: usize = 500;
/// bufio.Scanner's limit as grep_search sets it: a line (with its line break) must fit in 1 MiB
const MAX_SCAN_TOKEN: usize = 1 << 20;

const PRIVATE_NOTE: &str =
    "\n[a folder the owner keeps private was skipped: what is in it cannot be seen; say so rather than \"not found\"]";

/// Go's `splitGlob`.
fn split_glob(pattern: &str) -> Result<Vec<&[u8]>, String> {
    let p = pattern.as_bytes();
    if p.starts_with(b"/") || p.starts_with(b"~") || (p.len() >= 2 && p[1] == b':') {
        return Err("the pattern must be relative to the workspace, not an absolute path".into());
    }
    let mut segs = Vec::new();
    for s in p.split(|&c| c == b'/') {
        match s {
            b"" | b"." => continue,
            b".." => return Err("the pattern must not contain '..'".into()),
            _ => segs.push(s),
        }
    }
    if segs.is_empty() {
        return Err("the pattern is empty".into());
    }
    Ok(segs)
}

fn seg_match(pat: &[u8], name: &[u8]) -> bool {
    gopath::path_match(pat, name) == Ok(true)
}

/// Go's `globMatch`: "**" matches zero or more whole segments.
fn glob_match(pat: &[&[u8]], name: &[&[u8]]) -> bool {
    let Some((first, rest)) = pat.split_first() else { return name.is_empty() };
    if *first == b"**" {
        return (0..=name.len()).any(|i| glob_match(rest, &name[i..]));
    }
    match name.split_first() {
        Some((n, nrest)) => seg_match(first, n) && glob_match(rest, nrest),
        None => false,
    }
}

/// Go's `couldMatchBelow`: whether anything under the folder name could still match.
fn could_match_below(pat: &[&[u8]], name: &[&[u8]]) -> bool {
    if name.is_empty() || pat.is_empty() {
        return !pat.is_empty() && name.is_empty();
    }
    if pat[0] == b"**" {
        return true;
    }
    if !seg_match(pat[0], name[0]) {
        return false;
    }
    if name.len() == 1 {
        return pat.len() > 1;
    }
    could_match_below(&pat[1..], &name[1..])
}

/// Go's `glob_search` handler.
pub fn glob_search(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let pattern = arg_str(args, "pattern");
    let pat = match split_glob(&pattern) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let root = real_root(tc.root.as_bytes());
    let has_star = pat.iter().any(|s| *s == b"**");
    let mut matches: Vec<Vec<u8>> = Vec::new();
    let mut capped = false;
    let mut skipped_private = 0;
    let _ = gopath::walk_dir(&root, &mut |p, d, err| {
        let Some(d) = d else { return Walk::Continue };
        let is_dir = d.kind == Kind::Dir;
        if err.is_some() {
            return if is_dir { Walk::SkipDir } else { Walk::Continue };
        }
        if p == root.as_slice() {
            return Walk::Continue;
        }
        if is_private(p) {
            skipped_private += 1;
            return if is_dir { Walk::SkipDir } else { Walk::Continue };
        }
        let rel = gopath::rel(&root, p).unwrap_or_default();
        let segs: Vec<&[u8]> = rel.split(|&c| c == b'/').collect();
        if glob_match(&pat, &segs) {
            if d.kind == Kind::Symlink {
                match gopath::eval_symlinks(p) {
                    Ok(real) if within(&root, &real) => {}
                    _ => return Walk::Continue,
                }
            }
            let mut name = rel.clone();
            if is_dir {
                name.push(b'/');
            }
            matches.push(name);
            if matches.len() >= MAX_GLOB_MATCHES {
                capped = true;
                return Walk::SkipAll;
            }
        }
        if is_dir && ((!has_star && segs.len() >= pat.len()) || !could_match_below(&pat, &segs)) {
            return Walk::SkipDir;
        }
        Walk::Continue
    });
    let note = if skipped_private > 0 { PRIVATE_NOTE } else { "" };
    if matches.is_empty() {
        return ok(format!("(no matches){note}"));
    }
    matches.sort();
    let mut out = matches.iter().map(|m| gotext::lossy(m)).collect::<Vec<_>>().join("\n");
    if capped {
        out.push_str(&format!("\n[stopped at {MAX_GLOB_MATCHES} matches; use a narrower pattern]"));
    }
    out.push_str(note);
    ok(truncate_output(&out, tc.max_output_chars))
}

/// bufio.Scanner with ScanLines over r: each line without its line break (and one carriage return before it), until the
/// end, a read error, a line that does not fit in MAX_SCAN_TOKEN, or `f` answering false.
fn scan_lines(mut r: impl Read, f: &mut dyn FnMut(&[u8]) -> bool) {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut from = 0; // where the search for the next line break starts
    loop {
        while let Some(i) = pending[from..].iter().position(|&c| c == b'\n').map(|i| i + from) {
            if i >= MAX_SCAN_TOKEN {
                return;
            }
            let line = pending[..i].strip_suffix(b"\r").unwrap_or(&pending[..i]);
            if !f(line) {
                return;
            }
            pending.drain(..=i);
            from = 0;
        }
        from = pending.len();
        if pending.len() >= MAX_SCAN_TOKEN {
            return;
        }
        match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
        }
    }
    if !pending.is_empty() {
        let line = pending.strip_suffix(b"\r").unwrap_or(&pending);
        f(line);
    }
}

/// Go's `grep_search` handler.
pub fn grep_search(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let src = arg_str(args, "pattern");
    let re: Regex = if arg_bool(args, "literal", false) {
        match regex::RegexBuilder::new(&regex::escape(&src)).size_limit(4 << 20).dfa_size_limit(1 << 20).build() {
            Ok(r) => r,
            Err(e) => return fail(format!("invalid pattern: {e}")),
        }
    } else {
        match goregexp::compile(&src) {
            Ok(r) => r,
            Err(e) => return fail(format!("invalid pattern: {e}")),
        }
    };
    let mut given = arg_str(args, "path");
    if given.is_empty() {
        given = ".".into();
    }
    let start = match resolve_in_root(&tc.root, &given) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if let Err(e) = private_guard(&given, &start, "search") {
        return fail(e);
    }
    let info = match std::fs::metadata(gopath::os_path(&start)) {
        Ok(i) => i,
        Err(e) => return fail(format!("cannot access {given}: {}", goerr::path("stat", &start, &e))),
    };
    if !info.is_dir() {
        if let Err(e) = secret_guard(&given, &start, "search") {
            return fail(e);
        }
        if !info.is_file() {
            // a named pipe or a device would be read without end, or wait for a writer forever
            return fail(format!("{given} is not a regular file"));
        }
    }
    let limit = arg_int(args, "max_matches", 100).max(0) as usize;
    let root = real_root(tc.root.as_bytes());
    let root_s = gotext::lossy(&root);
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;
    let mut skipped_private = 0;

    let search_file = |p: &[u8], out: &mut Vec<String>, truncated: &mut bool| {
        let rel = gotext::lossy(&rel_path(&root_s, p));
        let Ok(mut f) = std::fs::File::open(gopath::os_path(p)) else { return };
        let mut head = Vec::with_capacity(8000);
        let _ = (&mut f).take(8000).read_to_end(&mut head);
        if head.contains(&0) {
            return;
        }
        let mut ln = 0;
        scan_lines(std::io::Cursor::new(head).chain(f), &mut |line| {
            ln += 1;
            if !re.is_match(&gotext::lossy(line)) {
                return true;
            }
            let mut text = to_valid_utf8(line);
            if let Some((i, _)) = text.char_indices().nth(MAX_GREP_LINE) {
                text.truncate(i);
                text.push_str("...");
            }
            out.push(format!("{rel}:{ln}: {text}"));
            if out.len() >= limit {
                *truncated = true;
                return false;
            }
            true
        });
    };

    if !info.is_dir() {
        search_file(&start, &mut out, &mut truncated);
    } else {
        let _ = gopath::walk_dir(&start, &mut |p, d, err| {
            if truncated {
                return Walk::SkipAll;
            }
            let Some(d) = d else { return Walk::Continue };
            if err.is_some() {
                return if d.kind == Kind::Dir { Walk::SkipDir } else { Walk::Continue };
            }
            if d.kind == Kind::Dir {
                if matches!(&d.name[..], b".git" | b".hg" | b".svn") {
                    return Walk::SkipDir;
                }
                if is_private(p) {
                    skipped_private += 1;
                    return Walk::SkipDir;
                }
                return Walk::Continue;
            }
            // only regular files: links are never followed, so a link cannot lead the search out of the workspace or to a
            // secret file
            if d.kind != Kind::Regular || secret_name(&d.name) {
                return Walk::Continue;
            }
            match std::fs::symlink_metadata(gopath::os_path(p)) {
                Ok(m) if m.len() <= MAX_GREP_FILE_SIZE && !hard_linked_to_secret(p) => {}
                _ => return Walk::Continue,
            }
            search_file(p, &mut out, &mut truncated);
            if truncated {
                return Walk::SkipAll;
            }
            Walk::Continue
        });
    }
    let note = if skipped_private > 0 { PRIVATE_NOTE } else { "" };
    if out.is_empty() {
        return ok(format!("(no matches){note}"));
    }
    let mut text = out.join("\n") + note;
    if truncated {
        text.push_str(&format!("\n[stopped at {limit} matches; narrow the pattern or path]"));
    }
    ok(truncate_output(&text, tc.max_output_chars))
}
