//! Go: `go/tools/git.go`: `git_read` (status, diff, log, show; read-only), the settings that keep a repository's own
//! config from running programs while git reads, the refusal of a repository whose config names programs that cannot be
//! switched off (filter drivers, includes), and the bounds on what is read from git (testdata/parity/git.json, against a
//! stand-in git).

use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use crate::goany::Any;
use crate::goregexp;
use crate::gotext::{self, to_valid_utf8, trim_space};
use crate::tools::exec::{child_env, look_path_go, run_program, Capture};
use crate::tools::files::{arg_str, clip_text, fail};
use crate::tools::fsutil::resolve_in_root;
use crate::tools::private::private_guard;
use crate::tools::registry::{Context, Output};

/// What is kept of git's output (the answer is cut to 12,000 characters after); the rest is read and dropped.
pub const MAX_GIT_OUTPUT: usize = 1 << 20;

/// Settings that keep a repository's own config from running programs when git only reads (Go's gitSafe).
pub const GIT_SAFE: &[&str] = &[
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "color.ui=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "gpg.program=/bin/false",
    "-c",
    "gpg.ssh.program=/bin/false",
    "-c",
    "gpg.x509.program=/bin/false",
    "-c",
    "core.sshCommand=/bin/false",
    "-c",
    "credential.helper=",
    "--no-pager",
];

static GIT_REF_RE: LazyLock<Regex> = LazyLock::new(|| goregexp::compile(r"^[\w./~^@{}-]{1,100}$").expect("a fixed pattern"));
static GIT_RISKY_KEY: LazyLock<Regex> =
    LazyLock::new(|| goregexp::compile(r"^(filter\..+\.(clean|smudge|process)|include\..*|includeif\..*)$").expect("a fixed pattern"));

fn args_with(root: &str, rest: &[&str]) -> Vec<String> {
    GIT_SAFE.iter().copied().chain(["-C", root]).chain(rest.iter().copied()).map(str::to_owned).collect()
}

fn with_vars(mut env: Vec<Vec<u8>>, vars: &[&str]) -> Vec<Vec<u8>> {
    env.extend(vars.iter().map(|v| v.as_bytes().to_vec()));
    env
}

/// Go's `GitConfigRisk`: the settings in root's own git config that would make git run a program, or "" when there are
/// none (and when git cannot list them). A listing past MAX_GIT_OUTPUT is not read whole, so it counts as risky.
pub fn git_config_risk(root: &str, deadline: Instant, ctx: &crate::goctx::Ctx) -> String {
    let Ok(git) = look_path_go("git") else { return String::new() };
    let env = with_vars(child_env(&Context::default()), &["GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0"]);
    let args = args_with(root, &["config", "--local", "--name-only", "--list"]);
    let Ok(cap) = run_program(&git, &args, &env, Capture::Stdout, MAX_GIT_OUTPUT, deadline, ctx) else { return String::new() };
    if !cap.status.is_some_and(|s| s.success()) {
        return String::new();
    }
    if cap.over {
        return format!("more than {MAX_GIT_OUTPUT} bytes of settings, not read whole");
    }
    let text = gotext::lossy(&cap.out);
    let risky: Vec<String> =
        trim_space(&text).split('\n').map(|k| gotext::to_lower(trim_space(k))).filter(|k| GIT_RISKY_KEY.is_match(k)).collect();
    risky.join(", ")
}

