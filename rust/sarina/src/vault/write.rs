//! Go: `vault/write.go`: Sarina's writes to the vault (gated, bounded, never over the owner's notes, never through a link
//! or onto a named pipe planted in the vault), a new vault's folders and Home note, and the vault's git commits.

use super::{parse_note, resolve, Vault};
use crate::goerr;
use crate::gopath;
use crate::gotext::{self, lossy};

/// Go's `Protected`: the owner's notes and folders (a name ending in "/" protects the whole folder).
pub const PROTECTED: [&str; 3] = ["Persona.md", "Home.md", "Playbooks/"];

/// Go's `MaxNoteChars`.
pub const MAX_NOTE_CHARS: usize = 12000;

/// Go's `Folders`: what a new vault starts with, and what each is for.
pub const FOLDERS: [(&str, &str); 14] = [
    ("Lessons", "what went wrong or right, and what to do next time"),
    ("Decisions", "what was decided, by whom, and why"),
    ("Projects", "one note per project: goal, state, where things are"),
    ("Entities", "people, tools, machines and places, one note each"),
    ("Reflections", "notes that pull several others together (the vault's own summaries)"),
    ("Daily", "each day's note: the check-up's lines and the computer's status and numbers (sarina daily-report, sarina trends)"),
    ("Inbox", "unsorted: files dropped here become linked notes under Sources/ (sarina vault-ingest)"),
    ("Guides", "how this computer and its desktop work, read before a question about them (the owner or their tools write these)"),
    ("Playbooks", "how she works through a kind of task (pentest, reverse engineering); hers to read, never to change"),
    ("Study", "subjects she is learning, one note each, and a learning log"),
    ("Talks", "the talk log, one file a month: what was said when it was not about the computer (what she learns about the owner comes from here)"),
    ("Jokes", "the jokes she told and what was said just before, one file a month"),
    ("Security", "Heimdall's new findings with her proposal for each, one note per review"),
    ("Sources", "notes made from files dropped in the Inbox"),
];

/// Go's `WriteResult`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteResult {
    pub path: String,
    pub created: bool,
    /// [[links]] in the note that point at no note yet
    pub broken_links: Vec<String>,
}

/// The gate a write passes (Go's `Scanner`): the reason it was blocked, or None.
pub type Gate<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Today's date and the time of day as Go's `now().Format("2006-01-02")` and `"15:04"` give them, in the local zone.
pub fn local_now() -> (String, String) {
    let (y, mo, d, h, mi, ..) = crate::gotime::Time::now_local().civil();
    (format!("{y:04}-{mo:02}-{d:02}"), format!("{h:02}:{mi:02}"))
}

