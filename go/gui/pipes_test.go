//go:build unix

package gui

import (
	"net/url"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// A named pipe planted under the to-do file's name or an instruction file's name is not waited on: before, GET
// /api/tasks and GET /api/instructions/file opened it with os.ReadFile and waited for a writer for ever (and read any
// size of regular file whole).
func TestNamedPipesAreNotWaitedOn(t *testing.T) {
	r := newRig(t, nil)
	if err := os.MkdirAll(filepath.Join(r.work, ".port_sessions"), 0o755); err != nil {
		t.Fatal(err)
	}
	pipes := []string{filepath.Join(r.work, ".port_sessions", "task_runtime.json"), filepath.Join(r.work, "CLAUDE.md")}
	for _, p := range pipes {
		if err := syscall.Mkfifo(p, 0o644); err != nil {
			t.Skipf("no named pipes here: %v", err)
		}
	}
	for i, tc := range []struct {
		path string
		code int
	}{
		{"/api/tasks", 200},
		{"/api/instructions/file?path=" + url.QueryEscape(pipes[1]), 404},
	} {
		done := make(chan int, 1)
		go func() {
			code, _ := r.do("GET", tc.path, nil)
			done <- code
		}()
		select {
		case code := <-done:
			if code != tc.code {
				t.Fatalf("%s: %d, want %d", tc.path, code, tc.code)
			}
		case <-time.After(3 * time.Second):
			// let the stuck reader go (a writer that closes at once), so the test can end
			if w, err := os.OpenFile(pipes[i], os.O_WRONLY, 0); err == nil {
				w.Close()
			}
			<-done
			t.Fatalf("%s waited on a named pipe", tc.path)
		}
	}
}

// An instruction file larger than what the console shows is read only as far as it shows (400 KiB).
func TestInstructionFileReadIsBounded(t *testing.T) {
	r := newRig(t, nil)
	p := filepath.Join(r.work, "CLAUDE.md")
	f, err := os.Create(p)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(400<<10 + 5); err != nil {
		t.Fatal(err)
	}
	f.Close()
	b, err := readHead(p, 400<<10)
	if err != nil || len(b) != 400<<10 {
		t.Fatalf("read %d bytes, %v", len(b), err)
	}
	if _, err := readHead(filepath.Dir(p), 10); err == nil {
		t.Fatal("a folder was read")
	}
}
