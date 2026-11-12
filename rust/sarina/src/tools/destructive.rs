//! Go: `go/tools/destructive.go` (with `realRoot` and `within` from `fsutil.go`). The bash guards: what the bash tool
//! refuses when destructive commands are not allowed ([`check_destructive`]) and what it refuses at every tier
//! ([`check_privileged`]).
//!
//! They read the command with the shell parser ([`crate::shellparse`]): every simple command is found whatever separates it,
//! nested in a subshell, a substitution or a `-c` string, and the program is identified by name after removing env
//! assignments, sudo/env/xargs-style wrappers, paths and quoting. Commands whose program or script only exists at run time are
//! refused rather than guessed at.
//!
//! This is a guard-rail against a model's mistakes, not a sandbox. The real boundaries are the shell permission itself and
//! the Limbo sandbox. Reasons are bytes, as Go's: a refused word that is not UTF-8 is quoted as it is.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::gopath;
use crate::gotext::{lossy, quote, to_lower_bytes, trim_space};
use crate::shellparse::{self, Command, Word, DYN};

/// The outcome of a check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub blocked: bool,
    pub reason: Vec<u8>,
}

fn verdict(why: Vec<u8>) -> Verdict {
    Verdict { blocked: !why.is_empty(), reason: why }
}

/// Go's `CheckDestructive`: whether a bash command should be refused when destructive commands are not allowed. `root` is the
/// workspace; redirections and tee targets outside it are refused.
pub fn check_destructive(command: &[u8], root: &[u8]) -> Verdict {
    verdict(Checker { root: root.to_vec(), depth: 0, privileged: false }.script(command))
}

/// Go's `CheckPrivileged`: whether a bash command would run something with root's (or another account's) rights, found
/// wherever it sits. The bash tool refuses such a command at every tier.
pub fn check_privileged(command: &[u8]) -> Verdict {
    verdict(Checker { root: Vec::new(), depth: 0, privileged: true }.script(command))
}

struct Checker {
    root: Vec<u8>,
    depth: usize,
    /// look only for privileged programs; the destructive checks are skipped
    privileged: bool,
}

#[derive(Default)]
struct Member {
    prog: String,
    download: bool,
    shell_stdin: bool,
}

fn s(x: &str) -> Vec<u8> {
    x.as_bytes().to_vec()
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

// ---- program tables ---------------------------------------------------------

fn destructive_prog(p: &str) -> Option<&'static str> {
    Some(match p {
        "rm" => "removes files",
        "rmdir" => "removes directories",
        "unlink" => "removes files",
        "shred" => "destroys file contents",
        "srm" => "destroys file contents",
        "truncate" => "truncates files",
        "mv" => "moves or overwrites files",
        "dd" => "writes raw data to files or devices",
        "mke2fs" => "creates a filesystem",
        "mkswap" => "formats a device",
        "fdisk" | "sfdisk" | "cfdisk" | "parted" | "gdisk" | "sgdisk" => "edits partition tables",
        "wipefs" => "erases filesystem signatures",
        "blkdiscard" => "discards device contents",
        "shutdown" => "powers the machine off",
        "reboot" => "reboots the machine",
        "poweroff" => "powers the machine off",
        "halt" => "halts the machine",
        _ => return None,
    })
}

fn power_subcommand(t: &[u8]) -> bool {
    matches!(
        t,
        b"poweroff"
            | b"reboot"
            | b"halt"
            | b"kexec"
            | b"suspend"
            | b"hibernate"
            | b"hybrid-sleep"
            | b"suspend-then-hibernate"
            | b"rescue"
            | b"emergency"
            | b"isolate"
    )
}

#[derive(Default)]
struct WrapSpec {
    short: &'static [u8],
    long: &'static [&'static str],
    positional: usize,
    assigns: bool,
}

