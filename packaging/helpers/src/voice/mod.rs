//! `sarina voice-chat` (the Python's `voice_bridge.py`): push-to-talk voice chat that drives the agent one turn at a time.
//! The bridge owns only audio (record, transcribe, speak); each turn it runs Sarina as a subprocess and speaks its stdout:
//!
//! - first turn: `sarina chat --cwd C --session-dir S --no-auto-memory-review [--agent-arg ...] -- <text>`
//! - later turns: `sarina resume --cwd C --session-dir S --no-auto-memory-review [...] <session_id> <text>`
//!
//! `chat` does not print its session id, so the bridge takes it from the session file in S that appeared or changed during
//! the first call. Unless `--no-auto-memory-review` was given, one `sarina memory-review` runs when the chat ends.
//! The prompts, the lines printed, the subprocesses' command lines and the exit statuses are the Python's.

pub mod audio;
pub mod espeak;
pub mod piper;
pub mod stt;

use crate::argparse::{Opt, Outcome, Parser, Ty};
use crate::embed::pathlib_str;
use sarina_voice::py;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

pub const QUIT_WORDS: [&str; 4] = ["q", "quit", "exit", "stop"];

pub const PARSER: Parser = Parser {
    prog: "sarina voice-chat",
    opts: &[
        Opt::store("--stt-model-dir").required(),
        Opt::store("--tts-model").required(),
        Opt::store("--tts-config"),
        Opt::store("--stt-device").choices(&["cpu", "cuda"]),
        Opt::store("--stt-compute-type"),
        Opt::store("--input-device"),
        Opt::store("--output-device"),
        Opt::store("--max-record-seconds").ty(Ty::Float),
        Opt::store("--resume-session-id"),
        Opt::flag("--no-auto-memory-review"),
        Opt::store("--cwd"),
        Opt::store("--session-dir"),
        Opt::store("--sarina-bin").help("the Go binary to run per turn"),
        Opt::append("--agent-arg").help("extra flag passed to every chat/resume call (repeatable), e.g. --agent-arg=--allow-write"),
    ],
    positional: None,
};

/// Python's `str(OSError)`: "[Errno 2] No such file or directory: 'name'"
pub fn os_error(e: &std::io::Error, name: &str) -> String {
    match e.raw_os_error() {
        Some(n) => format!("[Errno {n}] {}: {}", os_error_text(e), py::str_repr(name)),
        None => e.to_string(),
    }
}

/// strerror(errno)
pub fn os_error_text(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => unsafe { std::ffi::CStr::from_ptr(libc::strerror(n)) }.to_string_lossy().into_owned(),
        None => e.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Ctrl-C and the terminal: Python raised KeyboardInterrupt in its read of the line; here the handler sets a flag the read
// looks at while it waits.

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

fn catch_sigint() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_sigint as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0; // no SA_RESTART: a blocked read returns EINTR
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

/// An uncaught KeyboardInterrupt: Python ended by SIGINT itself (the shell sees 130).
fn die_by_sigint() -> ! {
    let _ = std::io::stdout().flush();
    eprintln!("KeyboardInterrupt");
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::raise(libc::SIGINT);
    }
    std::process::exit(130)
}

pub enum Read {
    Line(String),
    Eof,
    Interrupt,
}

/// `input('')`: one line from stdin without its newline (a last line without one too); EOF with nothing read is EOFError.
pub struct Stdin {
    buf: Vec<u8>,
    eof: bool,
}

impl Stdin {
    fn new() -> Stdin {
        Stdin { buf: Vec::new(), eof: false }
    }

    pub fn read_line(&mut self) -> Read {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return Read::Line(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
            }
            if self.eof {
                if self.buf.is_empty() {
                    return Read::Eof;
                }
                let line = std::mem::take(&mut self.buf);
                return Read::Line(String::from_utf8_lossy(&line).into_owned());
            }
            if INTERRUPTED.swap(false, Ordering::SeqCst) {
                return Read::Interrupt;
            }
            // wait for input a tenth of a second at a time: the signal may land on another thread of this process, so the
            // read is not counted on to be interrupted
            let mut pfd = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut pfd, 1, 100) } == 0 {
                continue;
            }
            let mut chunk = [0u8; 8192];
            let n = unsafe { libc::read(0, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
            if n < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                self.eof = true;
            } else if n == 0 {
                self.eof = true;
            } else {
                self.buf.extend_from_slice(&chunk[..n as usize]);
            }
        }
    }
}

