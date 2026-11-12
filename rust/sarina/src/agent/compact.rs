//! Go: `go/agent/compact.go` and `clear.go`, with the pure parts of `notes.go` and `summarize.go`: shrinking a transcript
//! without a model (old tool outputs cleared, bulky messages snipped, history folded into a digest), the session notes'
//! coverage and format, the transcript a summarising model reads, and its summary cleaned.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::limits::{count_tokens, message_tokens};
use super::Event;
use crate::config::{as_int, ContextSettings};
use crate::goany::Any;
use crate::goregexp;
use crate::gotext::{fields, to_lower, trim_space};
use crate::session::message::{
    KIND_COMPACT_BOUNDARY, KIND_CONTINUATION, KIND_FILE_HISTORY_REPLAY, KIND_SNIPPED, REMINDER_CLOSE, REMINDER_OPEN, STATE_FINAL,
    STATE_TOMBSTONED,
};
use crate::session::{Message, Session};

fn int(n: usize) -> Any {
    Any::Int(n as i64)
}

fn strs(v: &[String]) -> Any {
    Any::Array(v.iter().map(|s| Any::String(s.clone())).collect())
}

fn ev(pairs: Vec<(&str, Any)>) -> Event {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// Go's `collapse`: runs of spaces as one, at most limit characters (then "...").
pub fn collapse(s: &str, limit: usize) -> String {
    let s = fields(s).join(" ");
    match s.char_indices().nth(limit) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s,
    }
}

/// Go's `protectedPrefix`: the leading messages compaction never touches.
pub fn protected_prefix(msgs: &[Message]) -> usize {
    if msgs.is_empty() || msgs[0].role != "system" {
        return 0;
    }
    if msgs.len() > 1 && msgs[1].is_reminder() && msgs[1].kind().is_empty() {
        return 2;
    }
    1
}

/// Go's `reduceOpts`.
#[derive(Clone, Debug, Default)]
pub struct ReduceOpts {
    pub target: i64,
    pub allow_compaction: bool,
    pub reactive: bool,
    pub preserve: i64,
    pub turn: i64,
    pub notes: String,
}

/// Go's `reduce`: snip, then fold, toward the token target, without a model; whether anything changed, and the events.
pub fn reduce(s: &mut Session, o: &ReduceOpts) -> (bool, Vec<Event>) {
    let mut events = Vec::new();
    let mut changed = false;
    for _ in 0..6 {
        if message_tokens(&s.messages) <= o.target {
            break;
        }
        if let Some(e) = snip_pass(s, o) {
            changed = true;
            events.push(e);
            continue;
        }
        if o.allow_compaction {
            if let Some(e) = compact_pass(s, o) {
                changed = true;
                events.push(e);
                if o.reactive {
                    continue;
                }
            }
        }
        break;
    }
    (changed, events)
}

fn snippable(m: &Message) -> bool {
    if matches!(m.kind(), KIND_COMPACT_BOUNDARY | KIND_SNIPPED | KIND_FILE_HISTORY_REPLAY) {
        return false;
    }
    match m.role.as_str() {
        "tool" => true,
        "assistant" => !m.tool_calls.is_empty() || m.content.chars().count() > 600,
        "user" => m.kind() == KIND_CONTINUATION,
        _ => false,
    }
}

