package testguard

import (
	"os"
	"path/filepath"
	"testing"
)

func TestOnlyTheTestsFoldersTakeWritesUnderTest(t *testing.T) {
	root := t.TempDir()
	home := t.TempDir()
	t.Setenv("TMPDIR", t.TempDir()) // a temporary folder of its own: neither the test root nor the "home" below
	t.Setenv(RootEnv, filepath.Join(root, "work"))
	t.Setenv(Env, "")
	if Allow("/home/someone/Documents/Obsidian/Sarina/Daily/x.md") != nil {
		t.Fatal("outside a test run everything is allowed")
	}
	t.Setenv(Env, "1")
	if err := Allow(filepath.Join(root, "work", "vault", "Daily", "2026-11-07.md")); err != nil {
		t.Fatal(err)
	}
	if Allow(filepath.Join(home, "Documents", "Obsidian", "Sarina", "Jokes", "2026-11.md")) == nil {
		t.Fatal("the real vault must refuse under test")
	}
	// a link inside the test folder that leads out of it is followed
	os.MkdirAll(filepath.Join(root, "work"), 0o755)
	os.Symlink(home, filepath.Join(root, "work", "escape"))
	if Allow(filepath.Join(root, "work", "escape", "note.md")) == nil {
		t.Fatal("a link out of the test folder must refuse")
	}
	if Allow(filepath.Join(root, "work", "..", "work2", "x")) == nil {
		t.Fatal("a sibling folder is not the test folder")
	}
}
