//! Go: `helpers`: the optional ML and voice helper, `sarina-helpers` (packaging/helpers, installed as
//! /usr/lib/sarina/sarina-helpers; `SARINA_HELPERS_BIN` names another), run as a program and talked to over pipes (JSON
//! lines, protocol 1; packaging/helpers/PROTOCOL.md).
//!
//! Every read from the helper is bounded (as in Go since 2026-11-12): an answer line of at most `MAX_ANSWER_BYTES` (a
//! longer one stops the helper), and only the last 64 KiB of its error output is kept. Once the helper is gone or
//! stopped, every later call answers the same error.

use crate::goany::Any;
use crate::gojson::{self, Obj};
use crate::gostruct::{self, Field, Ty, Val};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Another helper program (as given).
pub const BIN_ENV: &str = "SARINA_HELPERS_BIN";
/// Where the package installs the helper.
pub const RUST_HELPER: &str = "/usr/lib/sarina/sarina-helpers";
/// The JSON-lines protocol version this build speaks.
pub const PROTOCOL_VERSION: i64 = 1;
/// How the operator gets the helper.
pub const INSTALL_HINT: &str = "/usr/lib/sarina/sarina-helpers from the sarina package, with onnxruntime-cpu (or set SARINA_HELPERS_BIN)";
/// One answer line from the helper at most (64 MiB; its answers are small: a label, groups of names, a few vectors).
pub const MAX_ANSWER_BYTES: usize = 64 << 20;
const STDERR_KEEP: usize = 64 << 10;

/// Go's `Program`: the helper to run, `SARINA_HELPERS_BIN` when set, else `RUST_HELPER`.
pub fn program(getenv: &dyn Fn(&str) -> String) -> String {
    let bin = getenv(BIN_ENV);
    if bin.is_empty() {
        RUST_HELPER.to_string()
    } else {
        bin
    }
}

/// Go's `Command`: the helper with these arguments.
pub fn command(getenv: &dyn Fn(&str) -> String, args: &[String]) -> Command {
    let mut cmd = Command::new(program(getenv));
    cmd.args(args);
    cmd
}

/// The last `max` bytes written (the helper's error output).
#[derive(Default)]
struct Tail {
    b: Vec<u8>,
    max: usize,
}

impl Tail {
    fn push(&mut self, p: &[u8]) {
        self.b.extend_from_slice(p);
        if self.b.len() > self.max {
            self.b.drain(..self.b.len() - self.max);
        }
    }
}

fn last_line(s: &str) -> String {
    let s = crate::gotext::trim_space(s);
    if s.is_empty() {
        return "no output".into();
    }
    crate::gotext::trim_space(s.rsplit('\n').next().unwrap_or("")).to_string()
}

/// Reads all of `r` into a tail of `max` bytes.
fn read_tail(mut r: impl Read, max: usize) -> Tail {
    let mut t = Tail { b: Vec::new(), max };
    let mut buf = [0u8; 8192];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        t.push(&buf[..n]);
    }
    t
}

/// Go's `Available`: whether the helper runs, with the reason if not (it prints its usage line when run without
/// arguments).
pub fn available(getenv: &dyn Fn(&str) -> String) -> Result<(), String> {
    let mut cmd = command(getenv, &[]);
    // one pipe for both, as Go gives the same writer to both
    let out = match std::io::pipe() {
        Ok((r, w)) => {
            let w2 = w.try_clone().ok();
            cmd.stdin(Stdio::null()).stdout(w);
            if let Some(w2) = w2 {
                cmd.stderr(w2);
            }
            let child = cmd.spawn();
            drop(cmd);
            let t = read_tail(r, STDERR_KEEP);
            if let Ok(mut c) = child {
                let _ = c.wait();
            }
            String::from_utf8_lossy(&t.b).into_owned()
        }
        Err(_) => String::new(),
    };
    if out.contains("usage: sarina-helpers ") {
        return Ok(());
    }
    Err(format!("the helper does not run: {}", last_line(&out)))
}

/// Go's `*exec.ExitError` text for a status.
pub(crate) fn exit_text(st: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(c) = st.code() {
        return if c == 0 { String::new() } else { format!("exit status {c}") };
    }
    let sig = st.signal().unwrap_or(0);
    let name = match sig {
        1 => "hangup".to_string(),
        2 => "interrupt".into(),
        3 => "quit".into(),
        6 => "aborted".into(),
        9 => "killed".into(),
        11 => "segmentation fault".into(),
        13 => "broken pipe".into(),
        15 => "terminated".into(),
        n => format!("signal {n}"),
    };
    format!("signal: {name}")
}

