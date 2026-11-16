package vault

import (
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// Write never goes through a link or waits on a named pipe planted in the vault: a link under the note's name had its
// target's contents copied into the vault by an append (a key file, say), a link under the note's ".tmp" name had the
// note written wherever it pointed, outside the vault too, and a named pipe under the note's name held the write for ever.
func TestWriteRefusesPlantedLinksAndPipes(t *testing.T) {
	root, outside := t.TempDir(), t.TempDir()
	v, err := Open(root)
	if err != nil {
		t.Fatal(err)
	}
	secret := filepath.Join(outside, "id_ed25519")
	if err := os.WriteFile(secret, []byte("PRIVATE KEY MATERIAL\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(secret, filepath.Join(v.Root, "Leak.md")); err != nil {
		t.Fatal(err)
	}
	if _, err := v.Write(nil, "Leak", "more", Append); err == nil || !strings.Contains(err.Error(), "not a regular file") {
		t.Fatalf("append through a link: %v", err)
	}
	if b, _ := os.ReadFile(secret); string(b) != "PRIVATE KEY MATERIAL\n" {
		t.Fatalf("the link's target changed: %q", b)
	}
	if _, err := os.Lstat(filepath.Join(v.Root, "Leak.md")); err != nil {
		t.Fatal(err)
	}
	planted := filepath.Join(outside, "planted")
	if err := os.Symlink(planted, filepath.Join(v.Root, "New.md.tmp")); err != nil {
		t.Fatal(err)
	}
	if _, err := v.Write(nil, "New", "a new note", Create); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(planted); !os.IsNotExist(err) {
		t.Fatalf("the note was written through the .tmp link: %v", err)
	}
	if b, _ := os.ReadFile(filepath.Join(v.Root, "New.md")); !strings.Contains(string(b), "a new note") {
		t.Fatalf("New.md: %q", b)
	}
	if err := syscall.Mkfifo(filepath.Join(v.Root, "Pipe.md"), 0o644); err != nil {
		t.Fatal(err)
	}
	done := make(chan error, 1)
	go func() { _, err := v.Write(nil, "Pipe", "x", Append); done <- err }()
	select {
	case err := <-done:
		if err == nil || !strings.Contains(err.Error(), "not a regular file") {
			t.Fatalf("append to a pipe: %v", err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("Write waited on a named pipe")
	}
}
