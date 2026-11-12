//! Go: `tiers/wake.go`: powering a tier on. Only a tier's configured wake entry is ever run, and only when the tier is
//! unreachable: an http(s) URL is requested with GET; anything else is a shell command, run only with shell commands
//! allowed and, unless destructive ones are, after the bash tool's destructive-command check. The command gets this
//! process's environment without secret-looking variables.

use super::config::Tier;
use crate::config::Permissions;
use crate::goctx::Ctx;
use crate::gohttp::{self, Dial, Fail, Opts};
use crate::gotext;
use crate::tools::exec::{exit_error, look_path_go, parent_env, run_group, GroupKill};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Powers a tier on (Go's `WakeFunc`).
pub type WakeFunc = Box<dyn Fn(&Ctx, &Tier) -> Result<(), String>>;

const SECRET_NAME_PARTS: &[&str] = &["SECRET", "TOKEN", "PASSWORD", "PRIVATE_KEY", "API_KEY", "CREDENTIAL", "AUTH"];

/// Go's `filteredEnv`: the entries whose name looks like a secret's are left out.
pub fn filtered_env(env: &[String]) -> Vec<String> {
    env.iter()
        .filter(|kv| {
            let name = kv.split_once('=').map(|(n, _)| n).unwrap_or(kv);
            let up = super::cli::to_upper(name);
            !SECRET_NAME_PARTS.iter().any(|p| up.contains(p))
        })
        .cloned()
        .collect()
}

/// Go's `NewWake`: the wake runner under the command permission model (`root` is the workspace for the destructive
/// check).
pub fn new_wake(perms: Permissions, root: String) -> WakeFunc {
    Box::new(move |ctx: &Ctx, t: &Tier| {
        let w = gotext::trim_space(&t.wake).to_string();
        if w.is_empty() {
            return Err(format!("no wake entry configured for tier {}", gotext::quote(&t.name)));
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        if w.starts_with("http://") || w.starts_with("https://") {
            if crate::gourl::parse(&w).is_none() {
                return Err(format!("bad wake URL: parse {}: invalid URL", gotext::quote(&w)));
            }
            let o = Opts { dial: Dial::Tcp, method: "GET", url: &w, headers: &[], body: None, tls: None };
            let dl: gohttp::Deadline = Rc::new(Cell::new(deadline));
            return match gohttp::open(&o, ctx, &dl) {
                Err(Fail::Deadline | Fail::Ctx | Fail::Other(_)) => Err("wake request failed".into()),
                Ok(r) if r.status >= 400 => Err(format!("wake request returned HTTP {}", r.status)),
                Ok(_) => Ok(()),
            };
        }
        if !perms.allow_shell_commands {
            return Err("wake command not run: shell commands are disabled (the operator can re-run with --allow-shell)".into());
        }
        if !perms.allow_destructive_shell_commands {
            let v = crate::tools::destructive::check_destructive(w.as_bytes(), root.as_bytes());
            if v.blocked {
                return Err(format!("wake command not run: {}", String::from_utf8_lossy(&v.reason)));
            }
        }
        let sh = look_path_go("sh").map_err(|_| "wake command not run: no shell found".to_string())?;
        let env: Vec<String> = parent_env().iter().map(|kv| String::from_utf8_lossy(kv).into_owned()).collect();
        let env: Vec<Vec<u8>> = filtered_env(&env).into_iter().map(String::into_bytes).collect();
        let mut ignore = |_: &'static str, _: &[u8]| {};
        let end = run_group(&sh, &["-c".to_string(), w], &env, None, deadline, ctx, GroupKill::Process, &mut ignore)
            .map_err(|e| format!("wake command failed: {e}"))?;
        match end.status {
            None => Err("wake command failed: signal: killed".into()),
            Some(st) => match exit_error(st) {
                Some(e) => Err(format!("wake command failed: {e}")),
                None => Ok(()),
            },
        }
    })
}
