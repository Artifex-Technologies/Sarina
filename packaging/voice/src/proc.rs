//! Running other programs the way subprocess.run did in the scripts: an optional environment (None = this process's own), an
//! optional time limit after which the program is killed (TimeoutExpired), and the returncode Python reports (negative for a
//! signal). A program that cannot be started is an OSError.

use crate::py::Env;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub enum RunErr {
    /// OSError (not found, not executable, ...)
    Os,
    /// subprocess.TimeoutExpired
    Timeout,
}

pub struct Output {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub fn returncode(st: std::process::ExitStatus) -> i32 {
    match st.code() {
        Some(c) => c,
        None => -st.signal().unwrap_or(0),
    }
}

pub fn command(argv: &[String], env: Option<&Env>, cwd: Option<&str>) -> Command {
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]);
    if let Some(e) = env {
        c.env_clear();
        for (k, v) in &e.vars {
            c.env(k, v);
        }
    }
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c
}

/// subprocess.run(argv, capture_output=True, timeout=..., env=..., cwd=...)
pub fn run_capture(argv: &[String], env: Option<&Env>, cwd: Option<&str>, timeout: Option<f64>) -> Result<Output, RunErr> {
    let mut c = command(argv, env, cwd);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = c.spawn().map_err(|_| RunErr::Os)?;
    finish(child, timeout)
}

/// The same with stdout captured and stderr left to this process's (subprocess.run(..., stdout=PIPE)).
pub fn run_stdout(argv: &[String], env: Option<&Env>, timeout: Option<f64>) -> Result<Output, RunErr> {
    let mut c = command(argv, env, None);
    c.stdout(Stdio::piped());
    let child = c.spawn().map_err(|_| RunErr::Os)?;
    finish(child, timeout)
}

/// subprocess.run(argv) with everything inherited; the returncode.
pub fn run_inherit(argv: &[String], env: Option<&Env>) -> Result<i32, RunErr> {
    let mut c = command(argv, env, None);
    let st = c.status().map_err(|_| RunErr::Os)?;
    Ok(returncode(st))
}

enum Ev {
    Out(Vec<u8>),
    Err(Vec<u8>),
    Exit(i32),
}

/// Collect a started program's output and returncode, killing it when the time is up.
pub fn finish(mut child: Child, timeout: Option<f64>) -> Result<Output, RunErr> {
    let (tx, rx) = mpsc::channel();
    let mut waiting = 1;
    if let Some(mut s) = child.stdout.take() {
        let tx = tx.clone();
        waiting += 1;
        thread::spawn(move || {
            let mut v = Vec::new();
            let _ = s.read_to_end(&mut v);
            let _ = tx.send(Ev::Out(v));
        });
    }
    if let Some(mut s) = child.stderr.take() {
        let tx = tx.clone();
        waiting += 1;
        thread::spawn(move || {
            let mut v = Vec::new();
            let _ = s.read_to_end(&mut v);
            let _ = tx.send(Ev::Err(v));
        });
    }
    let pid = child.id() as libc::pid_t;
    let (etx, erx) = mpsc::channel();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let code = child.wait().map(returncode).unwrap_or(-1);
            let _ = etx.send(());
            let _ = tx.send(Ev::Exit(code));
        });
    }
    drop(tx);
    let deadline = timeout.map(|t| Instant::now() + Duration::from_secs_f64(t.max(0.0)));
    let mut out = Output { code: 0, stdout: Vec::new(), stderr: Vec::new() };
    let mut exited = false;
    while waiting > 0 {
        let ev = match deadline {
            Some(d) => {
                let now = Instant::now();
                if now >= d {
                    None
                } else {
                    rx.recv_timeout(d - now).ok()
                }
            }
            None => rx.recv().ok(),
        };
        match ev {
            Some(Ev::Out(v)) => out.stdout = v,
            Some(Ev::Err(v)) => out.stderr = v,
            Some(Ev::Exit(c)) => {
                out.code = c;
                exited = true;
            }
            None => {
                // time is up: killed, waited for, and reported as TimeoutExpired
                if !exited {
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                    let _ = erx.recv();
                }
                return Err(RunErr::Timeout);
            }
        }
        waiting -= 1;
    }
    Ok(out)
}
