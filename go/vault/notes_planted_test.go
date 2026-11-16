package vault

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// Notes reads notes as bounded regular files inside the vault: a named pipe planted under a note's name held every search,
// recall and lint for ever, and a link named like a note put the file it named, anywhere on the computer, into her search
// results (a key file, say). A link to a note inside the vault is still read.
func TestNotesSkipsPlantedPipesAndLinksOut(t *testing.T) {
	root, outside := t.TempDir(), t.TempDir()
	write := func(p, s string) {
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(s), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write(filepath.Join(root, "Home.md"), "# Home\n")
	write(filepath.Join(root, "Lessons", "Real.md"), "# Real\nthe real lesson\n")
	write(filepath.Join(outside, "id_ed25519"), "PRIVATE KEY MATERIAL\n")
	if err := os.Symlink(filepath.Join(outside, "id_ed25519"), filepath.Join(root, "Leak.md")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(filepath.Join(root, "Lessons", "Real.md"), filepath.Join(root, "Alias.md")); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(filepath.Join(root, "Pipe.md"), 0o644); err != nil {
		t.Fatal(err)
	}
	v, err := Open(root)
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan []Note, 1)
	go func() { ns, _ := v.Notes(); done <- ns }()
	var notes []Note
	select {
	case notes = <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("Notes waited on a named pipe")
	}
	var paths []string
	for _, n := range notes {
		paths = append(paths, n.Path)
		if strings.Contains(n.Body, "PRIVATE KEY") {
			t.Fatalf("a link out of the vault was read: %s", n.Path)
		}
	}
	got := strings.Join(paths, ",")
	for _, want := range []string{"Home.md", "Lessons/Real.md", "Alias.md"} {
		if !strings.Contains(got, want) {
			t.Fatalf("notes %s: %s missing", got, want)
		}
	}
	if strings.Contains(got, "Pipe.md") || strings.Contains(got, "Leak.md") {
		t.Fatalf("notes %s", got)
	}
}
