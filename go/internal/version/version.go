// Package version is Sarina's own version, for everything that names it (sarina version, the MCP handshake).
package version

// Version is the VERSION file at the top of the repository (Labyrinth_OS's docs/VERSIONING.md). This Go module (go/) cannot
// embed a file above it, so the number is kept here as well: Labyrinth_OS's scripts/release.sh writes both, and
// version_test.go fails when they differ.
const Version = "1.0.0"