fn echo(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes());
    let _ = o.write_all(b"\n");
    let _ = o.flush();
}

// ---------------------------------------------------------------------------------------------------------------------
// Agent turns

/// What `subprocess.run(..., capture_output=True, text=True, errors='replace')` gave.
pub struct Ran {
    pub stdout: String,
    pub stderr: String,
    /// the return code; negative for a signal, as Python's
    pub code: i32,
}

/// text=True: decoded with replacement characters, "\r\n" and "\r" read as "\n".
fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).replace("\r\n", "\n").replace('\r', "\n")
}

/// A subprocess run with its output captured and stdin inherited (as Python's).
pub fn run(argv: &[String]) -> Result<Ran, String> {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| os_error(&e, &argv[0]))?;
    use std::os::unix::process::ExitStatusExt;
    let code = out.status.code().unwrap_or_else(|| -out.status.signal().unwrap_or(0));
    Ok(Ran { stdout: text(&out.stdout), stderr: text(&out.stderr), code })
}

pub struct AgentTurns {
    pub sarina_bin: String,
    pub cwd: String,
    pub session_dir: String,
    pub agent_args: Vec<String>,
    pub session_id: Option<String>,
}

impl AgentTurns {
    fn flags(&self) -> Vec<String> {
        let mut f =
            vec!["--cwd".into(), self.cwd.clone(), "--session-dir".into(), self.session_dir.clone(), "--no-auto-memory-review".into()];
        f.extend(self.agent_args.iter().cloned());
        f
    }

    /// {stem: st_mtime_ns} of the session files, in the folder's order
    fn snapshot(&self) -> Vec<(String, i128)> {
        let Ok(rd) = std::fs::read_dir(&self.session_dir) else { return Vec::new() };
        let mut v = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") {
                continue;
            }
            if let Ok(md) = std::fs::metadata(e.path()) {
                use std::os::unix::fs::MetadataExt;
                let lead = name.len() - name.trim_start_matches('.').len();
                let stem = match name[lead..].rfind('.') {
                    Some(j) => name[..lead + j].to_string(),
                    None => name.clone(),
                };
                v.push((stem, md.mtime() as i128 * 1_000_000_000 + md.mtime_nsec() as i128));
            }
        }
        v
    }

    /// One turn: (reply, exit code).
    pub fn send(&mut self, text: &str) -> Result<(String, i32), String> {
        let before = self.snapshot();
        let mut argv = vec![self.sarina_bin.clone()];
        match &self.session_id {
            Some(sid) => {
                argv.push("resume".into());
                argv.extend(self.flags());
                argv.push(sid.clone());
                argv.push(text.into());
            }
            None => {
                argv.push("chat".into());
                argv.extend(self.flags());
                argv.push("--".into());
                argv.push(text.into());
            }
        }
        let proc = run(&argv)?;
        if self.session_id.is_none() {
            let after = self.snapshot();
            let mut best: Option<(String, i128)> = None;
            for (sid, mtime) in &after {
                let was = before.iter().rev().find(|(s, _)| s == sid).map(|(_, m)| *m);
                if was != Some(*mtime) && best.as_ref().is_none_or(|(_, m)| mtime > m) {
                    best = Some((sid.clone(), *mtime));
                }
            }
            if let Some((sid, _)) = best {
                self.session_id = Some(sid);
            }
        }
        let mut reply = py::strip(&proc.stdout).to_string();
        if proc.code != 0 && reply.is_empty() {
            let err = py::strip(&proc.stderr).to_string();
            let lines = py::splitlines(&err);
            let tail = lines.last().map(|s| s.to_string()).unwrap_or_else(|| "no output".into());
            reply = format!("(agent turn failed, exit {}: {})", proc.code, tail);
        }
        Ok((reply, proc.code))
    }

    pub fn review_memory(&self) -> Result<Option<Ran>, String> {
        let Some(sid) = &self.session_id else { return Ok(None) };
        let argv = vec![
            self.sarina_bin.clone(),
            "memory-review".into(),
            "--cwd".into(),
            self.cwd.clone(),
            "--session-dir".into(),
            self.session_dir.clone(),
            sid.clone(),
        ];
        run(&argv).map(Some)
    }
}