/// Go's `snipPass`: bulky old messages replaced by short tombstones, roles, ids and tool-call links kept.
fn snip_pass(s: &mut Session, o: &ReduceOpts) -> Option<Event> {
    let prefix = protected_prefix(&s.messages);
    let end = s.messages.len() as i64 - o.preserve;
    let mut total = message_tokens(&s.messages);
    let mut ids: Vec<String> = Vec::new();
    let mut removed = 0;
    let mut i = prefix as i64;
    while i < end {
        let idx = i as usize;
        i += 1;
        let orig = s.messages[idx].clone();
        if !snippable(&orig) {
            continue;
        }
        if !o.reactive && total <= o.target {
            break;
        }
        let repl = format!(
            "{REMINDER_OPEN}\nAn earlier {} message ({}) was removed to save context. Preview: {}\n{REMINDER_CLOSE}",
            orig.role,
            orig.message_id,
            collapse(&orig.content, 120)
        );
        let (before, after) = (count_tokens(&orig.content), count_tokens(&repl));
        if after >= before {
            continue;
        }
        let mut m = orig.clone();
        m.content = repl;
        m.state = STATE_TOMBSTONED.into();
        m.stop_reason = "snipped_for_context".into();
        let mut meta: BTreeMap<String, Any> = BTreeMap::new();
        meta.insert("kind".into(), Any::String(KIND_SNIPPED.into()));
        meta.insert("turn_index".into(), Any::Int(o.turn));
        meta.insert("original_token_estimate".into(), Any::Int(before));
        meta.insert("replacement_token_estimate".into(), Any::Int(after));
        meta.insert("snipped_role".into(), Any::String(orig.role.clone()));
        meta.insert("snipped_message_id".into(), Any::String(orig.message_id.clone()));
        meta.insert("snipped_kind".into(), Any::String(orig.kind().to_owned()));
        if orig.role == "tool" {
            meta.insert("tool_name".into(), Any::String(orig.name.clone()));
            meta.insert("tool_call_id".into(), Any::String(orig.tool_call_id.clone()));
        }
        m.metadata = meta;
        m.mutate("snip_tombstone");
        s.messages[idx] = m;
        total -= before - after;
        removed += before - after;
        ids.push(orig.message_id);
        if o.reactive && ids.len() >= 3 {
            break;
        }
    }
    if ids.is_empty() {
        return None;
    }
    let typ = if o.reactive { "reactive_snip_boundary" } else { "snip_boundary" };
    Some(ev(vec![
        ("type", Any::String(typ.into())),
        ("turn_index", Any::Int(o.turn)),
        ("snipped_count", int(ids.len())),
        ("estimated_tokens_removed", Any::Int(removed)),
        ("snipped_message_ids", strs(&ids)),
    ]))
}

fn label(m: &Message) -> String {
    if m.role == "tool" {
        format!("tool:{}", m.name)
    } else {
        m.role.clone()
    }
}

/// Go's `pairSafeCut`: where the preserved tail starts, moved back so a kept tool result keeps its call.
pub fn pair_safe_cut(msgs: &[Message], prefix: usize, preserve: i64) -> usize {
    let mut cut = (msgs.len() as i64 - preserve).max(prefix as i64) as usize;
    while cut > prefix && cut < msgs.len() && msgs[cut].role == "tool" {
        cut -= 1;
    }
    cut
}

/// Go's `compactPass`: everything between the prefix and the preserved tail folded into one digest.
fn compact_pass(s: &mut Session, o: &ReduceOpts) -> Option<Event> {
    let prefix = protected_prefix(&s.messages);
    let cut = pair_safe_cut(&s.messages, prefix, o.preserve);
    let cand = &s.messages[prefix..cut];
    if cand.is_empty() {
        return None;
    }
    let mut b =
        format!("{REMINDER_OPEN}\nEarlier conversation history ({} messages) was compacted to save context. Oldest first:\n", cand.len());
    for m in cand.iter().take(24) {
        let mut text = collapse(&m.content, 160);
        if text.is_empty() {
            text = "(empty)".into();
        }
        b.push_str(&format!("- {}: {text}\n", label(m)));
    }
    if cand.len() > 24 {
        b.push_str(&format!("... plus {} older messages\n", cand.len() - 24));
    }
    if !o.notes.is_empty() {
        b.push_str(&format!("Session notes kept alongside this digest (they may lag the newest messages):\n{}\n", o.notes));
    }
    b.push_str("Treat the messages after this note as the working set; rely on them over this digest.\n");
    b.push_str(REMINDER_CLOSE);
    let typ = if o.reactive { "reactive_compact_boundary" } else { "compact_boundary" };
    Some(fold_history(s, prefix, cut, &b, o, typ, Vec::new()))
}