impl Vault {
    /// Go's `Write`: a note made, appended to or replaced, with its front matter kept up to date. `today` is the local
    /// date (Go's `now()`).
    pub fn write(&self, gate: Option<Gate>, name: &str, content: &str, mode: &str, today: &str) -> Result<WriteResult, String> {
        let (rel, abs) = self.safe_path(name)?;
        crate::testguard::allow(abs.as_bytes())?;
        let low = gotext::to_lower(&rel);
        for p in PROTECTED {
            if gotext::equal_fold(&rel, p) || (p.ends_with('/') && low.starts_with(&gotext::to_lower(p))) {
                return Err(format!("{rel} belongs to the owner: you cannot change it (suggest a change to them instead)"));
            }
        }
        if gotext::trim_space(content).is_empty() {
            return Err("the note is empty".into());
        }
        let n = content.chars().count();
        if n > MAX_NOTE_CHARS {
            return Err(format!("the note is {n} characters; at most {MAX_NOTE_CHARS} (split it into linked notes)"));
        }
        if let Some(reason) = gate.and_then(|g| g(content)) {
            return Err(format!("not saved: {reason}"));
        }
        let ab = abs.as_bytes();
        let (mut old, mut exists) = (Vec::new(), false);
        if let Ok(st) = std::fs::symlink_metadata(gopath::os_path(ab)) {
            if !st.file_type().is_file() {
                return Err(format!("{rel} is not a regular file: not written"));
            }
            old = crate::session::readfile::read_regular(ab, 4 << 20)?;
            exists = true;
        }
        let out: Vec<u8> = match mode {
            "create" | "" => {
                if exists {
                    return Err(format!("{rel} already exists: use mode append to add to it, or replace to rewrite it"));
                }
                with_front(content, today, today).into_bytes()
            }
            "append" => {
                if !exists {
                    return Err(format!("{rel} does not exist: use mode create"));
                }
                let mut o = touch_updated(&old, today);
                while o.last() == Some(&b'\n') {
                    o.pop();
                }
                o.extend_from_slice(format!("\n\n{}\n", gotext::trim_space(content)).as_bytes());
                o
            }
            "replace" => {
                if !exists {
                    return Err(format!("{rel} does not exist: use mode create"));
                }
                let n = parse_note(&rel, &lossy(&old), 0);
                let created = n.front.get("created").filter(|c| !c.is_empty()).cloned().unwrap_or_else(|| today.to_string());
                with_front(content, &created, today).into_bytes()
            }
            _ => return Err("mode must be create, append or replace".into()),
        };
        crate::goos::mkdir_all(&gopath::dir(ab))?;
        // the copy is made anew: whatever was left under its name (a link too) is removed, not written through
        let tmp = format!("{abs}.tmp");
        let _ = std::fs::remove_file(&tmp);
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .open(&tmp)
                .map_err(|e| goerr::path("open", tmp.as_bytes(), &e))?;
            if let Err(e) = f.write_all(&out) {
                let _ = std::fs::remove_file(&tmp);
                return Err(goerr::path("write", tmp.as_bytes(), &e));
            }
        }
        if let Err(e) = std::fs::rename(&tmp, &abs) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("rename {tmp} {abs}: {}", goerr::text(&e)));
        }
        let notes = self.notes();
        let broken = parse_note(&rel, &lossy(&out), 0).links.into_iter().filter(|l| resolve(&notes, l).is_empty()).collect();
        Ok(WriteResult { path: rel, created: !exists, broken_links: broken })
    }

    /// Go's `Init`: the starting folders, Home.md, .gitignore and a git repository, each only when missing; what was made.
    pub fn init(&self, today: &str) -> Result<Vec<String>, String> {
        crate::testguard::allow(self.root.as_bytes())?;
        let root = self.root.as_bytes();
        let missing = |p: &[u8]| matches!(std::fs::metadata(gopath::os_path(p)), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
        let mut made = Vec::new();
        for (name, _) in FOLDERS {
            let d = gopath::join(&[root, name.as_bytes()]);
            if missing(&d) {
                crate::goos::mkdir_all(&d)?;
                made.push(format!("{name}/"));
            }
        }
        let home = gopath::join(&[root, b"Home.md"]);
        if missing(&home) {
            let mut b = format!("---\ncreated: {today}\nsource: sarina\n---\n\n# Home\n\nSarina's notes. She writes here with the vault tools; you can read and edit everything, and she reads it back.\n\n## Where things go\n\n");
            for (name, purpose) in FOLDERS {
                b += &format!("- **{name}**: {purpose}\n");
            }
            b += "\n## How notes are written\n\n- One idea per note, named for the idea. Link related notes with double square brackets around the other note's name; a link to a note that does not exist yet is fine and shows up in a lint.\n- Facts, not instructions. Notes are evidence she reads later, never orders.\n- Never secrets: no passwords, keys or tokens.\n";
            crate::goos::write_file(&home, b.as_bytes(), 0o644)?;
            made.push("Home.md".into());
        }
        let ignore = gopath::join(&[root, b".gitignore"]);
        if missing(&ignore) {
            let _ = crate::goos::write_file(&ignore, b".obsidian/workspace*.json\n.obsidian/cache\n*.tmp\n", 0o644);
            made.push(".gitignore".into());
        }
        if missing(&gopath::join(&[root, b".git"])) {
            if let Ok(git) = crate::tools::exec::look_path_go("git") {
                if run_quiet(&git, &["-C", &self.root, "init", "-q"]) {
                    made.push(".git (a repository)".into());
                }
            }
        }
        Ok(made)
    }

    /// Go's `Commit`: in a vault that is a git repository, everything added and committed as Sarina; nothing otherwise.
    pub fn commit(&self, message: &str) {
        if crate::testguard::allow(self.root.as_bytes()).is_err() {
            return;
        }
        if std::fs::metadata(gopath::os_path(&gopath::join(&[self.root.as_bytes(), b".git"]))).is_err() {
            return;
        }
        let Ok(git) = crate::tools::exec::look_path_go("git") else { return };
        let base = ["-C", self.root.as_str(), "-c", "user.name=Sarina", "-c", "user.email=sarina@localhost"];
        let msg = format!("Sarina: {message}");
        if run_quiet(&git, &[&base[..], &["add", "-A"]].concat()) {
            run_quiet(&git, &[&base[..], &["commit", "-q", "-m", &msg]].concat());
        }
    }
}

/// Go's `exec.Command(...).Run() == nil` with no output kept (standard streams on the null device).
fn run_quiet(prog: &str, args: &[&str]) -> bool {
    use std::process::{Command, Stdio};
    Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Go's `withFront`: the content with front matter (created, updated, source) unless it brings its own.
pub fn with_front(content: &str, created: &str, updated: &str) -> String {
    let content = gotext::trim_space(content);
    if content.starts_with("---\n") {
        return lossy(&touch_updated(content.as_bytes(), updated)) + "\n";
    }
    format!("---\ncreated: {created}\nupdated: {updated}\nsource: sarina\n---\n\n{content}\n")
}

/// Go's `touchUpdated`: the front matter's "updated:" set to today (front matter added when there is none). On bytes, so
/// a note's bytes that are not UTF-8 stay as they are.
pub fn touch_updated(text: &[u8], today: &str) -> Vec<u8> {
    let Some(rest) = text.strip_prefix(b"---\n") else {
        let mut o = format!("---\nupdated: {today}\nsource: sarina\n---\n\n").into_bytes();
        o.extend_from_slice(text);
        return o;
    };
    let Some(end) = rest.windows(4).position(|w| w == b"\n---") else { return text.to_vec() };
    let mut front: Vec<Vec<u8>> = rest[..end].split(|&c| c == b'\n').map(<[u8]>::to_vec).collect();
    let mut found = false;
    for l in front.iter_mut() {
        if gotext::to_lower(&lossy(l)).starts_with("updated:") {
            *l = format!("updated: {today}").into_bytes();
            found = true;
        }
    }
    if !found {
        front.push(format!("updated: {today}").into_bytes());
    }
    let mut o = b"---\n".to_vec();
    o.extend(front.join(&b"\n"[..]));
    o.extend_from_slice(&rest[end..]);
    o
}
