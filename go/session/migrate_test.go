package session

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

type retiredCase struct {
	name, args, wantName, wantArgs string
}

var retiredCases = []retiredCase{
	{"task_get", `{"task_id":"a"}`, "task_list", `{"task_id":"a"}`},
	{"task_get", `{"id":"b"}`, "task_list", `{"task_id":"b"}`},
	{"task_next", `{"max_tasks":3}`, "task_list", `{"actionable_only":true,"max_tasks":3}`},
	{"task_start", `{"task_id":"a","owner":"bob","active_form":"Doing"}`, "task_update", `{"active_form":"Doing","owner":"bob","status":"in_progress","task_id":"a"}`},
	{"task_complete", `{"task_id":"a"}`, "task_update", `{"status":"completed","task_id":"a"}`},
	{"task_cancel", `{"task_id":"a","reason":"dropped"}`, "task_update", `{"reason":"dropped","status":"cancelled","task_id":"a"}`},
	{"task_block", `{"task_id":"a","reason":"needs input"}`, "task_update", `{"reason":"needs input","status":"blocked","task_id":"a"}`},
	{"task_block", `{"task_id":"a","blocked_by":["x","y"]}`, "task_update", `{"add_blocked_by":["x","y"],"status":"blocked","task_id":"a"}`},
	{"plan_clear", `{}`, "update_plan", `{"clear":true}`},
	{"plan_clear", `{"sync_tasks":true}`, "update_plan", `{"clear":true,"prune_tasks":true}`},
	{"plan_clear", `{"sync_tasks":false}`, "update_plan", `{"clear":true}`},
}

func retiredSession(t *testing.T) *Session {
	s := sample(t)
	s.Messages = s.Messages[:2]
	for i, c := range retiredCases {
		id := "r" + string(rune('a'+i))
		s.Append(Message{Role: "assistant", MessageID: "a", ToolCalls: []llm.WireToolCall{{ID: id, Type: "function", Function: llm.FunctionCall{Name: c.name, Arguments: c.args}}}})
		s.Append(Message{Role: "tool", Name: c.name, ToolCallID: id, Content: ToolResultBody(c.name, true, "done", nil),
			Metadata: map[string]any{"action": c.name, "tool_name": c.name, "task_id": "a"}})
	}
	return s
}

func TestMigrateRewritesEveryRetiredName(t *testing.T) {
	s := retiredSession(t)
	m := s.MigrateRetiredTools()
	if m.Calls != len(retiredCases) || m.Results != len(retiredCases) {
		t.Fatalf("counts: %+v", m)
	}
	for i, c := range retiredCases {
		call := s.Messages[2+2*i].ToolCalls[0].Function
		res := s.Messages[3+2*i]
		if call.Name != c.wantName || call.Arguments != c.wantArgs {
			t.Errorf("%s %s -> %s %s, want %s %s", c.name, c.args, call.Name, call.Arguments, c.wantName, c.wantArgs)
		}
		if res.Name != c.wantName || !strings.Contains(res.Content, "\"tool\": \""+c.wantName+"\"") || strings.Contains(res.Content, "\""+c.name+"\"") {
			t.Errorf("result %d not renamed: %q %q", i, res.Name, res.Content)
		}
		if res.Metadata["tool_name"] != c.wantName {
			t.Errorf("metadata tool_name %v", res.Metadata["tool_name"])
		}
		wantAction := c.wantName
		if c.name == "plan_clear" {
			wantAction = "plan_clear" // the current tool still records clears under this action
		}
		if res.Metadata["action"] != wantAction || res.Metadata["task_id"] != "a" {
			t.Errorf("metadata action %v", res.Metadata["action"])
		}
	}
	if again := s.MigrateRetiredTools(); again.Changed() {
		t.Fatalf("second pass changed things: %+v", again)
	}
}

