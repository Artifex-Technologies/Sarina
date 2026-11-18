package insight

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// What reflect, vault-ingest and vault-tidy read in the vault and in the state folder: only regular files and folders
// of their own. Before, a named pipe planted in Talks/, at the About note or at a state file held the command for ever;
// a link planted there had a file anywhere on the computer read (and, under Talks/, rewritten by vault-tidy), and the
// state written through it.

type countingModel struct{ calls *int32 }

func (m countingModel) Complete(context.Context, llm.Request, func(llm.StreamEvent)) (*llm.Turn, error) {
	atomic.AddInt32(m.calls, 1)
	return &llm.Turn{Content: `{"title": "A note", "points": ["One point."], "matters": "It does.", "observation": "They have a cat", "quote": "I have a cat named Pixel", "kind": "fact"}`}, nil
}

func plantedEnv(out, errb *bytes.Buffer, calls *int32) *cli.Env {
	return &cli.Env{Stdout: out, Stderr: errb, Stdin: strings.NewReader(""), Getenv: func(string) string { return "" },
		NewClient: func(config.ModelConfig) agent.Completer { return countingModel{calls} }}
}

func within(t *testing.T, what string, fn func() int) int {
	t.Helper()
	done := make(chan int, 1)
	go func() { done <- fn() }()
	select {
	case rc := <-done:
		return rc
	case <-time.After(10 * time.Second):
		t.Fatalf("%s waited on a named pipe", what)
		return -1
	}
}

func mkfifo(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(path, 0o644); err != nil {
		t.Skip(err)
	}
}

func write(t *testing.T, path, text string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

func symlink(t *testing.T, target, path string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(target, path); err != nil {
		t.Fatal(err)
	}
}

const catLine = "- 2020-01-01 10:00 | they said: I have a cat named Pixel | I said: Lovely.\n"

func TestReflectSkipsPlantedLinksAndPipesInTalks(t *testing.T) {
	vault, outside, state := t.TempDir(), t.TempDir(), t.TempDir()
	write(t, filepath.Join(outside, "theirs.md"), catLine)
	symlink(t, filepath.Join(outside, "theirs.md"), filepath.Join(vault, "Talks", "2020-01.md"))
	mkfifo(t, filepath.Join(vault, "Talks", "2020-02.md"))
	var out, errb bytes.Buffer
	var calls int32
	rc := within(t, "reflect", func() int {
		return reflectCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "r.json"), "--force"})
	})
	if rc != 0 || !strings.Contains(out.String(), "nothing new to reflect on") || calls != 0 {
		t.Fatalf("rc %d, %d model calls, out %q, err %q", rc, calls, out.String(), errb.String())
	}
}

func TestTalksAndInboxFolderLinksAreNotFollowed(t *testing.T) {
	vault, outside, state := t.TempDir(), t.TempDir(), t.TempDir()
	old := "- 2020-01-01 10:00 | they said: hi | I said: Hello.\n" + catLine
	write(t, filepath.Join(outside, "talks", "2020-01.md"), old)
	write(t, filepath.Join(outside, "inbox", "article.md"), "Something to summarise.\n")
	symlink(t, filepath.Join(outside, "talks"), filepath.Join(vault, "Talks"))
	symlink(t, filepath.Join(outside, "inbox"), filepath.Join(vault, "Inbox"))
	var out, errb bytes.Buffer
	var calls int32
	if rc := tidyCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--yes", "--older-than", "0"}); rc != 0 || !strings.Contains(out.String(), "nothing to tidy") {
		t.Fatalf("vault-tidy: rc %d, out %q, err %q", rc, out.String(), errb.String())
	}
	if b, _ := os.ReadFile(filepath.Join(outside, "talks", "2020-01.md")); string(b) != old {
		t.Fatalf("vault-tidy rewrote a file outside the vault: %q", b)
	}
	if lines := ReadTalks(vault, time.Time{}); len(lines) != 0 {
		t.Fatalf("ReadTalks read %d lines through a link in place of Talks/", len(lines))
	}
	if files, _ := NewInboxFiles(vault, map[string]string{}); len(files) != 0 {
		t.Fatalf("NewInboxFiles read %d files through a link in place of Inbox/", len(files))
	}
	out.Reset()
	if rc := ingestCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "i.json"), "--force"}); rc != 0 || calls != 0 {
		t.Fatalf("vault-ingest: rc %d, %d model calls, out %q", rc, calls, out.String())
	}
}