/// Go's `Passthrough`: the helper run with the given streams; its exit code (1 when it cannot run, after saying why).
pub fn passthrough(
    stdin: &mut (dyn Read + Send),
    stdout: &mut (dyn Write + Send),
    stderr: &mut (dyn Write + Send),
    args: &[String],
    getenv: &dyn Fn(&str) -> String,
) -> i32 {
    if let Err(e) = available(getenv) {
        let _ = write!(stderr, "sarina: {e}\n  install: {INSTALL_HINT}\n");
        return 1;
    }
    let mut cmd = command(getenv, args);
    let mut child = match cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(stderr, "sarina: {e}");
            return 1;
        }
    };
    let (mut cin, mut cout, mut cerr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let err_w = &mut *stderr;
    std::thread::scope(|sc| {
        sc.spawn(move || {
            if let Some(w) = cin.as_mut() {
                let _ = std::io::copy(stdin, w);
            }
            drop(cin);
        });
        sc.spawn(move || {
            if let Some(r) = cout.as_mut() {
                let _ = std::io::copy(r, stdout);
            }
        });
        sc.spawn(move || {
            if let Some(r) = cerr.as_mut() {
                let _ = std::io::copy(r, err_w);
            }
        });
    });
    match child.wait() {
        Ok(st) => st.code().unwrap_or(-1),
        Err(e) => {
            let _ = writeln!(stderr, "sarina: {e}");
            1
        }
    }
}

/// Go's `Passthrough` as the commands run it: Go's `env.Stdin`, `Stdout` and `Stderr` are the process's own files there, so
/// the helper gets them as they are (a terminal stays a terminal for voice-chat's push-to-talk), not pipes.
pub fn passthrough_inherit(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    env.flush();
    let getenv = |k: &str| env.getenv(k);
    if let Err(e) = available(&getenv) {
        env.stderr.extend_from_slice(format!("sarina: {e}\n  install: {INSTALL_HINT}\n").as_bytes());
        env.flush();
        return 1;
    }
    let mut cmd = command(&getenv, args);
    let program = cmd.get_program().to_owned();
    let st = cmd.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn().and_then(|mut c| c.wait());
    match st {
        // Go's `ExitError.ExitCode()`: -1 for a helper ended by a signal
        Ok(st) => st.code().unwrap_or(-1),
        Err(e) => {
            env.eprintln(&format!("sarina: {}", crate::goerr::path("fork/exec", program.as_encoded_bytes(), &e)));
            1
        }
    }
}

/// Go's `memory-search` command: the helper's semantic search over saved sessions.
pub fn memory_search_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let mut a = vec!["memory-search".to_string()];
    a.extend_from_slice(args);
    passthrough_inherit(env, &a)
}

/// Go's `voiceChat`: the helper's audio loop, told which binary to run per turn (this one, unless `--sarina-bin` is
/// given). This one is sarina-rs where Go named its own build, so each turn's chat runs in Rust.
pub fn voice_chat_command(env: &mut crate::cli::Env, args: &[String]) -> i32 {
    let have = args.iter().any(|a| a == "--sarina-bin" || a.starts_with("--sarina-bin="));
    let mut a = vec!["voice-chat".to_string()];
    if !have {
        // Go's os.Executable: /proc/self/exe, without the " (deleted)" of a replaced file
        if let Ok(p) = std::fs::read_link("/proc/self/exe") {
            let p = p.into_os_string().into_string().unwrap_or_else(|p| crate::gotext::lossy(p.as_encoded_bytes()));
            a.push("--sarina-bin".into());
            a.push(p.strip_suffix(" (deleted)").unwrap_or(&p).to_string());
        }
    }
    a.extend_from_slice(args);
    passthrough_inherit(env, &a)
}

static RESPONSE: Ty = Ty::Struct(
    "helpers.response",
    "response",
    &[
        Field("ok", &Ty::Bool),
        Field("error", &Ty::Str),
        Field("protocol", &Ty::Int),
        Field("prototypes", &Ty::Str),
        Field("label", &Ty::Ptr(&Ty::Str)),
        Field("score", &Ty::Float),
        Field("groups", &Ty::Slice(&Ty::Slice(&Ty::Str))),
        Field("vectors", &Ty::Slice(&Ty::Slice(&Ty::Float))),
    ],
);