fn wrapper(p: &str) -> Option<WrapSpec> {
    let w = |short: &'static [u8], long: &'static [&'static str]| WrapSpec { short, long, ..WrapSpec::default() };
    Some(match p {
        "sudo" => w(
            b"ughprtUCDRT",
            &["user", "group", "host", "prompt", "role", "type", "other-user", "chdir", "chroot", "close-from", "command-timeout"],
        ),
        "doas" => w(b"uC", &[]),
        "env" => WrapSpec { assigns: true, ..w(b"uCS", &["unset", "chdir", "split-string"]) },
        "command" | "builtin" | "nohup" | "setsid" | "busybox" => WrapSpec::default(),
        "exec" => w(b"a", &[]),
        "time" => w(b"fo", &["format", "output"]),
        "nice" => w(b"n", &["adjustment"]),
        "ionice" => w(b"cnpPu", &["class", "classdata", "pid", "pgid", "uid"]),
        "timeout" => WrapSpec { positional: 1, ..w(b"sk", &["signal", "kill-after"]) },
        "stdbuf" => w(b"ioe", &["input", "output", "error"]),
        "xargs" => w(b"InPLsdEa", &["max-args", "max-procs", "max-lines", "max-chars", "delimiter", "arg-file", "eof"]),
        "watch" => w(b"n", &["interval"]),
        "flock" => WrapSpec { positional: 1, ..w(b"wE", &[]) },
        "chroot" => WrapSpec { positional: 1, ..WrapSpec::default() },
        // claude-sudo CMD... and sarina-sudo CMD... run CMD as root through an agent account; pkexec and run0 through polkit
        "claude-sudo" | "sarina-sudo" => WrapSpec::default(),
        "pkexec" => w(b"", &["user"]),
        "run0" => w(
            b"ugD",
            &[
                "user",
                "group",
                "chdir",
                "unit",
                "property",
                "description",
                "slice",
                "nice",
                "setenv",
                "background",
                "machine",
                "shell-prompt-prefix",
                "area",
            ],
        ),
        _ => return None,
    })
}

/// Programs that run a command (or a shell) as root or as another account.
fn privileged_prog(p: &str) -> bool {
    matches!(p, "sudo" | "doas" | "pkexec" | "run0" | "su" | "runuser" | "claude-sudo" | "sarina-sudo")
}

/// A privileged program named in inline interpreter code (python -c "subprocess.run(['sudo', ...])").
static PRIVILEGED_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^a-z0-9_-])(sudo|doas|pkexec|run0|claude-sudo|sarina-sudo)([^a-z0-9_-]|$)").expect("regex"));

fn is_shell(p: &str) -> bool {
    matches!(p, "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "csh" | "tcsh" | "fish" | "rbash")
}

fn is_downloader(p: &str) -> bool {
    matches!(p, "curl" | "wget" | "fetch" | "aria2c" | "nc" | "ncat" | "netcat" | "lwp-download")
}

const INLINE_RISK_TOKENS: &[&str] = &[
    "rmtree",
    "os.remove",
    "os.unlink",
    "os.rmdir",
    "os.removedirs",
    "os.rename",
    "os.replace",
    "os.system",
    "os.popen",
    "os.exec",
    "os.spawn",
    "os.kill",
    "subprocess",
    "shutil",
    "__import__",
    "importlib",
    "eval(",
    "exec(",
    "unlink",
    "rmdir",
    "rmsync",
    "writefile",
    "appendfile",
    "renamesync",
    ".write_text(",
    ".write_bytes(",
    "child_process",
    "execsync",
    "spawnsync",
    "fs.rm",
    "fs.unlink",
    "fs.write",
    "fs.rename",
    "system(",
    "popen(",
    "file.delete",
    "fileutils",
    "`",
    "mkfs",
    "dd if",
];

/// `\s` is Go's ASCII set, written out.
static RM_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|[^a-z0-9_])rm([\t\n\f\r ]|$)").expect("regex"));
static PYTHON_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(python[0-9.]*|pypy[0-9]*)$").expect("regex"));
static NODE_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(node|nodejs|bun|deno)$").expect("regex"));

// ---- command analysis -------------------------------------------------------

fn base_name(text: &[u8]) -> &[u8] {
    match text.iter().rposition(|&c| c == b'/' || c == b'\\') {
        Some(i) => &text[i + 1..],
        None => text,
    }
}