/// Go's `foldHistory`: msgs[prefix:cut] replaced by one boundary message carrying body; the notes' coverage moved along.
pub fn fold_history(
    s: &mut Session,
    prefix: usize,
    cut: usize,
    body: &str,
    o: &ReduceOpts,
    ev_type: &str,
    extra: Vec<(&str, Any)>,
) -> Event {
    let before = message_tokens(&s.messages);
    let (cand, tail) = (&s.messages[prefix..cut], &s.messages[cut..]);
    let (mut depth, mut nested) = (0i64, 0usize);
    let mut kinds: BTreeMap<String, Any> = BTreeMap::new();
    let mut ids = Vec::with_capacity(cand.len());
    for m in cand {
        let k = if m.kind().is_empty() { m.role.clone() } else { m.kind().to_owned() };
        let n = as_int(kinds.get(&k)).unwrap_or(0);
        kinds.insert(k, Any::Int(n + 1));
        ids.push(m.message_id.clone());
        if m.kind() == KIND_COMPACT_BOUNDARY {
            nested += 1;
            depth = depth.max(as_int(m.metadata.get("compaction_depth")).unwrap_or(0));
        }
    }
    depth += 1;
    let tail_ids: Vec<String> = tail.iter().map(|m| m.message_id.clone()).collect();
    let removed = (message_tokens(cand) - count_tokens(body)).max(0);
    let n = cand.len();
    let mut meta: BTreeMap<String, Any> = BTreeMap::new();
    meta.insert("kind".into(), Any::String(KIND_COMPACT_BOUNDARY.into()));
    meta.insert("turn_index".into(), Any::Int(o.turn));
    meta.insert("compacted_message_count".into(), int(n));
    meta.insert("estimated_tokens_before".into(), Any::Int(before));
    meta.insert("estimated_tokens_removed".into(), Any::Int(removed));
    meta.insert("preserved_tail_count".into(), int(tail.len()));
    meta.insert("preserved_tail_ids".into(), strs(&tail_ids));
    meta.insert("compaction_depth".into(), Any::Int(depth));
    meta.insert("nested_compaction_count".into(), int(nested));
    meta.insert("compacted_kinds".into(), Any::Object(kinds));
    meta.insert("compacted_message_ids".into(), strs(&ids));
    for (k, v) in &extra {
        meta.insert((*k).to_owned(), v.clone());
    }
    let boundary = Message {
        role: "user".into(),
        content: body.to_owned(),
        state: STATE_FINAL.into(),
        message_id: format!("compact_boundary_{}_{n}", o.turn),
        metadata: meta,
        ..Message::default()
    };
    let (coverage, had_notes) = notes_coverage(s);
    let tail_len = tail.len();
    let rest: Vec<Message> = s.messages.drain(prefix..).skip(cut - prefix).collect();
    s.messages.push(boundary);
    s.messages.extend(rest);
    if had_notes {
        set_notes_coverage(s, remap_coverage(coverage, prefix as i64, cut as i64));
    }
    let mut e = ev(vec![
        ("type", Any::String(ev_type.into())),
        ("turn_index", Any::Int(o.turn)),
        ("compacted_message_count", int(n)),
        ("estimated_tokens_before", Any::Int(before)),
        ("estimated_tokens_removed", Any::Int(removed)),
        ("preserved_tail_count", int(tail_len)),
        ("preserved_tail_ids", strs(&tail_ids)),
        ("compaction_depth", Any::Int(depth)),
        ("nested_compaction_count", int(nested)),
        ("compacted_message_ids", strs(&ids)),
    ]);
    for (k, v) in extra {
        e.insert(k.to_owned(), v);
    }
    e
}

// ---- clearing old tool outputs ------------------------------------------------------------------------------------------

/// Go's `metaCleared`.
pub const META_CLEARED: &str = "tool_output_cleared";