// ---------------------------------------------------------------------------------------------------------------------

pub struct Voice {
    pub alsa: audio::Alsa,
    pub stt: stt::SpeechToText,
    pub tts: piper::Voice,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub max_record_seconds: f64,
}

enum Input {
    Text(String),
    Eof,
    Interrupt,
}

impl Voice {
    /// `make_input_func`'s function: a quit word instead of recording is "/exit"; else record, transcribe, return the text.
    fn input(&mut self, stdin: &mut Stdin, prompt: &str) -> Result<Input, String> {
        echo(prompt);
        echo("Press Enter to record, or type 'quit' then Enter to end the session.");
        match stdin.read_line() {
            Read::Eof => return Ok(Input::Eof),
            Read::Interrupt => return Ok(Input::Interrupt),
            Read::Line(l) => {
                if QUIT_WORDS.contains(&py::lower(py::strip(&l)).as_str()) {
                    return Ok(Input::Text("/exit".into()));
                }
            }
        }
        let dir = TempDir::new()?;
        let wav = dir.0.join("utterance.wav");
        let how =
            audio::record(&self.alsa, &wav, self.input_device.as_deref(), self.max_record_seconds, &mut |s| echo(s), &mut || match stdin
                .read_line()
            {
                Read::Line(_) => audio::Stop::Line,
                Read::Eof => audio::Stop::Eof,
                Read::Interrupt => audio::Stop::Interrupt,
            })?;
        match how {
            audio::Stop::Eof => return Ok(Input::Eof),
            audio::Stop::Interrupt => return Ok(Input::Interrupt),
            audio::Stop::Line => {}
        }
        if let Err(e) = std::fs::metadata(&wav) {
            return Err(os_error(&e, &wav.to_string_lossy()));
        }
        let text = self.stt.transcribe(&wav);
        // Ctrl-C while whisper ran: the Python's KeyboardInterrupt came out of this function too
        if INTERRUPTED.swap(false, Ordering::SeqCst) {
            return Ok(Input::Interrupt);
        }
        let text = text?;
        echo(&if text.is_empty() { "(heard nothing)".to_string() } else { format!("(heard: {text})") });
        Ok(Input::Text(text))
    }

    fn speak(&mut self, reply: &str) -> Result<(), String> {
        let wav = piper::synthesize(&mut self.tts, reply, &mut |w| eprintln!("{w}"))?;
        audio::play(&self.alsa, &wav, self.output_device.as_deref())
    }
}

/// A private temporary folder (`tempfile.TemporaryDirectory(prefix='sarina-voice-')`), removed when dropped.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Result<TempDir, String> {
        let base = std::env::var("TMPDIR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/tmp".into());
        let mut tmpl = format!("{base}/sarina-voice-XXXXXX\0").into_bytes();
        let p = unsafe { libc::mkdtemp(tmpl.as_mut_ptr() as *mut libc::c_char) };
        if p.is_null() {
            return Err(os_error(&std::io::Error::last_os_error(), &base));
        }
        tmpl.pop();
        Ok(TempDir(std::path::PathBuf::from(String::from_utf8_lossy(&tmpl).into_owned())))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `run_loop`: the conversation; the exit status (130 on Ctrl-C at the prompt).
pub fn run_loop(turns: &mut AgentTurns, voice: &mut Voice, stdin: &mut Stdin, auto_memory_review: bool) -> i32 {
    echo("# Voice Chat");
    echo("Say your request, or type /exit to stop.");
    if let Some(sid) = &turns.session_id {
        echo(&format!("resuming_session_id={sid}"));
    }
    let mut code = 0;
    loop {
        let text = match voice.input(stdin, "user> ") {
            Ok(Input::Text(t)) => py::strip(&t).to_string(),
            Ok(Input::Eof) => {
                echo("chat_ended=eof");
                break;
            }
            Ok(Input::Interrupt) => {
                echo("");
                echo("chat_ended=interrupt");
                code = 130;
                break;
            }
            // the Python's uncaught exception: a traceback and status 1
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        if text.is_empty() {
            continue;
        }
        if text == "/exit" || text == "/quit" {
            echo("chat_ended=user_exit");
            break;
        }
        let (reply, rc) = match turns.send(&text) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        };
        if INTERRUPTED.load(Ordering::SeqCst) {
            die_by_sigint();
        }
        echo(&reply);
        if rc != 0 || reply.is_empty() {
            continue;
        }
        let spoken = voice.speak(&reply);
        if INTERRUPTED.load(Ordering::SeqCst) {
            die_by_sigint();
        }
        if let Err(e) = spoken {
            echo(&format!("(voice playback failed: {e})"));
        }
    }
    if auto_memory_review && turns.session_id.is_some() {
        match turns.review_memory() {
            Ok(Some(proc)) => {
                let out = py::strip(&proc.stdout);
                if !out.is_empty() {
                    echo(out);
                }
            }
            Ok(None) => {}
            Err(e) => echo(&format!("memory_review_skipped=error ({e})")),
        }
        if INTERRUPTED.load(Ordering::SeqCst) {
            die_by_sigint();
        }
    }
    code
}

