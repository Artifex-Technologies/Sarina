//! Go: `tiers/cliclient.go`: the claude_cli tier hands the conversation to the local `claude` executable in print mode. A
//! stateless one-shot: the call the router was about to make becomes "finish this task", and the CLI's final text is the
//! turn's answer. It never sees an API key (secret-named variables, every tier's key variable and any variable holding a
//! secret this process has are left out of its environment), it runs in its own process group (killed whole at the
//! timeout or a cancel), and its output is read with a bound.

use super::config::{Tier, DEFAULT_CLI_PATH, DEFAULT_CLI_TIMEOUT};
use super::guard::redact_secrets;
use crate::goany::Any;
use crate::goctx::Ctx;
use crate::gojson;
use crate::gotext;
use crate::llm::openai::{Request, StreamEvent};
use crate::llm::types::{Message, Turn};
use crate::tools::exec::{run_group, GroupKill};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const CLI_PREVIEW_CHARS: usize = 2000;
const CLI_DIGEST_MESSAGES: usize = 8;
const CLI_DIGEST_CHARS: usize = 400;
/// What is read of the CLI's standard output (its JSON answer); past it the hand-off fails.
pub const MAX_CLI_OUTPUT: usize = 16 << 20;

/// What the last hand-off produced, for the session record (Go's `cliResult`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliResult {
    pub ok: bool,
    pub err: String,
    pub session_id: String,
    pub cost_usd: f64,
    pub stdout: String,
    pub stderr: String,
}

/// Go's `secretEnvName`: a variable name that looks like it holds a credential.
pub fn secret_env_name(name: &str) -> bool {
    let u = to_upper(name);
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL", "AUTH", "COOKIE", "PRIVATE", "SESSION"].iter().any(|w| u.contains(w))
}

/// Go's `strings.ToUpper` (each character's simple upper case).
pub fn to_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

/// The claude CLI as a tier's transport (Go's `cliClient`).
pub struct CliClient {
    pub tier: Tier,
    /// the child's folder ("" = this process's)
    pub workdir: String,
    /// the environment filtered for the child (None = this process's)
    pub environ: Option<Vec<String>>,
    /// literal secret values never to pass on
    pub secrets: Vec<String>,
    pub last: CliResult,
}

impl CliClient {
    /// Go's `childEnv`: the environment minus every secret-named variable, the API key variables and any variable whose
    /// value holds a secret this process has.
    pub fn child_env(&self, api_key_names: &[String]) -> Vec<String> {
        let mut skip: std::collections::HashSet<String> =
            ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"].iter().map(|s| s.to_string()).collect();
        for n in api_key_names {
            skip.insert(to_upper(n));
        }
        let environ = match &self.environ {
            Some(e) => e.clone(),
            None => crate::tools::exec::parent_env().iter().map(|kv| String::from_utf8_lossy(kv).into_owned()).collect(),
        };
        let mut out = Vec::new();
        for kv in environ {
            let Some((name, val)) = kv.split_once('=') else { continue };
            if name.is_empty() || skip.contains(&to_upper(name)) || secret_env_name(name) {
                continue;
            }
            if self.secrets.iter().any(|s| s.len() >= 8 && val.contains(s.as_str())) {
                continue;
            }
            out.push(kv);
        }
        out
    }

    fn timeout_nanos(&self) -> i64 {
        let s = if self.tier.timeout_seconds <= 0.0 { DEFAULT_CLI_TIMEOUT } else { self.tier.timeout_seconds };
        crate::config::go_int(s * 1e9)
    }

    fn path(&self) -> &str {
        if self.tier.cli_path.is_empty() {
            DEFAULT_CLI_PATH
        } else {
            &self.tier.cli_path
        }
    }