/// Go's `clearToolOutputs`: past the configured share of the window, every tool result but the newest few (and the
/// small ones) replaced by a one-line marker that keeps what a change's result said about itself.
pub fn clear_tool_outputs(s: &mut Session, window: i64, spec_tokens: i64, st: &ContextSettings, turn: i64) -> Option<Event> {
    if !st.clear || window <= 0 {
        return None;
    }
    if (message_tokens(&s.messages) + spec_tokens) as f64 <= st.clear_window_fraction * window as f64 {
        return None;
    }
    let prefix = protected_prefix(&s.messages);
    let tool_idx: Vec<usize> = (prefix..s.messages.len()).filter(|&i| s.messages[i].role == "tool").collect();
    let keep = st.clear_keep_recent.max(0) as usize;
    if tool_idx.len() <= keep {
        return None;
    }
    let mut ids = Vec::new();
    let mut removed = 0;
    for &i in &tool_idx[..tool_idx.len() - keep] {
        let m = &mut s.messages[i];
        if m.metadata.get(META_CLEARED) == Some(&Any::Bool(true)) || m.kind() == KIND_SNIPPED {
            continue;
        }
        let before = count_tokens(&m.content);
        if before <= st.clear_min_tokens {
            continue;
        }
        let name = if m.name.is_empty() { "a tool" } else { m.name.as_str() };
        let kept = outcome_lines(&m.content);
        let marker = if kept.is_empty() {
            format!("[Output cleared to save context: {name} returned about {before} tokens. Run the tool again if you need it.]")
        } else {
            format!("[Output cleared to save context (about {before} tokens); what it said: {kept}]")
        };
        let after = count_tokens(&marker);
        if after >= before {
            continue;
        }
        m.content = marker;
        m.set_meta(META_CLEARED, Any::Bool(true));
        m.set_meta("original_token_estimate", Any::Int(before));
        m.set_meta("cleared_turn_index", Any::Int(turn));
        m.mutate("tool_output_cleared");
        removed += before - after;
        ids.push(m.message_id.clone());
    }
    if ids.is_empty() {
        return None;
    }
    Some(ev(vec![
        ("type", Any::String("tool_output_cleared".into())),
        ("turn_index", Any::Int(turn)),
        ("cleared_count", int(ids.len())),
        ("estimated_tokens_removed", Any::Int(removed)),
        ("cleared_message_ids", strs(&ids)),
    ]))
}

static OUTCOME_RE: LazyLock<Regex> = LazyLock::new(|| {
    goregexp::compile(
        r#"(Edited [^|\\\n]{1,120}?\(\d+ replacements?\)|Wrote [^|\\\n]{1,120}?\(\d+ characters\)|Check passed: [^\\\n]{1,80}|CHECK FAILED \([^)]{1,80}\)[^\\\n]{0,10}|refused: [^\\\n]{1,120})"#,
    )
    .expect("outcome pattern")
});

/// Go's `outcomeLines`: what a write or edit result said about itself, short.
pub fn outcome_lines(content: &str) -> String {
    OUTCOME_RE.find_iter(content).take(4).map(|m| trim_space(m.as_str())).collect::<Vec<_>>().join(" ")
}

// ---- session notes (the pure parts) -------------------------------------------------------------------------------------

/// Go's `notesStateKey`.
pub const NOTES_STATE_KEY: &str = "session_notes";
const NOTES_SECTION_CAP: usize = 2500;
const NOTES_SECTIONS: [&str; 6] = ["Task / goal", "Current state", "Files touched", "Decisions", "Open questions", "Next steps"];

/// Go's `notesCoverage`: how many leading messages the notes cover, and whether the session has notes state.
pub fn notes_coverage(s: &Session) -> (i64, bool) {
    let Some(Any::Object(m)) = s.plugin_state.get(NOTES_STATE_KEY) else { return (0, false) };
    let c = as_int(m.get("last_summarised_index")).unwrap_or(0);
    let id = match m.get("last_summarised_message_id") {
        Some(Any::String(s)) => s.as_str(),
        _ => "",
    };
    if c <= 0 {
        return (0, true);
    }
    if c <= s.messages.len() as i64 && (id.is_empty() || s.messages[c as usize - 1].message_id == id) {
        return (c, true);
    }
    if !id.is_empty() {
        if let Some(i) = s.messages.iter().rposition(|m| m.message_id == id) {
            return (i as i64 + 1, true);
        }
    }
    (0, true)
}

