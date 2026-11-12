//! Go: `go/agent/secrets.go`. Secrets she read never come back out in an answer: values assigned to password-, secret-,
//! token- and key-like names in this task's tool output are blanked in the final answer and in a reply as it streams.

use std::sync::LazyLock;

use regex::Regex;

use crate::goany::Any;
use crate::gojson;
use crate::goregexp;
use crate::gotext::{equal_fold, trim_space};
use crate::session::Message;

static SECRET_ASSIGN_RE: LazyLock<Regex> = LazyLock::new(|| {
    goregexp::compile(
        r#"(?im)^[ \t]*(?:export[ \t]+)?["']?[\w.-]*(?:pass(?:word|wd|phrase)?|secret|token|api[_-]?key|private[_-]?key|credential|auth)[\w.-]*["']?[ \t]*[=:][ \t]*["']?([^\s"'#,;]{4,})"#,
    )
    .expect("secret pattern")
});

/// Go's `secretValues`: the values of secret-looking settings in tool output, longest first. Go's sort is not stable; for up
/// to twelve values it is an insertion sort, which is, so values of the same length keep their order as here.
pub fn secret_values(texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in texts {
        for c in SECRET_ASSIGN_RE.captures_iter(t) {
            let v = &c[1];
            if seen.contains(v)
                || v.starts_with("[REDACTED")
                || v.starts_with('$')
                || equal_fold(v, "true")
                || equal_fold(v, "false")
                || equal_fold(v, "null")
            {
                continue;
            }
            seen.insert(v.to_owned());
            out.push(v.to_owned());
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v.len()));
    out
}

fn tool_output_texts(msgs: &[Message]) -> Vec<String> {
    msgs.iter().filter(|m| m.role == "tool").flat_map(|m| tool_texts(&m.content)).collect()
}

/// Go's `hideSecretsRead`: secret values from this task's tool output blanked wherever they appear in the answer, and
/// whether any was.
pub fn hide_secrets_read(answer: &str, msgs: &[Message]) -> (String, bool) {
    let mut answer = answer.to_owned();
    let mut changed = false;
    for v in secret_values(&tool_output_texts(msgs)) {
        if answer.contains(&v) {
            answer = answer.replace(&v, "[hidden]");
            changed = true;
        }
    }
    (answer, changed)
}

/// Go's `hideAll`.
pub fn hide_all(text: &str, msgs: &[Message]) -> String {
    hide_secrets_read(text, msgs).0
}

/// Go's `toolTexts`: a tool result as the patterns can read it: the text itself, then, when it is JSON, every string inside
/// it (Go walks an object in random order; here in key order).
pub fn tool_texts(content: &str) -> Vec<String> {
    let mut out = vec![content.to_owned()];
    let t = trim_space(content);
    if !(t.starts_with('{') || t.starts_with('[')) {
        return out;
    }
    let (v, err) = gojson::unmarshal_any(t.as_bytes());
    if err.is_some() {
        return out;
    }
    // without recursion: a result may nest as deep as JSON allows
    let mut stack: Vec<&Any> = vec![&v];
    while let Some(x) = stack.pop() {
        match x {
            Any::String(s) => out.push(s.clone()),
            Any::Object(m) => stack.extend(m.values().rev()),
            Any::Array(a) => stack.extend(a.iter().rev()),
            _ => {}
        }
    }
    out
}

/// Go's `streamRedactor`: a streamed reply with secret values hidden as it goes, the last len(longest)-1 bytes held back so
/// a secret is always whole when it is replaced.
#[derive(Debug)]
pub struct StreamRedactor {
    secrets: Vec<String>,
    hold: usize,
    pending: String,
}

impl StreamRedactor {
    /// None when the task's tool output holds no secret values.
    pub fn new(msgs: &[Message]) -> Option<StreamRedactor> {
        let vals = secret_values(&tool_output_texts(msgs));
        let hold = vals.first()?.len() - 1;
        Some(StreamRedactor { secrets: vals, hold, pending: String::new() })
    }

    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        for v in &self.secrets {
            self.pending = self.pending.replace(v.as_str(), "[hidden]");
        }
        if self.pending.len() <= self.hold {
            return String::new();
        }
        let mut cut = self.pending.len() - self.hold;
        while cut > 0 && !self.pending.is_char_boundary(cut) {
            cut -= 1;
        }
        let rest = self.pending.split_off(cut);
        std::mem::replace(&mut self.pending, rest)
    }

    pub fn flush(&mut self) -> String {
        let mut out = std::mem::take(&mut self.pending);
        for v in &self.secrets {
            out = out.replace(v.as_str(), "[hidden]");
        }
        out
    }
}
