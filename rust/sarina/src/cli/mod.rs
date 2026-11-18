//! Go: `go/cli`: the command table and its commands, ported one by one. Each takes the arguments after its name and writes
//! to the `Env`'s outputs, returning the exit status. [`COMMANDS`] is the table (with Go's summaries, for the usage);
//! [`not_a_command`] is Go's `cli.Main` for no command, help and an unknown one. The `sarina-rs` binary dispatches
//! (docs/RUST_PORT.md "The switch").

pub mod chat;
pub mod chatopts;
pub mod doctorcmd;
pub mod entry;
pub mod extcmd;
pub mod guicmd;
pub mod limbocmd;
pub mod memorycmd;
pub mod servicecmd;
pub mod sessionmigrate;
pub mod tiercmd;
pub mod version;

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_USAGE: i32 = 2;

/// Where a command's output goes as it is written: the binary's standard streams (1 and 2). None keeps it in `stdout`
/// and `stderr` (tests).
pub type Sink = Box<dyn FnMut(i32, &[u8]) + Send>;

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

/// The command table (Go's `cli.Register` calls): each command's name, its one-line summary (Go's, byte for byte: the usage
/// lists them) and what runs it, in Go's sort order (byte order of the names, as the usage prints them). Every command is
/// Rust since 2026-11-18; a command moved here only after `rust/parity/cli-differential.py` showed no difference for it.
pub const COMMANDS: &[(&str, &str, Command)] = &[
    ("agents", "show sub-agent profiles: agents list", extcmd::agents),
    ("chat", "run a prompt once, or hold an interactive conversation", chat::chat_command),
    ("chymaera-analyze", "statistical triage of recent Argus App captures (duplicate NFC UIDs, rare Sub-GHz frequencies)", crate::chymaera::analyze_command),
    ("chymaera-console", "show the Argus App console status and recent captures", crate::chymaera::console_command),
    ("chymaera-device-info", "check captured device_info text from an Argus device (Flipper Zero; --file F or stdin)", crate::chymaera::device_info_command),
    ("chymaera-merge-report", "report which Argus firmware files a merge from upstream would touch", crate::chymaera::merge_report_command),
    ("council", "ask several models the same question and compare: council [--context-file F] [--members a,b] [--local-only] [--no-synthesis] [--json] QUESTION | council status [--probe]", crate::council::command),
    ("crash-detail", "the latest crash of one program (read-only): crash-detail PROGRAM", crate::sysinfo::crash_command),
    ("crypto", "identify, decode, hash or crack a string in code: crypto identify|decode|classic|hash|modern ... (sarina crypto for usage)", crate::cryptotools::tool::command),
    ("daily-report", "the day's status in her vault (Daily/YYYY-MM-DD.md): probe the computer and write it, or print it as it is when fresh enough: daily-report [--path VAULT] [--max-age MINUTES] [--no-write]", crate::daily::report_command),
    ("desktop-audit", "audit and repair the application menu (report only unless --apply)", crate::desktop::command),
    ("doctor", "read-only checkup of the whole setup: doctor [--json] [--cwd D] [--session-dir D]", doctorcmd::doctor),
    ("fleet-launch", "propose launching another agent, run it only after you confirm: fleet-launch AGENT [PROMPT] [--cwd D] [--yes]", crate::fleet::launch::command),
    ("fleet-status", "show other agents' usage and limits from Omarchy's records: fleet-status [--suggest] [--json]", crate::fleet::status::command),
    ("fleet-usage", "print (or --write) Sarina's usage record for Omarchy's agents panel", crate::fleet::usage::command),
    ("forge-approve", "look at an agent Sarina wrote and approve exactly that version: forge-approve NAME", crate::forge::tool::approve_command),
    ("forge-list", "list the agents Sarina has written, and whether you approved their current version", crate::forge::tool::list_command),
    ("forge-remove", "delete an agent Sarina wrote (and its approval): forge-remove NAME", crate::forge::tool::remove_command),
    ("gui", "run the web console (default port 8765, loopback only)", guicmd::gui),
    ("lessons", "her lessons from the vault: lessons [--path DIR] list | digest  (digest prints the active rules, for /usr/local/share/sarina/lessons.md)", crate::vault::tools::lessons_command),
    ("limbo-profiles", "list the validation profiles Limbo offers, with what each allows (needs SARINA_LIMBO_SOCKET)", limbocmd::profiles),
    ("limbo-validate", "run a command in a disposable Limbo guest: limbo-validate [--timeout S] PROFILE COMMAND [ARG...]", limbocmd::validate),
    ("mcp", "show configured MCP servers (nothing is started): mcp list", extcmd::mcp),
    ("memory-review", "ask the model what is worth remembering from a saved session: memory-review [flags] <session_id>", chat::memory_review_command),
    ("memory-review-approve", "approve a memory proposal: memory-review-approve <entry_id>", memorycmd::memory_review_approve),
    ("memory-review-list", "list memory proposals awaiting review", memorycmd::memory_review_list),
    ("memory-review-reject", "reject a memory proposal: memory-review-reject <entry_id>", memorycmd::memory_review_reject),
    ("memory-search", "semantic search over saved sessions (needs the Python helper): memory-search --model-dir D <query...>", crate::helpers::memory_search_command),
    ("memory-show", "print the curated-memory block exactly as sessions receive it", memorycmd::memory_show),
    ("memory-status", "show curated memory: paths, usage and entries", memorycmd::memory_status),
    ("note", "drop a quick note into the vault's Inbox (today's note, one line each, with the time): note [--path DIR] TEXT... (or the text on stdin)", crate::vault::tools::note_command),
    ("quantum", "simulate a small quantum circuit exactly: quantum [--shots N] QUBITS 'h 0; cx 0 1' (or the circuit on standard input)", crate::quantum::command),
    ("reflect", "learn from the talk log what she knows about the owner (Entities/About <SARINA_OWNER_NAME>.md): reflect [--path VAULT] [--dry-run]", crate::insight::reflect::command),
    ("resume", "continue a saved session: resume <session_id> [prompt]", chat::resume_command),
    ("route", "what to look up before answering a spoken question: route [--scores] QUESTION (prints topics, comma-separated, or nothing)", crate::route::command),
    ("service", "run the integration service (default port 8899, loopback only by default)", servicecmd::service),
    ("session-migrate", "rewrite retired tool names in saved sessions: session-migrate [--session-dir D] [--cwd D] [--dry-run]", sessionmigrate::session_migrate),
    ("session-search", "search saved sessions: session-search [--cwd D] [--session-dir D] [--max N] <query...>", memorycmd::session_search),
    ("skill", "named reusable prompts: skill list | skill run <name> [args]", chat::skill_command),
    ("stego", "look for hidden data in a file: stego inspect|lsb|text|diff FILE [FILE2]; stego embed IN.png OUT.png TEXT (a test image)", crate::stego::tool::command),
    ("system-fix", "do one named, reversible repair at user level (the same table the system_fix tool uses): system-fix --list | ACTION", crate::sysinfo::gaming::fix_command),
    ("system-status", "print what the computer reports about one topic (read-only): system-status performance|network|disk|logs|services|hardware|crashes|boot|audio|kernel|gaming", crate::sysinfo::status_command),
    ("tiers", "compute tiers: tiers status", tiercmd::tiers),
    ("trachtenberg", "work a multiplication by the Trachtenberg method, step by step: trachtenberg A B [ANSWER-TO-CHECK]", crate::trachtenberg::command),
    ("trends", "how the computer has been doing day by day, from the daily notes: disks, crashes, failed services: trends [--path VAULT] [--days N]", crate::daily::trends_command),
    ("vault-ingest", "turn new files in the vault's Inbox/ into linked notes under Sources/ (originals untouched): vault-ingest [--path VAULT] [--dry-run]", crate::insight::ingest::command),
    ("vault-init", "make the starting folders and Home note in the vault (never overwrites): vault-init [--path DIR]", crate::vault::tools::init_command),
    ("vault-lint", "broken links, orphans and notes without front matter: vault-lint [--path DIR]", crate::vault::tools::lint_command),
    ("vault-search", "search the vault: vault-search [--path DIR] WORDS...", crate::vault::tools::search_command),
    ("vault-tidy", "drop old small talk from the talk log (greetings, thanks, status checks) and her own voice heard back; shows the lines and asks first: vault-tidy [--path VAULT] [--older-than DAYS] [--yes] [--dry-run]", crate::insight::tidy_command),
    ("version", "which build this is: Sarina's version, the commit it was built from, when, and whether the tree had changes (--json)", version::version_command),
    ("voice-chat", "push-to-talk voice conversation (needs the Python helper): voice-chat --stt-model-dir D --tts-model F", crate::helpers::voice_chat_command),
];

