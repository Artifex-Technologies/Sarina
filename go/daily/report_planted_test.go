package daily

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// The day's note is read as a bounded regular file and written as a new file: a named pipe planted under its name held
// `sarina daily-report` (and the check-up's report) for ever, and a link planted under its ".tmp" name had the note
// written wherever it pointed, outside the vault too.
func TestDayNoteRefusesPlantedPipesAndLinks(t *testing.T) {
	vault, outside := t.TempDir(), t.TempDir()
	day := time.Date(2026, 11, 16, 9, 0, 0, 0, time.Local)
	if err := os.MkdirAll(filepath.Join(vault, "Daily"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(Path(vault, day), 0o644); err != nil {
		t.Fatal(err)
	}
	done := make(chan Note, 1)
	go func() { done <- Load(vault, day) }()
	select {
	case n := <-done:
		if n.Day != "2026-11-16" || len(n.Numbers) != 0 {
			t.Fatalf("note %+v", n)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("Load waited on a named pipe")
	}
	if err := os.Remove(Path(vault, day)); err != nil {
		t.Fatal(err)
	}
	planted := filepath.Join(outside, "planted")
	if err := os.Symlink(planted, Path(vault, day)+".tmp"); err != nil {
		t.Fatal(err)
	}
	if err := Save(vault, Parse("2026-11-16", "# day\n")); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(planted); !os.IsNotExist(err) {
		t.Fatalf("the note was written through the .tmp link: %v", err)
	}
	if b, _ := os.ReadFile(Path(vault, day)); !strings.Contains(string(b), "# day") {
		t.Fatalf("note %q", b)
	}
}

// The programs Probe runs are bounded: one that leaves a child holding its output open held the report for as long as
// the child lived (now at most 2 s more), and its whole output was kept.
func TestProbeDoesNotWaitForAChildHoldingTheOutput(t *testing.T) {
	bin := t.TempDir()
	for _, name := range []string{"systemctl", "coredumpctl", "journalctl"} {
		script := "#!/bin/sh\nexit 0\n"
		if name == "systemctl" {
			script = "#!/bin/sh\necho 'x.service loaded failed failed X'\nsleep 30 &\n"
		}
		if err := os.WriteFile(filepath.Join(bin, name), []byte(script), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	t.Setenv("PATH", bin+":/usr/bin:/bin")
	t.Setenv("HEIMDALL_REVIEW_URL", "http://127.0.0.1:1/v1/review")
	start := time.Now()
	s := Probe(t.Context())
	if d := time.Since(start); d > 15*time.Second {
		t.Fatalf("took %v", d)
	}
	if len(s.Failed) == 0 || s.Failed[0] != "x.service" {
		t.Fatalf("failed %v", s.Failed)
	}
}