    /// Go's `Complete`: `reason` is why the router hands over (Go carries it in the context).
    pub fn complete(
        &mut self,
        ctx: &Ctx,
        req: &Request,
        reason: Option<&str>,
        on: Option<&mut dyn FnMut(StreamEvent)>,
    ) -> Result<Turn, String> {
        self.last = CliResult::default();
        let prompt = build_prompt(&req.messages, reason.unwrap_or("the local backend is unavailable"));
        let cwd = if self.workdir.is_empty() {
            std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            self.workdir.clone()
        };
        let secrets = self.secrets.clone();
        let fail = |last: &mut CliResult, msg: String| -> Result<Turn, String> {
            let (msg, _) = redact_secrets(&msg, &secrets);
            last.err.clone_from(&msg);
            Err(msg)
        };
        let nanos = self.timeout_nanos();
        let started = Instant::now();
        let deadline = started + Duration::from_nanos(nanos.max(0) as u64);
        let env: Vec<Vec<u8>> = self.child_env(std::slice::from_ref(&self.tier.api_key_env)).into_iter().map(String::into_bytes).collect();
        let args = self.tier.command(&prompt, &cwd);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut over = false;
        let mut sink = |stream: &'static str, b: &[u8]| {
            let (buf, max) = if stream == "stdout" { (&mut out, MAX_CLI_OUTPUT) } else { (&mut err, CLI_PREVIEW_CHARS) };
            let room = max - buf.len();
            if b.len() > room && stream == "stdout" {
                over = true;
            }
            buf.extend_from_slice(&b[..b.len().min(room)]);
        };
        let run = run_group(self.path(), &args, &env, Some(&cwd), deadline, ctx, GroupKill::Group, &mut sink);
        let preview = |b: &[u8]| String::from_utf8_lossy(&b[..b.len().min(CLI_PREVIEW_CHARS)]).into_owned();
        self.last.stdout = redact_secrets(&preview(&out), &self.secrets).0;
        self.last.stderr = redact_secrets(&preview(&err), &self.secrets).0;
        if ctx.err().is_some() {
            return fail(&mut self.last, "claude CLI hand-off cancelled".into());
        }
        if Instant::now() >= deadline {
            return fail(&mut self.last, format!("claude CLI timed out after {}s", gotext::float_v(nanos as f64 / 1e9)));
        }
        let end = match run {
            Ok(e) => e,
            Err(e) => return fail(&mut self.last, format!("Failed to launch claude CLI: {e}")),
        };
        let status = end.status;
        let exit = status.and_then(|s| (!s.success()).then(|| s.code().unwrap_or(-1)));
        if exit.is_none() && end.io_incomplete {
            return fail(&mut self.last, "Failed to launch claude CLI: exec: WaitDelay expired before I/O complete".into());
        }
        if over {
            return fail(&mut self.last, format!("claude CLI's output is larger than {MAX_CLI_OUTPUT} bytes; not read"));
        }
        let (v, jerr) = gojson::unmarshal_any(&out);
        let payload = match v.into_object() {
            Ok(m) if jerr.is_none() && exit.is_none() => m,
            _ => return fail(&mut self.last, format!("claude CLI exited {} or returned no parseable JSON output", exit.unwrap_or(0))),
        };
        if let Some(Any::String(s)) = payload.get("session_id") {
            self.last.session_id.clone_from(s);
        }
        if let Some(Any::Number(n)) = payload.get("total_cost_usd") {
            self.last.cost_usd = *n;
        }
        let text = match payload.get("result") {
            Some(Any::String(s)) => s.clone(),
            _ => String::new(),
        };
        if truthy(payload.get("is_error")) {
            return fail(&mut self.last, "claude CLI reported is_error=true".into());
        }
        self.last.ok = true;
        let raw = BTreeMap::from([
            ("cli_session_id".to_string(), Any::String(self.last.session_id.clone())),
            ("total_cost_usd".to_string(), Any::Number(self.last.cost_usd)),
        ]);
        let turn = Turn { content: text.clone(), finish_reason: "stop".into(), has_finish_reason: true, raw: Some(raw), ..Turn::default() };
        if let Some(on) = on {
            on(StreamEvent { kind: "message_start".into(), ..StreamEvent::default() });
            on(StreamEvent { kind: "content_delta".into(), delta: text, ..StreamEvent::default() });
            on(StreamEvent { kind: "message_stop".into(), finish_reason: "stop".into(), ..StreamEvent::default() });
        }
        Ok(turn)
    }
}

fn truthy(v: Option<&Any>) -> bool {
    match v {
        Some(Any::Bool(b)) => *b,
        Some(Any::Number(n)) => *n != 0.0,
        Some(Any::String(s)) => !s.is_empty(),
        _ => false,
    }
}

/// Go's `BuildPrompt`: the self-contained prompt for `claude -p`: the task, why it is handed over, a digest of the
/// latest messages (already guarded by the router), and a request to finish.
pub fn build_prompt(msgs: &[Message], reason: &str) -> String {
    let task = msgs
        .iter()
        .rev()
        .find(|m| {
            m.role == "user"
                && !gotext::trim_space(&m.content).is_empty()
                && !gotext::trim_space(&m.content).starts_with("<system-reminder>")
        })
        .map(|m| m.content.as_str())
        .unwrap_or("");
    let tail: Vec<&Message> = msgs.iter().filter(|m| m.role == "user" || m.role == "assistant" || m.role == "tool").collect();
    let tail = &tail[tail.len().saturating_sub(CLI_DIGEST_MESSAGES)..];
    let mut recent = Vec::new();
    for m in tail {
        let content = gotext::trim_space(&m.content);
        if content.is_empty() {
            continue;
        }
        let content = if content.chars().count() > CLI_DIGEST_CHARS {
            format!("{}...", content.chars().take(CLI_DIGEST_CHARS).collect::<String>())
        } else {
            content.to_string()
        };
        recent.push(format!("[{}] {}", m.role, content));
    }
    let summary = if recent.is_empty() { "(no prior progress)".to_string() } else { recent.join("\n") };
    format!(
        "A local coding agent was working on the task below and could not continue locally, so this task has been handed off \
         to you to finish.\n\nEscalation reason: {reason}.\n\nOriginal task:\n{task}\n\nProgress so far (most recent \
         messages):\n{summary}\n\nPlease continue and complete the task."
    )
}
