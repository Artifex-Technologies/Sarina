package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/memory"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// filesUnder lists every file below dir.
func filesUnder(t *testing.T, dir string) []string {
	t.Helper()
	var out []string
	filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() {
			out = append(out, p)
		}
		return nil
	})
	return out
}

// A .port_sessions link planted in a workspace must not lead Sarina's session files (the whole conversation), their notes
// or her scratchpads out of it (found by the Rust port, 2026-11-12).
func TestSessionFilesStayInTheWorkspace(t *testing.T) {
	for _, link := range []string{".port_sessions", filepath.Join(".port_sessions", "agent")} {
		outside := t.TempDir()
		f := newFakeServer(t, reply("done", "stop", use(1, 1)))
		a := newTestAgent(t, f, func(o *Options) {
			p := filepath.Join(o.Runtime.CWD, link)
			os.MkdirAll(filepath.Dir(p), 0o755)
			if err := os.Symlink(outside, p); err != nil {
				t.Fatal(err)
			}
		})
		res, err := a.Run(t.Context(), "hello")
		if err == nil || !strings.Contains(err.Error(), "outside the workspace") {
			t.Fatalf("%s: want a refusal, got %v (result %+v)", link, err, res)
		}
		if got := filesUnder(t, outside); len(got) > 0 {
			t.Fatalf("%s: files written through the link: %v", link, got)
		}
	}
}

// session_search does not read the sessions a planted link points at.
func TestSessionSearchStaysInTheWorkspace(t *testing.T) {
	ws, outside := t.TempDir(), t.TempDir()
	os.WriteFile(filepath.Join(outside, "s1.json"), []byte(`{"session_id":"s1","messages":[{"role":"user","content":"where is the blue widget?"}]}`), 0o644)
	os.MkdirAll(filepath.Join(ws, ".port_sessions"), 0o755)
	if err := os.Symlink(outside, filepath.Join(ws, ".port_sessions", "agent")); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []*tools.Context{{Root: ws}, {Root: ws, SessionDirectory: filepath.Join(ws, ".port_sessions", "agent")}} {
		if dir := memory.SessionDir(tc); dir != "" {
			t.Fatalf("the linked session folder was offered for searching: %q", dir)
		}
		if out := memory.SearchSessions("blue widget", memory.SessionDir(tc), 5); strings.Contains(out, "widget") {
			t.Fatalf("session_search read outside the workspace: %s", out)
		}
	}
	// an operator's own folder elsewhere is used as given
	if dir := memory.SessionDir(&tools.Context{Root: ws, SessionDirectory: outside}); dir != outside {
		t.Fatalf("an operator-chosen folder must be used: %q", dir)
	}
}
