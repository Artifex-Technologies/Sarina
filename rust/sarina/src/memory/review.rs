//! Go: `memory/review.go`: the review pass over a finished session (the model proposes what is worth remembering) and the
//! queue of proposals waiting for the operator, `<workspace>/.port_sessions/memory/pending_review.json` (resolved inside
//! the workspace on every use, read only as a bounded regular file). Nothing is written to memory without an approval,
//! and an approval goes through the same gate as an interactive write.

use super::{read_state_file, workspace_file, Mutation, Runtime, WRITE};
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gojson::{self, Obj};
use crate::gostruct::{self, Ty, Val};
use crate::gotext;
use crate::llm::openai::Request;
use crate::llm::types::Message;
use crate::llm::Completer;
use crate::session::{self, Message as SessionMessage};
use crate::{goos, gopath};
use regex::Regex;
use std::sync::LazyLock;

pub const MAX_PROPOSALS_PER_REVIEW: usize = 5;
pub const MAX_TRANSCRIPT_CHARS: usize = 8000;

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_APPROVED: &str = "approved";
pub const STATUS_REJECTED: &str = "rejected";
pub const STATUS_BLOCKED: &str = "blocked";

/// The fixed prompt of the review pass.
pub const REVIEW_SYSTEM_PROMPT: &str = "You are reviewing a finished coding-agent session to decide what, if anything, is worth remembering for future sessions.

Propose at most 5 durable memory entries. A durable entry is a fact that will still be true and useful in a different session: an environment quirk, a project convention, a workaround for a specific bug, or a stated user preference. Do NOT propose entries for: task-specific details that won't recur, anything already obvious from reading the code, or instructions to change your own behavior.

Respond with ONLY a JSON array (no prose, no markdown fences). Each item:
{\"scope\": \"agent\"|\"user\", \"action\": \"add\"|\"replace\"|\"remove\", \"old_text\": string or null, \"content\": string or null, \"rationale\": string}

- scope \"agent\": project-specific facts. scope \"user\": facts about the operator that hold across projects.
- action \"add\": content is the new entry. action \"replace\": old_text is a substring uniquely identifying an existing entry to update, content is its replacement. action \"remove\": old_text identifies an entry to delete, content should be null.
- If nothing is worth remembering, respond with an empty array: []
";

/// Vets a proposal's content: (blocked, reason).
pub type ScanFn<'a> = &'a dyn Fn(&str) -> (bool, String);
type ApplyFn<'a> = &'a dyn Fn(&Proposal) -> Result<Mutation, String>;

/// One queued suggestion; the field order and keys are pending_review.json's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proposal {
    pub entry_id: String,
    pub session_id: String,
    pub scope: String,
    pub action: String,
    pub old_text: Option<String>,
    pub content: Option<String>,
    pub rationale: String,
    pub status: String,
    pub blocked_reason: Option<String>,
    pub created_at: String,
}

fn opt_json(v: &Option<String>, html: bool) -> String {
    match v {
        Some(s) => Obj::new(html).str("v", s).end().map(|o| o[5..o.len() - 1].to_string()).unwrap_or_default(),
        None => "null".into(),
    }
}

impl Proposal {
    /// Go's `json.Marshal` (html true) or the Encoder without HTML escaping (false) of the struct, compact.
    pub fn to_json(&self, html: bool) -> String {
        Obj::new(html)
            .str("entry_id", &self.entry_id)
            .str("session_id", &self.session_id)
            .str("scope", &self.scope)
            .str("action", &self.action)
            .raw("old_text", &opt_json(&self.old_text, html))
            .raw("content", &opt_json(&self.content, html))
            .str("rationale", &self.rationale)
            .str("status", &self.status)
            .raw("blocked_reason", &opt_json(&self.blocked_reason, html))
            .str("created_at", &self.created_at)
            .end()
            .unwrap_or_default()
    }
}

fn s(p: &Option<String>) -> &str {
    p.as_deref().unwrap_or("")
}

/// Go's `BuildTranscript`: the user, assistant and tool text of a session, the last `max_chars` characters kept;
/// system messages and the context reminder left out.
pub fn build_transcript(msgs: &[SessionMessage], max_chars: usize) -> String {
    let parts: Vec<String> = msgs
        .iter()
        .filter_map(|m| {
            let c = gotext::trim_space(&m.content);
            (!(m.role.is_empty() || m.role == "system" || c.is_empty() || m.is_reminder())).then(|| format!("{}: {c}", m.role))
        })
        .collect();
    let t = parts.join("\n\n");
    let n = t.chars().count();
    if n > max_chars {
        return format!("...(truncated)...\n{}", t.chars().skip(n - max_chars).collect::<String>());
    }
    t
}

