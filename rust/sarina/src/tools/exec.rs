//! Go: `go/tools/exec.go`, `proc_unix.go`, `procs.go` and `bash.go`: the bash tool (its guards before anything runs, the
//! child's environment without secret-looking names, its own process group killed on a timeout or a cancel, output kept
//! to the last 2 MiB), background processes and the TaskOutput and TaskStop tools (testdata/parity/bash.json).
//!
//! Every read from the child is bounded: its output goes through a buffer that keeps the last 2 MiB, and after the shell
//! exits its pipes are read for at most 2 s more (Go's WaitDelay), so a process that escaped the group and holds them
//! open cannot hold the call.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::goany::Any;
use crate::gotext::{to_valid_utf8, trim_space};
use crate::tools::destructive::{check_destructive, check_privileged};
use crate::tools::exfil::exfil_risk;
use crate::tools::files::{arg_bool, arg_int, arg_str, exfil_refusal, fail};
use crate::tools::fsutil::{preview, truncate_output};
use crate::tools::registry::{Context, Emit, Output, ToolError};
use crate::{goerr, gotext};

/// The parent environment a context hands its children ("KEY=value" entries).
pub type Environ = Arc<dyn Fn() -> Vec<Vec<u8>> + Send + Sync>;

const SECRET_ENV_MARKERS: &[&str] = &["SECRET", "TOKEN", "PASSWORD", "PRIVATE_KEY", "API_KEY", "CREDENTIAL", "AUTH"];

/// The default time of a foreground command.
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const BUFFER_CAP: usize = 2 << 20;
const WAIT_DELAY: Duration = Duration::from_secs(2);
const MAX_BACKGROUND_PROCS: usize = 16;

/// Go's `strings.ToUpper` as far as the markers can tell: each character's single uppercase form (Go's simple mapping
/// gives the same ASCII letters).
fn upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let mut u = c.to_uppercase();
            match (u.next(), u.next()) {
                (Some(x), None) => x,
                _ => c,
            }
        })
        .collect()
}

/// Go's `isSecretEnvName`: a name holding any marker, in any case (GIT_AUTHOR_NAME and SSH_AUTH_SOCK go too, on purpose).
pub fn is_secret_env_name(name: &str) -> bool {
    let up = upper(name);
    SECRET_ENV_MARKERS.iter().any(|m| up.contains(m))
}

fn this_environ() -> Vec<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    std::env::vars_os().map(|(k, v)| [k.as_bytes(), b"=", v.as_bytes()].concat()).collect()
}

/// Go's `childEnv`: the parent environment without secret-looking names, then the session's variables, then the extra
/// ones (later layers win), sorted.
pub fn child_env(tc: &Context) -> Vec<Vec<u8>> {
    let environ = tc.environ.as_ref().map(|f| f()).unwrap_or_else(this_environ);
    let mut vals: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    for kv in &environ {
        let i = match kv.iter().position(|&c| c == b'=') {
            Some(0) => match kv[1..].iter().position(|&c| c == b'=') {
                Some(j) if j > 0 => j + 1,
                _ => continue,
            },
            Some(i) => i,
            None => continue,
        };
        if is_secret_env_name(&gotext::lossy(&kv[..i])) {
            continue;
        }
        vals.insert(kv[..i].to_vec(), kv[i + 1..].to_vec());
    }
    for (k, v) in tc.session_env.iter().chain(&tc.extra_env) {
        vals.insert(k.as_bytes().to_vec(), v.as_bytes().to_vec());
    }
    let mut out: Vec<Vec<u8>> = vals.into_iter().map(|(k, v)| [&k[..], b"=", &v[..]].concat()).collect();
    out.sort();
    out
}

/// Go's `exec.LookPath` for a bare name on this process's PATH.
fn look_path(name: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let p = format!("{dir}/{name}");
        if let Ok(m) = std::fs::metadata(&p) {
            if !m.is_dir() && m.permissions().mode() & 0o111 != 0 {
                // Go refuses a program found relative to the current folder
                return p.starts_with('/').then_some(p);
            }
        }
    }
    None
}

/// Go's `shellPath`: bash, else sh, from PATH.
fn shell_path() -> Result<String, String> {
    look_path("bash")
        .or_else(|| look_path("sh"))
        .ok_or_else(|| "bash is not available on this machine, so shell commands cannot run".to_owned())
}

/// Go's `capBuffer`: the most recent output up to 2 MiB (when it grows past, the last 1 MiB is kept).
#[derive(Default, Debug)]
pub struct CapBuffer {
    data: Vec<u8>,
    dropped: usize,
}

impl CapBuffer {
    pub fn write(&mut self, p: &[u8]) {
        self.data.extend_from_slice(p);
        if self.data.len() > BUFFER_CAP {
            let cut = self.data.len() - BUFFER_CAP / 2;
            self.dropped += cut;
            self.data.drain(..cut);
        }
    }

