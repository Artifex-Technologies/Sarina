//! Go: `go/agent/claims.go`. The checks an answer passes before it is accepted: a change claimed that no file tool made, a
//! quote no tool showed, tool work described when no tool ran, a made-up history, an identifier never read, advice to turn
//! security off, code said to work that nothing checked, and the "carry on" nudges for a step announced or named and not
//! done. Each is a pure function of the answer, the question, this task's messages and its file history (two also look at
//! the workspace's files, one at PATH). The texts sent back are Go's, word for word.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::secrets::tool_texts;
use crate::goany::Any;
use crate::gopath;
use crate::goregexp;
use crate::gostruct::{self, Field, Ty, Val};
use crate::gotext::{fields, to_lower, trim_space};
use crate::session::Message;

/// One entry of a session's file history (Go's `map[string]any`).
pub type Hist = BTreeMap<String, Any>;

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| goregexp::compile($pat).expect(stringify!($name)));
    };
}

re!(CLAIM_FILE_RE, r#"(?i)\b([\w.-]+\.(?:md|txt|json|py|go|rs|toml|ya?ml|sh|conf|env|ini|csv|js|ts|html|css|c|h|cpp|java|lua|xml))\b"#);
re!(
    CLAIM_VERB_RE,
    r#"(?i)\b(added|updated|changed|wrote|written|saved|edited|appended|created|modified|replaced|fixed|inserted|removed|deleted|renamed)\b"#
);
re!(NEGATION_RE, r#"(?i)\b(not|n't|never|no|nothing|without|unable|cannot|could not|did not|didn't|won't|wasn't|haven't)\b"#);
re!(SENTENCE_END, r#"[.!?](?:\s+|$)|\n"#);
re!(
    KEPT_AS_IT_WAS_RE,
    r#"(?i)^[`*"']*\s+(file\s+)?(remains|stays|is|was|is left|was left|is still|was still)\s+(unchanged|untouched|intact|as it was|the same)\b"#
);
re!(FIND_OUT_RE, r#"(?i)\b(check|confirm|verify|see|find out|look at|determine|ask)\s+(if|whether|who|what|which)\b|\bwho or what\b"#);

/// Go's `Regexp.Split(s, -1)`.
pub fn go_split<'a>(re: &Regex, s: &'a str) -> Vec<&'a str> {
    if s.is_empty() {
        return vec![""];
    }
    let mut out = Vec::new();
    let (mut beg, mut end) = (0, 0);
    for m in re.find_iter(s) {
        end = m.start();
        if m.end() != 0 {
            out.push(&s[beg..end]);
        }
        beg = m.end();
    }
    if end != s.len() {
        out.push(&s[beg..]);
    }
    out
}

/// The sentences of an answer as the checks split it.
pub fn sentences(s: &str) -> Vec<&str> {
    go_split(&SENTENCE_END, s)
}

fn hstr<'a>(e: &'a Hist, k: &str) -> &'a str {
    match e.get(k) {
        Some(Any::String(s)) => s,
        _ => "",
    }
}

fn hbool(e: &Hist, k: &str) -> bool {
    matches!(e.get(k), Some(Any::Bool(true)))
}

fn mstr<'a>(m: &'a BTreeMap<String, Any>, k: &str) -> &'a str {
    match m.get(k) {
        Some(Any::String(s)) => s,
        _ => "",
    }
}

fn base(p: &str) -> String {
    String::from_utf8_lossy(&gopath::base(p.as_bytes())).into_owned()
}

fn join(a: &str, b: &str) -> String {
    String::from_utf8_lossy(&gopath::join(&[a.as_bytes(), b.as_bytes()])).into_owned()
}

fn clean(p: &str) -> String {
    String::from_utf8_lossy(&gopath::clean(p.as_bytes())).into_owned()
}

fn is_abs(p: &str) -> bool {
    gopath::is_abs(p.as_bytes())
}

/// Go's `filepath.Ext`.
fn ext(p: &str) -> &str {
    match p.rfind(['.', '/']) {
        Some(i) if p.as_bytes()[i] == b'.' => &p[i..],
        _ => "",
    }
}

fn is_file_write_tool(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "vault_write")
}

/// Go's `os.Stat(p)` and `Mode().IsRegular()`.
fn is_regular(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.file_type().is_file())
}

/// Go's `unbackedFileClaim`: the first file the answer says it changed that no file-writing tool changed successfully in
/// this task, or "".
pub fn unbacked_file_claim(answer: &str, history: &[Hist]) -> String {
    let mut written: HashSet<String> = HashSet::new();
    for e in history {
        let name = hstr(e, "tool_name");
        let ok = hbool(e, "ok");
        let c = hstr(e, "copy");
        if name == "stego_clean" && ok && !c.is_empty() {
            written.insert(to_lower(&base(c)));
            continue;
        }
        if !is_file_write_tool(name) || !ok {
            continue;
        }
        let p = hstr(e, "path");
        if !p.is_empty() {
            written.insert(to_lower(&base(p)));
        }
        if let Some(Any::Array(ps)) = e.get("changed_paths") {
            if ps.iter().all(|p| matches!(p, Any::String(_))) {
                for p in ps {
                    if let Any::String(p) = p {
                        written.insert(to_lower(&base(p)));
                    }
                }
            }
        }
    }
    for s in sentences(answer) {
        if !CLAIM_VERB_RE.is_match(s) || NEGATION_RE.is_match(s) {
            continue;
        }
        let q = FIND_OUT_RE.find(s).map(|m| m.start());
        for caps in CLAIM_FILE_RE.captures_iter(s) {
            let g = caps.get(1).expect("group");
            if q.is_some_and(|q| q < g.start()) || KEPT_AS_IT_WAS_RE.is_match(&s[g.end()..]) {
                continue;
            }
            let name = g.as_str();
            let f = to_lower(&base(name));
            if !written.contains(&f) && !written.iter().any(|w| w.ends_with(&format!(" {f}"))) {
                return name.to_owned();
            }
        }
    }
    String::new()
}

pub fn claim_check_text(file: &str) -> String {
    format!("Check before you finish: your answer says {file} was changed, but no file tool changed {file} in this task (a to-do or plan tool does not write files). Either make the change now with the file tools, or say plainly that {file} was not changed. Write your whole answer again, complete, and do not talk about this check.")
}

/// Go's `unreportedChange`: a file a file-writing tool changed in this task that the answer never mentions, or "".
pub fn unreported_change(answer: &str, history: &[Hist]) -> String {
    let low = to_lower(answer);
    for e in history {
        if !is_file_write_tool(hstr(e, "tool_name")) || !hbool(e, "ok") {
            continue;
        }
        let p = hstr(e, "path");
        if p.is_empty() {
            continue;
        }
        let b = base(p);
        let stem = b.strip_suffix(ext(&b)).unwrap_or(&b);
        if !low.contains(&to_lower(&b)) && !low.contains(&to_lower(stem)) {
            return b;
        }
    }
    String::new()
}

pub fn unreported_text(file: &str) -> String {
    format!("Check before you finish: in this task you changed {file}, but your answer does not say so. Write your whole answer again, complete, and say plainly what you changed in {file} and what it holds now.")
}

