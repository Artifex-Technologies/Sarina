package insight

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// vault-tidy only tidies regular files in Talks/: a link planted there had the file it named (anywhere on the computer)
// rewritten without its "small talk" lines, and a named pipe held the command for ever.
func TestTidyLeavesPlantedLinksAndPipesAlone(t *testing.T) {
	vault, outside := t.TempDir(), t.TempDir()
	if err := os.MkdirAll(filepath.Join(vault, "Talks"), 0o755); err != nil {
		t.Fatal(err)
	}
	theirs := filepath.Join(outside, "notes.md")
	text := "- 2020-01-01 10:00 | they said: hi | I said: Hello.\n- 2020-01-01 10:01 | they said: I love my cat | I said: Nice.\n"
	if err := os.WriteFile(theirs, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(theirs, filepath.Join(vault, "Talks", "2020-01.md")); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(filepath.Join(vault, "Talks", "2020-02.md"), 0o644); err != nil {
		t.Fatal(err)
	}
	var out, errb bytes.Buffer
	env := &cli.Env{Stdout: &out, Stderr: &errb, Stdin: strings.NewReader(""), Getenv: func(string) string { return "" }}
	done := make(chan int, 1)
	go func() { done <- tidyCommand(env, []string{"--path", vault, "--yes"}) }()
	select {
	case rc := <-done:
		if rc != 0 || !strings.Contains(out.String(), "nothing to tidy") {
			t.Fatalf("rc %d, out %q, err %q", rc, out.String(), errb.String())
		}
	case <-time.After(10 * time.Second):
		t.Fatal("vault-tidy waited on a named pipe")
	}
	if b, _ := os.ReadFile(theirs); string(b) != text {
		t.Fatalf("the link's target was rewritten: %q", b)
	}
}
