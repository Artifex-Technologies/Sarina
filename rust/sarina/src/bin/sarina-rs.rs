//! `sarina-rs`: the Rust build's entry point, to be installed as `/usr/bin/sarina` (and its `sarina-service` and
//! `sarina-gui` names). As Go's `cmd/sarina`: the name it was started as picks the command, then Heimdall's containment
//! lock is checked before anything runs. The commands in `cli::ported` run here; every other command line (and one that
//! is not UTF-8) is handed, unchanged, to the Go build by `execve`: the Go build replaces this process, with the caller's
//! arguments (`argv[0]` too), environment, working folder, standard streams and signal dispositions. With
//! `SARINA_RS_NO_GO` set (the differential runs, rust/parity) a command not ported fails instead of reaching Go.

use std::ffi::{CString, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::atomic::{AtomicBool, Ordering};

use sarina::cli::entry;

/// Whether the caller started this process with SIGPIPE ignored. Rust's runtime sets SIGPIPE to ignored before `main`, and
/// an ignored signal stays ignored across `execve`, so the caller's disposition is read before the runtime starts (an
/// `.init_array` function) and put back before the hand-over.
static SIGPIPE_IGNORED: AtomicBool = AtomicBool::new(false);

extern "C" fn record_sigpipe() {
    // SAFETY: sigaction with a null new action only reads the current one into `old`.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGPIPE, std::ptr::null(), &mut old) == 0 {
            SIGPIPE_IGNORED.store(old.sa_sigaction == libc::SIG_IGN, Ordering::SeqCst);
        }
    }
}

#[used]
#[link_section = ".init_array"]
static RECORD_SIGPIPE: extern "C" fn() = record_sigpipe;

fn getenv(k: &str) -> Vec<u8> {
    std::env::var_os(k).map(OsStringExt::into_vec).unwrap_or_default()
}

fn main() {
    let argv: Vec<Vec<u8>> = std::env::args_os().map(OsStringExt::into_vec).collect();
    let argv0 = argv.first().cloned().unwrap_or_default();
    let mut args: Vec<Vec<u8>> = argv.iter().skip(1).cloned().collect();
    if let Some(c) = entry::alias_command(&argv0) {
        args.insert(0, c.as_bytes().to_vec());
    }
    let (on, why) = sarina::trust::contained(&|k: &str| sarina::gotext::lossy(&getenv(k)));
    if on && !entry::allowed_when_contained(&args) {
        eprint!("{}", entry::contained_message(&why));
        std::process::exit(3);
    }
    let command = args.first().map(Vec::as_slice).unwrap_or_default();
    let name = std::str::from_utf8(command).unwrap_or_default();
    let tried = std::env::var("SARINA_RS_TRY").unwrap_or_default().split(',').any(|c| c == name);
    let proving = sarina::cli::proving(name)
        .filter(|_| tried && sarina::cli::chat::needs_go(&|k: &str| std::env::var(k).unwrap_or_default(), "").is_none());
    if let Some(run) = std::str::from_utf8(command).ok().and_then(sarina::cli::ported).or(proving) {
        let words: Option<Vec<String>> = args[1..].iter().map(|a| String::from_utf8(a.clone()).ok()).collect();
        if let Some(words) = words {
            std::process::exit(run_here(run, &words));
        }
    }
    if std::env::var_os("SARINA_RS_NO_GO").is_some() {
        eprintln!("sarina-rs: {} is not ported (SARINA_RS_NO_GO is set)", sarina::gotext::lossy(command));
        std::process::exit(2);
    }
    hand_over(&argv, command);
}

/// A ported command, its output written as it comes. A write to a closed pipe on standard output or error ends the
/// process by SIGPIPE, as Go's runtime does (`os.epipecheck`: the default disposition put back and the signal raised,
/// even when the caller started it with SIGPIPE ignored; the differential checks both).
fn run_here(run: sarina::cli::Command, args: &[String]) -> i32 {
    use std::io::Write;
    let mut env = sarina::cli::Env {
        sink: Some(Box::new(|fd, b| {
            let r = if fd == 1 { std::io::stdout().lock().write_all(b) } else { std::io::stderr().lock().write_all(b) };
            if matches!(&r, Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe) {
                // SAFETY: the default disposition restored and the signal raised; the process ends here as Go's would.
                unsafe {
                    libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                    libc::raise(libc::SIGPIPE);
                }
            }
        })),
        ..Default::default()
    };
    let code = run(&mut env, args);
    env.flush();
    code
}

/// Replaces this process with the Go build, the command line exactly as it came.
fn hand_over(argv: &[Vec<u8>], command: &[u8]) -> ! {
    let path = entry::go_binary(&getenv);
    let cstr = |b: &[u8]| CString::new(b.to_vec()).unwrap_or_default();
    let c_path = cstr(&path);
    let c_args: Vec<CString> = argv.iter().map(|a| cstr(a)).collect();
    let c_env: Vec<CString> = std::env::vars_os()
        .map(|(k, v)| {
            let mut kv: OsString = k;
            kv.push("=");
            kv.push(v);
            cstr(kv.as_bytes())
        })
        .collect();
    let mut p_args: Vec<*const libc::c_char> = c_args.iter().map(|c| c.as_ptr()).collect();
    p_args.push(std::ptr::null());
    let mut p_env: Vec<*const libc::c_char> = c_env.iter().map(|c| c.as_ptr()).collect();
    p_env.push(std::ptr::null());
    // SAFETY: the pointers stay valid until execve (the CStrings live to the end of this function), both lists end in
    // null, and the signal call only restores the disposition the caller gave this process.
    let err = unsafe {
        if !SIGPIPE_IGNORED.load(Ordering::SeqCst) {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
        libc::execve(c_path.as_ptr(), p_args.as_ptr(), p_env.as_ptr());
        std::io::Error::last_os_error()
    };
    if err.kind() == std::io::ErrorKind::NotFound {
        eprint!("{}", entry::missing_go_message(command, &path));
        std::process::exit(127);
    }
    eprintln!("sarina: cannot start the Go build ({}): {err}", sarina::gotext::lossy(&path));
    std::process::exit(126);
}
