package tools

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestGitReadShowsTheChangeAndRunsNothingOfTheRepos(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	dir := t.TempDir()
	git := func(a ...string) {
		cmd := exec.Command("git", append([]string{"-C", dir, "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"}, a...)...)
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v %s", a, err, out)
		}
	}
	git("init", "-q")
	os.WriteFile(filepath.Join(dir, "f.go"), []byte("package f\n\nconst Tries = 1\n"), 0o644)
	git("add", ".")
	git("commit", "-qm", "first")
	os.WriteFile(filepath.Join(dir, "f.go"), []byte("package f\n\nconst Tries = 3\n"), 0o644)
	// a repository that asks git to run a program on status and on diff: reading must not run it
	marker := filepath.Join(dir, "ran")
	git("config", "core.fsmonitor", "touch "+marker)
	git("config", "diff.external", "touch "+marker)
	tool := GitRead()
	tc := &Context{Root: dir}
	out, _, err := tool.Handler(context.Background(), map[string]any{"command": "diff"}, tc)
	if err != nil || !strings.Contains(out, "-const Tries = 1") || !strings.Contains(out, "+const Tries = 3") {
		t.Fatalf("diff: %v %q", err, out)
	}
	if out, _, err = tool.Handler(context.Background(), map[string]any{"command": "status"}, tc); err != nil || !strings.Contains(out, "f.go") {
		t.Fatalf("status: %v %q", err, out)
	}
	if out, _, err = tool.Handler(context.Background(), map[string]any{"command": "log"}, tc); err != nil || !strings.Contains(out, "first") {
		t.Fatalf("log: %v %q", err, out)
	}
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("the repository's own config ran a program")
	}
	if _, _, err = tool.Handler(context.Background(), map[string]any{"command": "show", "ref": "--output=/tmp/x"}, tc); err == nil {
		t.Fatal("an option passed as a ref was accepted")
	}
}