/// Go's fenced-array pattern, written out: its `\s` is ASCII and its `[\s\S]` is any character.
static FENCED_ARRAY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"```(?:json)?[\t\n\f\r ]*(\[(?s:.)*?\])[\t\n\f\r ]*```").expect("a fixed pattern"));

/// Go's `extractJSONArray`: the array in a model's answer (fenced, the whole text, or from the first `[` to the last `]`).
pub fn extract_json_array(text: &str) -> Option<Vec<Any>> {
    let text = gotext::trim_space(text);
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(c) = FENCED_ARRAY.captures(text) {
        candidates.push(c.get(1).map_or("", |m| m.as_str()));
    }
    candidates.push(text);
    if let (Some(s), Some(e)) = (text.find('['), text.rfind(']')) {
        if e > s {
            candidates.push(&text[s..=e]);
        }
    }
    for c in candidates {
        // json.Unmarshal into []any: an array (null leaves it nil, which does not count)
        let (v, err) = gojson::unmarshal_any(c.as_bytes());
        if err.is_none() {
            if let Ok(items) = v.into_array() {
                return Some(items);
            }
        }
    }
    None
}

fn opt_text(v: Option<&Any>) -> Option<String> {
    match v {
        Some(Any::String(s)) => {
            let t = gotext::trim_space(s);
            (!t.is_empty()).then(|| t.to_string())
        }
        _ => None,
    }
}

fn now() -> String {
    // Go's "2006-01-02T15:04:05.000000-07:00" in UTC
    let t = crate::gotime::Time::now();
    let f = t.format_rfc3339_utc();
    format!("{}.{:06}+00:00", &f[..f.len() - 1], t.nanos() / 1000)
}

fn rand_hex(n: usize) -> String {
    use ring::rand::SecureRandom;
    let mut b = vec![0u8; n.div_ceil(2)];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    h[..n].to_string()
}

/// Go's `buildProposal`: one item of the model's answer as a proposal (its content scanned now: a known-unsafe one is
/// queued as blocked), or None when it does not hold one.
pub fn build_proposal(session_id: &str, item: &Any, scanner: &dyn Fn(&str) -> (bool, String)) -> Option<Proposal> {
    let Any::Object(m) = item else { return None };
    let get = |k: &str| match m.get(k) {
        Some(Any::String(s)) => s.as_str(),
        _ => "",
    };
    let (scope, action) = (get("scope"), get("action"));
    if !matches!(scope, "agent" | "user") || !matches!(action, "add" | "replace" | "remove") {
        return None;
    }
    let (old, content) = (opt_text(m.get("old_text")), opt_text(m.get("content")));
    let rationale = match m.get("rationale") {
        None | Some(Any::Null) => String::new(),
        Some(v) => gotext::trim_space(&session::sprint(v)).to_string(),
    };
    if matches!(action, "add" | "replace") && content.is_none() {
        return None;
    }
    if matches!(action, "replace" | "remove") && old.is_none() {
        return None;
    }
    let mut p = Proposal {
        entry_id: format!("mem_{}", rand_hex(12)),
        session_id: session_id.into(),
        scope: scope.into(),
        action: action.into(),
        old_text: old,
        content,
        rationale,
        status: STATUS_PENDING.into(),
        blocked_reason: None,
        created_at: now(),
    };
    if let Some(c) = &p.content {
        let (blocked, reason) = scanner(c);
        if blocked {
            p.status = STATUS_BLOCKED.into();
            p.blocked_reason = Some(reason);
        }
    }
    Some(p)
}

