//go:build unix

package sessionsearch

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// session_search over a folder with a named pipe called *.json: the pipe is skipped, the real session still found.
// Before 2026-11-12 the search read it with os.ReadFile and waited forever (a tool call that never returned).
func TestSearchSkipsNamedPipes(t *testing.T) {
	dir := t.TempDir()
	if err := syscall.Mkfifo(filepath.Join(dir, "a.json"), 0o600); err != nil {
		t.Skip("no named pipes here:", err)
	}
	os.WriteFile(filepath.Join(dir, "b.json"), []byte(`{"messages":[{"role":"user","content":"where is the blue widget"}]}`), 0o600)
	done := make(chan int)
	go func() { done <- len(Search("blue widget", dir, 5)) }()
	select {
	case n := <-done:
		if n != 1 {
			t.Fatalf("hits: %d", n)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the search waited on a named pipe")
	}
}
