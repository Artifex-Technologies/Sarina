//! Go: `go/vault`: Sarina's linked notes, a folder of Markdown files with [[wikilinks]], #tags and a small front matter.
//! Ported part by part, each proved against Go's own answers (testdata/parity/vault.json, ...): reading a vault
//! ([`parse_note`], [`Vault::notes`], [`resolve`], [`Vault::safe_path`]), the keyword, link and recency search
//! ([`search`], [`backlinks`], [`suggest`]), lessons and lint. A note's bytes that are not UTF-8 read as U+FFFD, one per
//! byte, as the model is shown them by Go too.

pub mod search;

use crate::gopath;
use crate::gotext::{self, lossy};
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub use search::{backlinks, search, suggest, Hit};

/// Go's `Note`.
#[derive(Clone, Debug, Default)]
pub struct Note {
    /// relative to the vault, with forward slashes
    pub path: String,
    pub title: String,
    pub tags: Vec<String>,
    /// targets of [[wikilinks]], as written (without #heading or |alias)
    pub links: Vec<String>,
    /// everything after the front matter
    pub body: String,
    pub front: BTreeMap<String, String>,
    /// the file's modification time in nanoseconds since 1970 (0: none)
    pub modified: i128,
}

/// Go's `Vault`.
#[derive(Clone, Debug)]
pub struct Vault {
    pub root: String,
}

fn re(go: &str) -> regex::Regex {
    crate::goregexp::compile(go).expect("a fixed pattern")
}

static WIKILINK_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"\[\[([^\]\|#]+)(?:#[^\]\|]*)?(?:\|[^\]]*)?\]\]"));
static TAG_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"(?:^|\s)#([A-Za-z][A-Za-z0-9_/-]*)"));
static HEADING_RE: LazyLock<regex::Regex> = LazyLock::new(|| re(r"(?m)^#\s+(.+?)\s*$"));

/// Go's `ErrOutside`.
pub const ERR_OUTSIDE: &str = "not a note name inside the vault: give only a name such as Lessons/Verify after writing or Inbox/Voice test (the vault folder is already known, so never a full path), with no hidden files";

/// Go's `strings.Trim(s, cutset)`.
fn trim_set<'a>(s: &'a str, set: &str) -> &'a str {
    s.trim_matches(|c| set.contains(c))
}

impl Vault {
    /// Go's `Open`: the folder made absolute, its links resolved; refused when it is not a folder.
    pub fn open(root: &str) -> Result<Vault, String> {
        let mut abs = gopath::abs(root.as_bytes()).map_err(|e| crate::goerr::text(&e))?;
        if let Ok(r) = gopath::eval_symlinks(&abs) {
            abs = r;
        }
        if !std::fs::metadata(gopath::os_path(&abs)).is_ok_and(|m| m.is_dir()) {
            return Err(format!("vault folder {root} does not exist"));
        }
        Ok(Vault { root: lossy(&abs) })
    }

    /// Go's `Notes`: every Markdown file of at most 512 KiB (not in hidden folders, not hidden), newest first; a link only
    /// when it leads to a note inside the vault, a named pipe or device under a note's name never waited on.
    pub fn notes(&self) -> Vec<Note> {
        let root = self.root.as_bytes().to_vec();
        let mut out = Vec::new();
        let _ = gopath::walk_dir(&root, &mut |p, d, err| {
            let Some(d) = d else { return gopath::Walk::Continue };
            if err.is_some() {
                return gopath::Walk::Continue;
            }
            if d.kind == gopath::Kind::Dir {
                if p != root.as_slice() && d.name.starts_with(b".") {
                    return gopath::Walk::SkipDir;
                }
                return gopath::Walk::Continue;
            }
            if !d.name.ends_with(b".md") || d.name.starts_with(b".") {
                return gopath::Walk::Continue;
            }
            let Ok(info) = std::fs::symlink_metadata(gopath::os_path(p)) else { return gopath::Walk::Continue };
            if info.len() > 512 * 1024 {
                return gopath::Walk::Continue;
            }
            if d.kind == gopath::Kind::Symlink {
                let Ok(target) = gopath::eval_symlinks(p) else { return gopath::Walk::Continue };
                match gopath::rel(&root, &target) {
                    Ok(rel) if rel != b".." && !rel.starts_with(b"../") => {}
                    _ => return gopath::Walk::Continue,
                }
            }
            let Ok(b) = crate::session::readfile::read_regular(p, 512 * 1024) else { return gopath::Walk::Continue };
            let rel = gopath::rel(&root, p).unwrap_or_default();
            let modified = {
                use std::os::unix::fs::MetadataExt;
                i128::from(info.mtime()) * 1_000_000_000 + i128::from(info.mtime_nsec())
            };
            out.push(parse_note(&lossy(&rel), &lossy(&b), modified));
            gopath::Walk::Continue
        });
        crate::gosort::slice(&mut out, |a, b| a.modified > b.modified);
        out
    }

