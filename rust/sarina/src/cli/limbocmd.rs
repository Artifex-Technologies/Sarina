//! Go: `limbo/integration.go`'s commands: `sarina limbo-profiles` (what Limbo offers, with what each profile allows) and
//! `sarina limbo-validate [--timeout S] PROFILE COMMAND [ARG...]` (a command run in a disposable guest), both through
//! Limbo's agent socket (`SARINA_LIMBO_SOCKET`, `limbo::Client`). Errors go to stdout, as Go's do.

use super::{Env, EXIT_FAILURE, EXIT_OK, EXIT_USAGE};
use crate::goflag::{FlagSet, HELP};
use crate::limbo::{empty_with_skipped, Client, ENV_VAR};

pub fn profiles(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = FlagSet::new("limbo-profiles");
    if let Err(e) = fs.parse(args, &mut env.stderr) {
        return if e == HELP { EXIT_OK } else { EXIT_USAGE };
    }
    let client = Client { path: env.getenv(ENV_VAR), ..Client::default() };
    let got = client.describe_profiles_and_skipped(&env.getenv_fn());
    let l = match got {
        Ok(l) => l,
        Err(e) => {
            env.println(&e.0);
            return EXIT_FAILURE;
        }
    };
    if l.profiles.is_empty() {
        if l.skipped.is_empty() {
            env.println("No validation profiles. A human adds them under /etc/limbo/validation/.");
        } else {
            env.println(&empty_with_skipped(l.skipped.len()));
        }
    }
    for p in &l.profiles {
        env.println(&p.as_text());
    }
    for s in l.skipped_lines() {
        env.println(&s);
    }
    EXIT_OK
}

/// Options come before the profile; everything after it is the guest's command, so its dashes are never this
/// program's options.
pub fn validate(env: &mut Env, args: &[String]) -> i32 {
    let mut fs = FlagSet::new("limbo-validate");
    fs.usage_header("usage: sarina limbo-validate [--timeout SECONDS] PROFILE COMMAND [ARG ...]");
    fs.int("timeout", 300, "seconds for the command; Limbo caps this per profile");
    let rest = match fs.parse(args, &mut env.stderr) {
        Ok(r) => r,
        Err(e) => return if e == HELP { EXIT_OK } else { EXIT_USAGE },
    };
    let Some((profile, mut parts)) = rest.split_first() else {
        env.eprintln("sarina limbo-validate: a PROFILE is required");
        fs.usage(&mut env.stderr);
        return EXIT_USAGE;
    };
    if parts.first().is_some_and(|p| p == "--") {
        parts = &parts[1..];
    }
    if parts.is_empty() {
        env.println("limbo-validate needs a command to run, e.g.: sarina limbo-validate PROFILE uname -r");
        return EXIT_USAGE;
    }
    let client = Client { path: env.getenv(ENV_VAR), ..Client::default() };
    let got = client.validate(profile, &parts.join(" "), fs.num("timeout"), &env.getenv_fn());
    match got {
        Ok(run) => {
            env.println(&run.as_text());
            if run.succeeded() {
                EXIT_OK
            } else {
                EXIT_FAILURE
            }
        }
        Err(e) => {
            env.println(&e.0);
            EXIT_FAILURE
        }
    }
}
