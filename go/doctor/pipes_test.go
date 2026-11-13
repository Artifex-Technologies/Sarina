//go:build unix

package doctor

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// A named pipe planted under a session file's name is counted unreadable, not waited on: before, the session scan
// opened it with os.ReadFile (a pipe's size is 0, under the bound) and waited for a writer for ever, so `sarina doctor`
// and the model's sarina_doctor tool never answered.
func TestSessionScanDoesNotWaitOnNamedPipes(t *testing.T) {
	dir := t.TempDir()
	saveSession(t, dir, nil)
	pipe := filepath.Join(dir, "planted.json")
	if err := syscall.Mkfifo(pipe, 0o600); err != nil {
		t.Skipf("no named pipes here: %v", err)
	}
	done := make(chan []Check, 1)
	go func() { done <- run(t, nil, Options{SessionDir: dir}) }()
	select {
	case cs := <-done:
		if c := byID(t, cs, "sessions"); c.Status != Warn || !strings.Contains(c.Finding, "2 session files, 1 parse") ||
			!strings.Contains(c.Finding, "planted.json") {
			t.Fatalf("%+v", c)
		}
	case <-time.After(3 * time.Second):
		// let the stuck reader go (writers that close at once), so the test can end
		for {
			if w, err := os.OpenFile(pipe, os.O_WRONLY|syscall.O_NONBLOCK, 0); err == nil {
				w.Close()
			}
			select {
			case <-done:
				t.Fatal("the session scan waited on a named pipe")
			case <-time.After(50 * time.Millisecond):
			}
		}
	}
}