    pub fn text(&self) -> String {
        let s = to_valid_utf8(&self.data);
        if self.dropped > 0 {
            return format!("[earlier output dropped]\n{s}");
        }
        s
    }
}

/// Go's `lineEmitter`: a byte stream to one emit call per line.
struct LineEmitter<'a> {
    stream: &'static str,
    emit: &'a Emit,
    part: Vec<u8>,
}

impl LineEmitter<'_> {
    fn write(&mut self, p: &[u8]) {
        self.part.extend_from_slice(p);
        while let Some(i) = self.part.iter().position(|&c| c == b'\n') {
            let line: Vec<u8> = self.part.drain(..=i).collect();
            (self.emit)(self.stream, &to_valid_utf8(&line));
        }
    }

    fn flush(&mut self) {
        if !self.part.is_empty() {
            (self.emit)(self.stream, &to_valid_utf8(&self.part));
            self.part.clear();
        }
    }
}

/// SIGKILL to a process group (Go's hardKill and reapGroup).
fn hard_kill(pid: i32) {
    if pid > 0 {
        // SAFETY: kill(2) on the process group our own child leads (made with process_group(0)).
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

/// SIGTERM to a process group (Go's softStop).
fn soft_stop(pid: i32) {
    if pid > 0 {
        // SAFETY: as hard_kill.
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
    }
}

/// Go's `newShellCmd` and `Start`: bash -c command in the workspace, the child environment, stdin from the null device,
/// its own process group.
fn spawn_shell(command: &str, tc: &Context, out: Stdio, err: Stdio) -> Result<Child, String> {
    use std::os::unix::ffi::OsStrExt;
    let shell = shell_path()?;
    if let Err(e) = std::fs::metadata(&tc.root) {
        return Err(goerr::path("chdir", tc.root.as_bytes(), &e));
    }
    let mut cmd = Command::new(&shell);
    cmd.arg("-c").arg(command).current_dir(&tc.root).env_clear().stdin(Stdio::null()).stdout(out).stderr(err).process_group(0);
    for kv in child_env(tc) {
        if let Some(i) = kv.iter().skip(1).position(|&c| c == b'=').map(|i| i + 1) {
            cmd.env(std::ffi::OsStr::from_bytes(&kv[..i]), std::ffi::OsStr::from_bytes(&kv[i + 1..]));
        }
    }
    cmd.spawn().map_err(|e| goerr::path("fork/exec", shell.as_bytes(), &e))
}

fn set_nonblocking(fd: i32) {
    // SAFETY: fcntl(2) on a pipe this process owns.
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
    }
}

/// The child's pipes, read with poll(2) so that the wait for the process, the timer and a cancel are all seen within
/// 50 ms, and nothing is read without a bound.
struct Pump<'a> {
    pipes: Vec<(Option<std::fs::File>, &'static str)>,
    sink: &'a mut dyn FnMut(&'static str, &[u8]),
    /// the child's pidfd: readable once it has ended, so the wait for its end wakes at once
    pidfd: Option<std::os::fd::OwnedFd>,
}

/// pidfd_open(2) for our own child (None on a kernel without it: the loop then wakes every few milliseconds).
fn pidfd(child: &Child) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // SAFETY: pidfd_open(2) on our own child's pid, which is not reaped yet; the fd it returns is owned here.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as libc::pid_t, 0) };
    (fd >= 0).then(|| unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
}

impl Pump<'_> {
    fn open(&self) -> bool {
        self.pipes.iter().any(|(p, _)| p.is_some())
    }

    /// Waits at most `wait` for output and reads what there is.
    fn step(&mut self, wait: Duration) {
        let mut fds: Vec<libc::pollfd> = self
            .pipes
            .iter()
            .filter_map(|(p, _)| p.as_ref())
            .map(|f| libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 })
            .collect();
        let open = fds.len();
        if let Some(p) = &self.pidfd {
            fds.push(libc::pollfd { fd: p.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        if fds.is_empty() {
            std::thread::sleep(wait.min(Duration::from_millis(5)));
            return;
        }
        // SAFETY: poll(2) on a buffer of pollfd this function owns, of the length given.
        unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait.as_millis().min(1000) as i32);
        }
        if open == 0 {
            return;
        }
        let mut buf = [0u8; 32 * 1024];
        for (slot, stream) in self.pipes.iter_mut() {
            let Some(f) = slot.as_mut() else { continue };
            loop {
                match f.read(&mut buf) {
                    Ok(0) => {
                        *slot = None;
                        break;
                    }
                    Ok(n) => (self.sink)(stream, &buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        *slot = None;
                        break;
                    }
                }
            }
        }
    }

    /// After the process ended: read until every pipe is closed, for at most WAIT_DELAY.
    fn drain(&mut self) {
        self.drain_until(Instant::now() + WAIT_DELAY);
        for (slot, _) in self.pipes.iter_mut() {
            *slot = None;
        }
    }

    /// Reads until every pipe is closed or `until`; the pipes still open stay open.
    fn drain_until(&mut self, until: Instant) {
        self.pidfd = None;
        while self.open() {
            let now = Instant::now();
            if now >= until {
                break;
            }
            self.step((until - now).min(Duration::from_millis(50)));
        }
    }
}

