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
