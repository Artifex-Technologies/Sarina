package tools

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityStateDir records StateDir (testdata/parity/statedir.json) on a made-up tree: workspaces whose .port_sessions
// is a plain folder, missing, a link out, a link inside, a dangling link, or whose agent folder is a link out; the
// default folders and others. "$BASE" is the test folder.
func TestParityStateDir(t *testing.T) {
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	hide := func(s string) string { return strings.ReplaceAll(s, top, "$BASE") }
	outside := filepath.Join(top, "outside")
	os.MkdirAll(outside, 0o755)
	var cases []parity.Case
	for _, v := range []string{"plain", "missing", "link-out", "link-in", "dangling", "agent-out", "scratch-out", "nested-out"} {
		ws := filepath.Join(top, v)
		ps := filepath.Join(ws, ".port_sessions")
		os.MkdirAll(ws, 0o755)
		switch v {
		case "plain":
			os.MkdirAll(filepath.Join(ps, "agent"), 0o755)
		case "link-out":
			os.Symlink(outside, ps)
		case "link-in":
			os.MkdirAll(filepath.Join(ws, "real"), 0o755)
			os.Symlink("real", ps)
		case "dangling":
			os.Symlink(filepath.Join(top, "nowhere"), ps)
		case "agent-out":
			os.MkdirAll(ps, 0o755)
			os.Symlink(outside, filepath.Join(ps, "agent"))
		case "scratch-out":
			os.MkdirAll(ps, 0o755)
			os.Symlink("../../outside", filepath.Join(ps, "scratchpad"))
		case "nested-out":
			os.MkdirAll(filepath.Join(ps, "agent"), 0o755)
			os.Symlink(outside, filepath.Join(ps, "agent", "deeper"))
		}
		for _, d := range []string{filepath.Join(ps, "agent"), filepath.Join(ps, "scratchpad"), filepath.Join(ps, "agent", "deeper", "x"), ps,
			filepath.Join(ws, "sessions"), outside, filepath.Join(ws, ".port_sessions_x"), "", filepath.Join(ps, "agent", "..", "..", "y")} {
			for _, root := range []string{ws, ws + "/", ""} {
				got, err := StateDir(root, d)
				cases = append(cases, parity.Case{Fn: "StateDir", In: []string{v, hide(root), hide(d)}, Out: []string{hide(got), hide(parity.Err(err))}})
			}
		}
	}
	parity.Fixture(t, "statedir", cases)
}