fn pipe_file(p: impl Into<std::os::fd::OwnedFd>) -> std::fs::File {
    let f = std::fs::File::from(p.into());
    set_nonblocking(f.as_raw_fd());
    f
}

/// What a foreground command left.
#[derive(Debug, Default)]
pub struct ShellResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i64,
    pub timed_out: bool,
    pub interrupted: bool,
}

fn exit_code(st: std::process::ExitStatus) -> i64 {
    st.code().map(i64::from).unwrap_or(-1)
}

/// Go's `runForeground`: the command to its end, the whole group killed on the timeout or a cancel; emit, when given,
/// gets the output line by line as it comes.
pub fn run_foreground(command: &str, tc: &Context, timeout: Duration, emit: Option<&Emit>) -> Result<ShellResult, String> {
    let timeout = if timeout.is_zero() { DEFAULT_COMMAND_TIMEOUT } else { timeout };
    let mut child = spawn_shell(command, tc, Stdio::piped(), Stdio::piped())?;
    let pid = child.id() as i32;
    let (mut out, mut errb) = (CapBuffer::default(), CapBuffer::default());
    let mut emitters = emit.map(|e| {
        (LineEmitter { stream: "stdout", emit: e, part: Vec::new() }, LineEmitter { stream: "stderr", emit: e, part: Vec::new() })
    });
    let mut res = ShellResult::default();
    let status;
    {
        let mut sink = |stream: &'static str, b: &[u8]| {
            let is_out = stream == "stdout";
            if is_out {
                out.write(b)
            } else {
                errb.write(b)
            }
            if let Some((o, e)) = emitters.as_mut() {
                if is_out {
                    o.write(b)
                } else {
                    e.write(b)
                }
            }
        };
        let mut pump = Pump {
            pipes: vec![(child.stdout.take().map(pipe_file), "stdout"), (child.stderr.take().map(pipe_file), "stderr")],
            sink: &mut sink,
            pidfd: pidfd(&child),
        };
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(st)) = child.try_wait() {
                status = Some(st);
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                res.timed_out = true;
                hard_kill(pid);
                status = child.wait().ok();
                break;
            }
            if tc.ctx.err().is_some() {
                res.interrupted = true;
                hard_kill(pid);
                status = child.wait().ok();
                break;
            }
            pump.step((deadline - now).min(Duration::from_millis(50)));
        }
        pump.drain();
    }
    hard_kill(pid);
    if let Some((o, e)) = emitters.as_mut() {
        o.flush();
        e.flush();
    }
    res.stdout = out.text();
    res.stderr = errb.text();
    res.exit_code = status.map(exit_code).unwrap_or(-1);
    Ok(res)
}

/// Go's `*exec.ExitError` text for a status that is not success ("exit status 3", "signal: killed"), or None.
pub(crate) fn exit_error(st: std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    if st.success() {
        return None;
    }
    if let Some(c) = st.code() {
        return Some(format!("exit status {c}"));
    }
    let sig = st.signal().unwrap_or(0);
    let name = match sig {
        1 => "hangup".to_owned(),
        2 => "interrupt".to_owned(),
        3 => "quit".to_owned(),
        6 => "aborted".to_owned(),
        9 => "killed".to_owned(),
        11 => "segmentation fault".to_owned(),
        13 => "broken pipe".to_owned(),
        15 => "terminated".to_owned(),
        n => format!("signal {n}"),
    };
    Some(format!("signal: {name}"))
}

/// Which outputs of a program `run_program` keeps (the others go to the null device).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    None,
    Stdout,
    /// stdout and stderr in one pipe, in the order written
    Both,
}

/// A program for `run_program`: its path, arguments, environment, folder (None: this process's) and the outputs kept.
pub(crate) struct Prog<'a> {
    pub path: &'a str,
    pub args: &'a [String],
    pub env: &'a [Vec<u8>],
    pub cwd: Option<&'a str>,
    pub capture: Capture,
}

/// Go's `headTail`: the first and the last `max` bytes written, and how many were left out between them.
#[derive(Debug)]
pub(crate) struct HeadTail {
    head: Vec<u8>,
    tail: Vec<u8>,
    max: usize,
    dropped: u64,
}

impl HeadTail {
    pub fn new(max: usize) -> HeadTail {
        HeadTail { head: Vec::new(), tail: Vec::new(), max, dropped: 0 }
    }

    pub fn write(&mut self, mut p: &[u8]) {
        let room = self.max.saturating_sub(self.head.len());
        if room > 0 {
            let k = room.min(p.len());
            self.head.extend_from_slice(&p[..k]);
            p = &p[k..];
        }
        self.tail.extend_from_slice(p);
        if self.tail.len() > 2 * self.max {
            let cut = self.tail.len() - self.max;
            self.dropped += cut as u64;
            self.tail.drain(..cut);
        }
    }