/// The command: its exit status.
pub fn main(args: &[String]) -> i32 {
    let p = match PARSER.parse(args) {
        Outcome::Ok(p) => p,
        Outcome::Help(h) => {
            print!("{h}");
            let _ = std::io::stdout().flush();
            return 0;
        }
        Outcome::Error(e) => {
            eprint!("{e}");
            return 2;
        }
    };
    let alsa = audio::Alsa::find();
    let whisper = audio::which(&stt::program());
    let mut missing = Vec::new();
    if alsa.is_none() {
        missing.push("alsa-utils (arecord/aplay)".to_string());
    }
    if whisper.is_none() {
        missing.push("whisper-cpp (whisper-cli)".to_string());
    }
    if let Err(e) = espeak::available() {
        missing.push(format!("espeak-ng ({e})"));
    }
    if let Err(e) = crate::ortrt::environment() {
        missing.push(format!("onnxruntime ({e})"));
    }
    if !missing.is_empty() {
        echo(&format!(
            "missing {} - install them with your package manager (Arch: alsa-utils, whisper-cpp, espeak-ng, onnxruntime-cpu)",
            missing.join(", ")
        ));
        return 1;
    }
    let model = match stt::resolve_model(p.get("--stt-model-dir").unwrap_or_default()) {
        Ok(m) => m,
        Err(e) => {
            echo(&e);
            return 1;
        }
    };
    let stt = stt::SpeechToText { whisper: whisper.unwrap_or_default(), model, gpu: p.get("--stt-device") == Some("cuda") };
    let tts_model = p.get("--tts-model").unwrap_or_default();
    if !std::path::Path::new(tts_model).is_file() {
        echo(&format!("no such TTS voice model: {}", pathlib_str(tts_model)));
        return 1;
    }
    let tts_config = p.get("--tts-config").map(|c| c.to_string());
    if let Some(c) = &tts_config {
        if !std::path::Path::new(c).is_file() {
            echo(&format!("no such TTS voice config: {}", pathlib_str(c)));
            return 1;
        }
    }
    let tts = match piper::Voice::load(tts_model, tts_config.as_deref()) {
        Ok(v) => v,
        Err(e) => {
            echo(&format!("failed to load TTS voice {}: {e}", pathlib_str(tts_model)));
            return 1;
        }
    };
    let cwd = p.get("--cwd").unwrap_or(".").to_string();
    let session_dir = match p.get("--session-dir") {
        Some(d) if !d.is_empty() => pathlib_str(d),
        _ => crate::embed::path_join(&crate::embed::path_join(&cwd, ".port_sessions"), "agent"),
    };
    let mut turns = AgentTurns {
        sarina_bin: p.get("--sarina-bin").unwrap_or("sarina").to_string(),
        cwd,
        session_dir,
        agent_args: p.all("--agent-arg"),
        session_id: p.get("--resume-session-id").filter(|s| !s.is_empty()).map(|s| s.to_string()),
    };
    let max_record_seconds = p.get("--max-record-seconds").and_then(crate::argparse::py_float).unwrap_or(120.0);
    let mut voice = Voice {
        alsa: alsa.expect("checked above"),
        stt,
        tts,
        input_device: p.get("--input-device").map(|s| s.to_string()),
        output_device: p.get("--output-device").map(|s| s.to_string()),
        max_record_seconds,
    };
    catch_sigint();
    run_loop(&mut turns, &mut voice, &mut Stdin::new(), !p.flag("--no-auto-memory-review"))
}
