//! `sarina-rs`: the Rust build's entry point, installed as `/usr/bin/sarina` (and its `sarina-service` and `sarina-gui`
//! names). As Go's `cmd/sarina`: the name it was started as picks the command, then Heimdall's containment lock is checked
//! before anything runs. Every command is in `cli::COMMANDS`, and the usage (no command, `help`, an unknown one) is
//! answered here too. A command whose arguments are not all UTF-8 is refused (one line on standard error, exit status 2,
//! nothing run): the commands take text. Nothing is handed to the Go build any more (since 2026-11-18).

use std::os::unix::ffi::OsStringExt;

use sarina::cli::entry;

fn getenv(k: &str) -> Vec<u8> {
    std::env::var_os(k).map(OsStringExt::into_vec).unwrap_or_default()
}

fn main() {
    let mut args: Vec<Vec<u8>> = std::env::args_os().map(OsStringExt::into_vec).collect();
    let argv0 = if args.is_empty() { Vec::new() } else { args.remove(0) };
    if let Some(c) = entry::alias_command(&argv0) {
        args.insert(0, c.as_bytes().to_vec());
    }
    let (on, why) = sarina::trust::contained(&|k: &str| sarina::gotext::lossy(&getenv(k)));
    if on && !entry::allowed_when_contained(&args) {
        eprint!("{}", entry::contained_message(&why));
        std::process::exit(3);
    }
    // Go's cli.Main: no command, help, or one not in the table
    if let Some((code, out, err)) = sarina::cli::not_a_command(&args) {
        write_through(1, &out);
        write_through(2, &err);
        std::process::exit(code);
    }
    // a command in the table (so its name is UTF-8); its arguments must be text, or nothing runs
    let command = String::from_utf8_lossy(&args[0]).into_owned();
    let words = match entry::text_arguments(&command, &args[1..]) {
        Ok(w) => w,
        Err(refusal) => {
            write_through(2, refusal.as_bytes());
            std::process::exit(sarina::cli::EXIT_USAGE);
        }
    };
    let Some(run) = sarina::cli::ported(&command) else { unreachable!("not_a_command answers a command not in the table") };
    // Go's cli.Main: SARINA_CRYPTO=auto and SARINA_MATH=auto decided from the command line's words, for every command, in
    // the process's environment (the commands' children see it)
    let line: Vec<String> = std::iter::once(command).chain(words.iter().cloned()).collect();
    for (k, v) in sarina::trachtenberg::auto_decisions(&|k: &str| sarina::gotext::lossy(&getenv(k)), &line) {
        // at the start of main, before any thread is started
        std::env::set_var(k, v);
    }
    std::process::exit(run_here(run, &words));
}

/// A ported command, its output written as it comes. A write to a closed pipe on standard output or error ends the
/// process by SIGPIPE, as Go's runtime does (`os.epipecheck`: the default disposition put back and the signal raised,
/// even when the caller started it with SIGPIPE ignored; the differential checks both).
fn run_here(run: sarina::cli::Command, args: &[String]) -> i32 {
    let mut env = sarina::cli::Env { sink: Some(Box::new(write_through)), ..Default::default() };
    let code = run(&mut env, args);
    env.flush();
    code
}

/// Writes to standard output (1) or error (2) at once, as Go's unbuffered os.Stdout: a prompt without a newline ("Launch?
/// [y/N] ") must be out before a program started next writes to the same terminal. A closed pipe ends the process by
/// SIGPIPE, as Go's runtime does.
fn write_through(fd: i32, b: &[u8]) {
    use std::io::Write;
    if b.is_empty() {
        return;
    }
    let r = if fd == 1 {
        let mut o = std::io::stdout().lock();
        o.write_all(b).and_then(|()| o.flush())
    } else {
        std::io::stderr().lock().write_all(b)
    };
    if matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe) {
        // SAFETY: the default disposition restored and the signal raised; the process ends here as Go's would.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::raise(libc::SIGPIPE);
        }
    }
}