/// Go's `Propose`: the model's proposals about a finished session (none for an empty transcript or an answer that holds
/// no array); `scanner` None is the memory gate.
pub fn propose(
    ctx: &Ctx,
    session_id: &str,
    msgs: &[SessionMessage],
    client: &dyn Completer,
    scanner: Option<ScanFn>,
) -> Result<Vec<Proposal>, String> {
    let transcript = build_transcript(msgs, MAX_TRANSCRIPT_CHARS);
    if gotext::trim_space(&transcript).is_empty() {
        return Ok(Vec::new());
    }
    let gate = |t: &str| {
        let getenv = |k: &str| std::env::var(k).unwrap_or_default();
        let v = crate::guard::Gate::memory().scan(t.as_bytes(), &getenv);
        (v.blocked, v.reason)
    };
    let scanner: &dyn Fn(&str) -> (bool, String) = scanner.unwrap_or(&gate);
    let msg = |role: &str, content: &str| Message { role: role.into(), content: content.into(), ..Message::default() };
    let req = Request { messages: vec![msg("system", REVIEW_SYSTEM_PROMPT), msg("user", &transcript)], ..Request::default() };
    let turn = client.complete(ctx, &req, None).map_err(|e| e.text().to_string())?;
    Ok(extract_json_array(&turn.content)
        .unwrap_or_default()
        .iter()
        .take(MAX_PROPOSALS_PER_REVIEW)
        .filter_map(|it| build_proposal(session_id, it, scanner))
        .collect())
}

/// The pending-review file (Go's `Queue`).
#[derive(Clone, Debug, Default)]
pub struct Queue {
    pub path: String,
    /// the workspace: the file is resolved inside it on every use ("" = taken as it is)
    pub root: String,
}

static FILE_TY: Ty = Ty::Struct(
    r#"struct { Entries []map[string]interface {} "json:\"entries\"" }"#,
    "",
    &[gostruct::Field("entries", &Ty::Slice(&Ty::Map(&Ty::Any)))],
);

fn text(v: Option<&Val>) -> String {
    match v {
        None | Some(Val::Any(Any::Null)) => String::new(),
        Some(Val::Any(Any::String(s))) => s.clone(),
        Some(Val::Any(a)) => session::sprint(a),
        Some(_) => String::new(),
    }
}

fn ptr(v: Option<&Val>) -> Option<String> {
    match v {
        None | Some(Val::Any(Any::Null)) => None,
        v => Some(text(v)),
    }
}

impl Queue {
    /// Go's `QueueFromWorkspace`: next to the agent-scope memory file.
    pub fn from_workspace(cwd: &str) -> Queue {
        let abs = gopath::abs(cwd.as_bytes()).map(|a| gotext::lossy(&a)).unwrap_or_else(|_| cwd.to_string());
        let path = gotext::lossy(&gopath::join(&[abs.as_bytes(), b".port_sessions", b"memory", b"pending_review.json"]));
        Queue { path, root: abs }
    }

    fn file(&self) -> Result<String, String> {
        if self.root.is_empty() {
            return Ok(self.path.clone());
        }
        workspace_file(&self.root, &self.path)
    }

    /// Go's `Load`: every entry; a missing or damaged file is an empty queue.
    pub fn load(&self) -> Vec<Proposal> {
        let Ok(p) = self.file() else { return Vec::new() };
        let Ok(Some(raw)) = read_state_file(&p) else { return Vec::new() };
        let (v, err) = gostruct::unmarshal(&FILE_TY, &raw);
        if err.is_some() {
            return Vec::new();
        }
        let Some(Val::Slice(Some(entries))) = gostruct::field(&FILE_TY, &v, "entries") else { return Vec::new() };
        entries
            .iter()
            .map(|e| {
                let m = match e {
                    Val::Map(Some(m)) => Some(m),
                    _ => None,
                };
                let get = |k: &str| m.and_then(|m| m.get(k));
                let mut status = text(get("status"));
                if status.is_empty() {
                    status = STATUS_PENDING.into();
                }
                Proposal {
                    entry_id: text(get("entry_id")),
                    session_id: text(get("session_id")),
                    scope: text(get("scope")),
                    action: text(get("action")),
                    old_text: ptr(get("old_text")),
                    content: ptr(get("content")),
                    rationale: text(get("rationale")),
                    status,
                    blocked_reason: ptr(get("blocked_reason")),
                    created_at: text(get("created_at")),
                }
            })
            .collect()
    }

    fn save(&self, entries: &[Proposal]) -> Result<(), String> {
        use std::io::Write;
        let list: Vec<String> = entries.iter().map(|p| p.to_json(false)).collect();
        let data = session::encode_json(&format!("{{\"entries\":[{}]}}", list.join(",")));
        let path = self.file()?;
        crate::testguard::allow(path.as_bytes())?;
        let dir = gopath::dir(path.as_bytes());
        goos::mkdir_all(&dir)?;
        let (mut f, name) = goos::create_temp(&dir, ".review-*.tmp")?;
        let werr = f.write_all(data.as_bytes());
        drop(f);
        let os_name = gopath::os_path(&name);
        if werr.is_err() {
            let _ = std::fs::remove_file(os_name);
            return Err("cannot write review queue".into());
        }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(os_name, std::fs::Permissions::from_mode(0o600));
        if let Err(e) = std::fs::rename(os_name, &path) {
            let _ = std::fs::remove_file(os_name);
            return Err(format!("rename {} {path}: {}", gotext::lossy(&name), crate::goerr::text(&e)));
        }
        Ok(())
    }

