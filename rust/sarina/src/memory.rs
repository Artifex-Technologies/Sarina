//! Go: `go/memory` (runtime.go, tools.go): agent-curated memory. Two scopes of short entries the agent writes itself and
//! that are put in front of the model every later session, so every write passes the shared gate first: the "agent"
//! scope at <workspace>/.port_sessions/memory/AGENT_MEMORY.md (resolved inside the workspace on every use, never a
//! secret or private file), the "user" scope at ~/.sarina/memory/USER_MEMORY.md. Files are plain text, entries
//! separated by a "---" line; only a regular file of at most 1 MiB is read, and a write over one that cannot be read is
//! refused (testdata/parity/memory.json). The review queue (review.go) comes with the CLI.

use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;

use crate::goany::Any;
use crate::gotext::{self, trim_space};
use crate::tools::fsutil::resolve_for_write;
use crate::tools::registry::{Context, Output, Tool};
use crate::{goos, gopath, goregexp};

pub const MAX_ENTRY_CHARS: usize = 1500;
pub const MAX_SCOPE_CHARS: usize = 4000;
pub const DEFAULT_MAX_RELEVANT_ENTRIES: usize = 8;
const ENTRY_DELIMITER: &str = "\n---\n";
const MAX_MEMORY_FILE_BYTES: u64 = 1 << 20;
pub const SCOPES: &[&str] = &["agent", "user"];

pub const PROMPT_HEADER: &str = "The entries below are notes you saved for yourself in earlier sessions (the \"agent\" scope) and notes \
                                 about the operator carried across projects (the \"user\" scope). This is a frozen snapshot captured \
                                 at the start of this session -- changes you make with memory_add/memory_replace/memory_remove during \
                                 this session take effect for future sessions, not this one.";

/// Vets a write: (blocked, reason).
pub type Scanner = Arc<dyn Fn(&str) -> (bool, String) + Send + Sync>;

/// What a write did.
#[derive(Clone, Debug, PartialEq)]
pub struct Mutation {
    pub scope: String,
    pub action: String,
    pub path: String,
    pub before_count: usize,
    pub after_count: usize,
}

/// The two memory files (Go's `Runtime`).
#[derive(Clone, Default)]
pub struct Runtime {
    pub agent_path: String,
    pub user_path: String,
    /// the workspace agent_path lies in: the agent file is resolved inside it on every use ("" = taken as it is)
    pub root: String,
    /// None: the memory gate (local checks, then Heimdall when it is configured)
    pub scanner: Option<Scanner>,
}

static WRITE: Mutex<()> = Mutex::new(());
static WORD_RE: LazyLock<Regex> = LazyLock::new(|| goregexp::compile("[a-z0-9']+").expect("a fixed pattern"));

fn join(a: &str, b: &str) -> String {
    gotext::lossy(&gopath::join(&[a.as_bytes(), b.as_bytes()]))
}

fn rune_len(s: &str) -> usize {
    s.chars().count()
}

fn used(entries: &[String], skip: Option<usize>) -> usize {
    entries.iter().enumerate().filter(|(i, _)| Some(*i) != skip).map(|(_, e)| rune_len(e)).sum()
}

fn normalize(s: &str) -> String {
    gotext::to_lower(&gotext::fields(s).join(" "))
}

/// Go's `workspaceFile`: p resolved inside root as a file tool's write is; never a secret or private file.
fn workspace_file(root: &str, p: &str) -> Result<String, String> {
    let rel = gotext::lossy(&gopath::rel(root.as_bytes(), p.as_bytes())?);
    let resolved = resolve_for_write(root, &rel)?;
    if crate::tools::secret::is_secret_path(&resolved) || crate::tools::private::is_private(&resolved) {
        return Err(format!("{rel} leads to a file the file tools never use"));
    }
    Ok(gotext::lossy(&resolved))
}

/// Go's `readStateFile`: None for no file; an error for anything but a regular file of at most 1 MiB.
fn read_state_file(p: &str) -> Result<Option<Vec<u8>>, String> {
    let st = match std::fs::metadata(p) {
        Ok(st) => st,
        Err(e) if e.raw_os_error() == Some(2) => return Ok(None),
        Err(e) => return Err(crate::goerr::path("stat", p.as_bytes(), &e)),
    };
    if !st.is_file() {
        return Err(format!("{p} is not a regular file"));
    }
    if st.len() > MAX_MEMORY_FILE_BYTES {
        return Err(format!("{p} is larger than {MAX_MEMORY_FILE_BYTES} bytes"));
    }
    let f = std::fs::File::open(p).map_err(|e| crate::goerr::path("open", p.as_bytes(), &e))?;
    let mut raw = Vec::new();
    f.take(MAX_MEMORY_FILE_BYTES + 1).read_to_end(&mut raw).map_err(|e| crate::goerr::path("read", p.as_bytes(), &e))?;
    if raw.len() as u64 > MAX_MEMORY_FILE_BYTES {
        return Err(format!("{p} is larger than {MAX_MEMORY_FILE_BYTES} bytes"));
    }
    Ok(Some(raw))
}

