//! Go: `cli/memorycmd.go`: memory-status, memory-show, session-search, memory-review-list, memory-review-approve and
//! memory-review-reject. memory-review (which needs the model settings of the chat command) runs its pass through
//! `memory::review::review_outcome`; its flags come with the chat command's port.

use super::{Env, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::goflag::{FlagSet, HELP};
use crate::gopath;
use crate::gotext;
use crate::memory::review::Queue;
use crate::memory::Runtime;

fn cwd_flag(name: &'static str) -> FlagSet {
    let mut fs = FlagSet::new(name);
    fs.string("cwd", ".", "workspace directory");
    fs
}

/// Go's `parseOrExit`: the arguments left, or the exit status (0 for -h).
fn parse_or_exit(fs: &mut FlagSet, args: &[String], env: &mut Env) -> Result<Vec<String>, i32> {
    fs.parse(args, &mut env.stderr).map_err(|e| if e == HELP { EXIT_OK } else { EXIT_USAGE })
}

fn runtime(env: &Env, cwd: &str) -> Runtime {
    Runtime::from_workspace(cwd, &env.home_dir())
}

/// `sarina memory-status`: curated memory's paths, usage and entries.
pub fn memory_status(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = cwd_flag("memory-status");
    if let Err(code) = parse_or_exit(&mut fs, args, env) {
        return code;
    }
    let s = runtime(env, &fs.str("cwd")).render_summary();
    env.println(&s);
    EXIT_OK
}

/// `sarina memory-show`: the curated-memory block exactly as sessions receive it.
pub fn memory_show(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = cwd_flag("memory-show");
    if let Err(code) = parse_or_exit(&mut fs, args, env) {
        return code;
    }
    let mut block = runtime(env, &fs.str("cwd")).render_prompt_block();
    if block.is_empty() {
        block = "No curated memory is stored.".into();
    }
    env.println(&block);
    EXIT_OK
}

/// `sarina session-search [--cwd D] [--session-dir D] [--max N] <query...>` (flags anywhere among the words).
pub fn session_search(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = cwd_flag("session-search");
    fs.string("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)");
    fs.int("max", 10, "maximum number of hits");
    let words = match crate::flagx::parse(&mut fs, args, &mut env.stderr) {
        Ok(w) => w,
        Err(e) => return if e == HELP { EXIT_OK } else { EXIT_USAGE },
    };
    let query = gotext::trim_space(&words.join(" ")).to_string();
    if query.is_empty() {
        env.eprintln("usage: sarina session-search [flags] <query...>");
        return EXIT_USAGE;
    }
    let mut dir = fs.str("session-dir");
    if dir.is_empty() {
        // config.DefaultRuntime(cwd).SessionDirectory
        let cwd = fs.str("cwd");
        let abs = gopath::abs(cwd.as_bytes()).unwrap_or_else(|_| cwd.clone().into_bytes());
        dir = gotext::lossy(&gopath::join(&[&abs, b".port_sessions", b"agent"]));
    }
    let out = crate::memory::search_sessions(&query, &dir, fs.num("max"));
    env.println(&out);
    EXIT_OK
}

/// `sarina memory-review-list`.
pub fn memory_review_list(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = cwd_flag("memory-review-list");
    if let Err(code) = parse_or_exit(&mut fs, args, env) {
        return code;
    }
    let s = Queue::from_workspace(&fs.str("cwd")).render_summary();
    env.println(&s);
    EXIT_OK
}

/// `sarina memory-review-approve [--cwd D] <entry_id>`: applied through the memory gate again.
pub fn memory_review_approve(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = cwd_flag("memory-review-approve");
    let rest = match parse_or_exit(&mut fs, args, env) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if rest.len() != 1 {
        env.eprintln("usage: sarina memory-review-approve [--cwd D] <entry_id>");
        return EXIT_USAGE;
    }
    let cwd = fs.str("cwd");
    match Queue::from_workspace(&cwd).approve(&rest[0], &runtime(env, &cwd)) {
        Ok(m) => {
            env.println(&format!("approved: {} {} memory entry ({} total)", m.action, m.scope, m.after_count));
            EXIT_OK
        }
        Err(e) => {
            env.eprintln(&format!("sarina: {e}"));
            EXIT_FAILURE
        }
    }
}

/// `sarina memory-review-reject [--cwd D] <entry_id>`.
pub fn memory_review_reject(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = cwd_flag("memory-review-reject");
    let rest = match parse_or_exit(&mut fs, args, env) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if rest.len() != 1 {
        env.eprintln("usage: sarina memory-review-reject [--cwd D] <entry_id>");
        return EXIT_USAGE;
    }
    match Queue::from_workspace(&fs.str("cwd")).reject(&rest[0]) {
        Ok(()) => {
            env.println(&format!("rejected: {}", rest[0]));
            EXIT_OK
        }
        Err(e) => {
            env.eprintln(&format!("sarina: {e}"));
            EXIT_FAILURE
        }
    }
}
