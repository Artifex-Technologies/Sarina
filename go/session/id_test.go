package session

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// An id that cannot name a file is refused by Save with a clear error, before the folder or any file is made.
func TestSaveRefusesIDsThatCannotNameAFile(t *testing.T) {
	for _, id := range []string{"a/b", "../escape", "/abs", "x\x00y", ""} {
		base := t.TempDir()
		dir := filepath.Join(base, "new")
		s := sample(t)
		s.ID = id
		_, err := s.Save(dir)
		if err == nil || !strings.Contains(err.Error(), "session id") {
			t.Fatalf("%q: %v", id, err)
		}
		if entries, _ := os.ReadDir(base); len(entries) != 0 {
			t.Fatalf("%q: something was written: %v", id, entries)
		}
	}
	for _, id := range []string{"..", ".", "a\\b", "x.json", " sp ace"} {
		s := sample(t)
		s.ID = id
		if _, err := s.Save(t.TempDir()); err != nil {
			t.Fatalf("%q must still save: %v", id, err)
		}
	}
}

// A session that needs migrating but cannot be saved is reported and left alone; the files after it are still migrated,
// on every run, and the dry run reports it the same way.
func TestMigrateDirSkipsAFileItCannotSaveAndCarriesOn(t *testing.T) {
	dir := t.TempDir()
	good := writeRaw(t, dir, retiredSession(t))
	bad := retiredSession(t)
	bad.ID = "x/y"
	data, _ := bad.Marshal()
	badPath := filepath.Join(dir, "0-bad.json") // sorts before the good file
	os.WriteFile(badPath, data, 0o600)
	for _, dry := range []bool{true, false, false} {
		r, err := MigrateDir(dir, dry)
		if err != nil {
			t.Fatalf("dry=%v: %v", dry, err)
		}
		if len(r.Failed) != 1 || !strings.HasPrefix(r.Failed[0], "0-bad.json: session id") {
			t.Fatalf("dry=%v: %+v", dry, r)
		}
		if _, err := os.Stat(badPath + ".bak"); err == nil {
			t.Fatal("a backup was written for a file that is not migrated")
		}
		if now, _ := os.ReadFile(badPath); string(now) != string(data) {
			t.Fatal("the unsaveable file was changed")
		}
		if dry && r.Changed != 1 {
			t.Fatalf("dry run: %+v", r)
		}
	}
	if b, _ := os.ReadFile(good); strings.Contains(string(b), "task_get") {
		t.Fatal("the file after the bad one was not migrated")
	}
}
