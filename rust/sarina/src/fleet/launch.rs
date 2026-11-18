//! Go: `chymaera/fleet/launch.go` and `fleet-launch`: launching another AI agent, proposal first, confirmed by a person,
//! audited. No auto-approve flag is ever added and there is no option to add one (the forms are Omarchy's with those
//! flags removed); the proposal runs nothing; every attempt, confirmed or refused, is written to a log Heimdall can read
//! (the prompt as a hash, a length and a short preview). Only the `fleet-launch` command reaches this: never a tool.

use super::{ascii, jstr};
use sha2::{Digest, Sha256};

const PROMPT_PREVIEW_LENGTH: usize = 60;
const MAX_PROMPT_LENGTH: usize = 8000;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Positional,
    Flag,
    Joined,
}

/// Go's `forms`: per agent, argv up to the prompt and how the prompt is attached ("--" wherever the agent takes it, so a
/// prompt beginning with a dash cannot be read as an option). No entry has an approval-skipping flag.
const FORMS: &[(&str, Kind, &[&str], &[&str])] = &[
    ("claude", Kind::Positional, &["claude"], &["--"]),
    ("codex", Kind::Positional, &["codex"], &["--"]),
    ("copilot", Kind::Flag, &["copilot"], &["--interactive"]),
    ("cursor-agent", Kind::Positional, &["cursor-agent"], &["agent", "--"]),
    ("gemini", Kind::Flag, &["gemini"], &["--prompt-interactive"]),
    ("grok", Kind::Positional, &["grok"], &["--"]),
    ("hermes", Kind::Joined, &["hermes", "chat", "--tui"], &["--query"]),
    ("muse", Kind::Positional, &["muse"], &["--"]),
    ("omp", Kind::Positional, &["omp"], &["--"]),
    ("opencode", Kind::Flag, &["opencode"], &["--prompt"]),
    ("pi", Kind::Positional, &["pi"], &[]),
];

/// Go's `Unsupported`: agents Omarchy can launch that this deliberately does not, with why (sorted by name).
const UNSUPPORTED: &[(&str, &str)] = &[
    ("crush", "its only prompt-seeded mode (`crush run`) never asks for approval"),
    ("openclaw", "it needs Omarchy's own launcher to attach to its gateway"),
];

/// Go's `NoApprovalPrompts`: agents that never ask; launching one is unattended whatever Sarina passes.
fn no_approval_prompts(agent: &str) -> bool {
    agent == "pi"
}

fn form(agent: &str) -> Option<(Kind, &'static [&'static str], &'static [&'static str])> {
    FORMS.iter().find(|f| f.0 == agent).map(|f| (f.1, f.2, f.3))
}

fn unsupported(agent: &str) -> Option<&'static str> {
    UNSUPPORTED.iter().find(|u| u.0 == agent).map(|u| u.1)
}

/// Go's `SupportedAgents`, sorted.
fn supported_agents() -> Vec<&'static str> {
    FORMS.iter().map(|f| f.0).collect()
}

/// A launch described, not done (Go's `LaunchProposal`).
struct Proposal {
    agent: String,
    argv: Vec<String>,
    cwd: Vec<u8>,
    /// empty when not found on PATH
    binary_path: String,
    display: String,
    prompt_sha256: String,
    prompt_length: usize,
    warnings: Vec<String>,
}

impl Proposal {
    /// Go's `AsText`: never the prompt itself.
    fn as_text(&self) -> String {
        let mut approval = "  approval: Sarina passes no auto-approve flag".to_string();
        if !no_approval_prompts(&self.agent) {
            approval.push_str("; the agent will ask before acting");
        }
        let mut lines = vec![
            format!("Proposed launch: {}", self.agent),
            format!("  command : {}", self.display),
            format!("  cwd     : {}", crate::gotext::lossy(&self.cwd)),
            format!("  prompt  : {} characters", self.prompt_length),
            approval,
        ];
        if self.binary_path.is_empty() {
            lines.push(format!("  warning : `{}` was not found on PATH", self.argv[0]));
        }
        for w in &self.warnings {
            lines.push(format!("  warning : {w}"));
        }
        lines.join("\n")
    }
}

