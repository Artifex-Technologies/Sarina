//! Go: `go/internal/version`. Sarina's own version, for everything that names it (`sarina version`, the MCP handshake): the
//! VERSION file at the top of the repository (Labyrinth_OS's docs/VERSIONING.md), read at build time, so the Rust side has
//! no copy to keep in step. A test checks the Go constant says the same.

pub const VERSION: &str = include_str!("../../../VERSION").trim_ascii();

/// The build's stamp (build.rs, the way Go's toolchain stamps the Go build): the commit it was built from ("unknown"
/// outside a git checkout), the commit's time in UTC ("" without one), whether the tree had uncommitted changes, and the
/// toolchain (`rustc 1.x.y`, where Go's stamp names `go1.27.4`).
pub const COMMIT: &str = env!("SARINA_BUILD_COMMIT");
pub const COMMIT_TIME: &str = env!("SARINA_BUILD_TIME");
pub const MODIFIED: bool = matches!(env!("SARINA_BUILD_MODIFIED").as_bytes(), b"true");
pub const TOOLCHAIN: &str = env!("SARINA_BUILD_TOOLCHAIN");

#[cfg(test)]
mod tests {
    #[test]
    fn go_says_the_same() {
        let go = include_str!("../../../go/internal/version/version.go");
        assert!(go.contains(&format!("const Version = \"{}\"", super::VERSION)), "go/internal/version and VERSION differ");
        assert!(!super::VERSION.is_empty() && !super::VERSION.contains(char::is_whitespace));
    }
}