/// Go's `setNotesCoverage`.
pub fn set_notes_coverage(s: &mut Session, c: i64) {
    let c = c.min(s.messages.len() as i64).max(0);
    let id = if c > 0 { s.messages[c as usize - 1].message_id.clone() } else { String::new() };
    let mut m = BTreeMap::new();
    m.insert("last_summarised_index".to_owned(), Any::Int(c));
    m.insert("last_summarised_message_id".to_owned(), Any::String(id));
    s.plugin_state.insert(NOTES_STATE_KEY.into(), Any::Object(m));
}

/// Go's `remapCoverage`: a coverage count from before msgs[prefix:cut] was folded into one message, after it.
pub fn remap_coverage(c: i64, prefix: i64, cut: i64) -> i64 {
    if c >= cut {
        prefix + 1 + c - cut
    } else {
        c.min(prefix)
    }
}

/// Go's `sectionIndex`: one of the six notes sections a heading names, or -1.
pub fn section_index(heading: &str) -> i64 {
    let h = to_lower(heading.trim_matches(['#', '*', ':', ' ', '\t']));
    let p = |x: &str| h.starts_with(x);
    if p("task") || p("goal") {
        0
    } else if p("current") || p("state") {
        1
    } else if p("files") {
        2
    } else if p("decision") {
        3
    } else if p("open") || p("question") {
        4
    } else if p("next") {
        5
    } else {
        -1
    }
}

/// Go's `normaliseNotes`: the model's reply as the canonical notes file, or None when fewer than three sections are there.
pub fn normalise_notes(raw: &str) -> Option<String> {
    let mut bodies: [Vec<&str>; 6] = Default::default();
    let mut cur: i64 = -1;
    for line in raw.split('\n') {
        let t = trim_space(line);
        if t.starts_with("```") {
            continue;
        }
        if t.starts_with('#') {
            let i = section_index(t);
            if i >= 0 {
                cur = i;
                continue;
            }
            if cur < 0 {
                continue;
            }
        }
        if cur >= 0 {
            bodies[cur as usize].push(line.trim_end_matches([' ', '\t', '\r']));
        }
    }
    let mut known = 0;
    let mut b = String::from("# Session notes\n");
    for (i, name) in NOTES_SECTIONS.iter().enumerate() {
        let joined = bodies[i].join("\n");
        let mut text = trim_space(&joined).to_owned();
        if text.is_empty() {
            text = "(none)".into();
        } else {
            known += 1;
        }
        if let Some((cut, _)) = text.char_indices().nth(NOTES_SECTION_CAP) {
            text = format!("{}\n... (truncated)", &text[..cut]);
        }
        b.push_str(&format!("\n## {name}\n{text}\n"));
    }
    if known < 3 {
        return None;
    }
    Some(b)
}

/// Go's `truncateChars`.
pub fn truncate_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{} ...[truncated]", &s[..i]),
        None => s.to_owned(),
    }
}

/// Go's `renderTranscript`: messages flattened into plain text for a summarising model.
pub fn render_transcript(msgs: &[Message], tool_chars: usize) -> String {
    let mut b = String::new();
    for m in msgs {
        match m.role.as_str() {
            "tool" => b.push_str(&format!("[tool result: {}]\n{}\n\n", m.name, truncate_chars(&m.content, tool_chars))),
            "assistant" => {
                if !trim_space(&m.content).is_empty() {
                    b.push_str(&format!("[assistant]\n{}\n", truncate_chars(&m.content, 4000)));
                }
                for c in &m.tool_calls {
                    b.push_str(&format!("[assistant called {} with {}]\n", c.function.name, truncate_chars(&c.function.arguments, 400)));
                }
                b.push('\n');
            }
            _ => b.push_str(&format!("[{}]\n{}\n\n", m.role, truncate_chars(&m.content, 6000))),
        }
    }
    b
}