/// The command `name` runs, if there is one (docs/RUST_PORT.md "The plan for `sarina-rs`").
pub fn ported(name: &str) -> Option<Command> {
    COMMANDS.iter().find(|(n, _, _)| *n == name).map(|&(_, _, run)| run)
}

/// Go's `usage`: every command with its summary, the name padded to 18 columns (`%-18s`).
pub fn usage() -> String {
    let mut b = String::from("Usage: sarina <command> [flags] [arguments]\n\nCommands:\n");
    for (name, summary, _) in COMMANDS {
        b += &format!("  {name:<18} {summary}\n");
    }
    b + "\nRun 'sarina <command> -h' for a command's flags.\n"
}

/// Go's `cli.Main` for a command line it does not dispatch: no command (the usage on standard output, exit status 2),
/// `-h`, `--help` or `help` first (the usage on standard output, 0), or a command not in the table (its name quoted as
/// Go's `%q` does, bytes that are not UTF-8 as `\xHH`, then the usage, on standard error, 2). None: a command to run.
pub fn not_a_command(args: &[Vec<u8>]) -> Option<(i32, Vec<u8>, Vec<u8>)> {
    let first = match args.first() {
        None => return Some((EXIT_USAGE, usage().into_bytes(), Vec::new())),
        Some(a) => a.as_slice(),
    };
    if matches!(first, b"-h" | b"--help" | b"help") {
        return Some((EXIT_OK, usage().into_bytes(), Vec::new()));
    }
    if std::str::from_utf8(first).ok().and_then(ported).is_some() {
        return None;
    }
    let err = format!("sarina: unknown command {}\n\n{}", crate::gotext::quote_bytes(first), usage());
    Some((EXIT_USAGE, Vec::new(), err.into_bytes()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_table_is_in_go_sort_order() {
        // Go's usage sorts the names (sort.Strings: byte order); the table is kept in that order, each name once
        let names: Vec<&str> = super::COMMANDS.iter().map(|c| c.0).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
        assert!(super::COMMANDS.iter().all(|c| !c.1.is_empty() && !c.1.contains('\n')));
    }
}
