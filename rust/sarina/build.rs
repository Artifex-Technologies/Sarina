//! The build's own stamp for `sarina version` (src/cli/version.rs), made the way the Go toolchain stamps the Go build
//! (cmd/go's `gitStatus`, the `vcs.*` build settings): in the git checkout this crate sits in, `git status --porcelain`
//! (any output, untracked files too, is "uncommitted changes"), then `git -c log.showsignature=false log -1
//! --format=%H:%ct` (the commit and its time in UTC, RFC 3339). Where Go names its toolchain (`go1.27.4`), this names
//! rustc's (`rustc 1.x.y`). Outside a checkout (a release tarball) or without git: commit "unknown", no time, not modified,
//! as Go's build without VCS information. Nothing is passed in by the recipes: git runs here, as Go runs it itself.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    // --no-optional-locks: `git status` leaves the index as it is (a refreshed index would make cargo run this again)
    let out = Command::new("git").arg("--no-optional-locks").args(args).current_dir(dir).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Go's `time.Unix(sec, 0).UTC().Format(time.RFC3339Nano)` for a whole second.
fn rfc3339(sec: i64) -> String {
    let (days, rem) = (sec.div_euclid(86400), sec.rem_euclid(86400));
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (mut commit, mut time, mut modified) = ("unknown".to_string(), String::new(), false);
    if let Some(top) = git(&dir, &["rev-parse", "--show-toplevel"]) {
        let top = PathBuf::from(top.trim_end_matches('\n'));
        if let Some(status) = git(&top, &["status", "--porcelain"]) {
            modified = !status.is_empty();
            // an empty repository has no commit to name (Go: the same when the tree has changes)
            if let Some(line) = git(&top, &["-c", "log.showsignature=false", "log", "-1", "--format=%H:%ct"]) {
                if let Some((rev, t)) = line.trim_end_matches('\n').split_once(':') {
                    if let Ok(t) = t.parse::<i64>() {
                        commit = rev.to_string();
                        time = rfc3339(t);
                    }
                }
            }
        }
        // run again when HEAD moves (a commit, a checkout) or the index changes (git add); the crate's own files are
        // watched too (an edit rebuilds the crate anyway). An edit elsewhere in the tree shows up at the next of these.
        for p in ["HEAD", "logs/HEAD", "index"] {
            if let Some(path) = git(&top, &["rev-parse", "--git-path", p]) {
                let path = top.join(path.trim_end_matches('\n'));
                if path.exists() {
                    println!("cargo:rerun-if-changed={}", path.display());
                }
            }
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let toolchain = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    println!("cargo:rustc-env=SARINA_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=SARINA_BUILD_TIME={time}");
    println!("cargo:rustc-env=SARINA_BUILD_MODIFIED={modified}");
    println!("cargo:rustc-env=SARINA_BUILD_TOOLCHAIN={toolchain}");
}
