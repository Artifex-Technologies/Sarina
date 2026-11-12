//! Go: `go/internal/version`. Sarina's own version, for everything that names it (`sarina version`, the MCP handshake): the
//! VERSION file at the top of the repository (Labyrinth_OS's docs/VERSIONING.md), read at build time, so the Rust side has
//! no copy to keep in step. A test checks the Go constant says the same.

pub const VERSION: &str = include_str!("../../../VERSION").trim_ascii();

#[cfg(test)]
mod tests {
    #[test]
    fn go_says_the_same() {
        let go = include_str!("../../../go/internal/version/version.go");
        assert!(go.contains(&format!("const Version = \"{}\"", super::VERSION)), "go/internal/version and VERSION differ");
        assert!(!super::VERSION.is_empty() && !super::VERSION.contains(char::is_whitespace));
    }
}
