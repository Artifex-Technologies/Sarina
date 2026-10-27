package tools

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestPrivateFoldersAreOutOfReach(t *testing.T) {
	e := newEnv(t, writeAll())
	e.put("book/chapter1.md", "secret story text")
	e.put("notes/todo.md", "public note")
	t.Setenv("SARINA_PRIVATE_DIRS", filepath.Join(e.root, "book"))
	e.fails("read_file", map[string]any{"path": "book/chapter1.md"}, "tool_execution_error")
	e.fails("list_dir", map[string]any{"path": "book"}, "tool_execution_error")
	e.fails("write_file", map[string]any{"path": "book/new.md", "content": "x"}, "tool_execution_error")
	r := e.ok("grep_search", map[string]any{"pattern": "story|public"})
	if strings.Contains(r.Content, "chapter1") || !strings.Contains(r.Content, "todo.md") {
		t.Fatalf("grep: %s", r.Content)
	}
	g := e.ok("glob_search", map[string]any{"pattern": "**/*.md"})
	if strings.Contains(g.Content, "chapter1") {
		t.Fatalf("glob: %s", g.Content)
	}
	if _, err := os.Stat(filepath.Join(e.root, "book", "new.md")); err == nil {
		t.Fatal("wrote into a private folder")
	}
}

// Credential stores and browser profiles are private even when the owner listed nothing.
func TestCredentialStoresArePrivateWithoutConfiguration(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("SARINA_PRIVATE_DIRS", "")
	for _, d := range []string{".ssh", ".config/vivaldi", ".config/Proton Pass", ".config/rclone", "notes"} {
		os.MkdirAll(filepath.Join(home, d), 0o700)
	}
	os.WriteFile(filepath.Join(home, ".claude.json"), []byte("{}"), 0o600)
	for _, p := range []string{".ssh/id_x", ".config/vivaldi/Default/Login Data", ".config/Proton Pass/x", ".config/rclone/rclone.conf", ".claude.json"} {
		if !IsPrivate(filepath.Join(home, p)) {
			t.Errorf("%s is readable", p)
		}
	}
	if IsPrivate(filepath.Join(home, "notes", "todo.md")) || IsPrivate(filepath.Join(home, ".claude.json.txt")) {
		t.Error("an ordinary file is private")
	}
}
