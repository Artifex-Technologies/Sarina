package cli

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

var retiredNames = []string{"task_get", "task_next", "task_start", "task_complete", "task_cancel", "task_block", "plan_clear"}

func retiredSessionFor(h *harness, url string) *session.Session {
	s := &session.Session{ID: session.NewID(), Model: config.ModelConfig{Model: "m", BaseURL: url, TimeoutSeconds: 30}, Runtime: config.DefaultRuntime(h.dir)}
	s.Append(session.Message{Role: "user", Content: "start"})
	for i, name := range retiredNames {
		id := fmt.Sprintf("c%d", i)
		s.Append(session.Message{Role: "assistant", ToolCalls: []llm.WireToolCall{{ID: id, Type: "function", Function: llm.FunctionCall{Name: name, Arguments: `{"task_id":"a"}`}}}})
		s.Append(session.Message{Role: "tool", Name: name, ToolCallID: id, Content: session.ToolResultBody(name, true, "fine", nil)})
	}
	return s
}

func TestResumeNeverShowsRetiredToolNames(t *testing.T) {
	b := newBackend(t, "ok")
	h := newHarness(t, strings.NewReader(""))
	dir := filepath.Join(h.dir, ".port_sessions", "agent")
	s := retiredSessionFor(h, b.URL)
	if _, err := s.Save(dir); err != nil {
		t.Fatal(err)
	}
	if code := h.run("resume", "--base-url", b.URL, "--cwd", h.dir, "--allow-write", "--no-auto-memory-review", s.ID, "go on"); code != 0 {
		t.Fatalf("code=%d err=%s", code, h.errb.String())
	}
	req, _ := json.Marshal(b.reqs[0])
	for _, old := range retiredNames {
		if strings.Contains(string(req), old) {
			t.Errorf("the model request still contains %q", old)
		}
	}
	if !strings.Contains(string(req), "task_update") {
		t.Fatalf("migrated calls missing: %s", req)
	}
	// the next save persists the refreshed history
	saved, _ := os.ReadFile(session.Path(dir, s.ID))
	if strings.Contains(string(saved), "task_complete") {
		t.Fatal("saved file still has a retired name")
	}
}

func TestSessionMigrateCommand(t *testing.T) {
	h := newHarness(t, strings.NewReader(""))
	dir := filepath.Join(h.dir, "sessions")
	s := retiredSessionFor(h, "http://x")
	if _, err := s.Save(dir); err != nil {
		t.Fatal(err)
	}
	path := session.Path(dir, s.ID)
	orig, _ := os.ReadFile(path)

	if code := h.run("session-migrate", "--session-dir", dir, "--dry-run"); code != 0 || !strings.Contains(h.out.String(), "1 files scanned, 1 would change, 7 calls rewritten") {
		t.Fatalf("dry: %d %q", code, h.out.String())
	}
	if now, _ := os.ReadFile(path); string(now) != string(orig) {
		t.Fatal("dry run changed the file")
	}
	h.out.Reset()
	if code := h.run("session-migrate", "--session-dir", dir); code != 0 || !strings.Contains(h.out.String(), "1 changed, 7 calls rewritten") {
		t.Fatalf("real: %d %q", code, h.out.String())
	}
	now, _ := os.ReadFile(path)
	for _, old := range retiredNames[:6] {
		if strings.Contains(string(now), old) {
			t.Fatalf("%s not migrated", old)
		}
	}
	if bak, err := os.ReadFile(path + ".bak"); err != nil || string(bak) != string(orig) {
		t.Fatal("backup missing or wrong")
	}
}
