//! Go: `cli/version.go`: `sarina version [--json]`, which build this is. Go's answer is the Go toolchain's stamp
//! (`debug.ReadBuildInfo`: `vcs.revision`, `vcs.time`, `vcs.modified`, the Go version); this one is the Rust build's own
//! stamp (build.rs, made the same way), with the same text and the same JSON keys. The one change: where Go names its
//! toolchain (`go1.27.4`, the `go` key) this names rustc's (`rustc 1.x.y`, still under `go`, so nothing that reads the
//! JSON breaks). Proved by the CLI differential's version cases (both builds from one tree: the same commit, time and flag).

use super::{Env, EXIT_OK};

/// Go's `BuildInfo`: Sarina's version (the VERSION file) and what the build recorded about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    pub commit: String,
    pub time: String,
    pub modified: bool,
    /// the toolchain (Go's `go` field and key)
    pub go: String,
}

/// Go's `ReadBuild`: this build's stamp.
pub fn read_build() -> BuildInfo {
    let v = crate::version::VERSION;
    BuildInfo {
        version: v.to_string(),
        commit: crate::version::COMMIT.to_string(),
        time: crate::version::COMMIT_TIME.to_string(),
        modified: crate::version::MODIFIED,
        go: crate::version::TOOLCHAIN.to_string(),
    }
}

impl BuildInfo {
    /// Go's `BuildInfo.String`: the commit cut to 12 bytes, "with uncommitted changes", the time and toolchain when known.
    pub fn text(&self) -> String {
        let mut c = self.commit.as_bytes().to_vec();
        c.truncate(12);
        let mut c = crate::gotext::lossy(&c);
        if self.modified {
            c += " with uncommitted changes";
        }
        let mut s = format!("sarina {} (commit {c}", self.version);
        if !self.time.is_empty() {
            s += &format!(", committed {}", self.time);
        }
        if !self.go.is_empty() {
            s += &format!(", {}", self.go);
        }
        s + ")"
    }

    /// Go's `json.Marshal` of the struct (its keys in field order, HTML characters escaped).
    pub fn json(&self) -> String {
        let mut out = String::from("{\"version\":");
        crate::gojson::write_str(&mut out, &self.version, true);
        out += ",\"commit\":";
        crate::gojson::write_str(&mut out, &self.commit, true);
        out += ",\"time\":";
        crate::gojson::write_str(&mut out, &self.time, true);
        out += if self.modified { ",\"modified\":true,\"go\":" } else { ",\"modified\":false,\"go\":" };
        crate::gojson::write_str(&mut out, &self.go, true);
        out + "}"
    }
}

/// `sarina version [--json]` (anything else after it is ignored, as in Go).
pub fn version_command(env: &mut Env, args: &[String]) -> i32 {
    let b = read_build();
    if args.first().map(String::as_str) == Some("--json") {
        env.println(&b.json());
    } else {
        env.println(&b.text());
    }
    EXIT_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_build_as_go_does() {
        // go/cli/cli_test.go TestVersionNamesTheBuild
        let b = BuildInfo {
            version: "1.2.3".into(),
            commit: "0123456789abcdef0123".into(),
            time: "2026-10-26T15:00:00Z".into(),
            modified: true,
            go: "go1.26".into(),
        };
        assert_eq!(b.text(), "sarina 1.2.3 (commit 0123456789ab with uncommitted changes, committed 2026-10-26T15:00:00Z, go1.26)");
        assert_eq!(
            b.json(),
            r#"{"version":"1.2.3","commit":"0123456789abcdef0123","time":"2026-10-26T15:00:00Z","modified":true,"go":"go1.26"}"#
        );
        let bare = BuildInfo { version: "1.0.0".into(), commit: "unknown".into(), time: String::new(), modified: false, go: String::new() };
        assert_eq!(bare.text(), "sarina 1.0.0 (commit unknown)");
        assert_eq!(bare.json(), r#"{"version":"1.0.0","commit":"unknown","time":"","modified":false,"go":""}"#);
        let got = read_build();
        assert!(!got.commit.is_empty() && got.version == crate::version::VERSION, "{got:?}");
        assert!(got.go.is_empty() || got.go.starts_with("rustc "), "{got:?}");
    }
}