re!(SUGGEST_RE, r#"(?i)\b(install|run|use|using|try|helper|via|with|e\.g\.|such as|like)\b"#);
re!(ARITHMETIC_RE, r#"^[\d\s,.()]+([×x*+\-/÷=^][\d\s,.()?]+)+$"#);

/// Go's `isCommand`: a command line starting with a program this computer has (or sudo).
pub fn is_command(q: &str) -> bool {
    let f = fields(q);
    match f.first() {
        None => false,
        Some(&"sudo") => true,
        Some(p) => crate::tiers::router::found(p),
    }
}

/// Go's `sentenceAt`: the sentence of text around byte position i.
pub fn sentence_at(text: &str, i: usize) -> &str {
    let (mut start, mut end) = (0, text.len());
    for l in SENTENCE_END.find_iter(text) {
        if l.end() <= i {
            start = l.end();
        } else if l.start() >= i {
            end = l.start();
            break;
        }
    }
    &text[start..end]
}

re!(QUOTED_RE, r#"(?:^|[\s(:,*_])(?:`([^`\n]{4,60})`|"([^"\n]{4,60})")(?:$|[\s).,:;!?*_])"#);

fn squash_join(s: &str, sep: &str) -> String {
    fields(s).join(sep)
}

/// Go's `ungroundedQuote`: a literal the answer quotes that appears in none of this task's tool output and not in the prompt,
/// when tools were used at all, or "".
pub fn ungrounded_quote(answer: &str, prompt: &str, msgs: &[Message]) -> String {
    let mut evidence = String::new();
    let mut used = false;
    for m in msgs {
        if m.role == "tool" {
            used = true;
            evidence.push_str(&tool_evidence(&m.content));
            evidence.push('\n');
        }
        for c in &m.tool_calls {
            evidence.push_str(&tool_evidence(&c.function.arguments));
            evidence.push('\n');
        }
    }
    if !used {
        return String::new();
    }
    let ev = to_lower(&evidence);
    let ev_tight = squash_join(&ev, "");
    let p = to_lower(prompt);
    for caps in QUOTED_RE.captures_iter(answer) {
        let start = caps.get(0).expect("match").start();
        let m1 = caps.get(1).map_or("", |g| g.as_str());
        let m2 = caps.get(2).map_or("", |g| g.as_str());
        let q = if m1.is_empty() { m2 } else { m1 };
        if ARITHMETIC_RE.is_match(trim_space(q)) {
            continue;
        }
        if !m1.is_empty() && (is_command(q) || (!q.contains([' ', '\t']) && SUGGEST_RE.is_match(sentence_at(answer, start)))) {
            continue;
        }
        let lq = to_lower(trim_space(q).trim_matches(['"', '\'', '`']).trim_end_matches([' ', ',', '.', ';', ':', '!', '?']));
        if lq.is_empty() || ev.contains(&lq) || p.contains(&lq) || ev_tight.contains(&squash_join(&lq, "")) {
            continue;
        }
        if (lq.starts_with('{') || lq.starts_with('[')) && json_in_evidence(&lq, &json_loose(&ev_tight)) {
            continue;
        }
        if let Some(fname) = FUNC_NAME_RE.captures(&lq) {
            if ev.contains(&format!("{}(", &fname[1])) {
                continue;
            }
        }
        if not_a_quote(&lq) || marked_example(&answer[..start]) {
            continue;
        }
        if q.contains(['/', '\\']) || q.starts_with('-') {
            continue;
        }
        return q.to_owned();
    }
    String::new()
}

re!(
    NOT_A_QUOTE_RE,
    r#"(?i)^(\S+=\S*(\s+\S+=\S*)*|.*\s\S+=\S+.*|((zero|one|two|three|four|five|six|seven|eight|nine)(,?\s+|$))+|null|nullptr|none|nil|true|false|undefined|nan|.*<[a-z_ -]+>.*|.*\byour[_ ]\w+.*|\w*_here|\.[a-z0-9]{1,5})$"#
);

pub fn not_a_quote(lq: &str) -> bool {
    NOT_A_QUOTE_RE.is_match(lq)
}

re!(EXAMPLE_RE, r#"(?i)\b(e\.g\.|for example|for instance|such as)[^().\n]{0,40}$"#);
re!(EXAMPLE_LABEL_RE, r#"(?i)\bexamples?\b[^().\n:]{0,25}:[^().\n]{0,40}$"#);
re!(EXAMPLE_HEADING_RE, r#"(?i)^\s*(#{1,6}\s*)?[*_]*[^.\n]{0,25}\bexamples?\b[^.\n]{0,25}[*_]*\s*:?[*_]*\s*$"#);
re!(LIST_ITEM_RE, r#"^\s*([-*+]|\d+[.)])(\s|$)"#);

/// Go's `markedExample`: the text before a quote marks it as her own example.
pub fn marked_example(before: &str) -> bool {
    if EXAMPLE_RE.is_match(before) || EXAMPLE_LABEL_RE.is_match(before) {
        return true;
    }
    let lines: Vec<&str> = before.split('\n').collect();
    let cur = lines[lines.len() - 1];
    let in_list = LIST_ITEM_RE.is_match(cur);
    for l in lines[..lines.len() - 1].iter().rev() {
        if trim_space(l).is_empty() {
            return false;
        }
        if in_list && LIST_ITEM_RE.is_match(l) {
            continue;
        }
        return EXAMPLE_HEADING_RE.is_match(l);
    }
    false
}

re!(FUNC_NAME_RE, r#"^([a-z_][\w.]*)\(\)$"#);

/// Go's `jsonInEvidence`.
fn json_in_evidence(lq: &str, ev_loose: &str) -> bool {
    let loose = json_loose(lq);
    let pairs: Vec<&str> = loose.split(',').collect();
    for kv in &pairs {
        if kv.is_empty() || !ev_loose.contains(kv) {
            return false;
        }
    }
    !pairs.is_empty()
}

/// Go's `jsonLoose`: braces, brackets, quote marks and spaces dropped.
fn json_loose(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '{' | '}' | '[' | ']' | '"' | '\'' | ' ' | '\t' | '\n')).collect()
}

pub fn ungrounded_text(q: &str) -> String {
    format!("Check before you finish: your answer quotes \"{q}\", but no tool output in this task contains it. Write your whole answer again, complete: keep everything the tools showed, and either leave that quote out, mark it plainly as your own example, or look it up with your tools first. Do not talk about this check.")
}

re!(
    TOOL_CLAIM_RE,
    r#"(?i)\b(the (reverse[- ]engineering |analysis |search )?tool (showed|shows|analy[sz]ed|found|returned|identified|reported)|i (ran|checked|looked (at|up|into)|analy[sz]ed|decompiled|read|searched|inspected|examined)|decompil(ed|ing|ation) (shows|showed|reveals|revealed)|after (analy[sz]ing|decompiling|reading|checking))\b"#
);

/// Go's `claimsToolWork`: an answer that describes tool work in a task where no tool ran.
pub fn claims_tool_work(answer: &str, prompt: &str, msgs: &[Message]) -> bool {
    if prompt.contains("already looked up for you")
        || prompt.contains("From your guides")
        || prompt.contains("Already computed for you (crypto_identify")
    {
        return false;
    }
    if msgs.iter().any(|m| m.role == "tool") {
        return false;
    }
    TOOL_CLAIM_RE.is_match(answer)
}

pub const TOOL_WORK_TEXT: &str = "Check before you finish: your answer describes looking, checking or analysing, but you used no tool in this task, so none of that happened. Use the tools now, or write your answer again saying plainly that you have not looked.";

fn is_planning_tool(name: &str) -> bool {
    matches!(name, "task_create" | "task_update" | "todo_write" | "plan_write")
}

/// Go's `taskInsteadOfFile`: a file the prompt names (and that exists in the workspace) when a planning tool was used and no
/// file tool changed that file.
pub fn task_instead_of_file(prompt: &str, cwd: &str, msgs: &[Message], history: &[Hist]) -> String {
    if !msgs.iter().any(|m| m.tool_calls.iter().any(|c| is_planning_tool(&c.function.name))) {
        return String::new();
    }
    for caps in CLAIM_FILE_RE.captures_iter(prompt) {
        let name = &caps[1];
        let p = if is_abs(name) { name.to_owned() } else { join(cwd, name) };
        if !is_regular(&p) {
            continue;
        }
        if unbacked_file_claim(&format!("I changed {name}."), history) == name {
            return name.to_owned();
        }
    }
    String::new()
}

pub fn task_instead_of_file_text(file: &str) -> String {
    format!("Check before you finish: the user asked about {file}, which is a file, but you used your own task list, which does not change any file. Make the change in {file} with the file tools now, then write your whole answer again.")
}

re!(
    SECURITY_OFF_RE,
    r#"(?i)\b(disabl(e|ing)|turn(ing)? off|switch(ing)? off|stop(ping)?|mask(ing)?|remov(e|ing)|uninstall(ing)?)\b[^.\n]{0,60}\b(ufw|firewall|firewalld|heimdall|apparmor|selinux|keyring|freshclam|clamav|antivirus|secure boot|limbo)\b"#
);
re!(
    WARNED_AFTER_RE,
    r#"(?i)\b(not recommended|is (unsafe|dangerous|a bad idea)|should not|shouldn'?t|never|won'?t help|will not help|do(es)? not help)\b"#
);
re!(WARNED_RE, r#"(?i)\b(do not|don'?t|never|avoid|no need to|without|not)\s+(\w+\s+){0,2}$"#);

/// Go's `suggestsSecurityOff`: the security piece an answer suggests turning off, or "".
pub fn suggests_security_off(answer: &str) -> String {
    for sentence in sentences(answer) {
        let Some(caps) = SECURITY_OFF_RE.captures(sentence) else { continue };
        let whole = caps.get(0).expect("match");
        if WARNED_RE.is_match(&sentence[..whole.start()]) || WARNED_AFTER_RE.is_match(&sentence[whole.end()..]) {
            continue;
        }
        return caps.get(caps.len() - 1).map_or("", |g| g.as_str()).to_owned();
    }
    String::new()
}

pub fn security_off_text(what: &str) -> String {
    format!("Check before you finish: your answer suggests turning off {what}, a security piece. Never advise that for speed or convenience. Write your whole answer again without that suggestion; if nothing else would help, say so.")
}

pub const COMMANDS_MARKER: &str = "Commands to copy exactly";

re!(BACKTICK_RE, r#"`([^`\n]{3,400})`"#);
re!(CMD_START_RE, r#"^(sudo |pacman |heimdalld |systemctl |printf |echo )"#);
re!(
    STD_CMD_RE,
    r#"^(sudo pacman -Syu|pacman -Qi [\w.+-]+|sudo pacman -Rs [\w.+-]+|sudo heimdalld immunity (confirm|release) ([0-9a-f]{8,64}))$"#
);

/// Go's `squash`: runs of spaces as one.
pub fn squash(s: &str) -> String {
    squash_join(s, " ")
}

/// Go's `unsourcedCommand`.
pub fn unsourced_command(answer: &str, prompt: &str, msgs: &[Message]) -> String {
    if !prompt.contains(COMMANDS_MARKER) {
        return String::new();
    }
    let mut mat = prompt.to_owned();
    for m in msgs.iter().filter(|m| m.role == "tool") {
        mat.push('\n');
        mat.push_str(&tool_evidence(&m.content));
    }
    let material = squash(&mat);
    for caps in BACKTICK_RE.captures_iter(answer) {
        let c = squash(&caps[1]);
        if !CMD_START_RE.is_match(&c) || material.contains(&c) {
            continue;
        }
        if STD_CMD_RE.is_match(&c) {
            let parts = fields(&c);
            let last = parts[parts.len() - 1];
            if c == "sudo pacman -Syu" || material.contains(last) {
                continue;
            }
        }
        return c;
    }
    String::new()
}

pub fn unsourced_text(c: &str) -> String {
    format!("Check before you finish: your answer gives the command `{c}`, but the report does not. Use only the commands the report gives, copied whole and unchanged (the block \"{COMMANDS_MARKER}\"), or the standard steps the instructions name; if no command is given for a finding, say so plainly. Write your whole answer again, complete. Do not talk about this check.")
}

re!(
    PAST_CLAIM_RE,
    r#"(?i)\b(you (told|mentioned|said|asked|showed) (me|that|it|this|so)?|as you (told|mentioned|said)|we (talked|spoke|discussed|chatted)|last (time|week|night|month)|the other day|yesterday|from earlier|earlier (today|you|we|on)|your usual|as (always|usual|before)|you usually|you always)\b"#
);
re!(
    TIME_CLAIM_RE,
    r#"(?i)\b(we (talked|spoke|discussed|chatted)|last (time|week|night|month)|the other day|yesterday|from earlier|earlier (today|you|we|on)|your usual|as (always|usual|before)|you usually|you always)\b"#
);
re!(WORD_RE, r#"[\p{L}\p{N}]{4,}"#);
re!(HISTORY_SENTENCE_RE, r#"[^.!?\n]+[.!?]?"#);
re!(ASKED_PAST_RE, r#"(?i)\b(have|do|did|does|would|were|are) you (always|usually)\b"#);
re!(
    ASK_RECALL_RE,
    r#"(?i)\b(do|did|can|could) you (still )?(remember|recall|know what i)\b|\bwhat did i (tell|say|mention)\b|\bdid i (ever )?(tell|mention)\b"#
);

const COMMON_WORDS: &[&str] = &[
    "that",
    "this",
    "with",
    "your",
    "from",
    "have",
    "were",
    "what",
    "told",
    "said",
    "last",
    "week",
    "time",
    "earlier",
    "today",
    "yesterday",
    "usual",
    "always",
    "before",
    "mentioned",
    "asked",
    "still",
    "about",
    "just",
    "like",
    "they",
    "there",
    "been",
    "will",
    "would",
    "could",
    "should",
    "when",
    "then",
    "than",
    "them",
    "those",
    "these",
    "talked",
    "discussed",
    "spoke",
    "usually",
    "night",
    "month",
];
const MEMORY_VERBS: &[&str] =
    &["remember", "remembered", "recall", "said", "told", "tell", "mentioned", "talked", "spoke", "asked", "shared", "loved", "liked"];

/// Go's `inventedHistory`: a sentence of the answer that claims shared history none of the evidence holds, or "".
pub fn invented_history(answer: &str, all: &[Message], msg_start: usize) -> String {
    let (mut ev, mut now) = (String::new(), String::new());
    for (i, m) in all.iter().enumerate() {
        if i >= msg_start && m.role == "user" {
            if !m.message_id.starts_with("claim_check") {
                now.push_str(&to_lower(&m.content));
            }
            continue;
        }
        if m.role == "assistant" && i >= msg_start {
            continue;
        }
        ev.push_str(&to_lower(&m.content));
        ev.push('\n');
    }
    for sentence in HISTORY_SENTENCE_RE.find_iter(answer).map(|m| m.as_str()) {
        if !PAST_CLAIM_RE.is_match(sentence) {
            continue;
        }
        if trim_space(sentence).ends_with('?') && !PAST_CLAIM_RE.is_match(&ASKED_PAST_RE.replace_all(sentence, "")) {
            continue;
        }
        let mut evidence = ev.clone();
        if !TIME_CLAIM_RE.is_match(sentence) && !ASK_RECALL_RE.is_match(&now) {
            evidence.push('\n');
            evidence.push_str(&now);
        }
        let mut words = 0;
        let low = to_lower(sentence);
        for w in WORD_RE.find_iter(&low).map(|m| m.as_str()) {
            if COMMON_WORDS.contains(&w) || MEMORY_VERBS.contains(&w) {
                continue;
            }
            words += 1;
            if evidence.contains(w) {
                return String::new();
            }
        }
        if words > 0 {
            return trim_space(sentence).to_owned();
        }
    }
    String::new()
}

pub fn invented_history_text(s: &str) -> String {
    format!("Check before you finish: your answer says \"{s}\" as if it came from an earlier conversation or something the user told you, but nothing you were given shows it (not this conversation, your memory or your tools). Do not invent what was said or done before. If you do not know or do not remember, say so plainly. Write your whole answer again, complete. Do not talk about this check.")
}

re!(SET_DEFAULT_RE, r#"(?i)\b(wpctl|pactl) set-default(-sink)?\b"#);
re!(ONE_APP_SILENT_RE, r#"(?i)\b(but|while|though|yet)\b[^.?!]{0,60}\b(does|works|plays|is fine|has sound|is working)\b"#);

/// Go's `defaultOutputForOneApp`.
pub fn default_output_for_one_app(answer: &str, prompt: &str) -> bool {
    SET_DEFAULT_RE.is_match(answer) && ONE_APP_SILENT_RE.is_match(prompt)
}

pub const ONE_APP_SILENT_TEXT: &str = "Check before you finish: another program plays sound, so the default output works; `wpctl set-default` would move every program and fix nothing. Leave the default as it is. The fix is the silent program's own stream: find it under Streams in `wpctl status` (unmute it, raise its volume, or move only that stream), or the program's own sound setting. Write your whole answer again, complete, without set-default. Do not talk about this check.";

/// Go's `evidenceOf`: the prompt and this task's tool output, lower-cased, and whether any tool ran.
pub fn evidence_of(prompt: &str, msgs: &[Message]) -> (String, bool) {
    let mut b = prompt.to_owned();
    let mut used = false;
    for m in msgs.iter().filter(|m| m.role == "tool") {
        used = true;
        b.push('\n');
        b.push_str(&tool_evidence(&m.content));
    }
    (to_lower(&b), used)
}

re!(
    IDENTIFIER_RE,
    r#"(?i)\b(?:v?\d+\.\d+(?:\.\d+)+(?:[-+][\w.]+)?|CVE-\d{4}-\d{4,}|0x[0-9a-f]{4,}|[0-9a-f]{12,64}|lines? \d{2,}|port \d{2,5})\b"#
);

/// Go's `ungroundedIdentifier`.
pub fn ungrounded_identifier(answer: &str, prompt: &str, msgs: &[Message]) -> String {
    let (ev, used) = evidence_of(prompt, msgs);
    if !used {
        return String::new();
    }
    for m in IDENTIFIER_RE.find_iter(answer) {
        let lm = to_lower(m.as_str());
        if lm.starts_with("line") || lm.starts_with("port") {
            let n = trim_space(&lm[lm.find(' ').map_or(0, |i| i + 1)..]);
            if ev.contains(n) {
                continue;
            }
            return m.as_str().to_owned();
        }
        if ev.contains(lm.strip_prefix('v').unwrap_or(&lm)) {
            continue;
        }
        return m.as_str().to_owned();
    }
    String::new()
}

re!(DIGEST_FIRST_RE, r#"\b1\. a hex digest: "#);
re!(HASH_WORD_RE, r#"(?i)\b(hash|hashes|digest|sha-?\d+|md5)\b"#);
re!(
    ONE_WAY_RE,
    r#"(?i)(one[- ]way|irreversible|not reversible|unrecoverable|(cannot|can'?t|can ?not) (be )?(reversed|recovered|derived|determined|known|decrypted|decoded|reverse|recover|derive|decrypt|decode)|no (way|means) (to|of) |(says|reveals|tells( you)?|contains) nothing about|does(n'?t| not) (say|contain|reveal|encode))"#
);
re!(COMPARE_RE, r#"(?i)\b(match(es|ed)?|the same|identical|equal)\b"#);

/// Go's `digestNotOneWay`.
pub fn digest_not_one_way(answer: &str, prompt: &str, msgs: &[Message]) -> bool {
    let (ev, _) = evidence_of(prompt, msgs);
    DIGEST_FIRST_RE.is_match(&ev) && HASH_WORD_RE.is_match(answer) && !ONE_WAY_RE.is_match(answer) && !COMPARE_RE.is_match(answer)
}

pub const DIGEST_NOT_ONE_WAY_TEXT: &str = "Check before you finish: crypto_identify found a hash here, and your answer does not say what a hash cannot give. Write your whole answer again, complete: keep what the tools showed, and say plainly that a hash is one-way: it cannot be reversed to its input, only compared against the hash of a candidate. Do not talk about this check.";

pub fn ungrounded_identifier_text(id: &str) -> String {
    format!("Check before you finish: your answer gives \"{id}\", but no tool output in this task and nothing in the question contains it. Such a value is read, never worked out: if you did not see it, do not state it. Write your whole answer again, complete, using only values you saw, or look it up with your tools first. Do not talk about this check.")
}

re!(
    CHECKED_FILE_RE,
    r#"(?i)\bI (?:ran|checked|looked (?:at|in|into|through)|read|opened|inspected|examined|searched|scanned|reviewed|went through)\b[^.!?\n]{0,80}?\b([\w./-]+\.(?:md|txt|json|py|go|rs|toml|ya?ml|sh|conf|env|ini|csv|js|ts|html|css|c|h|cpp|java|lua|xml|log))\b"#
);

/// Go's `checkedUnseenFile`.
pub fn checked_unseen_file(answer: &str, msgs: &[Message]) -> String {
    let mut seen = String::new();
    for m in msgs {
        for c in &m.tool_calls {
            seen.push_str(&to_lower(&c.function.arguments));
            seen.push('\n');
        }
        if m.role == "tool" {
            seen.push_str(&to_lower(&tool_evidence(&m.content)));
            seen.push('\n');
        }
    }
    for sentence in sentences(answer) {
        for caps in CHECKED_FILE_RE.captures_iter(sentence) {
            if NEGATION_RE.is_match(&sentence[..caps.get(0).expect("match").end()]) {
                continue;
            }
            let file = &caps[1];
            if !seen.contains(&to_lower(&base(file))) {
                return file.to_owned();
            }
        }
    }
    String::new()
}

pub fn checked_unseen_text(file: &str) -> String {
    format!("Check before you finish: your answer says you read or checked {file}, but no tool call in this task touched {file}, so that did not happen. Either read it now with your tools, or write your whole answer again saying plainly that you did not look at it. Do not talk about this check.")
}

re!(
    FAILURE_WORD_RE,
    r#"(?i)\b(error|errors|fail|failed|failing|failure|could not|couldn't|can't|cannot|unable|denied|refused|not found|missing|does not exist|doesn't exist|no such|timed out|timeout|not run|didn't run|did not run|not possible|wasn't able|was not able)\b"#
);
re!(ERROR_KIND_RE, r#""error_kind":\s*"([a-z_]+)""#);
re!(TOOL_NAME_RE, r#""tool":\s*"([a-z_]+)""#);

/// Go's `toolErrorIgnored`: the task's last tool call failed and the answer says nothing failed.
pub fn tool_error_ignored(answer: &str, msgs: &[Message]) -> bool {
    let Some(last) = msgs.iter().rposition(|m| m.role == "tool") else { return false };
    let m = &msgs[last];
    let ok = match m.metadata.get("ok") {
        Some(Any::Bool(b)) => *b,
        _ => !m.content.contains(r#""ok":false"#) && !m.content.contains(r#""ok": false"#),
    };
    let mut kind = mstr(&m.metadata, "error_kind").to_owned();
    if kind.is_empty() {
        if let Some(c) = ERROR_KIND_RE.captures(&m.content) {
            kind = c[1].to_owned();
        }
    }
    if matches!(kind.as_str(), "duplicate_call" | "invalid_arguments" | "not_run") {
        return false;
    }
    let mut name = m.name.clone();
    if name.is_empty() {
        if let Some(c) = TOOL_NAME_RE.captures(&m.content) {
            name = c[1].to_owned();
        }
    }
    if matches!(name.as_str(), "list_dir" | "glob_search" | "grep_search") {
        return false;
    }
    !ok && !FAILURE_WORD_RE.is_match(answer)
}

pub const TOOL_ERROR_IGNORED_TEXT: &str = "Check before you finish: your last tool call failed, but your answer does not say that anything failed. Do not answer as if it had worked. Write your whole answer again, complete, saying plainly what failed and what you could not find out because of it; keep whatever earlier tools really showed. Do not talk about this check.";

re!(CODE_FILE_RE, r#"(?i)\.(go|py|rs|sh|bash|js|mjs|ts|c|h|cpp|java|lua)$"#);
re!(
    NOT_WORKING_RE,
    r#"(?i)\w+n't\s+(?:\w+\s+){0,2}(?:compile|build|work|pass|run)(?:s|ed|ing)?\b|\b(?:fails?|failed|failing) to (?:compile|build|run|pass)\b|\b(?:build|compile|compilation|vet|tests?)(?: check)? (?:fails|failed|failure|errors?)\b|\b(?:can|could|would|if|whether|until|before)\s+(?:\w+\s+){0,2}(?:compile|build|work|pass|run)s?\b"#
);
re!(
    WORKS_CLAIM_RE,
    r#"(?i)\b(compiles?|compiled|builds?( successfully| fine| cleanly)?|built successfully|(all )?tests? (now )?(pass|passed|passes|succeed|succeeded|are passing|run successfully)|passes (all )?(the )?tests|works( as expected| correctly| fine)?|is working|runs? (correctly|fine|successfully)|verified (that )?it works|no errors|without errors|error-free)\b"#
);
re!(TESTS_WORD_RE, r#"(?i)\btests?\b"#);

/// Go's `codeWorksUnverified`: why an answer may not say the code it changed works, or "".
pub fn code_works_unverified(answer: &str, history: &[Hist]) -> String {
    let (mut changed, mut verified_ok, mut tests_ok) = (false, false, false);
    let mut failed = String::new();
    for e in history {
        let p = hstr(e, "path");
        if !is_file_write_tool(hstr(e, "tool_name")) || !hbool(e, "ok") || !CODE_FILE_RE.is_match(p) {
            continue;
        }
        changed = true;
        match hstr(e, "verify") {
            "ok" => {
                verified_ok = true;
                if hbool(e, "verify_tests") {
                    tests_ok = true;
                }
            }
            "failed" => {
                failed = base(p);
                verified_ok = false;
            }
            _ => {}
        }
    }
    if !changed {
        return String::new();
    }
    for s in sentences(answer) {
        if NEGATION_RE.is_match(s) || !WORKS_CLAIM_RE.is_match(&NOT_WORKING_RE.replace_all(s, " ")) {
            continue;
        }
        let claims_tests = TESTS_WORD_RE.is_match(s);
        if !failed.is_empty() {
            return format!("the check after changing {failed} failed");
        }
        if !verified_ok {
            return "no build or test check ran on what you changed".into();
        }
        if claims_tests && !tests_ok {
            return "no tests were run (only a build check), so you cannot say tests pass".into();
        }
    }
    String::new()
}

pub fn code_works_text(why: &str) -> String {
    format!("Check before you finish: your answer says the code works, compiles or passes tests, but {why}. Say only what was really checked: what the check after your change reported (quote its result), and what was not checked. Write your whole answer again, complete, without vouching for anything unchecked. Do not talk about this check.")
}

re!(
    CANNOT_READ_RE,
    r#"(?i)\b(cannot|can'?t|can ?not|unable to|do not have|don'?t have|no) (directly )?(access|read|open|see|view)\b|\bno access to\b|\b(is not|isn'?t|does not|doesn'?t) (present|exist|available)\b|\b(cannot|could not|couldn'?t|can'?t) (be )?(found|located)\b|\bnot found\b"#
);
re!(PROMPT_FILE_RE, r#"[\w./-]*[\w-]\.[A-Za-z0-9]{1,8}\b"#);

/// Go's `deniedReadable`: the path of a file the question names that exists in the workspace, when the answer says she
/// cannot access, read or find it and no tool call named it, or "".
pub fn denied_readable(answer: &str, prompt: &str, cwd: &str, msgs: &[Message], can_read: bool) -> String {
    if !can_read || cwd.is_empty() || !CANNOT_READ_RE.is_match(answer) {
        return String::new();
    }
    for f in PROMPT_FILE_RE.find_iter(prompt).map(|m| m.as_str()) {
        let p = if is_abs(f) { f.to_owned() } else { join(cwd, f) };
        if is_regular(&p) && said_of(answer, &base(f), &CANNOT_READ_RE) && !tool_named_file(msgs, cwd, &p) {
            return clean(&p);
        }
    }
    String::new()
}

static PATH_ARGS: Ty = Ty::Struct(
    "struct { Path string \"json:\\\"path\\\"\"; FilePath string \"json:\\\"file_path\\\"\" }",
    "",
    &[Field("path", &Ty::Str), Field("file_path", &Ty::Str)],
);

/// Go's `toolNamedFile`: whether a tool call had p as its path argument.
fn tool_named_file(msgs: &[Message], cwd: &str, p: &str) -> bool {
    let p = clean(p);
    for m in msgs {
        for c in &m.tool_calls {
            let (v, err) = gostruct::unmarshal(&PATH_ARGS, c.function.arguments.as_bytes());
            if err.is_some() {
                continue;
            }
            for k in ["path", "file_path"] {
                let q = gostruct::str_field(&PATH_ARGS, &v, k);
                if q.is_empty() {
                    continue;
                }
                let q = if is_abs(q) { q.to_owned() } else { join(cwd, q) };
                if clean(&q) == p {
                    return true;
                }
            }
        }
    }
    false
}

/// Go's `saidOf`: one sentence of the answer names the file and matches re.
fn said_of(answer: &str, name: &str, re: &Regex) -> bool {
    sentences(answer).iter().any(|s| s.contains(name) && re.is_match(s))
}

pub fn denied_readable_text(f: &str) -> String {
    format!(
        "Carry on: you said you cannot access or find {}, but it is in your workspace: {f}. Read it now with read_file (that path) and do what was asked; any secret values are blanked from your answer automatically, so you need not refuse to look.",
        base(f)
    )
}

re!(CHANGE_ASK_RE, r#"(?i)\b(fix|change|add|implement|update|modify|correct|rename|refactor|remove|replace)\b"#);
re!(NEW_FILE_ASK_RE, r#"(?i)\b(write|create|make)\b[^.\n]{0,40}\b[\w-]+\.[a-z]{1,5}\b"#);
re!(ADVICE_ASK_RE, r#"(?i)^\s*(how|what|why|which|where|when|can|could|would|should|is|are|do|does|explain|tell me|show me)\b"#);
re!(DESCRIBED_CHANGE_RE, r#"(?is)```|\b(change|replace)\b.{1,80}\b(to|with)\b|\bthis will (make|fix)\b|\bthe fix\b"#);

/// Go's `fixDescribedNotMade`.
pub fn fix_described_not_made(prompt: &str, answer: &str, can_write: bool, history: &[Hist]) -> bool {
    if !can_write
        || !history.is_empty()
        || ADVICE_ASK_RE.is_match(prompt)
        || !(CHANGE_ASK_RE.is_match(prompt) || NEW_FILE_ASK_RE.is_match(prompt))
    {
        return false;
    }
    DESCRIBED_CHANGE_RE.is_match(answer)
}

re!(WRITE_REFUSED_RE, r#"(?i)^refused:|is outside the workspace|permission denied|read-only file system|operation not permitted"#);

static RESULT_BODY: Ty = Ty::Struct(
    "struct { Tool string \"json:\\\"tool\\\"\"; OK bool \"json:\\\"ok\\\"\"; Content string \"json:\\\"content\\\"\" }",
    "",
    &[Field("tool", &Ty::Str), Field("ok", &Ty::Bool), Field("content", &Ty::Str)],
);

/// Go's `writeRefused`: a file-writing tool refused the write in this task.
pub fn write_refused(msgs: &[Message]) -> bool {
    for m in msgs.iter().filter(|m| m.role == "tool") {
        let (v, err) = gostruct::unmarshal(&RESULT_BODY, m.content.as_bytes());
        if err.is_some() {
            continue;
        }
        let ok = matches!(gostruct::field(&RESULT_BODY, &v, "ok"), Some(Val::Bool(true)));
        if !ok
            && is_file_write_tool(gostruct::str_field(&RESULT_BODY, &v, "tool"))
            && WRITE_REFUSED_RE.is_match(gostruct::str_field(&RESULT_BODY, &v, "content"))
        {
            return true;
        }
    }
    false
}

pub const FIX_DESCRIBED_TEXT: &str = "Check before you finish: you were asked to make this change, and you only described it; no file was changed. Make the change now: edit_file for a file that exists (read it first if you have not), write_file for a new one; then say what you changed and what the build check after it said. Do not say the code works unless that check passed.";

/// Go's `toolEvidence`: a tool result as text to compare an answer against.
pub fn tool_evidence(content: &str) -> String {
    tool_texts(content).join("\n")
}

re!(FENCE_RE, r#"(?s)```[\w+-]*\n(.*?)```"#);
re!(QUOTE_ASK_RE, r#"(?i)\bquote\b|\bwhich (line|function|method)\b|\bshow (me )?the (line|code|function)\b"#);
re!(QUOTE_FRAME_RE, r#"(?i)\b(line|lines|code|function|snippet|excerpt)s? (from|in)\b|\bthe relevant lines?\b"#);
re!(OMITTED_RE, r#"\[\.\.\. lines \d+-\d+ of \d+ omitted[^\]]*\]"#);

/// Go's `inventedCode`: a line of a code block presented as quoted that no tool showed, and the read's note of what it left
/// out.
pub fn invented_code(answer: &str, prompt: &str, msgs: &[Message]) -> (String, String) {
    let blocks: Vec<&str> = FENCE_RE.captures_iter(answer).map(|c| c.get(1).map_or("", |g| g.as_str())).collect();
    if blocks.is_empty() || !(QUOTE_ASK_RE.is_match(prompt) || QUOTE_FRAME_RE.is_match(answer)) {
        return (String::new(), String::new());
    }
    let mut ev = String::new();
    let mut used = false;
    for m in msgs {
        if m.tool_calls.iter().any(|c| c.function.name == "write_file" || c.function.name == "edit_file") {
            return (String::new(), String::new());
        }
        if m.role == "tool" {
            used = true;
            ev.push_str(&tool_evidence(&m.content));
            ev.push('\n');
        }
    }
    if !used {
        return (String::new(), String::new());
    }
    let evidence = squash(&ev);
    let mut unseen: Vec<String> = Vec::new();
    let mut total = 0usize;
    for b in blocks {
        for l in b.split('\n') {
            let l = squash(l);
            if l.replace(' ', "").len() < 10 {
                continue;
            }
            total += 1;
            if !evidence.contains(&l) {
                unseen.push(l);
            }
        }
    }
    let long = unseen.iter().any(|l| l.replace(' ', "").len() >= 20);
    if unseen.is_empty() || (unseen.len() < 2 && !long) || unseen.len() * 3 < total {
        return (String::new(), String::new());
    }
    let omitted = OMITTED_RE.find(&ev).map_or("", |m| m.as_str()).to_owned();
    (unseen.swap_remove(0), omitted)
}

pub fn invented_code_text(line: &str, omitted: &str) -> String {
    let mut t = format!("Check before you finish: your answer quotes code as if it were in the file, but the tools never showed it (for example: {line}). Do not quote code you have not seen.");
    if !omitted.is_empty() {
        t += &format!(" The file was not read in full: {omitted}. Read those lines (read_file with start_line and end_line) or search for the name with grep_search, then quote the real lines.");
    } else {
        t += " Look the code up with your tools, then quote the real lines, or say plainly that you have not found them.";
    }
    t + " Write your whole answer again. Do not talk about this check."
}

re!(DECOMPILED_RE, r#"(?i)\bdecompil(ed|ing|ation)\b"#);
re!(
    NOT_DECOMPILED_RE,
    r#"(?i)\b(not|never|without|no|before|until|would need|need to|should|could|can|will|let me|i'll)\b[^.]{0,30}\bdecompil"#
);

/// Go's `decompileClaimed`.
pub fn decompile_claimed(answer: &str, msgs: &[Message]) -> bool {
    if !DECOMPILED_RE.is_match(answer) || NOT_DECOMPILED_RE.is_match(answer) {
        return false;
    }
    let mut used = false;
    for c in msgs.iter().flat_map(|m| &m.tool_calls) {
        if c.function.name == "reverse_engineer" {
            used = true;
            if c.function.arguments.replace(' ', "").contains(r#""command":"decompile""#) {
                return false;
            }
        }
    }
    used
}

pub const DECOMPILE_CLAIMED_TEXT: &str = "Check before you finish: your answer says code was decompiled, but no decompile ran in this task; the overview does not show any logic. Decompile the function now (reverse_engineer, command decompile, target its name) and explain the rule from its code. Write your whole answer again. Do not talk about this check.";

re!(
    ANNOUNCE_RE,
    r#"(?i)^(ok(ay)?,? |now,? |first,? |next,? |so,? )*(let me|let's|i'?ll|i will|i'?m going to|i am going to|i need to)\s+(first |now |then |start by |begin by |go ahead and |quickly )*(check|read|look|examine|inspect|list|open|run|search|find|investigate|verify|see|fix|make|apply|edit|update|start|begin|review|take|test|try|create|write)\b"#
);
re!(SENTENCES_RE, r#"[.!?:]\s+"#);

/// Go's `announcedNotDone`: a reply that ends by saying what she will do next, with no tool call.
pub fn announced_not_done(answer: &str) -> bool {
    let a = trim_space(answer);
    if a.is_empty() || fields(a).len() > 150 || a.contains("```") {
        return false;
    }
    let last = trim_space(a.rsplit('\n').next().unwrap_or(""));
    let parts = go_split(&SENTENCES_RE, last.trim_end_matches(['.', '!', '?', ':', ' ']));
    let mut s = trim_space(parts[parts.len() - 1]);
    if last.ends_with(':') || parts.len() == 1 {
        s = trim_space(last.trim_end_matches(':'));
        let p = go_split(&SENTENCES_RE, s);
        if let Some(l) = p.last() {
            s = trim_space(l);
        }
    }
    let s = s.trim_start_matches(['*', '-', '#', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', ' ']);
    ANNOUNCE_RE.is_match(s) && !to_lower(s).contains("know")
}

pub const ANNOUNCED_TEXT: &str = "Carry on: you said what you would do next and then stopped, so nothing was done. Do it now with your tools, without announcing it first; answer when the task is finished.";

re!(OUTLINE_ENTRY_RE, r#"(?m)^  (\d+): .*?\b(?:fn|func|def|class|function)\s+(\w+)"#);

/// Go's `regexp.MustCompile(`\b` + regexp.QuoteMeta(n) + `\b`).MatchString(s)`: n somewhere in s with an ASCII word
/// boundary on both sides.
fn name_word(n: &str, s: &str) -> bool {
    let w = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let (sb, nb) = (s.as_bytes(), n.as_bytes());
    if nb.is_empty() {
        return false;
    }
    let boundary = |i: usize| {
        let before = i > 0 && w(sb[i - 1]);
        let after = i < sb.len() && w(sb[i]);
        before != after
    };
    (0..=sb.len().saturating_sub(nb.len())).any(|i| sb[i..].starts_with(nb) && boundary(i) && boundary(i + nb.len()))
}

/// Go's `strconv.Atoi` as `n, _ :=` reads it: the value, the largest or smallest int64 when it is out of range, else 0.
fn atoi_lenient(s: &str) -> i64 {
    match s.parse::<i64>() {
        Ok(n) => n,
        Err(_) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => i64::MAX,
        Err(_) => 0,
    }
}

static LINE_SPAN: Ty = Ty::Struct(
    "struct { Start int \"json:\\\"start_line\\\"\"; End int \"json:\\\"end_line\\\"\" }",
    "",
    &[Field("start_line", &Ty::Int), Field("end_line", &Ty::Int)],
);

fn int_field(t: &Ty, v: &Val, name: &str) -> i64 {
    match gostruct::field(t, v, name) {
        Some(Val::Int(n)) => *n,
        _ => 0,
    }
}

/// Go's `unreadDefinition`: a function the answer talks about that the task saw only as one line of a cut read's outline.
pub fn unread_definition(answer: &str, msgs: &[Message]) -> (String, i64) {
    let mut reads: Vec<(i64, i64)> = Vec::new();
    let mut place: HashMap<String, i64> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for m in msgs {
        for c in m.tool_calls.iter().filter(|c| c.function.name == "read_file") {
            let (v, err) = gostruct::unmarshal(&LINE_SPAN, c.function.arguments.as_bytes());
            let (start, mut end) = (int_field(&LINE_SPAN, &v, "start_line"), int_field(&LINE_SPAN, &v, "end_line"));
            if err.is_none() && (start > 0 || end > 0) {
                if end == 0 {
                    end = start.wrapping_add(2000);
                }
                reads.push((start, end));
            }
        }
        if m.role != "tool" || !m.content.contains("defined in the omitted lines") {
            continue;
        }
        for t in tool_texts(&m.content) {
            for e in OUTLINE_ENTRY_RE.captures_iter(&t) {
                let n = atoi_lenient(&e[1]);
                let name = e[2].to_owned();
                if !place.contains_key(&name) {
                    order.push(name.clone());
                }
                place.insert(name, n);
            }
        }
    }
    for name in order {
        let line = place[&name];
        let covered = reads.iter().any(|&(from, to)| from <= line.wrapping_add(1) && to >= line.wrapping_add(3));
        if !covered && name.len() > 3 && name_word(&name, answer) {
            return (name, line);
        }
    }
    (String::new(), 0)
}

pub fn unread_definition_text(name: &str, line: i64) -> String {
    format!(
        "Carry on: your answer says what {name} does, but you have seen only its first line (line {line}, in the list of what the read left out). Read its lines now (read_file start_line {line}, end_line {}), then write your whole answer again from the code you saw.",
        line.wrapping_add(40)
    )
}

re!(OWN_FUNCTIONS_RE, r#"(?m)^## functions \(the program's own\)\n\n(.+)$"#);
re!(R2_FUNCTION_RE, r#"(?m)^0x[0-9a-fA-F]+\s+\d+\s+\d+\s+(\S+)\s*$"#);
re!(NM_FUNCTION_RE, r#"(?m)^[0-9a-fA-F]{8,16} [Tt] (\S+)\s*$"#);
re!(LIMBO_LIST_RE, r#"limbo-decompile\s+functions\s+(\S+)"#);
re!(
    LOGIC_RE,
    r#"(?i)\b(checks?|verif(y|ies)|validates?|compares?|requires?|accepts?|rejects?|returns?|must|ensures?|enforces?|computes?|calculates?|decrypts?|encrypts?|decodes?|xors?|length|characters?)\b"#
);
re!(NOT_SEEN_LOGIC_RE, r#"(?i)\b(not (yet )?(seen|verified|decompiled)|have not|haven'?t|unverified)\b"#);

static COMMAND_TARGET: Ty =
    Ty::Struct("struct { Command string; Target string }", "", &[Field("Command", &Ty::Str), Field("Target", &Ty::Str)]);
static OK_PTR: Ty = Ty::Struct("struct { OK *bool \"json:\\\"ok\\\"\" }", "", &[Field("ok", &Ty::Ptr(&Ty::Bool))]);

/// undecompiledFunction's `add`: a listed name, unless it is the runtime's or too short to be the program's own.
fn add_listed(n: &str, via: &'static str, listed: &mut HashMap<String, &'static str>, order: &mut Vec<String>) {
    let n = n.strip_prefix("sym.").unwrap_or(n);
    if n.starts_with('_')
        || n.starts_with("imp.")
        || n.contains('.')
        || matches!(n, "deregister_tm_clones" | "register_tm_clones" | "frame_dummy")
        || n.len() <= 4
    {
        return;
    }
    if !listed.contains_key(n) {
        order.push(n.to_owned());
        listed.insert(n.to_owned(), via);
    }
}

/// Go's `undecompiledFunction`: one of the program's own functions a tool listed by name only, which the answer explains
/// when nothing in this task decompiled it, and how to decompile it.
pub fn undecompiled_function(answer: &str, msgs: &[Message]) -> (String, String) {
    let mut listed: HashMap<String, &'static str> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut decompiled: HashSet<String> = HashSet::new();
    let mut limbo_file = String::new();
    let mut call_tool: HashMap<String, String> = HashMap::new();
    let mut pending: HashMap<String, Vec<String>> = HashMap::new();
    for m in msgs {
        for c in &m.tool_calls {
            call_tool.insert(c.id.clone(), c.function.name.clone());
            let (v, _) = gostruct::unmarshal(&COMMAND_TARGET, c.function.arguments.as_bytes());
            let command = gostruct::str_field(&COMMAND_TARGET, &v, "Command");
            let target = gostruct::str_field(&COMMAND_TARGET, &v, "Target");
            match c.function.name.as_str() {
                "reverse_engineer" => {
                    if command == "decompile" {
                        pending.insert(c.id.clone(), vec![target.strip_prefix("sym.").unwrap_or(target).to_owned()]);
                    }
                }
                "limbo_validate" => {
                    if let Some(f) = LIMBO_LIST_RE.captures(command) {
                        limbo_file = f[1].to_owned();
                    }
                    if command.contains("limbo-decompile decompile") {
                        let p = pending.entry(c.id.clone()).or_default();
                        for w in fields(command) {
                            p.push(w.strip_prefix("sym.").unwrap_or(w).to_owned());
                        }
                    }
                }
                _ => {}
            }
        }
        if m.role != "tool" {
            continue;
        }
        let (res, err) = gostruct::unmarshal(&OK_PTR, m.content.as_bytes());
        let ok = match gostruct::field(&OK_PTR, &res, "ok") {
            Some(Val::Ptr(Some(b))) => Some(**b == Val::Bool(true)),
            _ => None,
        };
        if err.is_some() || ok != Some(false) {
            if let Some(ns) = pending.get(&m.tool_call_id) {
                decompiled.extend(ns.iter().cloned());
            }
        }
        let tool = if m.name.is_empty() { call_tool.get(&m.tool_call_id).map_or("", String::as_str) } else { m.name.as_str() };
        for t in tool_texts(&m.content) {
            match tool {
                "reverse_engineer" => {
                    if let Some(l) = OWN_FUNCTIONS_RE.captures(&t) {
                        for e in l[1].split(", ") {
                            if let Some((n, _)) = e.split_once(" (") {
                                add_listed(n, "reverse_engineer", &mut listed, &mut order);
                            }
                        }
                    }
                    for e in NM_FUNCTION_RE.captures_iter(&t) {
                        add_listed(&e[1], "reverse_engineer", &mut listed, &mut order);
                    }
                }
                "limbo_validate" => {
                    for e in R2_FUNCTION_RE.captures_iter(&t) {
                        add_listed(&e[1], "limbo_validate", &mut listed, &mut order);
                    }
                }
                _ => {}
            }
        }
    }
    let answer_sentences = sentences(answer);
    for n in &order {
        if decompiled.contains(n) {
            continue;
        }
        for sentence in &answer_sentences {
            if name_word(n, sentence)
                && LOGIC_RE.is_match(sentence)
                && !NOT_SEEN_LOGIC_RE.is_match(sentence)
                && !NOT_DECOMPILED_RE.is_match(sentence)
            {
                if listed[n] == "limbo_validate" {
                    let file = if limbo_file.is_empty() { "<the file>" } else { limbo_file.as_str() };
                    return (n.clone(), format!("limbo_validate, profile decompile, command limbo-decompile decompile {file} {n}"));
                }
                return (n.clone(), format!("reverse_engineer, command decompile, the same path, target {n}"));
            }
        }
    }
    (String::new(), String::new())
}

pub fn undecompiled_function_text(name: &str, how: &str) -> String {
    format!("Carry on: your answer says what {name} does, but nothing in this task decompiled it; the list of functions shows only its name. Decompile it now ({how}), then write your whole answer again from its code. If you cannot, say plainly that you have not seen its code.")
}

re!(
    CANNOT_CHECK_RE,
    r#"(?i)\b(my|the|system|status) (tools?|access)\b[^.\n]{0,30}\b(are|is) (restricted|unavailable|not available|disabled|limited)\b|\bwithout (seeing|reading|looking at|access to)\b[^\n]{0,60}?\b(i'?ll|i will|i'?d|let'?s) (assume|guess)\b|\b(can'?t|cannot|can not|unable to|not able to)\s+(directly\s+)?(check|look at|see|access|run|use|monitor)\b[^.\n]{0,30}\b(system|computer|machine|status|tools?|performance|logs?|processes)\b"#
);

/// Go's `claimsNoTools`: an answer that says it cannot look when it has not tried a tool and has some.
pub fn claims_no_tools(answer: &str, msgs: &[Message], tools: &[String]) -> bool {
    if tools.is_empty() || !CANNOT_CHECK_RE.is_match(answer) {
        return false;
    }
    !msgs.iter().any(|m| m.role == "tool" || !m.tool_calls.is_empty())
}

pub fn claims_no_tools_text(tools: &[String]) -> String {
    format!(
        "Carry on: you said you cannot check, but you have not tried, and these tools are yours to use now: {}. Call the one that answers the question, then answer from what it shows.",
        tools.join(", ")
    )
}

/// Go's `reminders`: the harness's `<system-reminder>` messages.
pub fn reminders(msgs: &[Message]) -> String {
    let mut b = String::new();
    for m in msgs.iter().filter(|m| m.role == "user" && m.content.contains("<system-reminder>")) {
        b.push_str(&m.content);
        b.push('\n');
    }
    b
}

re!(UP_TO_DATE_RE, r#"(?i)\b(is|are) (already |fully )?up[ -]to[ -]date\b"#);
re!(UPDATE_EV_RE, r#"(?i)up[ -]to[ -]date|no updates|0 updates|updates?: (none|0)\b|nothing to do"#);
re!(ADVISE_RE, r#"(?i)\b(ensure|make sure|check|verify|confirm|whether|if|keep|should)\b"#);

/// Go's `upToDateUnseen`.
pub fn up_to_date_unseen(answer: &str, evidence: &str) -> bool {
    if UPDATE_EV_RE.is_match(evidence) {
        return false;
    }
    sentences(answer).iter().any(|s| UP_TO_DATE_RE.find(s).is_some_and(|m| !ADVISE_RE.is_match(&s[..m.start()])))
}

pub const UP_TO_DATE_TEXT: &str = "Check before you finish: your answer says the system is up to date, but nothing in this task showed that. Leave it out, or check first with your tools. Write your whole answer again. Do not talk about this check.";

/// Go's `exfilInAnswer`: code in the answer that would take keys or passwords off the computer.
pub fn exfil_in_answer(answer: &str) -> String {
    for c in FENCE_RE.captures_iter(answer) {
        let why = crate::tools::exfil::exfil_risk(c.get(1).map_or("", |g| g.as_str()));
        if !why.is_empty() {
            return why;
        }
    }
    String::new()
}

pub const EXFIL_IN_ANSWER_TEXT: &str = "Check before you finish: your answer contains a script that sends keys or passwords off this computer. That is refused, as a file and as text, whoever asks. Write your whole answer again without any of that code: say plainly that you will not write it, and why (it would hand someone the owner's keys).";

/// Go's `ownToolNamed`: a quote that starts with one of her own tools' names.
pub fn own_tool_named(q: &str, tool_names: &[String]) -> bool {
    let f = fields(q.trim_matches(['`', '"', '\'', ' ']));
    f.first().is_some_and(|w| tool_names.iter().any(|n| n == w))
}

/// Go's `fixPendingAfterFailedCheck`: the file whose last change failed its check, when the answer describes the next fix.
pub fn fix_pending_after_failed_check(answer: &str, history: &[Hist]) -> String {
    let mut last = String::new();
    let mut failed = false;
    for e in history {
        let p = hstr(e, "path");
        if !is_file_write_tool(hstr(e, "tool_name")) || !hbool(e, "ok") || !CODE_FILE_RE.is_match(p) {
            continue;
        }
        let b = base(p);
        let test_file = b.starts_with("test_") || b.ends_with("_test.go") || b.ends_with("_test.py");
        failed = hstr(e, "verify") == "failed" && (hstr(e, "verify_kind") != "tests" || !test_file);
        last = b;
    }
    if !failed || !DESCRIBED_CHANGE_RE.is_match(answer) {
        return String::new();
    }
    last
}

re!(TEST_FAILS_RE, r#"(?i)\b(tests?|unittest|pytest)\b[^.\n]{0,40}\b(fails?|failing|failed)\b|\bfailing tests?\b"#);

/// Go's `unreproducedFailure`.
pub fn unreproduced_failure(prompt: &str, msgs: &[Message], offered: &[String]) -> bool {
    if !offered.iter().any(|n| n == "check_code") || !TEST_FAILS_RE.is_match(prompt) {
        return false;
    }
    !msgs.iter().flat_map(|m| &m.tool_calls).any(|c| c.function.name == "check_code" || is_file_write_tool(&c.function.name))
}

pub const UNREPRODUCED_FAILURE_TEXT: &str = "Carry on: the task says a test fails, and nothing has run it yet. Run check_code on the package first to see the failure and the code it points at, then make the fix with edit_file.";

pub fn fix_pending_text(file: &str) -> String {
    format!("Carry on: the check after your last change to {file} failed, and you have described the fix without making it. Make it now with edit_file, then answer with what the check said.")
}

/// Go's `editLoopStopped`.
pub fn edit_loop_stopped(msgs: &[Message]) -> bool {
    msgs.iter().any(|m| m.role == "tool" && m.content.contains("refusing attempt"))
}

/// Go's `failedWriteHidden`: a failed write the answer says nothing about, as the failure's message.
pub fn failed_write_hidden(answer: &str, msgs: &[Message]) -> String {
    if FAILURE_WORD_RE.is_match(answer) {
        return String::new();
    }
    for m in msgs {
        if m.role != "tool"
            || !(m.name == "write_file"
                || m.name == "edit_file"
                || m.content.contains(r#""tool": "write_file""#)
                || m.content.contains(r#""tool":"write_file""#))
        {
            continue;
        }
        let meta_failed = matches!(m.metadata.get("ok"), Some(Any::Bool(false)));
        if meta_failed || m.content.contains(r#""ok": false"#) || m.content.contains(r#""ok":false"#) {
            for t in tool_texts(&m.content) {
                if t.contains("denied") || t.contains("cannot write") || t.contains("read-only") {
                    return clip_quote(&t, 160);
                }
            }
        }
    }
    String::new()
}

/// Go's `clipQuote`: at most n characters, then "...".
pub fn clip_quote(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s.to_owned(),
    }
}

pub fn failed_write_hidden_text(why: &str) -> String {
    format!("Check before you finish: a write you were asked for failed ({why}), and your answer does not say so. Say plainly what failed and why; if you put the file somewhere else instead, say that this is not what was asked. Write your whole answer again. Do not talk about this check.")
}

/// Go's `unwrapReminder`: a `<system-reminder>` wrapper copied onto the answer taken off.
pub fn unwrap_reminder(answer: &str) -> String {
    let t = trim_space(answer);
    if let Some(inner) = t.strip_prefix("<system-reminder>").and_then(|r| r.strip_suffix("</system-reminder>")) {
        return trim_space(inner).to_owned();
    }
    answer.to_owned()
}

/// Go's `regexp.QuoteMeta`.
fn quote_meta(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Go's `suggestToolRE`: the tool's name as a whole word after a word that suggests using it.
fn suggest_tool_re(name: &str) -> Option<Regex> {
    let n = quote_meta(name);
    let tok = if name.contains('_') { format!("`?{n}`?") } else { format!("`{n}`") };
    goregexp::compile(&format!(
        r#"(?i)\b(use|run|call|try|with|next step|you can|i can|i will|i'?ll|should)\b[^.\n]{{0,40}}(^|[^\w./-]){tok}([^\w./-]|$)"#
    ))
    .ok()
}

re!(
    CONDITIONAL_RE,
    r#"(?i)\b(if you (have|want|suspect|like|know|can|give|provide|share)|provide|give me|candidate|guess|would you like|do you want|shall i|should i|let me know|once you)\b"#
);
re!(REFUSED_RE, r#"(?i)"?(content"?\s*:\s*"?)?refused:"#);

/// Go's `toolNamedNotCalled`: one of her own tools the answer tells the owner to use instead of calling it, or "".
pub fn tool_named_not_called(answer: &str, msgs: &[Message], offered: &[String]) -> String {
    let mut called: HashSet<&str> = HashSet::new();
    for m in msgs {
        for c in &m.tool_calls {
            called.insert(&c.function.name);
        }
        if m.role == "tool" && REFUSED_RE.is_match(&m.content) {
            return String::new();
        }
    }
    for name in offered {
        if called.contains(name.as_str()) || !answer.contains(name.as_str()) {
            continue;
        }
        let Some(re) = suggest_tool_re(name) else { continue };
        for loc in re.find_iter(answer) {
            let s = sentence_around(answer, loc.start(), loc.end());
            if CONDITIONAL_RE.is_match(s) || closing_advice(answer, loc.start(), s) {
                continue;
            }
            return name.clone();
        }
    }
    String::new()
}

re!(VERIFY_ADVICE_RE, r#"(?i)\bto (verify|confirm|reproduce|double[- ]?check|cross[- ]?check|validate)\b"#);
re!(IF_WORD_RE, r#"(?i)(\w+)?\W*\bif\b"#);

/// Go's `closingAdvice`: a tool named in advice after an answer that already holds the result.
pub fn closing_advice(answer: &str, at: usize, sentence: &str) -> bool {
    let start = answer[..at].rfind(['.', '!', '?', '\n']).map_or(0, |i| i + 1);
    if trim_space(&answer[..start]).len() < 80 {
        return false;
    }
    if VERIFY_ADVICE_RE.is_match(sentence) {
        return true;
    }
    for m in IF_WORD_RE.captures_iter(sentence) {
        let w = to_lower(m.get(1).map_or("", |g| g.as_str()));
        if !matches!(
            w.as_str(),
            "check" | "see" | "test" | "determine" | "know" | "tell" | "whether" | "verify" | "confirm" | "find" | "out"
        ) {
            return true;
        }
    }
    false
}

/// Go's `sentenceAround`: the sentence (or line) holding answer[i:j].
pub fn sentence_around(answer: &str, i: usize, j: usize) -> &str {
    let start = answer[..i].rfind(['.', '!', '?', '\n']).map_or(0, |k| k + 1);
    let end = answer[j..].find(['.', '!', '?', '\n']).map_or(answer.len(), |k| k + j);
    &answer[start..end]
}