    /// The text kept, with Go's note of what was left out (bytes as they are; the caller makes them valid UTF-8).
    pub fn bytes(&self) -> Vec<u8> {
        let mut tail: &[u8] = &self.tail;
        let mut dropped = self.dropped;
        if tail.len() > self.max {
            dropped += (tail.len() - self.max) as u64;
            tail = &tail[tail.len() - self.max..];
        }
        let mut out = self.head.clone();
        if dropped > 0 {
            out.extend_from_slice(format!("\n[... {dropped} bytes of output left out ...]\n").as_bytes());
        }
        out.extend_from_slice(tail);
        out
    }
}

/// Go's `exec.CommandContext(...).Run()` for a short-lived program: stdin from the null device, what it writes to the
/// outputs asked for handed to `sink` as it comes, killed at `deadline` or when `ctx` ends (only the process, as Go's
/// CommandContext does), its pipe read for at most 2 s after it ends. The end (None when it could not be waited for) or
/// Go's start error ("chdir /x: ...", "fork/exec /x: ...").
pub(crate) fn run_program(
    p: &Prog,
    sink: &mut dyn FnMut(&[u8]),
    deadline: Instant,
    ctx: &crate::goctx::Ctx,
) -> Result<Option<std::process::ExitStatus>, String> {
    use std::os::unix::ffi::OsStrExt;
    if let Some(dir) = p.cwd {
        if let Err(e) = std::fs::metadata(dir) {
            return Err(goerr::path("chdir", dir.as_bytes(), &e));
        }
    }
    let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let mut child = {
        // the write end lives only in this block: the parent's copies must be closed for the pipe to end with the child
        let writer = writer;
        let mut cmd = Command::new(p.path);
        cmd.args(p.args).env_clear().stdin(Stdio::null());
        if let Some(dir) = p.cwd {
            cmd.current_dir(dir);
        }
        for kv in p.env {
            if let Some(i) = kv.iter().skip(1).position(|&c| c == b'=').map(|i| i + 1) {
                cmd.env(std::ffi::OsStr::from_bytes(&kv[..i]), std::ffi::OsStr::from_bytes(&kv[i + 1..]));
            }
        }
        let w2 = writer.try_clone().map_err(|e| e.to_string())?;
        cmd.stdout(if p.capture != Capture::None { Stdio::from(writer) } else { Stdio::null() });
        cmd.stderr(if p.capture == Capture::Both { Stdio::from(w2) } else { Stdio::null() });
        cmd.spawn().map_err(|e| goerr::path("fork/exec", p.path.as_bytes(), &e))?
    };
    let status;
    {
        let mut s = |_: &'static str, b: &[u8]| sink(b);
        let pfd = pidfd(&child);
        let mut pump = Pump { pipes: vec![(Some(pipe_file(reader)), "out")], sink: &mut s, pidfd: pfd };
        loop {
            if let Ok(Some(st)) = child.try_wait() {
                status = Some(st);
                break;
            }
            let now = Instant::now();
            if now >= deadline || ctx.err().is_some() {
                let _ = child.kill();
                status = child.wait().ok();
                break;
            }
            pump.step((deadline - now).min(Duration::from_millis(50)));
        }
        pump.drain();
    }
    Ok(status)
}

/// What `run_group` kills at the deadline or a cancel: the program's process group (Go's tiers `prepareTreeKill`) or only
/// the program (Go's `CommandContext` default).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GroupKill {
    Group,
    Process,
}

/// How a `run_group` program ended: its status (None when it could not be waited for), and whether its pipes were still
/// open 2 s after it ended (Go's `exec.ErrWaitDelay`).
pub(crate) struct RunEnd {
    pub status: Option<std::process::ExitStatus>,
    pub io_incomplete: bool,
}