fn build_argv(kind: Kind, head: &[&str], attach: &[&str], prompt: &str) -> Vec<String> {
    let mut argv: Vec<String> = head.iter().map(|s| s.to_string()).collect();
    if prompt.is_empty() {
        return argv;
    }
    match kind {
        Kind::Positional => {
            argv.extend(attach.iter().map(|s| s.to_string()));
            argv.push(prompt.to_string());
        }
        Kind::Flag => {
            argv.push(attach[0].to_string());
            argv.push(prompt.to_string());
        }
        Kind::Joined => argv.push(format!("{}={prompt}", attach[0])),
    }
    argv
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Go's `PlanLaunch` with `exec.LookPath`: the launch described, or why it cannot be.
fn plan(agent: &str, prompt: &str, cwd: &[u8]) -> Result<Proposal, String> {
    if let Some(reason) = unsupported(agent) {
        return Err(format!("{agent} is not supported: {reason}."));
    }
    let Some((kind, head, attach)) = form(agent) else {
        return Err(format!("unknown agent {}; supported: {}", crate::gotext::quote(agent), supported_agents().join(", ")));
    };
    let n = prompt.chars().count();
    if n > MAX_PROMPT_LENGTH {
        return Err(format!("prompt is {n} characters; the limit is {MAX_PROMPT_LENGTH}"));
    }
    if prompt.contains('\0') {
        return Err("prompt contains a NUL byte".into());
    }
    let display = build_argv(kind, head, attach, if prompt.is_empty() { "" } else { "<prompt>" }).join(" ");
    let mut warnings = Vec::new();
    if no_approval_prompts(agent) {
        warnings.push(format!("{agent} has no approval prompts of its own, so it will run unattended"));
    }
    let cwd: &[u8] = if cwd.is_empty() { b"." } else { cwd };
    let mut abs = crate::gopath::abs(cwd).unwrap_or_else(|_| cwd.to_vec());
    if let Ok(resolved) = crate::gopath::eval_symlinks(&abs) {
        abs = resolved;
    }
    Ok(Proposal {
        agent: agent.to_string(),
        argv: build_argv(kind, head, attach, prompt),
        cwd: abs,
        binary_path: crate::tools::exec::look_path_go(head[0]).unwrap_or_default(),
        display,
        prompt_sha256: sha256_hex(prompt.as_bytes()),
        prompt_length: n,
        warnings,
    })
}

/// Go's `AuditLogPath`.
fn audit_log_path(getenv: &dyn Fn(&str) -> String, home: &str) -> Vec<u8> {
    let s = getenv("XDG_STATE_HOME");
    if !s.is_empty() {
        return super::join(&[s.as_bytes(), b"sarina", b"agent-launch.jsonl"]);
    }
    super::join(&[home.as_bytes(), b".local", b"state", b"sarina", b"agent-launch.jsonl"])
}

/// Go's `time.Now().UTC().Truncate(time.Microsecond).Format("2006-01-02T15:04:05.999999+00:00")`.
fn audit_time() -> String {
    let t = crate::gotime::Time::now();
    let (y, m, d, hh, mm, ss, _) = t.civil();
    let mut frac = format!("{:06}", t.nanos() / 1000);
    while frac.ends_with('0') {
        frac.pop();
    }
    let frac = if frac.is_empty() { String::new() } else { format!(".{frac}") };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{frac}+00:00")
}

/// Go's `audit`: one JSON line appended to the log, which is opened only as a regular file of its own (a link at its
/// name is not followed, a named pipe is not waited on).
fn audit(p: &Proposal, prompt: &str, outcome: &str, log: &[u8], detail: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let preview: String = prompt.chars().take(PROMPT_PREVIEW_LENGTH).collect::<String>().replace('\n', " ");
    let line = ascii(&format!(
        "{{\"time\":{},\"agent\":{},\"argv0\":{},\"cwd\":{},\"promptSha256\":{},\"promptLength\":{},\"promptPreview\":{},\
         \"outcome\":{},\"detail\":{}}}",
        jstr(&audit_time()),
        jstr(&p.agent),
        jstr(&p.argv[0]),
        jstr(&crate::gotext::lossy(&p.cwd)),
        jstr(&p.prompt_sha256),
        p.prompt_length,
        jstr(&preview),
        jstr(outcome),
        jstr(detail)
    ));
    crate::goos::mkdir_all(&crate::gopath::dir(log))?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o644)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(crate::gopath::os_path(log))
        .map_err(|e| crate::goerr::path("open", log, &e))?;
    if !f.metadata().is_ok_and(|m| m.file_type().is_file()) {
        return Err(format!("{} is not a regular file", crate::gotext::lossy(log)));
    }
    f.write_all(format!("{line}\n").as_bytes()).map_err(|e| crate::goerr::path("write", log, &e))
}

/// Go's `Launch` with `ExecRunner`: the agent run in the foreground with this terminal, in the proposal's folder; its exit
/// status (Go's -1 for one ended by a signal). Refusals are audited too; when the audit cannot be written, nothing runs.
fn launch(p: &Proposal, prompt: &str, confirmed: bool, log: &[u8]) -> Result<i32, String> {
    let refuse = |detail: &str, e: String| -> Result<i32, String> {
        if let Err(aerr) = audit(p, prompt, "refused", log, detail) {
            return Err(format!("{e} (and the audit log could not be written: {aerr})"));
        }
        Err(e)
    };
    if sha256_hex(prompt.as_bytes()) != p.prompt_sha256 {
        return refuse("prompt differs from the proposal", "prompt does not match the proposal that was shown".into());
    }
    if !confirmed {
        return refuse("not confirmed by the operator", "launch was not confirmed".into());
    }
    if p.binary_path.is_empty() {
        return refuse(&format!("{} not on PATH", p.argv[0]), format!("`{}` was not found on PATH", p.argv[0]));
    }
    if let Err(e) = audit(p, prompt, "launched", log, "") {
        return Err(format!("not launching: the audit log could not be written: {e}"));
    }
    // Go's exec.Command: the program found on PATH now, argv[0] as given, the folder entered at the start
    let path = crate::tools::exec::look_path_go(&p.argv[0])?;
    if let Err(e) = std::fs::metadata(crate::gopath::os_path(&p.cwd)) {
        return Err(crate::goerr::path("chdir", &p.cwd, &e));
    }
    use std::os::unix::process::CommandExt;
    let st = std::process::Command::new(&path)
        .arg0(&p.argv[0])
        .args(&p.argv[1..])
        .current_dir(crate::gopath::os_path(&p.cwd))
        .status()
        .map_err(|e| crate::goerr::path("fork/exec", path.as_bytes(), &e))?;
    Ok(st.code().unwrap_or(-1))
}

