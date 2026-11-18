//! sarina-tone: reads how a spoken question sounded and prints a short, private style hint for Sarina (or nothing). The
//! Rust build of `go/cmd/sarina-tone` (docs/RUST_PORT.md "sarina-tone"), the same on every command line, file and error.
//!
//!     sarina-tone [--baseline FILE] [--json] RECORDING.wav   measure, compare with the usual, update the usual, note the mood, print the hint
//!     sarina-tone --pattern                                  how the owner has sounded lately (one or two sentences, or nothing)
//!     sarina-tone --forget                                   delete what has been learned about the voice, and the mood history
//!
//! The command line is read as Go's `flag` package reads it for a program (`flag.Parse` on `os.Args`): on bytes, so a
//! path that is not UTF-8 works and is named byte for byte, as in Go.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

use sarina::gopath;
use sarina::tone;

/// Writes to standard output (1) or error (2) at once, as Go's unbuffered os.Stdout and os.Stderr. A closed pipe ends the
/// process by SIGPIPE, as Go's runtime does (even when SIGPIPE was ignored by the caller).
fn write_through(fd: i32, b: &[u8]) {
    use std::io::Write;
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

fn line(fd: i32, parts: &[&[u8]]) {
    let mut b = parts.concat();
    b.push(b'\n');
    write_through(fd, &b);
}

/// The program's flags as Go's `flag` package has them (sorted by name for the usage).
struct Flags {
    baseline: Vec<u8>,
    history: Vec<u8>,
    forget: bool,
    json: bool,
    no_update: bool,
    pattern: bool,
}

const USAGE: [(&str, bool, &str); 6] = [
    ("baseline", false, "where the running averages are kept"),
    ("forget", true, "delete the baseline and the mood history"),
    ("history", false, "when each question was asked and the mood it was heard as"),
    ("json", true, "print the measurements and the verdict as JSON"),
    ("no-update", true, "measure and compare without learning from this recording"),
    ("pattern", true, "print how the owner has sounded lately, or nothing"),
];

/// Go's default `flag.Usage`: "Usage of ARGV0:" and `PrintDefaults` (a string flag's default quoted with %q).
fn usage(argv0: &[u8], defaults: (&[u8], &[u8])) -> Vec<u8> {
    let mut b = b"Usage of ".to_vec();
    b.extend_from_slice(argv0);
    b.extend_from_slice(b":\n");
    for (name, is_bool, text) in USAGE {
        b.extend_from_slice(format!("  -{name}").as_bytes());
        if !is_bool {
            b.extend_from_slice(b" string");
        }
        b.extend_from_slice(b"\n    \t");
        b.extend_from_slice(text.as_bytes());
        let def = match name {
            "baseline" => defaults.0,
            "history" => defaults.1,
            _ => b"",
        };
        if !def.is_empty() {
            b.extend_from_slice(format!(" (default {})", sarina::gotext::quote_bytes(def)).as_bytes());
        }
        b.push(b'\n');
    }
    b
}

/// Go's `strconv.ParseBool`.
fn parse_bool(s: &[u8]) -> Option<bool> {
    match s {
        b"1" | b"t" | b"T" | b"TRUE" | b"true" | b"True" => Some(true),
        b"0" | b"f" | b"F" | b"FALSE" | b"false" | b"False" => Some(false),
        _ => None,
    }
}

/// Go's `flag.Parse()` (ExitOnError): the arguments left, or the exit status after the message and the usage went to
/// standard error (0 for -h and -help, 2 otherwise).
fn parse(args: &[Vec<u8>], f: &mut Flags, argv0: &[u8], defaults: (&[u8], &[u8])) -> Result<Vec<Vec<u8>>, i32> {
    let fail = |msg: Vec<u8>| {
        let mut b = msg;
        b.push(b'\n');
        b.extend_from_slice(&usage(argv0, defaults));
        write_through(2, &b);
        2
    };
    let mut args = args;
    loop {
        let Some(s) = args.first() else { return Ok(Vec::new()) };
        if s.len() < 2 || s[0] != b'-' {
            return Ok(args.to_vec());
        }
        let mut minuses = 1;
        if s[1] == b'-' {
            minuses = 2;
            if s.len() == 2 {
                return Ok(args[1..].to_vec());
            }
        }
        let name = &s[minuses..];
        if name.is_empty() || name[0] == b'-' || name[0] == b'=' {
            return Err(fail([b"bad flag syntax: ".as_slice(), s].concat()));
        }
        args = &args[1..];
        let (name, value) = match name[1..].iter().position(|&c| c == b'=') {
            Some(i) => (&name[..i + 1], Some(&name[i + 2..])),
            None => (name, None),
        };
        let quoted = || sarina::gotext::quote_bytes(value.unwrap_or_default());
        match name {
            b"baseline" | b"history" => {
                let v = match value {
                    Some(v) => v.to_vec(),
                    None => match args.first() {
                        Some(v) => {
                            args = &args[1..];
                            v.clone()
                        }
                        None => return Err(fail([b"flag needs an argument: -".as_slice(), name].concat())),
                    },
                };
                if name == b"baseline" {
                    f.baseline = v;
                } else {
                    f.history = v;
                }
            }
            b"forget" | b"json" | b"no-update" | b"pattern" => {
                let Some(b) = parse_bool(value.unwrap_or(b"true")) else {
                    let mut m = format!("invalid boolean value {} for -", quoted()).into_bytes();
                    m.extend_from_slice(name);
                    m.extend_from_slice(b": parse error");
                    return Err(fail(m));
                };
                match name {
                    b"forget" => f.forget = b,
                    b"json" => f.json = b,
                    b"no-update" => f.no_update = b,
                    _ => f.pattern = b,
                }
            }
            b"help" | b"h" => {
                write_through(2, &usage(argv0, defaults));
                return Err(0);
            }
            _ => return Err(fail([b"flag provided but not defined: -".as_slice(), name].concat())),
        }
    }
}

fn main() {
    let mut args: Vec<Vec<u8>> = std::env::args_os().map(OsString::into_vec).collect();
    let argv0 = if args.is_empty() { Vec::new() } else { args.remove(0) };
    // Go's os.UserHomeDir: $HOME, its error (when it is empty) ignored, so the defaults are relative then
    let home = std::env::var_os("HOME").map(OsString::into_vec).unwrap_or_default();
    let state = |file: &[u8]| gopath::join(&[&home, b".local", b"state", b"labyrinth", file]);
    let (def_base, def_hist) = (state(b"tone-baseline.json"), state(b"mood-history.jsonl"));
    let mut f =
        Flags { baseline: def_base.clone(), history: def_hist.clone(), forget: false, json: false, no_update: false, pattern: false };
    let rest = match parse(&args, &mut f, &argv0, (&def_base, &def_hist)) {
        Ok(r) => r,
        Err(code) => std::process::exit(code),
    };
    std::process::exit(run(&f, &rest));
}

fn run(f: &Flags, rest: &[Vec<u8>]) -> i32 {
    if f.forget {
        for p in [&f.baseline, &f.history] {
            if let Err((e, not_exist)) = tone::remove(p) {
                if !not_exist {
                    line(2, &[b"sarina-tone: ", &e]);
                    return 1;
                }
            }
        }
        line(1, &[b"forgotten"]);
        return 0;
    }
    if f.pattern {
        let now = sarina::gotime::Time::now();
        let p = tone::pattern(&tone::read_history(&f.history), now.unix(), now.nanos(), sarina::tz::local());
        if !p.is_empty() {
            line(1, &[p.as_bytes()]);
        }
        return 0;
    }
    if rest.len() != 1 {
        line(2, &[b"usage: sarina-tone [--baseline FILE] [--json] RECORDING.wav"]);
        return 2;
    }
    let (x, rate) = match tone::read_wav(&rest[0]) {
        Ok(r) => r,
        Err(e) => {
            line(2, &[b"sarina-tone: ", &e]);
            return 1;
        }
    };
    let Some(feat) = tone::analyze(&x, rate) else {
        if f.json {
            line(1, &[b"{\"ok\":false}"]);
        }
        return 0; // too little speech: nothing to say, nothing learned
    };
    let mut b = tone::load(&f.baseline);
    let mood = b.mood(&feat);
    let hint = tone::hint_for(mood);
    if !f.no_update {
        b.update(&feat);
        if let Err(e) = b.save(&f.baseline) {
            line(2, &[b"sarina-tone: ", &e]);
        }
        if !mood.is_empty() {
            let t = sarina::gotime::Time::now().unix();
            if let Err(e) = tone::append_history(&f.history, tone::Entry { t, mood: mood.to_string() }) {
                line(2, &[b"sarina-tone: ", &e]);
            }
        }
    }
    if f.json {
        // Go's json.Marshal of a map: its keys in order; a value JSON cannot hold leaves nothing but the newline
        let out = feat.json().map(|fj| {
            let mut s = format!("{{\"features\":{fj},\"hint\":");
            sarina::gojson::write_str(&mut s, hint, true);
            s.push_str(",\"mood\":");
            sarina::gojson::write_str(&mut s, mood, true);
            s.push_str(&format!(",\"ok\":true,\"samples\":{}}}", b.n));
            s
        });
        line(1, &[out.unwrap_or_default().as_bytes()]);
        return 0;
    }
    if !hint.is_empty() {
        line(1, &[hint.as_bytes()]);
    }
    0
}