/// Go's `exec.CommandContext(...).Run()` with `WaitDelay` 2 s for a program in its own process group, its standard output
/// and error each handed to `sink` ("stdout", "stderr") as they come, stdin from the null device; killed (as `kill` says) at
/// `deadline` or when `ctx` ends. Err is Go's start error (`exec: "x": executable file not found in $PATH`,
/// `fork/exec /x: ...`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_group(
    path: &str,
    args: &[String],
    env: &[Vec<u8>],
    cwd: Option<&str>,
    deadline: Instant,
    ctx: &crate::goctx::Ctx,
    kill: GroupKill,
    sink: &mut dyn FnMut(&'static str, &[u8]),
) -> Result<RunEnd, String> {
    use std::os::unix::ffi::OsStrExt;
    let path = if path.contains('/') { path.to_string() } else { look_path_go(path)? };
    // a folder that cannot be entered fails the start as Go's does: "fork/exec <program>: ..."
    let mut cmd = Command::new(&path);
    cmd.args(args).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for kv in env {
        if let Some(i) = kv.iter().skip(1).position(|&c| c == b'=').map(|i| i + 1) {
            cmd.env(std::ffi::OsStr::from_bytes(&kv[..i]), std::ffi::OsStr::from_bytes(&kv[i + 1..]));
        }
    }
    let mut child = cmd.spawn().map_err(|e| goerr::path("fork/exec", path.as_bytes(), &e))?;
    let pid = child.id() as i32;
    let status;
    let io_incomplete;
    {
        let mut pump = Pump {
            pipes: vec![(child.stdout.take().map(pipe_file), "stdout"), (child.stderr.take().map(pipe_file), "stderr")],
            sink,
            pidfd: pidfd(&child),
        };
        loop {
            if let Ok(Some(st)) = child.try_wait() {
                status = Some(st);
                break;
            }
            let now = Instant::now();
            if now >= deadline || ctx.err().is_some() {
                match kill {
                    GroupKill::Group => hard_kill(pid),
                    GroupKill::Process => {
                        let _ = child.kill();
                    }
                }
                status = child.wait().ok();
                break;
            }
            pump.step((deadline - now).min(Duration::from_millis(50)));
        }
        pump.drain_until(Instant::now() + WAIT_DELAY);
        io_incomplete = pump.open();
        for (slot, _) in pump.pipes.iter_mut() {
            *slot = None;
        }
    }
    Ok(RunEnd { status, io_incomplete })
}

/// This process's environment as "KEY=value" entries (what Go's exec hands a child when no environment is set).
pub fn parent_env() -> Vec<Vec<u8>> {
    this_environ()
}

/// Go's `exec.LookPath` for a bare name, with its error text.
pub fn look_path_go(name: &str) -> Result<String, String> {
    look_path(name).ok_or_else(|| format!("exec: {}: executable file not found in $PATH", gotext::quote(name)))
}

// ---- background processes ------------------------------------------------------------------------------------------------

pub const PROC_RUNNING: &str = "running";
pub const PROC_COMPLETED: &str = "completed";
pub const PROC_FAILED: &str = "failed";
pub const PROC_KILLED: &str = "killed";

#[derive(Debug)]
struct ProcState {
    out: CapBuffer,
    status: &'static str,
    exit_code: i64,
    stopped: bool,
    done: bool,
}

/// One process started with bash run_in_background.
#[derive(Debug)]
pub struct BackgroundProc {
    pub id: String,
    pub command: String,
    pub pid: i32,
    started: Instant,
    state: Mutex<ProcState>,
    cv: Condvar,
}

impl BackgroundProc {
    fn lock(&self) -> std::sync::MutexGuard<'_, ProcState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The current state, exit code and output.
    pub fn snapshot(&self) -> (&'static str, i64, String) {
        let st = self.lock();
        (st.status, st.exit_code, st.out.text())
    }

    /// Waits until the process has exited, at most `d` (and less when ctx ends); true when it has.
    fn wait_done(&self, d: Duration, ctx: Option<&crate::goctx::Ctx>) -> bool {
        let until = Instant::now() + d;
        let mut st = self.lock();
        loop {
            if st.done {
                return true;
            }
            let now = Instant::now();
            if now >= until || ctx.is_some_and(|c| c.err().is_some()) {
                return false;
            }
            let wait = (until - now).min(Duration::from_millis(50));
            st = self.cv.wait_timeout(st, wait).unwrap_or_else(|p| p.into_inner()).0;
        }
    }
}

/// The background processes of one agent (Go's ProcManager).
#[derive(Debug, Default)]
pub struct ProcManager {
    procs: Mutex<HashMap<String, Arc<BackgroundProc>>>,
}

static DEFAULT_PROCS: LazyLock<Arc<ProcManager>> = LazyLock::new(|| Arc::new(ProcManager::default()));

fn proc_manager(tc: &Context) -> Arc<ProcManager> {
    tc.procs.clone().unwrap_or_else(|| DEFAULT_PROCS.clone())
}