fn unreadable(e: &str) -> String {
    format!("cannot use the stored memory, so nothing was changed: {e}")
}

impl Runtime {
    /// Go's `FromWorkspace`: home "" is the user's real home.
    pub fn from_workspace(cwd: &str, home: &str) -> Runtime {
        let home = if home.is_empty() { std::env::var("HOME").unwrap_or_default() } else { home.to_owned() };
        let abs = gopath::abs(cwd.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or_else(|_| cwd.to_owned());
        Runtime {
            agent_path: join(&abs, ".port_sessions/memory/AGENT_MEMORY.md"),
            user_path: join(&home, ".sarina/memory/USER_MEMORY.md"),
            root: abs,
            scanner: None,
        }
    }

    fn path(&self, scope: &str) -> Result<String, String> {
        match scope {
            "agent" if !self.root.is_empty() => workspace_file(&self.root, &self.agent_path),
            "agent" => Ok(self.agent_path.clone()),
            "user" => Ok(self.user_path.clone()),
            _ => Err(format!("unknown memory scope '{scope}' (expected 'agent' or 'user')")),
        }
    }

    fn scan(&self, text: &str) -> (bool, String) {
        match &self.scanner {
            Some(s) => s(text),
            None => {
                let getenv = |k: &str| std::env::var(k).unwrap_or_default();
                let v = crate::guard::Gate::memory().scan(text.as_bytes(), &getenv);
                (v.blocked, v.reason)
            }
        }
    }

    /// Go's `Entries`: the stored entries of a scope (empty when there are none); an error when the file cannot be used.
    pub fn entries(&self, scope: &str) -> Result<Vec<String>, String> {
        let p = self.path(scope)?;
        let Some(raw) = read_state_file(&p)? else { return Ok(Vec::new()) };
        Ok(gotext::to_valid_utf8(&raw).split(ENTRY_DELIMITER).map(trim_space).filter(|e| !e.is_empty()).map(str::to_owned).collect())
    }

    fn check_content(&self, content: &str) -> Result<String, String> {
        let content = trim_space(content);
        if content.is_empty() {
            return Err("memory entry content must not be empty".into());
        }
        let n = rune_len(content);
        if n > MAX_ENTRY_CHARS {
            return Err(format!("entry is {n} chars, over the {MAX_ENTRY_CHARS}-char limit per entry"));
        }
        let (blocked, reason) = self.scan(content);
        if blocked {
            return Err(format!("entry rejected: {reason}"));
        }
        Ok(content.to_owned())
    }

    /// Go's `Add`.
    pub fn add(&self, scope: &str, content: &str) -> Result<Mutation, String> {
        let _g = WRITE.lock().unwrap_or_else(|p| p.into_inner());
        self.path(scope)?;
        let content = self.check_content(content)?;
        let existing = self.entries(scope).map_err(|e| unreadable(&e))?;
        let norm = normalize(&content);
        if existing.iter().any(|e| normalize(e) == norm) {
            return Err("an identical entry is already stored; nothing to add".into());
        }
        let u = used(&existing, None);
        if u + rune_len(&content) > MAX_SCOPE_CHARS {
            return Err(format!(
                "memory scope \"{scope}\" is full ({u}/{MAX_SCOPE_CHARS} chars); replace or remove an existing entry before adding more"
            ));
        }
        let mut updated = existing.clone();
        updated.push(content);
        self.commit(scope, "add", existing.len(), &updated)
    }

    /// Go's `Replace`.
    pub fn replace(&self, scope: &str, old_text: &str, content: &str) -> Result<Mutation, String> {
        let _g = WRITE.lock().unwrap_or_else(|p| p.into_inner());
        self.path(scope)?;
        let content = self.check_content(content)?;
        let existing = self.entries(scope).map_err(|e| unreadable(&e))?;
        let idx = find_unique(&existing, old_text)?;
        if used(&existing, Some(idx)) + rune_len(&content) > MAX_SCOPE_CHARS {
            return Err(format!("replacement would grow memory scope \"{scope}\" past the {MAX_SCOPE_CHARS}-char limit"));
        }
        let mut updated = existing.clone();
        updated[idx] = content;
        self.commit(scope, "replace", existing.len(), &updated)
    }

    /// Go's `Remove`.
    pub fn remove(&self, scope: &str, old_text: &str) -> Result<Mutation, String> {
        let _g = WRITE.lock().unwrap_or_else(|p| p.into_inner());
        self.path(scope)?;
        let existing = self.entries(scope).map_err(|e| unreadable(&e))?;
        let idx = find_unique(&existing, old_text)?;
        let mut updated = existing.clone();
        updated.remove(idx);
        self.commit(scope, "remove", existing.len(), &updated)
    }

    fn commit(&self, scope: &str, action: &str, before: usize, entries: &[String]) -> Result<Mutation, String> {
        use std::io::Write;
        let p = self.path(scope).map_err(|e| unreadable(&e))?;
        crate::testguard::allow(p.as_bytes())?;
        let dir = gopath::dir(p.as_bytes());
        goos::mkdir_all(&dir).map_err(|e| format!("cannot write memory: {e}"))?;
        let (mut f, name) = goos::create_temp(&dir, ".mem-*.tmp").map_err(|e| format!("cannot write memory: {e}"))?;
        let werr = f.write_all(entries.join(ENTRY_DELIMITER).as_bytes());
        drop(f);
        let os_name = gopath::os_path(&name);
        if let Err(e) = werr {
            let _ = std::fs::remove_file(os_name);
            return Err(format!("cannot write memory: {}", crate::goerr::path("write", &name, &e)));
        }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(os_name, std::fs::Permissions::from_mode(0o600));
        if let Err(e) = std::fs::rename(os_name, &p) {
            let _ = std::fs::remove_file(os_name);
            return Err(format!("cannot write memory: rename {} {p}: {}", gotext::lossy(&name), crate::goerr::text(&e)));
        }
        Ok(Mutation { scope: scope.into(), action: action.into(), path: p, before_count: before, after_count: entries.len() })
    }

    /// Go's `RenderPromptBlock`: the whole frozen snapshot ("" when there is no memory).
    pub fn render_prompt_block(&self) -> String {
        let mut blocks = Vec::new();
        for scope in SCOPES {
            let entries = self.entries(scope).unwrap_or_default();
            if entries.is_empty() {
                continue;
            }
            let mut lines =
                vec![format!("## {scope} memory ({}/{MAX_SCOPE_CHARS} chars, {} entries)", used(&entries, None), entries.len())];
            lines.extend(entries.iter().map(|e| format!("- {e}")));
            blocks.push(lines.join("\n"));
        }
        if blocks.is_empty() {
            return String::new();
        }
        let mut all = vec![PROMPT_HEADER.to_owned()];
        all.extend(blocks);
        all.join("\n\n")
    }

    /// Go's `RenderRelevantPromptBlock`: the max_entries entries sharing the most words with the task, in their order.
    pub fn render_relevant_prompt_block(&self, task_text: &str, max_entries: usize) -> String {
        let max_entries = if max_entries == 0 { DEFAULT_MAX_RELEVANT_ENTRIES } else { max_entries };
        let mut all: Vec<(&str, String)> = Vec::new();
        for scope in SCOPES {
            for e in self.entries(scope).unwrap_or_default() {
                all.push((scope, e));
            }
        }
        if all.is_empty() {
            return String::new();
        }
        if all.len() <= max_entries {
            return self.render_prompt_block();
        }
        let task = tokenize(task_text);
        let scores: Vec<usize> = all.iter().map(|(_, e)| tokenize(e).iter().filter(|w| task.contains(*w)).count()).collect();
        let mut order: Vec<usize> = (0..all.len()).collect();
        order.sort_by(|a, b| scores[*b].cmp(&scores[*a]));
        let mut keep: Vec<usize> = order[..max_entries].to_vec();
        keep.sort();
        let omitted = all.len() - keep.len();
        let mut blocks = Vec::new();
        for scope in SCOPES {
            let total = all.iter().filter(|(s, _)| s == scope).count();
            let lines: Vec<String> = keep.iter().filter(|&&i| all[i].0 == *scope).map(|&i| format!("- {}", all[i].1)).collect();
            if lines.is_empty() {
                continue;
            }
            let mut b = vec![format!("## {scope} memory (showing {} of {total} most relevant entries)", lines.len())];
            b.extend(lines);
            blocks.push(b.join("\n"));
        }
        let noun = if omitted == 1 { "entry" } else { "entries" };
        let mut out = vec![format!(
            "{PROMPT_HEADER}\n\n{omitted} less-relevant {noun} omitted for this task; ask the operator if you need the full list."
        )];
        out.extend(blocks);
        out.join("\n\n")
    }

    /// Go's `RenderSummary` (`memory-show`).
    pub fn render_summary(&self) -> String {
        let mut lines = vec!["# Curated Memory".to_owned(), String::new()];
        for scope in SCOPES {
            let entries = self.entries(scope).unwrap_or_default();
            let p = if *scope == "user" { &self.user_path } else { &self.agent_path };
            lines.push(format!("## {scope}"));
            lines.push(format!("- Path: {p}"));
            lines.push(format!("- Usage: {}/{MAX_SCOPE_CHARS} chars, {} entries", used(&entries, None), entries.len()));
            lines.extend(entries.iter().map(|e| format!("  - {}", snapshot(e, 140))));
            lines.push(String::new());
        }
        lines.join("\n").trim_end_matches(['\n', ' ']).to_owned()
    }
}

fn tokenize(s: &str) -> HashSet<String> {
    WORD_RE.find_iter(&gotext::to_lower(s)).map(|m| m.as_str().to_owned()).collect()
}

fn snapshot(s: &str, limit: usize) -> String {
    let n = gotext::fields(s).join(" ");
    let r: Vec<char> = n.chars().collect();
    if r.len() > limit {
        return r[..limit - 3].iter().collect::<String>() + "...";
    }
    n
}

fn find_unique(entries: &[String], old_text: &str) -> Result<usize, String> {
    let old = trim_space(old_text);
    if old.is_empty() {
        return Err("old_text must not be empty".into());
    }
    let matches: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| e.contains(old)).map(|(i, _)| i).collect();
    match matches.len() {
        0 => Err("no stored entry contains that text".into()),
        1 => Ok(matches[0]),
        n => Err(format!("that text matches {n} stored entries; give a more specific substring")),
    }
}