fn starts(t: &[u8], p: &str) -> bool {
    t.starts_with(p.as_bytes())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// Go's `strings.Join` of the words' texts.
fn joined(ws: &[Word], sep: &[u8]) -> Vec<u8> {
    ws.iter().map(|w| w.text.as_slice()).collect::<Vec<_>>().join(sep)
}

impl Checker {
    fn script(&self, src: &[u8]) -> Vec<u8> {
        if self.depth > 6 {
            return s("commands are nested too deeply to check");
        }
        let cmds = match shellparse::parse(src) {
            Ok(c) => c,
            Err(e) => return s(&format!("the command could not be parsed safely ({e})")),
        };
        let funcs: HashSet<&[u8]> = cmds.iter().filter(|c| !c.func_name.is_empty()).map(|c| c.func_name.as_slice()).collect();
        // Go keeps the pipelines in a map and reads it in its random order; the port reads them in source order (when two
        // pipelines are refused for different reasons, Go names either one)
        let mut pipes: BTreeMap<usize, Vec<Member>> = BTreeMap::new();
        for cmd in &cmds {
            let (why, m) = self.command(cmd);
            if !why.is_empty() {
                return why;
            }
            if self.privileged {
                continue;
            }
            if cmd.pipe_id != 0 {
                pipes.entry(cmd.pipe_id).or_default().push(m);
            }
        }
        for ms in pipes.values() {
            let (mut downloaded, mut own) = (false, 0);
            for m in ms {
                if downloaded && m.shell_stdin {
                    return s("it pipes downloaded content straight into a shell or interpreter");
                }
                if m.download {
                    downloaded = true;
                }
                if funcs.contains(m.prog.as_bytes()) {
                    own += 1;
                }
            }
            if own >= 2 {
                return s("it defines a function that pipes into itself (a fork bomb)");
            }
        }
        Vec::new()
    }

    fn command(&self, cmd: &Command) -> (Vec<u8>, Member) {
        if cmd.data {
            return (Vec::new(), Member::default());
        }
        if self.privileged {
            return self.words(&cmd.words, &cmd.heredocs, 0);
        }
        for r in &cmd.redirects {
            if r.writes() {
                let why = self.write_target(&r.target, "redirection");
                if !why.is_empty() {
                    return (why, Member::default());
                }
            }
        }
        if cmd.words.is_empty() {
            for r in &cmd.redirects {
                if r.writes() && r.op != ">>" {
                    return (s("a redirection with no command truncates a file"), Member::default());
                }
            }
            return (Vec::new(), Member::default());
        }
        let w = &cmd.words[0].text;
        if (w == b":" || w == b"true") && cmd.words.len() == 1 {
            for r in &cmd.redirects {
                if r.writes() && r.op != ">>" {
                    return (s("': > file' truncates a file"), Member::default());
                }
            }
        }
        self.words(&cmd.words, &cmd.heredocs, 0)
    }

    fn words(&self, ws: &[Word], heredocs: &[Vec<u8>], depth: usize) -> (Vec<u8>, Member) {
        if ws.is_empty() {
            return (Vec::new(), Member::default());
        }
        if depth > 8 {
            return (s("commands are wrapped too deeply to check"), Member::default());
        }
        let base = base_name(&ws[0].text);
        if base.iter().any(|c| [DYN, b'*', b'?', b'[', b'{'].contains(c)) {
            return (
                s("the command name is built at run time (variable, substitution, glob or brace expansion), so it cannot be checked"),
                Member::default(),
            );
        }
        let lower = to_lower_bytes(base);
        let prog = lower.strip_suffix(".exe").unwrap_or(&lower).to_owned();
        let args = &ws[1..];
        let mut m = Member { prog: prog.clone(), download: is_downloader(&prog), shell_stdin: false };
        if self.privileged {
            return (self.privileged_check(&prog, args, heredocs, depth), m);
        }

        if let Some(why) = destructive_prog(&prog) {
            return (s(&format!("{prog} {why}")), m);
        }
        if prog.starts_with("mkfs") {
            return (s("mkfs creates a filesystem, destroying what is on the device"), m);
        }
        match prog.as_str() {
            "eval" => {
                // run-time parts stay marked, so a command made of them is refused
                return (self.nested(&joined(args, b" ")), m);
            }
            "init" | "telinit" => {
                if args.iter().any(|a| a.text == b"0" || a.text == b"6") {
                    return (s(&format!("{prog} would shut down or reboot the machine")), m);
                }
            }
            "systemctl" | "loginctl" => {
                for a in args {
                    if power_subcommand(&a.text) {
                        return (cat(&[prog.as_bytes(), b" ", &a.text, b" would change the machine's power state"]), m);
                    }
                }
            }
            "crontab" => {
                if has_flag(args, b'r', &[]) {
                    return (s("crontab -r deletes all scheduled jobs"), m);
                }
            }
            "chmod" | "chown" | "chgrp" => {
                if has_flag(args, b'R', &["--recursive"]) {
                    return (s(&format!("{prog} -R changes permissions or ownership of a whole tree")), m);
                }
            }
            "git" => return (git_risk(args), m),
            "rsync" => {
                if args.iter().any(|a| starts(&a.text, "--del") || starts(&a.text, "--remove-source")) {
                    return (s("rsync with deletion options removes files"), m);
                }
            }
            "tee" => {
                for a in args {
                    if !starts(&a.text, "-") || a.text == b"-" {
                        let why = self.write_target(a, "tee");
                        if !why.is_empty() {
                            return (why, m);
                        }
                    }
                }
            }
            "alias" => {
                for a in args {
                    if let Some(i) = a.text.iter().position(|&c| c == b'=') {
                        if i > 0 {
                            let why = self.nested(&a.text[i + 1..]);
                            if !why.is_empty() {
                                return (why, m);
                            }
                        }
                    }
                }
            }
            "source" | "." => {
                if args.iter().any(|a| a.proc_sub) {
                    return (s("it sources a process substitution, which cannot be checked"), m);
                }
            }
            "find" => return (self.find(args), m),
            "su" | "runuser" => {
                if let Some(script) = find_dash_c(args, true) {
                    return (self.nested(&script), m);
                }
                return (Vec::new(), m);
            }
            _ => {}
        }

        if let Some(spec) = wrapper(&prog) {
            let (rest, scripts, no_exec) = unwrap(&prog, &spec, args);
            for sc in &scripts {
                let why = self.nested(&sc.text);
                if !why.is_empty() {
                    return (why, m);
                }
            }
            if no_exec || rest.is_empty() {
                return (Vec::new(), m);
            }
            return self.words(&rest, heredocs, depth + 1);
        }

        if is_shell(&prog) {
            return self.shell(args, heredocs, m);
        }
        if let Some((why, stdin)) = self.interpreter(&prog, args, heredocs) {
            m.shell_stdin = stdin;
            return (why, m);
        }
        (Vec::new(), m)
    }

    fn nested(&self, src: &[u8]) -> Vec<u8> {
        Checker { root: self.root.clone(), depth: self.depth + 1, privileged: self.privileged }.script(src)
    }

    /// words() for check_privileged: it names a privileged program and otherwise only follows what runs another command
    /// (wrappers, shells, eval, find -exec, interpreters' inline code).
    fn privileged_check(&self, prog: &str, args: &[Word], heredocs: &[Vec<u8>], depth: usize) -> Vec<u8> {
        const NEVER: &str = "runs a command with root's or another account's rights; privileged commands are never run from this tool";
        if privileged_prog(prog) {
            return s(&format!("{prog} {NEVER}"));
        }
        if prog == "systemd-run" {
            if !has_flag(args, 0, &["--user"]) {
                return s(
                    "systemd-run outside --user starts a system unit with root's rights; privileged commands are never run from this tool",
                );
            }
            // its many options take values: any word naming a privileged program is enough
            for a in args {
                let p = to_lower_bytes(base_name(&a.text));
                if privileged_prog(&p) {
                    return s(&format!("{p} {NEVER}"));
                }
            }
            return Vec::new();
        }
        match prog {
            "eval" => return self.nested(&joined(args, b" ")),
            "find" => {
                let mut i = 0;
                while i < args.len() {
                    if matches!(args[i].text.as_slice(), b"-exec" | b"-execdir" | b"-ok" | b"-okdir") {
                        let mut j = i + 1;
                        while j < args.len() && args[j].text != b";" && args[j].text != b"+" {
                            j += 1;
                        }
                        let (why, _) = self.words(&args[i + 1..j], &[], depth + 1);
                        if !why.is_empty() {
                            return why;
                        }
                        i = j;
                    }
                    i += 1;
                }
                return Vec::new();
            }
            _ => {}
        }
        if let Some(spec) = wrapper(prog) {
            let (rest, scripts, no_exec) = unwrap(prog, &spec, args);
            for sc in &scripts {
                let why = self.nested(&sc.text);
                if !why.is_empty() {
                    return why;
                }
            }
            if no_exec || rest.is_empty() {
                return Vec::new();
            }
            return self.words(&rest, heredocs, depth + 1).0;
        }
        if is_shell(prog) {
            return self.shell(args, heredocs, Member::default()).0;
        }
        if PYTHON_LIKE.is_match(prog)
            || NODE_LIKE.is_match(prog)
            || matches!(prog, "perl" | "ruby" | "php" | "lua" | "luajit" | "pwsh" | "powershell")
            || prog.ends_with("awk")
        {
            let mut code = joined(args, b" ");
            code.push(b'\n');
            code.extend_from_slice(&heredocs.join(&b'\n'));
            let lower = to_lower_bytes(&code);
            if let Some(c) = PRIVILEGED_WORD.captures(&lower) {
                return s(&format!(
                    "{prog} code names {}, which runs a command with root's rights; privileged commands are never run from this tool",
                    &c[2]
                ));
            }
        }
        Vec::new()
    }

    fn shell(&self, args: &[Word], heredocs: &[Vec<u8>], mut m: Member) -> (Vec<u8>, Member) {
        if args.iter().any(|a| a.proc_sub) {
            return (s("a shell is run on a process substitution, which cannot be checked"), m);
        }
        if let Some(script) = find_dash_c(args, false) {
            return (self.nested(&script), m);
        }
        // No -c: a script file runs unseen; otherwise the shell reads stdin.
        let mut file_arg = false;
        let mut i = 0;
        while i < args.len() {
            let t = &args[i].text;
            if starts(t, "-") && t.len() > 1 || starts(t, "+") && t.len() > 1 {
                if matches!(t.as_slice(), b"-o" | b"+o" | b"-O" | b"+O") {
                    i += 1;
                }
                if starts(t, "-") && !starts(t, "--") && t[1..].contains(&b's') {
                    break;
                }
                i += 1;
                continue;
            }
            file_arg = true;
            break;
        }
        if file_arg {
            return (Vec::new(), m);
        }
        m.shell_stdin = true;
        for h in heredocs {
            let why = self.nested(h);
            if !why.is_empty() {
                return (why, m);
            }
        }
        (Vec::new(), m)
    }

    /// Go's interpreter(): `None` when prog is not an interpreter it knows (Go's handled=false), else the reason and
    /// whether it reads its program from stdin.
    fn interpreter(&self, prog: &str, args: &[Word], heredocs: &[Vec<u8>]) -> Option<(Vec<u8>, bool)> {
        let flags: &[&str] = if PYTHON_LIKE.is_match(prog) {
            &["-c"]
        } else if prog == "perl" {
            &["-e", "-E"]
        } else if prog == "ruby" {
            &["-e"]
        } else if NODE_LIKE.is_match(prog) {
            &["-e", "--eval", "-p", "--print", "eval"]
        } else if prog == "php" {
            &["-r"]
        } else if prog == "lua" || prog == "luajit" {
            &["-e"]
        } else if prog == "pwsh" || prog == "powershell" {
            &["-c", "-command", "-encodedcommand"]
        } else if matches!(prog, "awk" | "gawk" | "mawk" | "nawk") {
            for a in args {
                if contains(&a.text, b"system(") || a.dynamic() && contains(&a.text, b"system") {
                    return Some((s("an awk program calls system()"), false));
                }
            }
            return Some((Vec::new(), false));
        } else {
            return None;
        };
        for (i, a) in args.iter().enumerate() {
            let t = to_lower_bytes(&a.text);
            let tb = t.as_bytes();
            for f in flags {
                let fb = f.as_bytes();
                let cluster = (prog == "perl" || prog == "ruby")
                    && tb.len() > 2
                    && t.starts_with('-')
                    && !t.starts_with("--")
                    && fb.len() == 2
                    && tb[tb.len() - 1] == fb[1];
                if (t == *f || cluster) && i + 1 < args.len() {
                    let code = &args[i + 1];
                    let without: Vec<u8> = code.text.iter().copied().filter(|&c| c != DYN).collect();
                    if trim_space(&lossy(&without)).is_empty() {
                        return Some((s(&format!("{prog} {f} is given code built at run time, so it cannot be checked")), false));
                    }
                    if let Some(tok) = risky_inline(&code.text) {
                        return Some((
                            s(&format!(
                                "inline {prog} code uses {} to touch files or processes; put it in a script file with write_file and run that, or re-run with --unsafe",
                                quote(tok)
                            )),
                            false,
                        ));
                    }
                    return Some((Vec::new(), false));
                }
            }
        }
        // Reads its program from stdin when no script file or inline code is given.
        let non_opt = args.iter().filter(|a| !starts(&a.text, "-") || a.text == b"-").count();
        if non_opt == 0 || args.len() == 1 && args[0].text == b"-" {
            for h in heredocs {
                if let Some(tok) = risky_inline(h) {
                    return Some((
                        s(&format!("{prog} reads a program from stdin that uses {} to touch files or processes", quote(tok))),
                        true,
                    ));
                }
            }
            return Some((Vec::new(), true));
        }
        Some((Vec::new(), false))
    }

    fn find(&self, args: &[Word]) -> Vec<u8> {
        let mut i = 0;
        while i < args.len() {
            match args[i].text.as_slice() {
                b"-delete" => return s("find -delete removes files"),
                b"-exec" | b"-execdir" | b"-ok" | b"-okdir" => {
                    let mut j = i + 1;
                    while j < args.len() && args[j].text != b";" && args[j].text != b"+" {
                        j += 1;
                    }
                    let (why, _) = self.words(&args[i + 1..j], &[], 1);
                    if !why.is_empty() {
                        return why;
                    }
                    i = j;
                }
                _ => {}
            }
            i += 1;
        }
        Vec::new()
    }

    /// Refuses a write that is built at run time, goes to a device, or leaves the workspace.
    fn write_target(&self, w: &Word, what: &str) -> Vec<u8> {
        let t = &w.text;
        if w.dynamic() {
            return s(&format!("a {what} target is built at run time, so it cannot be checked"));
        }
        if t.is_empty() {
            return Vec::new();
        }
        if starts(t, "/dev/") {
            if matches!(t.as_slice(), b"/dev/null" | b"/dev/zero" | b"/dev/stdout" | b"/dev/stderr" | b"/dev/tty") {
                return Vec::new();
            }
            if starts(t, "/dev/fd/") {
                return Vec::new();
            }
            return cat(&[b"a ", what.as_bytes(), b" writes to the device ", t]);
        }
        if self.root.is_empty() {
            return Vec::new();
        }
        if self.outside_root(t) {
            return cat(&[b"a ", what.as_bytes(), b" writes to ", t, b", outside the workspace"]);
        }
        Vec::new()
    }

    fn outside_root(&self, t: &[u8]) -> bool {
        if starts(t, "~") {
            return true;
        }
        let mut p = t.to_vec();
        if !gopath::is_abs(&p) {
            p = gopath::join(&[&self.root, &p]);
        }
        let p = gopath::clean(&p);
        !within(&self.root, &p) && !within(&real_root(&self.root), &p)
    }
}

/// fsutil.go's realRoot: the root with its links followed, or as given when that fails.
fn real_root(root: &[u8]) -> Vec<u8> {
    gopath::eval_symlinks(root).unwrap_or_else(|_| root.to_vec())
}

/// fsutil.go's within.
fn within(root: &[u8], p: &[u8]) -> bool {
    match gopath::rel(root, p) {
        Ok(r) => r != b".." && !r.starts_with(b"../"),
        Err(_) => false,
    }
}

/// Whether args hold the short option letter (alone or in a cluster such as -rf) or one of the long options (also as
/// --name=value). Go passes the byte 0 for "no short option", and a word holding a run-time part (byte 0) after its first
/// byte then matches: kept.
fn has_flag(args: &[Word], short: u8, long: &[&str]) -> bool {
    for a in args {
        let t = &a.text;
        if t == b"--" {
            return false;
        }
        if starts(t, "--") {
            for l in long {
                if t == l.as_bytes() || t.len() > l.len() && t.starts_with(l.as_bytes()) && t[l.len()] == b'=' {
                    return true;
                }
            }
            continue;
        }
        if starts(t, "-") && t.len() > 1 && t.iter().position(|&c| c == short).is_some_and(|i| i > 0) {
            return true;
        }
    }
    false
}

/// Strips a wrapper's own options (and env assignments) and returns the command it would run, any command strings it takes
/// directly (env -S), and whether it only looks a command up without running it (command -v).
fn unwrap(prog: &str, spec: &WrapSpec, args: &[Word]) -> (Vec<Word>, Vec<Word>, bool) {
    let mut scripts = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let t = &args[i].text;
        if t == b"--" {
            i += 1;
            break;
        } else if starts(t, "--") && t.len() > 2 {
            let mut name = &t[2..];
            let mut has_eq = false;
            if let Some(j) = name.iter().position(|&c| c == b'=') {
                name = &name[..j];
                has_eq = true;
            }
            i += 1;
            for l in spec.long {
                if l.as_bytes() == name && !has_eq && i < args.len() {
                    if name == b"split-string" {
                        scripts.push(args[i].clone());
                    }
                    i += 1;
                    break;
                }
            }
            if has_eq && name == b"split-string" {
                let eq = t.iter().position(|&c| c == b'=').unwrap_or(0);
                scripts.push(Word { text: t[eq + 1..].to_vec(), ..Word::default() });
            }
        } else if starts(t, "-") && t.len() > 1 {
            let mut consumed = false;
            let mut j = 1;
            while j < t.len() && !consumed {
                if prog == "command" && (t[j] == b'v' || t[j] == b'V') {
                    return (Vec::new(), Vec::new(), true);
                }
                if spec.short.contains(&t[j]) {
                    consumed = true;
                    if j == t.len() - 1 {
                        if i + 1 < args.len() {
                            if prog == "env" && t[j] == b'S' {
                                scripts.push(args[i + 1].clone());
                            }
                            i += 1;
                        }
                    } else if prog == "env" && t[j] == b'S' {
                        scripts.push(Word { text: t[j + 1..].to_vec(), ..Word::default() });
                    }
                }
                j += 1;
            }
            i += 1;
        } else if spec.assigns && t.contains(&b'=') && !starts(t, "=") {
            i += 1;
        } else {
            break;
        }
    }
    let i = (i + spec.positional).min(args.len());
    (args[i..].to_vec(), scripts, false)
}