/// A running `sarina-helpers serve --model-dir D` (Go's `Server`).
pub struct Server {
    child: Mutex<Child>,
    inner: Mutex<Inner>,
    stderr: Arc<Mutex<Tail>>,
    /// a message (or a hang-up) when the helper's error output has all been read (it has exited); true once seen
    done: Mutex<(mpsc::Receiver<()>, bool)>,
    /// the helper's category-prototype fingerprint (from ping)
    pub prototypes: String,
}

struct Inner {
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    max_line: usize,
    broken: Option<String>,
}

enum Line {
    Text(Vec<u8>, Option<String>),
    TooLong,
}

/// Go's `readLine`: up to and including the next newline, at most `max` bytes.
fn read_line(r: &mut impl BufRead, max: usize) -> Line {
    let mut line = Vec::new();
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) => return Line::Text(line, Some(crate::goerr::text(&e))),
        };
        if buf.is_empty() {
            return Line::Text(line, Some("EOF".into()));
        }
        let (take, end) = match buf.iter().position(|&c| c == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        if line.len() + take > max {
            return Line::TooLong;
        }
        line.extend_from_slice(&buf[..take]);
        r.consume(take);
        if end {
            return Line::Text(line, None);
        }
    }
}

impl Server {
    /// Go's `StartServer`: the embedding server launched and pinged; the error says why the helper is unavailable.
    pub fn start(model_dir: &str, getenv: &dyn Fn(&str) -> String) -> Result<Server, String> {
        Server::start_with(model_dir, 0, getenv)
    }

