//! Go: `go/cli`: the command table's commands, ported one by one. Each takes the arguments after its name and writes to
//! the `Env`'s outputs, returning the exit status; the table and the hand-over of unported commands to the Go build come
//! with the `sarina-rs` binary (docs/RUST_PORT.md "The switch"): [`ported`] is the table.

pub mod chatopts;
pub mod entry;
pub mod limbocmd;
pub mod memorycmd;
pub mod sessionmigrate;
pub mod tiercmd;

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_USAGE: i32 = 2;

/// Where a command's output goes as it is written: the binary's standard streams (1 and 2). None keeps it in `stdout`
/// and `stderr` (tests).
pub type Sink = Box<dyn FnMut(i32, &[u8])>;

/// Go's `Getenv`, shareable (the tiers' Router keeps one).
pub type Getenv = std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>;

/// What a command reads and writes (Go's `Env`, as far as the ported commands use it).
#[derive(Default)]
pub struct Env {
    /// the home folder for ~/.sarina ("" = the user's own)
    pub home: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Go's `Getenv`; None: the process's environment
    pub getenv: Option<Getenv>,
    pub sink: Option<Sink>,
}

impl Env {
    /// Go's `homeDir`.
    pub fn home_dir(&self) -> String {
        if !self.home.is_empty() {
            return self.home.clone();
        }
        std::env::var("HOME").unwrap_or_default()
    }

    /// Go's `env.Getenv`.
    pub fn getenv(&self, k: &str) -> String {
        match &self.getenv {
            Some(f) => f(k),
            None => {
                std::env::var_os(k).map(|v| crate::gotext::lossy(std::os::unix::ffi::OsStrExt::as_bytes(v.as_os_str()))).unwrap_or_default()
            }
        }
    }

    /// `getenv` as a function of its own, for what keeps it (the tiers' Router).
    pub fn getenv_owned(&self) -> Box<dyn Fn(&str) -> String> {
        match &self.getenv {
            Some(f) => {
                let f = f.clone();
                Box::new(move |k: &str| f(k))
            }
            None => Box::new(|k: &str| Env::default().getenv(k)),
        }
    }

    /// A function for the clients that take Go's `Getenv`.
    pub fn getenv_fn(&self) -> impl Fn(&str) -> String + '_ {
        move |k: &str| self.getenv(k)
    }

    /// What was written so far, handed to the sink (Go writes as it goes).
    pub fn flush(&mut self) {
        if let Some(sink) = self.sink.as_mut() {
            if !self.stdout.is_empty() {
                sink(1, &self.stdout);
                self.stdout.clear();
            }
            if !self.stderr.is_empty() {
                sink(2, &self.stderr);
                self.stderr.clear();
            }
        }
    }

    /// `fmt.Fprintln(env.Stdout, s)`.
    pub fn println(&mut self, s: &str) {
        self.stdout.extend_from_slice(s.as_bytes());
        self.stdout.push(b'\n');
        self.flush();
    }

    /// `fmt.Fprintln(env.Stderr, s)`.
    pub fn eprintln(&mut self, s: &str) {
        self.stderr.extend_from_slice(s.as_bytes());
        self.stderr.push(b'\n');
        self.flush();
    }
}

/// A ported command: the arguments after its name, the exit status back.
pub type Command = fn(&mut Env, &[String]) -> i32;

/// The commands `sarina-rs` runs itself (docs/RUST_PORT.md "The plan for `sarina-rs`"); every other one goes to the Go
/// build. A command moves here only after `rust/parity/cli-differential.py` shows no difference for it.
pub fn ported(name: &str) -> Option<Command> {
    Some(match name {
        "memory-status" => memorycmd::memory_status,
        "memory-show" => memorycmd::memory_show,
        "session-search" => memorycmd::session_search,
        "memory-review-list" => memorycmd::memory_review_list,
        "memory-review-approve" => memorycmd::memory_review_approve,
        "memory-review-reject" => memorycmd::memory_review_reject,
        "session-migrate" => sessionmigrate::session_migrate,
        "limbo-profiles" => limbocmd::profiles,
        "limbo-validate" => limbocmd::validate,
        "tiers" => tiercmd::tiers,
        _ => return None,
    })
}
