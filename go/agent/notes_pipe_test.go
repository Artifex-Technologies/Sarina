//go:build unix

package agent

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// A named pipe (or a file past 1 MiB) where a session's notes live reads as no notes, at once. Before 2026-11-12 the
// notes were read with os.ReadFile: the pipe made the session wait forever, any size was read.
func TestNotesReadRefusesPipesAndHugeFiles(t *testing.T) {
	dir := t.TempDir()
	a, err := New(Options{Model: config.ModelConfig{Model: "test", BaseURL: "http://127.0.0.1:1/v1"}, Runtime: config.DefaultRuntime(dir), Home: dir})
	if err != nil {
		t.Fatal(err)
	}
	p := a.notesPath("s1")
	os.MkdirAll(filepath.Dir(p), 0o755)
	if err := syscall.Mkfifo(p, 0o600); err != nil {
		t.Skip("no named pipes here:", err)
	}
	done := make(chan string)
	go func() { done <- a.readNotes("s1") }()
	select {
	case got := <-done:
		if got != "" {
			t.Fatalf("pipe read as %q", got)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("reading the notes waited on a named pipe")
	}
	big := a.notesPath("s2")
	os.WriteFile(big, nil, 0o600)
	os.Truncate(big, 1<<20+1)
	if got := a.readNotes("s2"); got != "" {
		t.Fatalf("a file past the bound read as %d bytes", len(got))
	}
	os.WriteFile(a.notesPath("s3"), []byte("  kept notes \n"), 0o600)
	if got := a.readNotes("s3"); got != "kept notes" {
		t.Fatalf("notes: %q", got)
	}
}