fn new_proc_id() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 4];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);
    format!("bg_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

fn no_proc_error(id: &str) -> String {
    format!(
        "no background process {}. Ids look like bg_xxxxxxxx and come from bash with run_in_background; to-do items are separate and use \
         the task_* tools",
        gotext::quote(id)
    )
}

impl ProcManager {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<BackgroundProc>>> {
        self.procs.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Go's `Start`: the command detached, in its own group, its stdout and stderr in one pipe (in the order written), with
    /// the same environment as a foreground command; it outlives the call.
    pub fn start(&self, command: &str, tc: &Context) -> Result<Arc<BackgroundProc>, String> {
        let running = self.lock().values().filter(|p| p.lock().status == PROC_RUNNING).count();
        if running >= MAX_BACKGROUND_PROCS {
            return Err(format!("{running} background processes are already running; stop some with TaskStop first"));
        }
        shell_path()?;
        let start_err = |e: String| format!("cannot start the process: {e}");
        let (reader, writer) = std::io::pipe().map_err(|e| start_err(e.to_string()))?;
        let child = {
            let w2 = writer.try_clone().map_err(|e| start_err(e.to_string()))?;
            // the Command and with it the parent's copies of the write end are dropped here, so the pipe ends with the child
            spawn_shell(command, tc, Stdio::from(writer), Stdio::from(w2)).map_err(start_err)?
        };
        let p = Arc::new(BackgroundProc {
            id: new_proc_id(),
            command: command.to_owned(),
            pid: child.id() as i32,
            started: Instant::now(),
            state: Mutex::new(ProcState { out: CapBuffer::default(), status: PROC_RUNNING, exit_code: 0, stopped: false, done: false }),
            cv: Condvar::new(),
        });
        self.lock().insert(p.id.clone(), p.clone());
        let bp = p.clone();
        let reader = pipe_file(reader);
        std::thread::spawn(move || watch(child, reader, bp));
        Ok(p)
    }

    pub fn get(&self, id: &str) -> Option<Arc<BackgroundProc>> {
        self.lock().get(id).cloned()
    }

    /// The ids, oldest first.
    pub fn ids(&self) -> Vec<String> {
        let procs = self.lock();
        let mut v: Vec<&Arc<BackgroundProc>> = procs.values().collect();
        v.sort_by_key(|p| p.started);
        v.iter().map(|p| p.id.clone()).collect()
    }

    /// Go's `Stop`: SIGTERM to the group, SIGKILL after 2 s, the final state.
    pub fn stop(&self, id: &str) -> Result<Arc<BackgroundProc>, (Option<Arc<BackgroundProc>>, String)> {
        let Some(p) = self.get(id) else { return Err((None, no_proc_error(id))) };
        let status = p.lock().status;
        if status != PROC_RUNNING {
            return Err((Some(p), format!("background process {id} is not running (it {status})")));
        }
        p.lock().stopped = true;
        soft_stop(p.pid);
        if !p.wait_done(Duration::from_secs(2), None) {
            hard_kill(p.pid);
            if !p.wait_done(Duration::from_secs(3), None) {
                return Err((Some(p), format!("background process {id} did not exit after being killed")));
            }
        }
        hard_kill(p.pid);
        Ok(p)
    }

    /// Stops every running process (when the agent exits).
    pub fn stop_all(&self) {
        let ids = self.ids();
        std::thread::scope(|sc| {
            for id in &ids {
                sc.spawn(move || {
                    let _ = self.stop(id);
                });
            }
        });
    }
}

/// The thread behind one background process: its output into the buffer, its end, the group reaped.
fn watch(mut child: Child, reader: std::fs::File, p: Arc<BackgroundProc>) {
    let status;
    {
        let mut sink = |_: &'static str, b: &[u8]| p.lock().out.write(b);
        let mut pump = Pump { pipes: vec![(Some(reader), "out")], sink: &mut sink, pidfd: pidfd(&child) };
        loop {
            if let Ok(Some(st)) = child.try_wait() {
                status = Some(st);
                break;
            }
            pump.step(Duration::from_millis(50));
        }
        pump.drain();
    }
    hard_kill(p.pid);
    let mut st = p.lock();
    st.exit_code = status.map(exit_code).unwrap_or(-1);
    st.status = if st.stopped {
        PROC_KILLED
    } else if st.exit_code == 0 {
        PROC_COMPLETED
    } else {
        PROC_FAILED
    };
    st.done = true;
    drop(st);
    p.cv.notify_all();
}

// ---- the tools -------------------------------------------------------------------------------------------------------------

fn denied(text: String) -> Output {
    Output { error: Some(ToolError::Denied(text)), ..Output::default() }
}

fn s(v: &str) -> Any {
    Any::String(v.to_owned())
}

/// Go's `time.Duration.String`.
pub fn go_duration(d: Duration) -> String {
    let u = d.as_nanos().min(u64::MAX as u128) as u64;
    // the fraction of v/10^prec without trailing zeros (and no point when it is zero), and v/10^prec
    let frac = |v: u64, prec: u32| -> (String, u64) {
        let p = 10u64.pow(prec);
        let f = v % p;
        if f == 0 {
            return (String::new(), v / p);
        }
        let digits = format!("{f:0width$}", width = prec as usize);
        (format!(".{}", digits.trim_end_matches('0')), v / p)
    };
    if u < 1_000_000_000 {
        let (unit, prec) = match u {
            0 => return "0s".into(),
            1..=999 => ("ns", 0),
            1000..=999_999 => ("µs", 3),
            _ => ("ms", 6),
        };
        let (f, i) = frac(u, prec);
        return format!("{i}{f}{unit}");
    }
    let (f, secs) = frac(u, 9);
    let (h, m, sec) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}h{m}m{sec}{f}s")
    } else if secs >= 60 {
        format!("{m}m{sec}{f}s")
    } else {
        format!("{sec}{f}s")
    }
}

