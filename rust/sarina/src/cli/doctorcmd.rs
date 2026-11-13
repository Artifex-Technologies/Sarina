//! Go: `doctor/command.go`: `sarina doctor [--json] [--cwd D] [--session-dir D]`, the read-only check-up
//! ([`crate::doctor`]) printed as a table or as JSON; exit status 1 when a check failed.

use super::{Env, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::goctx::Ctx;
use crate::goflag::{FlagSet, HELP};

/// `sarina doctor`.
pub fn doctor(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = FlagSet::new("doctor");
    fs.bool("json", false, "print the report as a JSON array of {id,status,finding,remedy}");
    fs.string("cwd", ".", "workspace directory");
    fs.string("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)");
    let rest = match fs.parse(args, &mut env.stderr) {
        Ok(rest) => rest,
        Err(e) => {
            env.flush();
            return if e == HELP { EXIT_OK } else { EXIT_USAGE };
        }
    };
    if let Some(a) = rest.first() {
        env.eprintln(&format!("sarina doctor: unexpected argument {}", crate::gotext::quote(a)));
        return EXIT_USAGE;
    }
    let cs = crate::doctor::run(
        &Ctx::background(),
        crate::doctor::Options {
            cwd: fs.str("cwd"),
            session_dir: fs.str("session-dir"),
            home: env.home.clone(),
            getenv: env.getenv.clone(),
            ..Default::default()
        },
    );
    if fs.is_true("json") {
        env.println(&crate::doctor::json(&cs));
    } else {
        env.stdout.extend_from_slice(crate::doctor::text(&cs).as_bytes());
        env.flush();
    }
    if crate::doctor::has_failure(&cs) {
        EXIT_FAILURE
    } else {
        EXIT_OK
    }
}