    /// Go's `SafePath`: a note name as a path inside the vault, ".md" added; hidden files and folders, "..", absolute
    /// paths and anything that is not a plain note name refused, and a linked folder that leads out of the vault. (relative,
    /// absolute).
    pub fn safe_path(&self, name: &str) -> Result<(String, String), String> {
        let name = gotext::trim_space(name);
        if name.is_empty() || name.starts_with('/') || name.contains(['\0', '\\']) || name.len() > 200 {
            return Err(ERR_OUTSIDE.into());
        }
        let name = if name.ends_with(".md") { name.to_string() } else { format!("{name}.md") };
        let clean = lossy(&gopath::clean(name.as_bytes()));
        if clean.split('/').any(|part| part == ".." || part == "." || part.is_empty() || part.starts_with('.')) {
            return Err(ERR_OUTSIDE.into());
        }
        let root = self.root.as_bytes();
        let abs = gopath::join(&[root, clean.as_bytes()]);
        let inside = |p: &[u8]| matches!(gopath::rel(root, p), Ok(rel) if !rel.starts_with(b".."));
        if !inside(&abs) {
            return Err(ERR_OUTSIDE.into());
        }
        if let Ok(dir) = gopath::eval_symlinks(&gopath::dir(&abs)) {
            if !inside(&dir) {
                return Err(ERR_OUTSIDE.into());
            }
        }
        Ok((clean, lossy(&abs)))
    }

    /// Go's `Lessons`: the notes under Lessons/ with a rule in their front matter (status "proposed" when none), by name.
    pub fn lessons(&self) -> Vec<Lesson> {
        let mut out: Vec<Lesson> = self
            .notes()
            .into_iter()
            .filter(|n| n.path.starts_with("Lessons/"))
            .filter_map(|n| {
                let rule = trim_set(n.front.get("rule").map_or("", String::as_str), "\"' ");
                if rule.is_empty() {
                    return None;
                }
                let mut status = gotext::to_lower(trim_set(n.front.get("status").map_or("", String::as_str), "\"' "));
                if status.is_empty() {
                    status = "proposed".into();
                }
                Some(Lesson { note: n.path.clone(), rule: gotext::fields(rule).join(" "), status })
            })
            .collect();
        out.sort_by(|a, b| a.note.cmp(&b.note));
        out
    }
}

/// Go's `ParseNote`: a note's structure from its text (rel: its path inside the vault).
pub fn parse_note(rel: &str, text: &str, modified: i128) -> Note {
    let mut n = Note { path: rel.to_string(), modified, ..Note::default() };
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].split('\n') {
                if let Some((k, v)) = line.split_once(':') {
                    n.front.insert(gotext::to_lower(gotext::trim_space(k)), gotext::trim_space(v).to_string());
                }
            }
            body = rest[end + 4..].trim_start_matches('\n');
        }
    }
    n.body = body.to_string();
    let base = lossy(&gopath::base(rel.as_bytes()));
    n.title = base.strip_suffix(".md").unwrap_or(&base).to_string();
    if let Some(m) = HEADING_RE.captures(body).and_then(|c| c.get(1)) {
        n.title = m.as_str().to_string();
    }
    for c in WIKILINK_RE.captures_iter(body) {
        n.links.push(gotext::trim_space(&c[1]).to_string());
    }
    let mut add = |t: &str| {
        let t = gotext::to_lower(trim_set(t, " ,#\"'"));
        if !t.is_empty() && !n.tags.contains(&t) {
            n.tags.push(t);
        }
    };
    for c in TAG_RE.captures_iter(body) {
        add(&c[1]);
    }
    if let Some(t) = n.front.get("tags").cloned() {
        for p in trim_set(&t, "[]").split([',', ' ']).filter(|p| !p.is_empty()) {
            add(p);
        }
    }
    n
}

/// Go's `Resolve`: the note a [[link]] means, by path (or by file name, as Obsidian does), then by its title, ignoring
/// case; "" when there is none. A leading "/" means the vault's top.
pub fn resolve(notes: &[Note], target: &str) -> String {
    let s = gotext::trim_space(target).trim_start_matches('/');
    let t = gotext::to_lower(s.strip_suffix(".md").unwrap_or(s));
    for n in notes {
        let p = gotext::to_lower(n.path.strip_suffix(".md").unwrap_or(&n.path));
        if p == t || p.ends_with(&format!("/{t}")) {
            return n.path.clone();
        }
    }
    let want = gotext::trim_space(target.strip_suffix(".md").unwrap_or(target));
    for n in notes {
        if gotext::equal_fold(gotext::trim_space(&n.title), want) {
            return n.path.clone();
        }
    }
    String::new()
}

/// Go's `Lesson`.
#[derive(Clone, Debug, PartialEq)]
pub struct Lesson {
    pub note: String,
    pub rule: String,
    pub status: String,
}

/// Go's `Digest`: the active lessons' rules as a Markdown list.
pub fn digest(ls: &[Lesson]) -> String {
    let mut b = String::from(
        "# Sarina's lessons (made by `sarina lessons digest` from the vault's active lessons; edit the notes, not this file)\n\n",
    );
    for l in ls.iter().filter(|l| l.status == "active") {
        b += &format!("- {}\n", l.rule);
    }
    b
}

/// Go's `Problem`: one thing [`lint`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub path: String,
    pub what: String,
}

/// Go's `Lint`: broken links, notes nothing links to or from, notes with no front matter; by path (in Go's sort.Slice
/// order where a note has several).
pub fn lint(notes: &[Note]) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut linked = std::collections::HashSet::new();
    for n in notes {
        for l in &n.links {
            let t = resolve(notes, l);
            if t.is_empty() {
                out.push(Problem { path: n.path.clone(), what: format!("links to [[{l}]], which does not exist") });
            } else if t != n.path {
                linked.insert(t);
                linked.insert(n.path.clone());
            }
        }
    }
    for n in notes {
        if !linked.contains(&n.path) && n.path != "Home.md" {
            out.push(Problem { path: n.path.clone(), what: "no links in or out (an orphan)".into() });
        }
        if n.front.is_empty() {
            out.push(Problem { path: n.path.clone(), what: "no front matter (created / updated / source)".into() });
        }
    }
    crate::gosort::slice(&mut out, |a, b| a.path < b.path);
    out
}