/// Whether standard input is a character device (Go's `os.ModeCharDevice` on `os.Stdin`).
fn stdin_is_char_device() -> bool {
    // SAFETY: fstat on descriptor 0 into a zeroed stat buffer of the right type.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    (unsafe { libc::fstat(0, &mut st) }) == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFCHR
}

/// Go's `splitArgs`: flags anywhere among the positionals (as argparse takes them); everything after a first "--" is
/// positional, so a prompt may begin with a dash.
fn split_args(fs: &mut crate::goflag::FlagSet, args: &[String], env: &mut crate::cli::Env) -> Result<Vec<String>, i32> {
    let (mut head, tail): (Vec<String>, Vec<String>) = match args.iter().position(|a| a == "--") {
        Some(i) => (args[..i].to_vec(), args[i + 1..].to_vec()),
        None => (args.to_vec(), Vec::new()),
    };
    let mut positional = Vec::new();
    loop {
        let rest = super::parse_flags(fs, &head, env)?;
        if rest.is_empty() {
            positional.extend(tail);
            return Ok(positional);
        }
        positional.push(rest[0].clone());
        head = rest[1..].to_vec();
    }
}

/// Go's `launchCommand`: `fleet-launch AGENT [PROMPT] [--cwd D] [--yes]`. Exit status: the agent's own when it ran; 1
/// cancelled; 2 could not ask or plan. `--yes` only skips the question: the agent still asks before acting.
pub fn command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut fs = crate::goflag::FlagSet::new("fleet-launch");
    let usage_line =
        format!("usage: sarina fleet-launch AGENT [PROMPT] [--cwd DIR] [--yes]\n  supported agents: {}", supported_agents().join(", "));
    fs.usage_header(&usage_line);
    fs.string("cwd", ".", "directory to launch the agent in");
    fs.bool("yes", false, "skip the question; the launched agent still asks before acting");
    let pos = match split_args(&mut fs, args, env) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if pos.is_empty() || pos.len() > 2 {
        env.eprintln("sarina fleet-launch: expected AGENT and at most one PROMPT");
        fs.usage(&mut env.stderr);
        env.flush();
        return 2;
    }
    let agent = pos[0].as_str();
    let prompt = pos.get(1).map(String::as_str).unwrap_or("");
    if unsupported(agent).is_none() && form(agent).is_none() {
        let mut all = supported_agents();
        all.extend(UNSUPPORTED.iter().map(|u| u.0));
        env.eprintln(&format!("sarina fleet-launch: invalid agent {} (choose from {})", crate::gotext::quote(agent), all.join(", ")));
        return 2;
    }
    let cwd = fs.str("cwd");
    if !super::stat(cwd.as_bytes()).is_some_and(|s| s.is_dir) {
        env.println(&format!("fleet-launch: --cwd {} is not a directory", crate::gotext::quote(&cwd)));
        return 2;
    }
    let proposal = match plan(agent, prompt, cwd.as_bytes()) {
        Ok(p) => p,
        Err(e) => {
            env.println(&format!("fleet-launch: {e}"));
            return 2;
        }
    };
    env.println(&proposal.as_text());
    let log = audit_log_path(&env.getenv_fn(), &super::home_of(env));
    if !fs.is_true("yes") {
        if !stdin_is_char_device() {
            env.println("Not launching: standard input is not a terminal, so there is no one to confirm. Re-run interactively, or pass --yes to skip the question.");
            return 2;
        }
        env.stdout.extend_from_slice(b"Launch? [y/N] ");
        env.flush();
        let mut line = Vec::new();
        {
            use std::io::BufRead;
            let _ = std::io::stdin().lock().read_until(b'\n', &mut line);
        }
        // Go's ReadString: an error (the end of input too) with nothing read is no answer
        if line.is_empty() {
            env.println("");
            env.println("Not launching: no answer was given.");
            return 2;
        }
        let a = crate::gotext::to_lower(crate::gotext::trim_space(&crate::gotext::lossy(&line)));
        if a != "y" && a != "yes" {
            env.println("Cancelled.");
            // the refusal is audited too: every attempt is recorded
            let _ = launch(&proposal, prompt, false, &log);
            return 1;
        }
    }
    env.flush();
    match launch(&proposal, prompt, true, &log) {
        Ok(status) => status,
        Err(e) => {
            env.println(&format!("fleet-launch: {e}"));
            2
        }
    }
}