    /// Go's `Add`: appends proposals.
    pub fn add(&self, ps: &[Proposal]) -> Result<(), String> {
        if ps.is_empty() {
            return Ok(());
        }
        let _g = WRITE.lock().unwrap_or_else(|p| p.into_inner());
        let mut all = self.load();
        all.extend_from_slice(ps);
        self.save(&all)
    }

    /// Go's `Pending`: the proposals awaiting a decision.
    pub fn pending(&self) -> Vec<Proposal> {
        self.load().into_iter().filter(|e| e.status == STATUS_PENDING).collect()
    }

    /// Go's `Get`: a proposal by id.
    pub fn get(&self, id: &str) -> Option<Proposal> {
        self.load().into_iter().find(|e| e.entry_id == id)
    }

    fn set_status(
        &self,
        id: &str,
        status: &str,
        apply: Option<ApplyFn>,
    ) -> Result<Option<Mutation>, String> {
        let mut entries = self.load();
        let Some(i) = entries.iter().position(|e| e.entry_id == id) else {
            return Err(format!("no pending review entry with id {id}"));
        };
        if entries[i].status != STATUS_PENDING {
            return Err(format!("entry {id} is not pending (status={})", entries[i].status));
        }
        let m = match apply {
            Some(f) => Some(f(&entries[i])?),
            None => None,
        };
        entries[i].status = status.into();
        self.save(&entries)?;
        Ok(m)
    }

    /// Go's `Approve`: applies a pending proposal through the same entry points an interactive tool call uses (so it is
    /// scanned again).
    pub fn approve(&self, id: &str, rt: &Runtime) -> Result<Mutation, String> {
        let apply = |e: &Proposal| match e.action.as_str() {
            "add" => rt.add(&e.scope, s(&e.content)),
            "replace" => rt.replace(&e.scope, s(&e.old_text), s(&e.content)),
            _ => rt.remove(&e.scope, s(&e.old_text)),
        };
        self.set_status(id, STATUS_APPROVED, Some(&apply)).map(|m| m.expect("applied"))
    }

    /// Go's `Reject`.
    pub fn reject(&self, id: &str) -> Result<(), String> {
        self.set_status(id, STATUS_REJECTED, None).map(|_| ())
    }

    /// Go's `RenderSummary` (`memory-review-list`).
    pub fn render_summary(&self) -> String {
        let entries = self.load();
        if entries.is_empty() {
            return "# Memory Review Queue\n\nNo review proposals are currently stored.".into();
        }
        let mut lines = vec!["# Memory Review Queue".to_string(), String::new()];
        for e in &entries {
            lines.push(format!("- {}; status={}; scope={}; action={}", e.entry_id, e.status, e.scope, e.action));
            if !e.rationale.is_empty() {
                lines.push(format!("  rationale: {}", e.rationale));
            }
            if !s(&e.content).is_empty() {
                lines.push(format!("  content: {}", s(&e.content)));
            }
            if !s(&e.old_text).is_empty() {
                lines.push(format!("  old_text: {}", s(&e.old_text)));
            }
            if !s(&e.blocked_reason).is_empty() {
                lines.push(format!("  blocked: {}", s(&e.blocked_reason)));
            }
        }
        lines.join("\n")
    }
}

/// Go's `reviewOutcome` (cli/memorycmd.go): one review pass over a finished session, the proposals staged, and the one
/// line to print.
pub fn review_outcome(ctx: &Ctx, cwd: &str, session_id: &str, msgs: &[SessionMessage], client: &dyn Completer) -> Result<String, String> {
    let proposals = propose(ctx, session_id, msgs, client, None)?;
    Queue::from_workspace(cwd).add(&proposals)?;
    let blocked = proposals.iter().filter(|p| p.status == STATUS_BLOCKED).count();
    let pending = proposals.len() - blocked;
    let noun = if proposals.len() == 1 { "entry" } else { "entries" };
    Ok(format!(
        "memory_review: proposed {} {noun} ({pending} pending, {blocked} blocked) -- run 'sarina memory-review-list' to inspect, \
         'sarina memory-review-approve <id>' to apply",
        proposals.len()
    ))
}
