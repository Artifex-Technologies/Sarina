//! Go: `go/cli`: the command table's commands, ported one by one. Each takes the arguments after its name and writes to
//! the `Env`'s outputs, returning the exit status; the table and the hand-over of unported commands to the Go build come
//! with the `sarina-rs` binary (docs/RUST_PORT.md "The switch").

pub mod entry;
pub mod memorycmd;

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_USAGE: i32 = 2;

/// What a command reads and writes (Go's `Env`, as far as the ported commands use it).
#[derive(Default)]
pub struct Env {
    /// the home folder for ~/.sarina ("" = the user's own)
    pub home: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Env {
    /// Go's `homeDir`.
    pub fn home_dir(&self) -> String {
        if !self.home.is_empty() {
            return self.home.clone();
        }
        std::env::var("HOME").unwrap_or_default()
    }

    /// `fmt.Fprintln(env.Stdout, s)`.
    pub fn println(&mut self, s: &str) {
        self.stdout.extend_from_slice(s.as_bytes());
        self.stdout.push(b'\n');
    }

    /// `fmt.Fprintln(env.Stderr, s)`.
    pub fn eprintln(&mut self, s: &str) {
        self.stderr.extend_from_slice(s.as_bytes());
        self.stderr.push(b'\n');
    }
}
