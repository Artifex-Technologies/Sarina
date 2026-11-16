package stego

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The cleaned copy is never written through a link planted under its name: a link to a file that does not exist yet
// made stego_clean (and `sarina stego clean`) create that file wherever the link pointed, outside the workspace too.
func TestCleanDoesNotWriteThroughALink(t *testing.T) {
	dir, elsewhere := t.TempDir(), t.TempDir()
	orig := filepath.Join(dir, "a.txt")
	if err := os.WriteFile(orig, []byte("zero​width\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(elsewhere, "planted")
	if err := os.Symlink(target, filepath.Join(dir, "a.clean.txt")); err != nil {
		t.Fatal(err)
	}
	out, _, err := Clean([]byte("zero​width\n"), orig)
	if err == nil || !strings.Contains(err.Error(), "a.clean.txt already exists: I do not overwrite it") || out != "" {
		t.Fatalf("Clean = %q, %v", out, err)
	}
	if _, err := os.Lstat(target); !os.IsNotExist(err) {
		t.Fatalf("the link's target was written: %v", err)
	}
	// a copy that does not exist yet is still written
	if err := os.Remove(filepath.Join(dir, "a.clean.txt")); err != nil {
		t.Fatal(err)
	}
	out, _, err = Clean([]byte("zero​width\n"), orig)
	if err != nil || out != filepath.Join(dir, "a.clean.txt") {
		t.Fatalf("Clean = %q, %v", out, err)
	}
	if b, _ := os.ReadFile(out); string(b) != "zerowidth\n" {
		t.Fatalf("copy %q", b)
	}
}
