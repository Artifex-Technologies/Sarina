//! Go: `cli/sessionmigrate.go`: `sarina session-migrate [--session-dir D] [--cwd D] [--dry-run]`, retired tool names in
//! saved sessions rewritten (`session::save::migrate_dir`).

use super::{Env, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::goflag::{FlagSet, HELP};

pub fn session_migrate(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = FlagSet::new("session-migrate");
    fs.string("cwd", ".", "workspace directory");
    fs.string("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)");
    fs.bool("dry-run", false, "report what would change without writing anything");
    if let Err(e) = fs.parse(args, &mut env.stderr) {
        return if e == HELP { EXIT_OK } else { EXIT_USAGE };
    }
    let mut dir = fs.str("session-dir");
    if dir.is_empty() {
        dir = crate::config::default_runtime(&fs.str("cwd")).session_directory;
    }
    let dry = fs.is_true("dry-run");
    let (r, err) = crate::session::save::migrate_dir(&dir, dry, &env.getenv_fn());
    if let Some(e) = err {
        env.eprintln(&format!("session-migrate: {e}"));
        return EXIT_FAILURE;
    }
    let verb = if dry { "would change" } else { "changed" };
    for f in &r.files {
        env.println(&format!("{verb}: {f}"));
    }
    for f in &r.failed {
        env.eprintln(&format!("session-migrate: not migrated, left as it was: {f}"));
    }
    env.println(&format!(
        "session-migrate: {} files scanned, {} {verb}, {} calls rewritten, {} results renamed, {} unreadable skipped, {} failed",
        r.scanned,
        r.changed,
        r.calls,
        r.results,
        r.skipped,
        r.failed.len()
    ));
    if r.failed.is_empty() {
        EXIT_OK
    } else {
        EXIT_FAILURE
    }
}