/// The script of a "-c" style invocation.
fn find_dash_c(args: &[Word], anywhere: bool) -> Option<Vec<u8>> {
    let mut seen_c = false;
    let mut i = 0;
    while i < args.len() {
        let t = &args[i].text;
        if t == b"--command" || starts(t, "--command=") {
            if let Some(rest) = t.strip_prefix(b"--command=") {
                return Some(rest.to_vec());
            }
            seen_c = true;
            i += 1;
            continue;
        }
        if starts(t, "-") && !starts(t, "--") && t.len() > 1 {
            if t[1..].contains(&b'c') {
                seen_c = true;
            }
            if matches!(t.as_slice(), b"-o" | b"+o" | b"-O" | b"+O") {
                i += 1;
            }
            i += 1;
            continue;
        }
        if seen_c {
            return Some(t.clone());
        }
        if !anywhere {
            return None;
        }
        i += 1;
    }
    None
}

fn risky_inline(code: &[u8]) -> Option<&'static str> {
    let lc = to_lower_bytes(code);
    for t in INLINE_RISK_TOKENS {
        if lc.contains(t) {
            return Some(t.trim());
        }
    }
    if RM_WORD.is_match(&lc) {
        return Some("rm");
    }
    None
}

fn git_risk(args: &[Word]) -> Vec<u8> {
    let mut i = 0;
    while i < args.len() {
        let t = &args[i].text;
        if !starts(t, "-") {
            break;
        }
        match t.as_slice() {
            b"-C" | b"--git-dir" | b"--work-tree" | b"--namespace" | b"--super-prefix" | b"--config-env" => i += 1,
            b"-c" => {
                if i + 1 < args.len() {
                    let kv = &args[i + 1].text;
                    if to_lower_bytes(kv).starts_with("alias.") && contains(kv, b"=!") {
                        return s("a git alias is defined to run a shell command");
                    }
                }
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if i >= args.len() {
        return Vec::new();
    }
    if args[i].dynamic() {
        return s("the git subcommand is built at run time, so it cannot be checked");
    }
    let (sub, rest) = (args[i].text.as_slice(), &args[i + 1..]);
    let first_positional: &[u8] = rest.iter().find(|a| !starts(&a.text, "-")).map_or(b"", |a| a.text.as_slice());
    let long = |names: &[&str]| has_flag(rest, 0, names);
    match sub {
        b"reset" if long(&["--hard"]) => s("git reset --hard discards uncommitted work"),
        b"clean" if !has_flag(rest, b'n', &["--dry-run"]) => s("git clean deletes untracked files"),
        b"push" => {
            if has_flag(rest, b'f', &["--force", "--force-with-lease", "--force-if-includes", "--delete", "--mirror", "--prune"])
                || has_flag(rest, b'd', &[])
            {
                return s("git push with force or delete rewrites or removes remote history");
            }
            for a in rest {
                if !starts(&a.text, "-") && (starts(&a.text, "+") || starts(&a.text, ":")) {
                    return s("git push with a forcing or deleting refspec rewrites or removes remote history");
                }
            }
            Vec::new()
        }
        b"checkout" => {
            if has_flag(rest, b'f', &["--force"]) || long(&["--"]) || first_positional == b"." || rest.iter().any(|a| a.text == b"--") {
                return s("git checkout would discard working-tree changes");
            }
            Vec::new()
        }
        b"restore" if !(long(&["--staged"]) && !long(&["--worktree"]) && !has_flag(rest, b'W', &[])) => {
            s("git restore would discard working-tree changes")
        }
        b"switch" if has_flag(rest, b'f', &["--force", "--discard-changes"]) => s("git switch would discard working-tree changes"),
        b"branch" if has_flag(rest, b'D', &[]) || has_flag(rest, b'd', &[]) && has_flag(rest, b'f', &["--force"]) => {
            s("git branch -D deletes a branch regardless of its merge state")
        }
        b"stash" if first_positional == b"drop" || first_positional == b"clear" => s("git stash drop/clear destroys saved work"),
        b"reflog" if first_positional == b"expire" || first_positional == b"delete" => {
            s("git reflog expire/delete destroys recovery history")
        }
        b"gc" if rest.iter().any(|a| starts(&a.text, "--prune=") && a.text != b"--prune=never") => {
            s("git gc --prune discards unreachable objects")
        }
        b"prune" | b"filter-branch" | b"filter-repo" => cat(&[b"git ", sub, b" rewrites or destroys history"]),
        b"update-ref" if has_flag(rest, b'd', &[]) => s("git update-ref -d deletes a ref"),
        b"rm" if !long(&["--cached"]) => s("git rm deletes files from the working tree"),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::gotext::to_lower;

    #[test]
    fn to_lower_is_go_s() {
        // the dotted capital I lowers to one character, as Go's simple mapping
        assert_eq!(to_lower("İ"), "i");
    }
}