func TestReflectRefusesAPlantedAboutNote(t *testing.T) {
	for _, kind := range []string{"pipe", "link"} {
		t.Run(kind, func(t *testing.T) {
			vault, outside, state := t.TempDir(), t.TempDir(), t.TempDir()
			write(t, filepath.Join(vault, "Talks", "2020-01.md"), catLine)
			note := filepath.Join(vault, filepath.FromSlash(NoteName()))
			theirs := filepath.Join(outside, "secret.md")
			write(t, theirs, "## Observations\n\n- a line from outside the vault\n")
			if kind == "pipe" {
				mkfifo(t, note)
			} else {
				symlink(t, theirs, note)
			}
			var out, errb bytes.Buffer
			var calls int32
			rc := within(t, "reflect", func() int {
				return reflectCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "r.json"), "--force"})
			})
			if rc != 1 || calls != 0 || !strings.Contains(errb.String(), "is not a regular file") {
				t.Fatalf("rc %d, %d model calls, out %q, err %q", rc, calls, out.String(), errb.String())
			}
			if _, err := os.Stat(filepath.Join(state, "r.json")); err == nil {
				t.Fatal("the state moved on although nothing was learned")
			}
		})
	}
}

func TestStateFilesAreNotWaitedOnOrWrittenThroughLinks(t *testing.T) {
	vault, outside, state := t.TempDir(), t.TempDir(), t.TempDir()
	write(t, filepath.Join(vault, "Talks", "2020-01.md"), "- 2020-01-01 10:00 | they said: i'm here whenever you need me | I said: Okay.\n")
	var out, errb bytes.Buffer
	var calls int32
	// a named pipe at each state file's name
	mkfifo(t, filepath.Join(state, "pipe-r.json"))
	mkfifo(t, filepath.Join(state, "pipe-i.json"))
	if rc := within(t, "reflect", func() int {
		return reflectCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "pipe-r.json"), "--force"})
	}); rc != 0 {
		t.Fatalf("reflect: rc %d, out %q, err %q", rc, out.String(), errb.String())
	}
	if rc := within(t, "vault-ingest", func() int {
		return ingestCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "pipe-i.json"), "--force"})
	}); rc != 0 {
		t.Fatalf("vault-ingest: rc %d, out %q, err %q", rc, out.String(), errb.String())
	}
	// a link at the state file's name: the state is written in its place, not into the file it names
	theirs := filepath.Join(outside, "theirs.txt")
	write(t, theirs, "theirs\n")
	symlink(t, theirs, filepath.Join(state, "link-r.json"))
	if rc := reflectCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "link-r.json"), "--force"}); rc != 0 {
		t.Fatalf("reflect: rc %d, err %q", rc, errb.String())
	}
	if b, _ := os.ReadFile(theirs); string(b) != "theirs\n" {
		t.Fatalf("the state was written through a planted link: %q", b)
	}
	if st, err := os.Lstat(filepath.Join(state, "link-r.json")); err != nil || !st.Mode().IsRegular() {
		t.Fatalf("the state file is not a regular file of its own: %v %v", st, err)
	}
	write(t, filepath.Join(vault, "Inbox", "article.md"), "Something to summarise.\n")
	symlink(t, theirs, filepath.Join(state, "link-i.json"))
	if rc := ingestCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "link-i.json"), "--force"}); rc != 0 {
		t.Fatalf("vault-ingest: rc %d, err %q", rc, errb.String())
	}
	if b, _ := os.ReadFile(theirs); string(b) != "theirs\n" {
		t.Fatalf("the ingest state was written through a planted link: %q", b)
	}
}

// A link planted under a Sources/ note's name (pointing nowhere, so Stat said the name was free) made every ingest of a
// file with that title fail: the name is taken by whatever is there.
func TestFreeNameCountsAPlantedLinkAsTaken(t *testing.T) {
	vault := t.TempDir()
	symlink(t, "/nonexistent-sarina-test/x", filepath.Join(vault, "Sources", "A note.md"))
	if got := FreeName(vault, "A note"); got != "Sources/A note (2).md" {
		t.Fatalf("FreeName = %q", got)
	}
}

// A vault whose path has a glob's special characters ("Notes [old]") had no talk log: the folder's path was used as a
// pattern.
func TestReadTalksInAVaultWithBrackets(t *testing.T) {
	vault := filepath.Join(t.TempDir(), "Notes [old]")
	write(t, filepath.Join(vault, "Talks", "2020-01.md"), catLine)
	if lines := ReadTalks(vault, time.Time{}); len(lines) != 1 {
		t.Fatalf("ReadTalks read %d lines", len(lines))
	}
}