/// Go's `git_read` handler.
pub fn git_read(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let command = arg_str(args, "command");
    let mut r#ref = arg_str(args, "ref");
    if !r#ref.is_empty() && (r#ref.starts_with('-') || !GIT_REF_RE.is_match(&r#ref)) {
        return fail(format!("ref {} is not a plain commit name", gotext::quote(&r#ref)));
    }
    let mut git_args: Vec<String>;
    let mut diff_head = false;
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match command.as_str() {
        "status" => git_args = v(&["status", "--short", "--branch"]),
        "diff" => {
            git_args = v(&["diff", "--no-ext-diff", "--no-textconv", "--stat", "--patch"]);
            if !r#ref.is_empty() {
                git_args.push(r#ref.clone());
            }
            diff_head = r#ref.is_empty();
        }
        "log" => git_args = v(&["log", "-n", "15", "--date=short", "--format=%h %ad %an: %s"]),
        "show" => {
            if r#ref.is_empty() {
                r#ref = "HEAD".into();
            }
            git_args = v(&["show", "--no-ext-diff", "--no-textconv", "--stat", "--patch", "--format=%h %ad %an%n%n%B", "--date=short"]);
            git_args.push(r#ref.clone());
        }
        _ => return fail(format!("unknown command {}: status, diff, log or show", gotext::quote(&command))),
    }
    let mut path_args: Vec<String> = Vec::new();
    let given = arg_str(args, "path");
    if !given.is_empty() {
        let p = match resolve_in_root(&tc.root, &given) {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        if let Err(e) = private_guard(&given, &p, "read") {
            return fail(e);
        }
        path_args = vec!["--".into(), gotext::lossy(&p)];
    }
    let mut deadline = Instant::now() + Duration::from_secs(20);
    if let Some(d) = tc.ctx.deadline() {
        deadline = deadline.min(d);
    }
    let risky = git_config_risk(&tc.root, deadline, &tc.ctx);
    if !risky.is_empty() {
        return fail(format!(
            "this repository's own git settings name programs for git to run ({risky}): git_read does not run git here; ask the owner to \
             look at .git/config"
        ));
    }
    let env = with_vars(child_env(tc), &["GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0"]);
    let git = look_path_go("git");
    if diff_head {
        if let Ok(git) = &git {
            let rev =
                run_program(git, &args_with(&tc.root, &["rev-parse", "-q", "--verify", "HEAD"]), &env, Capture::None, 0, deadline, &tc.ctx);
            if rev.is_ok_and(|c| c.status.is_some_and(|s| s.success())) {
                r#ref = "HEAD".into();
                git_args.push(r#ref.clone());
            }
        }
    }
    git_args.extend(path_args);
    let env = with_vars(env, &["GIT_PAGER=cat", "PAGER=cat"]);
    let refs: Vec<&str> = git_args.iter().map(String::as_str).collect();
    let (out, err) = match &git {
        Err(e) => (Vec::new(), Some(e.clone())),
        Ok(git) => match run_program(git, &args_with(&tc.root, &refs), &env, Capture::Both, MAX_GIT_OUTPUT, deadline, &tc.ctx) {
            Err(e) => (Vec::new(), Some(e)),
            Ok(c) => {
                let err = match c.status {
                    Some(st) => crate::tools::exec::exit_error(st),
                    None => Some("signal: killed".into()),
                };
                (c.out, err)
            }
        },
    };
    let mut text = trim_space(&to_valid_utf8(&out)).to_owned();
    if let Some(e) = err {
        if text.is_empty() {
            text = e;
        }
        return fail(format!("git {command} failed: {}", clip_text(&text, 1000)));
    }
    if text.is_empty() {
        text = match command.as_str() {
            "diff" => format!(
                "No changes: the working tree matches {} (new files git does not track yet are listed by status).",
                if r#ref.is_empty() { "the index" } else { &r#ref }
            ),
            "log" => "No commits yet.".into(),
            "status" => "Clean.".into(),
            _ => String::new(),
        };
    }
    Output {
        content: clip_text(&text, 12000),
        metadata: BTreeMap::from([("action".to_owned(), Any::String("git_read".into())), ("command".to_owned(), Any::String(command))]),
        error: None,
    }
}

/// git_read with the Go build's definition.
pub fn git_tool() -> crate::tools::registry::Tool {
    crate::tools::defs::tool("git_read", std::sync::Arc::new(git_read))
}
