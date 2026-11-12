//! `sarina-rs`: the Rust build's entry point, to be installed as `/usr/bin/sarina` (and its `sarina-service` and
//! `sarina-gui` names). As Go's `cmd/sarina`: the name it was started as picks the command, then Heimdall's containment
//! lock is checked before anything runs. No command is ported to this binary yet, so every command line is handed,
//! unchanged, to the Go build by `execve`: the Go build replaces this process, with the caller's arguments (`argv[0]`
//! too), environment, working folder, standard streams and signal dispositions.

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
    hand_over(&argv, args.first().map(Vec::as_slice).unwrap_or_default());
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