func TestMigrateLeavesUnknownAndUnparseableAlone(t *testing.T) {
	s := sample(t)
	s.Append(Message{Role: "assistant", ToolCalls: []llm.WireToolCall{{ID: "bad", Type: "function", Function: llm.FunctionCall{Name: "task_get", Arguments: "{not json"}}}})
	s.Append(Message{Role: "tool", Name: "task_get", ToolCallID: "bad", Content: "x"})
	s.Append(Message{Role: "assistant", ToolCalls: []llm.WireToolCall{{ID: "z", Type: "function", Function: llm.FunctionCall{Name: "frobnicate", Arguments: `{"a":1}`}}}})
	s.Append(Message{Role: "tool", Name: "frobnicate", ToolCallID: "z", Content: "x"})
	before, _ := s.Marshal()
	if m := s.MigrateRetiredTools(); m.Changed() {
		t.Fatalf("nothing is recognisable: %+v", m)
	}
	after, _ := s.Marshal()
	if string(before) != string(after) {
		t.Fatal("untouched session changed")
	}
}

func TestParseMigratesInMemoryAndKeepsShape(t *testing.T) {
	s := retiredSession(t)
	raw, _ := s.Marshal()
	got, err := Parse(raw)
	if err != nil {
		t.Fatal(err)
	}
	out, _ := got.Marshal()
	for _, old := range []string{"task_get", "task_next", "task_start", "task_complete", "task_cancel", "task_block"} {
		if strings.Contains(string(out), old) {
			t.Errorf("%s survived a load", old)
		}
	}
	if strings.Join(topLevelKeys(t, raw), ",") != strings.Join(topLevelKeys(t, out), ",") {
		t.Fatal("top-level keys changed")
	}
	var doc struct {
		Messages []map[string]any `json:"messages"`
	}
	json.Unmarshal(out, &doc)
	last := doc.Messages[len(doc.Messages)-2]
	for _, k := range []string{"role", "content", "tool_calls", "blocks", "message_id"} {
		if _, ok := last[k]; !ok {
			t.Errorf("assistant message lost key %s", k)
		}
	}
}

func writeRaw(t *testing.T, dir string, s *Session) string {
	t.Helper()
	data, _ := s.Marshal()
	p := Path(dir, s.ID)
	if err := os.WriteFile(p, data, 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

func TestMigrateDirDryRunThenReal(t *testing.T) {
	dir := t.TempDir()
	oldPath := writeRaw(t, dir, retiredSession(t))
	cleanPath := writeRaw(t, dir, sample(t))
	os.WriteFile(filepath.Join(dir, "junk.json"), []byte("nope"), 0o600)
	os.WriteFile(filepath.Join(dir, "x.notes.md"), []byte("notes"), 0o600)
	origOld, _ := os.ReadFile(oldPath)
	origClean, _ := os.ReadFile(cleanPath)

	r, err := MigrateDir(dir, true)
	if err != nil || r.Scanned != 2 || r.Changed != 1 || r.Calls != len(retiredCases) || r.Skipped != 1 {
		t.Fatalf("dry run: %+v %v", r, err)
	}
	if now, _ := os.ReadFile(oldPath); string(now) != string(origOld) {
		t.Fatal("dry run wrote the file")
	}
	if _, err := os.Stat(oldPath + ".bak"); err == nil {
		t.Fatal("dry run wrote a backup")
	}

	r, err = MigrateDir(dir, false)
	if err != nil || r.Changed != 1 || r.Calls != len(retiredCases) {
		t.Fatalf("real run: %+v %v", r, err)
	}
	if bak, _ := os.ReadFile(oldPath + ".bak"); string(bak) != string(origOld) {
		t.Fatal("backup is not the original")
	}
	if now, _ := os.ReadFile(cleanPath); string(now) != string(origClean) {
		t.Fatal("an unaffected file was rewritten")
	}
	if _, err := os.Stat(cleanPath + ".bak"); err == nil {
		t.Fatal("backup for an unchanged file")
	}
	if migrated, _ := os.ReadFile(oldPath); strings.Contains(string(migrated), "task_get") {
		t.Fatal("file still has a retired name")
	}
	r, _ = MigrateDir(dir, false)
	if r.Changed != 0 || r.Calls != 0 {
		t.Fatalf("not idempotent: %+v", r)
	}
	if bak, _ := os.ReadFile(oldPath + ".bak"); string(bak) != string(origOld) {
		t.Fatal("backup was overwritten")
	}
	if r, err := MigrateDir(filepath.Join(dir, "missing"), false); err != nil || r.Scanned != 0 {
		t.Fatalf("missing dir: %+v %v", r, err)
	}
}
