//go:build unix

package session

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// within fails the test when fn has not returned in five seconds (a read that waits for a pipe's writer).
func within(t *testing.T, what string, fn func()) {
	t.Helper()
	done := make(chan struct{})
	go func() { fn(); close(done) }()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatalf("%s did not return: it waits on a named pipe", what)
	}
}

// A named pipe or an oversized (sparse) file under a session file's name is refused at once by Load, skipped by
// MigrateDir; before 2026-11-12 both read with os.ReadFile, which waited forever on the pipe and read any size.
func TestSessionReadsRefusePipesAndHugeFiles(t *testing.T) {
	dir := t.TempDir()
	if err := syscall.Mkfifo(filepath.Join(dir, "pipe.json"), 0o600); err != nil {
		t.Skip("no named pipes here:", err)
	}
	huge := filepath.Join(dir, "huge.json")
	if err := os.WriteFile(huge, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Truncate(huge, MaxFileBytes+1); err != nil {
		t.Fatal(err)
	}
	within(t, "LoadByID on a pipe", func() {
		if _, err := LoadByID(dir, "pipe"); err == nil || !strings.Contains(err.Error(), "not a regular file") {
			t.Errorf("pipe: %v", err)
		}
	})
	if _, err := LoadByID(dir, "huge"); err == nil || !strings.Contains(err.Error(), "larger than") {
		t.Errorf("huge: %v", err)
	}
	within(t, "MigrateDir over a pipe", func() {
		r, err := MigrateDir(dir, true)
		if err != nil || r.Skipped != 2 || r.Scanned != 0 {
			t.Errorf("MigrateDir: %+v %v", r, err)
		}
	})
	if b, err := ReadRegular(filepath.Join(dir, "huge.json"), 1<<40); err != nil || len(b) != MaxFileBytes+1 {
		t.Errorf("a larger bound reads it: %d %v", len(b), err)
	}
}