/// Go's `tailWithin`: the longest suffix whose rendering stays under max_chars (always at least the last message).
pub fn tail_within(msgs: &[Message], tool_chars: usize, max_chars: i64) -> &[Message] {
    let mut total = 0i64;
    for i in (0..msgs.len()).rev() {
        total += render_transcript(&msgs[i..i + 1], tool_chars).chars().count() as i64;
        if total > max_chars && i < msgs.len() - 1 {
            return &msgs[i + 1..];
        }
    }
    msgs
}

/// Go's `sideMaxTokens`.
pub fn side_max_tokens(window: i64, want: i64) -> i64 {
    want.min(window / 4).max(256)
}

// ---- the summary (the pure parts) ---------------------------------------------------------------------------------------

static ANALYSIS_RE: LazyLock<Regex> = LazyLock::new(|| goregexp::compile(r"(?s)<analysis>.*?</analysis>").expect("analysis"));
static SUMMARY_RE: LazyLock<Regex> = LazyLock::new(|| goregexp::compile(r"(?s)<summary>(.*?)</summary>").expect("summary"));
static BLANK_RUN_RE: LazyLock<Regex> = LazyLock::new(|| goregexp::compile(r"\n{3,}").expect("blank run"));

/// Go's `cleanSummary`: a scratch <analysis> block taken out, <summary> unwrapped.
pub fn clean_summary(s: &str) -> String {
    let s = ANALYSIS_RE.replace_all(s, "").into_owned();
    let s = match SUMMARY_RE.captures(&s) {
        Some(c) => c.get(1).map_or("", |g| g.as_str()).to_owned(),
        None => s.replace("<summary>", "").replace("</summary>", ""),
    };
    let s = s.replace(REMINDER_OPEN, "").replace(REMINDER_CLOSE, "");
    trim_space(&BLANK_RUN_RE.replace_all(&s, "\n\n")).to_owned()
}

const PROMPT_TOO_LONG_PHRASES: [&str; 6] =
    ["prompt is too long", "maximum context length", "context length exceeded", "too many tokens", "input too long", "context window"];

/// Go's `isPromptTooLong` on an error's text.
pub fn is_prompt_too_long(err: &str) -> bool {
    let msg = to_lower(err);
    PROMPT_TOO_LONG_PHRASES.iter().any(|p| msg.contains(p))
}

/// Go's `replyTooLong`.
pub fn reply_too_long(text: &str) -> bool {
    let l = to_lower(text);
    ["prompt_too_long", "context_length_exceeded"].iter().chain(PROMPT_TOO_LONG_PHRASES.iter()).any(|p| l.contains(p))
}

/// Go's `errTooLong` on an error's text.
pub fn err_too_long(err: &str) -> bool {
    if is_prompt_too_long(err) {
        return true;
    }
    let l = to_lower(err);
    l.contains("prompt") && l.contains("long")
}

/// Go's `groupMessages`: rounds, each a user or assistant message with the tool results after it.
pub fn group_messages(msgs: &[Message]) -> Vec<Vec<Message>> {
    let mut groups: Vec<Vec<Message>> = Vec::new();
    for m in msgs {
        if m.role == "tool" {
            if let Some(g) = groups.last_mut() {
                g.push(m.clone());
                continue;
            }
        }
        groups.push(vec![m.clone()]);
    }
    groups
}

/// Go's `foldedBody`.
pub fn folded_body(n: usize, text: &str, closing: &str) -> String {
    format!("{REMINDER_OPEN}\nEarlier conversation history ({n} messages) was compacted to save context; the session continues from this.\n{text}\n{closing}\n{REMINDER_CLOSE}")
}

/// Go's `reminder`.
pub fn reminder(text: &str) -> String {
    format!("{REMINDER_OPEN}\n{text}\n{REMINDER_CLOSE}")
}

/// Go's `planModeNotice`.
pub const PLAN_MODE_NOTICE: &str = "Plan mode is on. You can only read and search. Investigate, then submit your plan with exit_plan_mode; nothing is changed until the operator approves it.";