/// Go's `bashTimeout`: timeout_seconds, else the context's, else the default.
pub fn bash_timeout(tc: &Context, args: &BTreeMap<String, Any>) -> Duration {
    let mut t = tc.command_timeout;
    let n = arg_int(args, "timeout_seconds", 0);
    if n > 0 {
        t = Duration::from_secs(n as u64);
    }
    if t.is_zero() {
        t = DEFAULT_COMMAND_TIMEOUT;
    }
    t
}

/// Go's `bash` handler (and its stream handler, with emit).
pub fn bash(args: &BTreeMap<String, Any>, tc: &Context, emit: Option<&Emit>) -> Output {
    let command = arg_str(args, "command");
    if trim_space(&command).is_empty() {
        return fail("command is empty".into());
    }
    // never, not even with --unsafe
    let why = exfil_risk(&command);
    if !why.is_empty() {
        return denied(exfil_refusal("this command", &why));
    }
    // at every tier and with --unsafe: nothing sets allow_privileged
    if !tc.permissions.allow_privileged {
        let v = check_privileged(command.as_bytes());
        if v.blocked {
            return denied(format!(
                "refused: {}. Tell the user the exact command and let them run it themselves if they want it.",
                gotext::lossy(&v.reason)
            ));
        }
    }
    if !tc.permissions.allow_destructive_shell_commands {
        let v = check_destructive(command.as_bytes(), tc.root.as_bytes());
        if v.blocked {
            return denied(format!(
                "refused: {}. Destructive commands are blocked unless the operator re-runs with --unsafe (allow destructive shell commands). \
                 If this is a mistake, rephrase the command without it; otherwise tell the user what you wanted to do and let them decide.",
                gotext::lossy(&v.reason)
            ));
        }
    }
    if arg_bool(args, "run_in_background", false) {
        let p = match proc_manager(tc).start(&command, tc) {
            Ok(p) => p,
            Err(e) => return fail(e),
        };
        return Output {
            content: format!(
                "Started background process {} (pid {}). Read its output with TaskOutput and stop it with TaskStop, using task_id {}.",
                p.id,
                p.pid,
                gotext::quote(&p.id)
            ),
            metadata: BTreeMap::from([
                ("action".to_owned(), s("bash")),
                ("command".to_owned(), s(&command)),
                ("background".to_owned(), Any::Bool(true)),
                ("task_id".to_owned(), s(&p.id)),
                ("pid".to_owned(), Any::Int(p.pid.into())),
            ]),
            error: None,
        };
    }
    let timeout = bash_timeout(tc, args);
    let res = match run_foreground(&command, tc, timeout, emit) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let mut md = BTreeMap::from([
        ("action".to_owned(), s("bash")),
        ("command".to_owned(), s(&command)),
        ("exit_code".to_owned(), Any::Int(res.exit_code)),
        ("stdout_preview".to_owned(), Any::String(preview(&res.stdout, 240))),
        ("stderr_preview".to_owned(), Any::String(preview(&res.stderr, 240))),
    ]);
    if res.timed_out {
        md.insert("timed_out".into(), Any::Bool(true));
        return Output {
            metadata: md,
            error: Some(ToolError::Failed(format!(
                "command timed out after {} and was killed (with everything it started): {}. For something long-running use \
                 run_in_background, or raise timeout_seconds",
                go_duration(timeout),
                preview(&command, 120)
            ))),
            ..Output::default()
        };
    }
    if res.interrupted {
        md.insert("interrupted".into(), Any::Bool(true));
        return Output {
            metadata: md,
            error: Some(ToolError::Failed(format!("command was interrupted and killed: {}", preview(&command, 120)))),
            ..Output::default()
        };
    }
    let cut = |t: &str| t.trim_end_matches([' ', '\t', '\r', '\n']).to_owned();
    let raw = format!("exit_code={}\n[stdout]\n{}\n[stderr]\n{}", res.exit_code, cut(&res.stdout), cut(&res.stderr));
    let mut text = trim_space(&raw).to_owned();
    md.insert("output_preview".into(), Any::String(preview(&text, 240)));
    let names = tc.state.authored_in(&command);
    if !names.is_empty() {
        text = format!(
            "[note: this command involves files you wrote earlier in this session ({}). Its output is not an independent measurement of \
             anything; treat it with skepticism.]\n{text}",
            names.join(", ")
        );
    }
    Output { content: truncate_output(&text, tc.max_output_chars), metadata: md, error: None }
}