    /// `start` with an answer line limit (0: MAX_ANSWER_BYTES).
    pub fn start_with(model_dir: &str, max_line: usize, getenv: &dyn Fn(&str) -> String) -> Result<Server, String> {
        let mut cmd = command(getenv, &["serve".into(), "--model-dir".into(), model_dir.into()]);
        let program = cmd.get_program().to_owned();
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| crate::goerr::path("fork/exec", program.as_encoded_bytes(), &e))?;
        let stdin = child.stdin.take();
        let stdout = BufReader::with_capacity(1 << 20, child.stdout.take().ok_or("no stdout")?);
        let err_pipe = child.stderr.take();
        let stderr = Arc::new(Mutex::new(Tail { b: Vec::new(), max: STDERR_KEEP }));
        let (tx, rx) = mpsc::channel();
        let tail = stderr.clone();
        std::thread::spawn(move || {
            if let Some(mut r) = err_pipe {
                let mut buf = [0u8; 8192];
                while let Ok(n) = r.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    tail.lock().unwrap_or_else(|e| e.into_inner()).push(&buf[..n]);
                }
            }
            let _ = tx.send(());
        });
        let mut s = Server {
            child: Mutex::new(child),
            inner: Mutex::new(Inner { stdin, stdout, max_line, broken: None }),
            stderr,
            done: Mutex::new((rx, false)),
            prototypes: String::new(),
        };
        let r = match s.call(Obj::new(true).str("op", "ping").end().unwrap_or_default()) {
            Ok(r) => r,
            Err(e) => {
                let _ = s.close();
                return Err(e);
            }
        };
        let protocol = match gostruct::field(&RESPONSE, &r, "protocol") {
            Some(Val::Int(n)) => *n,
            _ => 0,
        };
        if protocol != PROTOCOL_VERSION {
            let _ = s.close();
            return Err(format!("helper speaks protocol {protocol}, this build expects {PROTOCOL_VERSION}"));
        }
        s.prototypes = gostruct::str_field(&RESPONSE, &r, "prototypes").to_string();
        Ok(s)
    }

    /// Waits at most 2 s for the helper's error output to be read to its end.
    fn wait_done(&self) {
        let mut d = self.done.lock().unwrap_or_else(|e| e.into_inner());
        if !d.1 && !matches!(d.0.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Timeout)) {
            d.1 = true;
        }
    }

    fn dead(&self, err: &str) -> String {
        // the reason is the helper's last line of error output: wait (briefly) until it has all been read
        self.wait_done();
        let text = String::from_utf8_lossy(&self.stderr.lock().unwrap_or_else(|e| e.into_inner()).b).into_owned();
        let msg = last_line(&text);
        if msg != "no output" {
            return format!("helper exited: {msg}");
        }
        format!("helper exited: {err}")
    }

    fn call(&self, req: String) -> Result<Val, String> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = &inner.broken {
            return Err(b.clone());
        }
        let mut line = req.into_bytes();
        line.push(b'\n');
        let wrote = match inner.stdin.as_mut() {
            Some(w) => w.write_all(&line).and_then(|_| w.flush()).map_err(|e| format!("write |1: {}", crate::goerr::text(&e))),
            None => Err("write |1: file already closed".into()),
        };
        if let Err(e) = wrote {
            let b = self.dead(&e);
            inner.broken = Some(b.clone());
            return Err(b);
        }
        let max = if inner.max_line == 0 { MAX_ANSWER_BYTES } else { inner.max_line };
        let (text, err) = match read_line(&mut inner.stdout, max) {
            Line::TooLong => {
                // the stream is mid-line now: the helper cannot be talked to any more
                let _ = self.child.lock().unwrap_or_else(|e| e.into_inner()).kill();
                let b = format!("helper answer is longer than {max} bytes; the helper was stopped");
                inner.broken = Some(b.clone());
                return Err(b);
            }
            Line::Text(t, e) => (t, e),
        };
        if let Some(e) = err {
            if crate::gotext::trim_space_bytes(&text).is_empty() {
                let b = self.dead(&e);
                inner.broken = Some(b.clone());
                return Err(b);
            }
        }
        let (r, e) = gostruct::unmarshal(&RESPONSE, &text);
        if let Some(e) = e {
            return Err(format!("bad helper response: {}", e.0));
        }
        if !matches!(gostruct::field(&RESPONSE, &r, "ok"), Some(Val::Bool(true))) {
            return Err(gostruct::str_field(&RESPONSE, &r, "error").to_string());
        }
        Ok(r)
    }

    /// Go's `Classify`: the nearest prototype label (";"-joined categories, "" if none) and its score.
    pub fn classify(&self, text: &str) -> Result<(String, f64), String> {
        let r = self.call(Obj::new(true).str("op", "classify").str("text", text).end().unwrap_or_default())?;
        let score = match gostruct::field(&RESPONSE, &r, "score") {
            Some(Val::Float(f)) => *f,
            _ => 0.0,
        };
        let label = match gostruct::field(&RESPONSE, &r, "label") {
            Some(Val::Ptr(Some(b))) => match &**b {
                Val::Str(s) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        Ok((label, score))
    }

    /// Go's `Group`: semantic groups of captures (records as capture.list returns them); None is Go's nil.
    pub fn group(&self, captures: &[BTreeMap<String, Any>], threshold: f64) -> Result<Option<Vec<Vec<String>>>, String> {
        let req = Any::Object(BTreeMap::from([
            ("captures".to_string(), Any::Array(captures.iter().map(|c| Any::Object(c.clone())).collect())),
            ("op".to_string(), Any::String("group".into())),
            ("threshold".to_string(), Any::Number(threshold)),
        ]));
        let r = self.call(gojson::marshal_any(&req).map_err(|e| e.0)?)?;
        Ok(match gostruct::field(&RESPONSE, &r, "groups") {
            Some(Val::Slice(Some(gs))) => Some(
                gs.iter()
                    .map(|g| match g {
                        Val::Slice(Some(xs)) => xs.iter().map(|x| if let Val::Str(s) = x { s.clone() } else { String::new() }).collect(),
                        _ => Vec::new(),
                    })
                    .collect(),
            ),
            _ => None,
        })
    }

    /// Go's `Close`: stdin closed (the helper then exits) and its exit waited for; Go's text for a failed exit.
    pub fn close(&self) -> Result<(), String> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).stdin = None;
        let st = self.child.lock().unwrap_or_else(|e| e.into_inner()).wait().map_err(|e| e.to_string())?;
        self.wait_done();
        match exit_text(st) {
            t if t.is_empty() => Ok(()),
            t => Err(t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> String {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| m.get(k).cloned().unwrap_or_default()
    }

    #[test]
    fn which_helper_runs() {
        let dir = std::env::temp_dir().join(format!("sarina-helpers-choice-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("sarina-helpers");
        std::fs::write(&bin, "#!/bin/sh\necho 'usage: sarina-helpers {serve|memory-search|voice-chat} ...' >&2\nexit 2\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let b = bin.to_str().unwrap();
        // named: the Rust helper, and it is accepted by its usage line
        let e = env(&[(BIN_ENV, b), ("PATH", "/nonexistent")]);
        assert_eq!(program(&e), b);
        assert_eq!(command(&e, &["serve".into()]).get_program(), b);
        assert_eq!(available(&e), Ok(()));
        // not named: the installed one
        assert_eq!(program(&env(&[])), RUST_HELPER);
        // a named helper that does not run says so
        let e = env(&[(BIN_ENV, "/nonexistent/sarina-helpers")]);
        assert_eq!(available(&e), Err("the helper does not run: no output".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