// ---- the tools (Go: tools.go) --------------------------------------------------------------------------------------------

fn runtime_for(tc: &Context) -> Runtime {
    match &tc.memory {
        Some(rt) => (**rt).clone(),
        None => Runtime::from_workspace(&tc.root, &tc.memory_home),
    }
}

fn arg(a: &BTreeMap<String, Any>, k: &str) -> String {
    match a.get(k) {
        Some(Any::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn result(verb: &str, m: Result<Mutation, String>) -> Output {
    let m = match m {
        Ok(m) => m,
        Err(e) => return crate::tools::files::fail(e),
    };
    Output {
        content: format!("{verb} {} memory entry ({} total)", m.scope, m.after_count),
        metadata: BTreeMap::from([
            ("action".to_owned(), Any::String(format!("memory_{}", m.action))),
            ("scope".to_owned(), Any::String(m.scope.clone())),
            ("path".to_owned(), Any::String(m.path.clone())),
            ("before_count".to_owned(), Any::Int(m.before_count as i64)),
            ("after_count".to_owned(), Any::Int(m.after_count as i64)),
        ]),
        error: None,
    }
}

/// Go's `SessionDir`: where saved sessions live for a tool context; "" (nothing to search) when that is the workspace's
/// own folder and it leads out of the workspace (`state_dir`).
pub fn session_dir(tc: &Context) -> String {
    let dir = if tc.session_directory.is_empty() { join(&tc.root, ".port_sessions/agent") } else { tc.session_directory.clone() };
    if crate::tools::fsutil::state_dir(&tc.root, &dir).is_err() {
        return String::new();
    }
    dir
}

/// Go's `SearchSessions`: session search rendered as the tool returns it.
pub fn search_sessions(query: &str, dir: &str, max: i64) -> String {
    let hits = crate::sessionsearch::search(query, dir, max);
    if hits.is_empty() {
        return "No matching messages were found in the saved sessions.".into();
    }
    let mut out = format!("Found {} matching message(s) in saved sessions:", hits.len());
    for h in hits {
        out.push_str(&format!("\nsession={} turn={} role={}\n  {}", h.session_id, h.turn_index, h.role, h.snippet));
    }
    out
}

/// Go's `Tools`: memory_add, memory_replace, memory_remove (behind the write permission) and session_search.
pub fn tools() -> Vec<Tool> {
    use crate::tools::defs::tool;
    vec![
        tool("memory_add", Arc::new(|a, tc| result("saved", runtime_for(tc).add(&arg(a, "scope"), &arg(a, "content"))))),
        tool(
            "memory_replace",
            Arc::new(|a, tc| result("replaced", runtime_for(tc).replace(&arg(a, "scope"), &arg(a, "old_text"), &arg(a, "content")))),
        ),
        tool("memory_remove", Arc::new(|a, tc| result("removed", runtime_for(tc).remove(&arg(a, "scope"), &arg(a, "old_text"))))),
        tool(
            "session_search",
            Arc::new(|a, tc| {
                let max = match a.get("max_results") {
                    Some(Any::Number(f)) => *f as i64,
                    Some(Any::Int(i)) => *i,
                    _ => 10,
                };
                crate::tools::files::ok(search_sessions(&arg(a, "query"), &session_dir(tc), max))
            }),
        ),
    ]
}