/// Go's `TaskOutput` handler.
pub fn task_output(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let id = arg_str(args, "task_id");
    let Some(p) = proc_manager(tc).get(&id) else { return fail(no_proc_error(&id)) };
    let mut retrieval = "success";
    if p.lock().status == PROC_RUNNING {
        if !arg_bool(args, "block", true) {
            retrieval = "not_ready";
        } else {
            let wait = Duration::from_millis(arg_int(args, "timeout", 30000).max(0) as u64);
            if !p.wait_done(wait, Some(&tc.ctx)) {
                retrieval = "timeout";
            }
        }
    }
    let (status, code, output) = p.snapshot();
    let mut b = format!("<retrieval_status>{retrieval}</retrieval_status>\n<task_id>{}</task_id>\n<status>{status}</status>\n", p.id);
    if status != PROC_RUNNING {
        b.push_str(&format!("<exit_code>{code}</exit_code>\n"));
    }
    b.push_str(&format!("<output>\n{}\n</output>", output.trim_end_matches(['\r', '\n'])));
    let mut md = BTreeMap::from([
        ("action".to_owned(), s("task_output")),
        ("task_id".to_owned(), s(&p.id)),
        ("status".to_owned(), s(status)),
        ("retrieval_status".to_owned(), s(retrieval)),
    ]);
    if status != PROC_RUNNING {
        md.insert("exit_code".into(), Any::Int(code));
    }
    Output { content: truncate_output(&b, tc.max_output_chars), metadata: md, error: None }
}

/// Go's `TaskStop` handler.
pub fn task_stop(args: &BTreeMap<String, Any>, tc: &Context) -> Output {
    let id = arg_str(args, "task_id");
    match proc_manager(tc).stop(&id) {
        Err((_, e)) => fail(e),
        Ok(p) => {
            let (status, code, _) = p.snapshot();
            Output {
                content: format!("Stopped background process {} (pid {}); it is now {status}.", p.id, p.pid),
                metadata: BTreeMap::from([
                    ("action".to_owned(), s("task_stop")),
                    ("task_id".to_owned(), s(&p.id)),
                    ("status".to_owned(), s(status)),
                    ("exit_code".to_owned(), Any::Int(code)),
                ]),
                error: None,
            }
        }
    }
}

/// bash, TaskOutput and TaskStop with the Go build's definitions.
pub fn shell_tools() -> Vec<crate::tools::registry::Tool> {
    use crate::tools::defs::tool;
    let mut b = tool("bash", Arc::new(|a, tc| bash(a, tc, None)));
    b.stream = Some(Arc::new(|a, tc, e| bash(a, tc, Some(e))));
    vec![b, tool("TaskOutput", Arc::new(task_output)), tool("TaskStop", Arc::new(task_stop))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_duration_texts() {
        let ns = Duration::from_nanos;
        for (d, want) in [
            (ns(0), "0s"),
            (ns(1), "1ns"),
            (ns(1500), "1.5µs"),
            (ns(1_000_000), "1ms"),
            (ns(1_234_567), "1.234567ms"),
            (Duration::from_millis(1500), "1.5s"),
            (Duration::from_secs(1), "1s"),
            (Duration::from_secs(30), "30s"),
            (Duration::from_secs(90), "1m30s"),
            (Duration::from_secs(120), "2m0s"),
            (Duration::from_secs(3600), "1h0m0s"),
            (Duration::from_millis(3_661_500), "1h1m1.5s"),
        ] {
            assert_eq!(go_duration(d), want, "{d:?}");
        }
    }

    #[test]
    fn the_buffer_keeps_the_last_part() {
        let mut b = CapBuffer::default();
        for _ in 0..3 {
            b.write(&vec![b'x'; 1 << 20]);
        }
        b.write(b"end");
        let t = b.text();
        assert!(t.starts_with("[earlier output dropped]\n") && t.ends_with("xend"), "{}", &t[..40]);
        assert!(b.data.len() <= BUFFER_CAP);
    }

    #[test]
    fn the_timeout_named_is_the_one_used() {
        let tc = Context::default();
        assert_eq!(bash_timeout(&tc, &BTreeMap::new()), DEFAULT_COMMAND_TIMEOUT);
        let args = BTreeMap::from([("timeout_seconds".to_owned(), Any::Number(2.0))]);
        assert_eq!(bash_timeout(&tc, &args), Duration::from_secs(2));
        let tc = Context { command_timeout: Duration::from_secs(5), ..Context::default() };
        assert_eq!(bash_timeout(&tc, &BTreeMap::new()), Duration::from_secs(5));
    }

    /// A job left running in the command's group keeps its output pipe open: the call ends 2 s after the shell (Go's
    /// WaitDelay) and the group is killed, instead of waiting for the job. bash's builtins only, in a temporary folder.
    #[test]
    fn a_job_holding_the_pipes_does_not_hold_the_call() {
        if look_path("bash").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("sarina-exec-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tc = Context { root: dir.to_str().unwrap().to_owned(), environ: Some(Arc::new(Vec::new)), ..Context::default() };
        let start = Instant::now();
        let r = run_foreground("echo a; while :; do :; done &", &tc, Duration::from_secs(20), None).unwrap();
        let took = start.elapsed();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((r.stdout.as_str(), r.exit_code, r.timed_out), ("a\n", 0, false));
        assert!(took >= WAIT_DELAY && took < Duration::from_secs(10), "{took:?}");
    }
}
