package vault

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

func TestNoteDropsLinesIntoTodaysInboxNote(t *testing.T) {
	dir := t.TempDir()
	old := now
	defer func() { now = old }()
	now = func() time.Time { return time.Date(2026, 10, 26, 9, 41, 0, 0, time.Local) }
	var out, errb bytes.Buffer
	env := &cli.Env{Stdout: &out, Stderr: &errb, Getenv: func(string) string { return "" }, Stdin: strings.NewReader("")}
	if rc := noteCommand(env, []string{"--path", dir, "buy", "thermal", "paste"}); rc != 0 {
		t.Fatalf("rc %d: %s", rc, errb.String())
	}
	now = func() time.Time { return time.Date(2026, 10, 26, 13, 5, 0, 0, time.Local) }
	env.Stdin = strings.NewReader("ask Sarina about the\\nnew GPU\n")
	if rc := noteCommand(env, []string{"--path", dir}); rc != 0 {
		t.Fatalf("rc %d: %s", rc, errb.String())
	}
	b, err := os.ReadFile(filepath.Join(dir, "Inbox", "Notes 2026-10-26.md"))
	if err != nil {
		t.Fatal(err)
	}
	got := string(b)
	for _, want := range []string{"# Notes 2026-10-26", "- 09:41 buy thermal paste", "- 13:05 ask Sarina about the"} {
		if !strings.Contains(got, want) {
			t.Fatalf("missing %q in:\n%s", want, got)
		}
	}
	if strings.Count(got, "# Notes") != 1 {
		t.Fatalf("the heading was written twice:\n%s", got)
	}
	if rc := noteCommand(env, []string{"--path", dir}); rc != 2 {
		t.Fatal("an empty note is refused")
	}
}
