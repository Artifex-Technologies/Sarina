package tools

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

func TestDependencyCyclesAndSelfDependenciesAreRefused(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "a", "task_id": "a"})
	e.ok("task_create", map[string]any{"title": "b", "task_id": "b", "blocked_by": []any{"a"}})
	e.ok("task_create", map[string]any{"title": "c", "task_id": "c", "blocked_by": []any{"b"}})
	file := filepath.Join(e.root, ".port_sessions", taskFileName)
	before, _ := os.ReadFile(file)

	r := e.fails("task_update", map[string]any{"task_id": "a", "add_blocked_by": []any{"c"}}, "tool_execution_error")
	if !strings.Contains(r.Content, "dependency cycle a -> c -> b -> a") {
		t.Fatalf("the error must name the cycle: %q", r.Content)
	}
	r = e.fails("task_update", map[string]any{"task_id": "a", "blocked_by": []any{"b"}}, "tool_execution_error")
	if !strings.Contains(r.Content, "a -> b -> a") {
		t.Fatalf("blocked_by: %q", r.Content)
	}
	r = e.fails("task_update", map[string]any{"task_id": "a", "add_blocked_by": []any{"a"}}, "tool_execution_error")
	if !strings.Contains(r.Content, "cannot depend on itself") {
		t.Fatalf("self dependency: %q", r.Content)
	}
	e.fails("task_create", map[string]any{"title": "z", "task_id": "z", "blocked_by": []any{"z"}}, "tool_execution_error")
	e.fails("todo_write", map[string]any{"items": []any{
		map[string]any{"title": "p", "task_id": "p", "blocked_by": []any{"q"}},
		map[string]any{"title": "q", "task_id": "q", "blocked_by": []any{"p"}},
	}}, "tool_execution_error")
	e.fails("update_plan", map[string]any{"items": []any{
		map[string]any{"step": "x", "task_id": "x", "depends_on": []any{"y"}},
		map[string]any{"step": "y", "task_id": "y", "depends_on": []any{"x"}},
	}}, "tool_execution_error")
	if after, _ := os.ReadFile(file); string(after) != string(before) {
		t.Fatal("a refused change altered the store")
	}
	if _, err := os.Stat(filepath.Join(e.root, ".port_sessions", planFileName)); err == nil {
		t.Fatal("a refused plan was saved")
	}
	// a cycle closed through a task that did not exist yet is caught too
	e.ok("task_create", map[string]any{"title": "m", "task_id": "m", "blocked_by": []any{"n"}})
	e.fails("task_create", map[string]any{"title": "n", "task_id": "n", "blocked_by": []any{"m"}}, "tool_execution_error")
}

func TestUnknownDependenciesAreSurfacedAndDoNotBlock(t *testing.T) {
	e := newEnv(t, writeAll())
	r := e.ok("task_create", map[string]any{"title": "a", "task_id": "a", "blocked_by": []any{"ghost"}})
	if !strings.Contains(r.Content, "depends on unknown task ghost") {
		t.Fatalf("create output: %q", r.Content)
	}
	if r = e.ok("task_list", nil); !strings.Contains(r.Content, "depends on unknown task ghost (ignored, not blocking)") {
		t.Fatalf("list: %q", r.Content)
	}
	if r = e.ok("task_list", map[string]any{"actionable_only": true}); !strings.Contains(r.Content, "- a") {
		t.Fatalf("an unknown dependency must not block: %q", r.Content)
	}
	r = e.ok("task_update", map[string]any{"task_id": "a", "status": "in_progress"})
	if taskByID(t, e, "a").Status != "in_progress" || !strings.Contains(r.Content, "depends on unknown task ghost") {
		t.Fatalf("start: %q %+v", r.Content, taskByID(t, e, "a"))
	}
	if !strings.Contains(taskUpdateSpecDescription(), "unknown dependency id") {
		t.Fatal("the rule must be documented in the task_update description")
	}
}

func taskUpdateSpecDescription() string { return taskUpdate().Spec.Description }

func TestCancelledDependencyKeepsDependantsWaitingAndSaysSo(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "a", "task_id": "a"})
	e.ok("task_create", map[string]any{"title": "b", "task_id": "b", "blocked_by": []any{"a"}})
	e.ok("task_update", map[string]any{"task_id": "b", "status": "in_progress"})
	r := e.ok("task_update", map[string]any{"task_id": "a", "status": "cancelled", "reason": "dropped"})
	if !strings.Contains(r.Content, "Still waiting on this cancelled task: b") || !strings.Contains(r.Content, "remove_blocked_by") {
		t.Fatalf("cancel output: %q", r.Content)
	}
	if b := taskByID(t, e, "b"); b.Status != "blocked" || b.Metadata["blocked_reason"] != "waiting_on:a" {
		t.Fatalf("b must stay blocked, not be silently released: %+v", b)
	}
	if r = e.ok("task_list", nil); !strings.Contains(r.Content, "waiting_on: a (cancelled - remove or replace it)") {
		t.Fatalf("list: %q", r.Content)
	}
	if r = e.ok("task_list", map[string]any{"task_id": "b"}); !strings.Contains(r.Content, "waiting_on: a (cancelled - remove or replace it)") {
		t.Fatalf("detail: %q", r.Content)
	}
	// replacing the dependency: the block was only the dependency, so it lifts
	e.ok("task_create", map[string]any{"title": "a2", "task_id": "a2", "status": "completed"})
	e.ok("task_update", map[string]any{"task_id": "b", "remove_blocked_by": []any{"a"}, "add_blocked_by": []any{"a2"}})
	if b := taskByID(t, e, "b"); b.Status != "pending" || b.Metadata["blocked_reason"] != nil {
		t.Fatalf("after replacing the dependency: %+v", b)
	}
}

func TestTaskListExplainsWhyATaskIsBlocked(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "dep", "task_id": "d", "status": "in_progress"})
	e.ok("task_create", map[string]any{"title": "waits", "task_id": "w", "blocked_by": []any{"d"}, "status": "in_progress"})
	e.ok("task_create", map[string]any{"title": "manual", "task_id": "m"})
	e.ok("task_update", map[string]any{"task_id": "m", "status": "blocked", "reason": "needs a decision"})
	r := e.ok("task_list", nil)
	if !strings.Contains(r.Content, "waiting_on: d (in_progress)") || !strings.Contains(r.Content, "blocked_reason: needs a decision") {
		t.Fatalf("list: %q", r.Content)
	}
	if !strings.Contains(e.ok("task_list", map[string]any{"task_id": "m"}).Content, "blocked_reason: needs a decision") {
		t.Fatal("detail view lacks the manual reason")
	}
}

const legacyDependencyFile = `{"tasks": [
 {"task_id": "t0", "title": "root", "status": "pending", "blocks": [], "blocked_by": [], "metadata": {}},
 {"task_id": "t1", "title": "started too early", "status": "in_progress", "blocked_by": ["t0"], "metadata": {}},
 {"task_id": "t2", "title": "dependency done", "status": "blocked", "blocked_by": ["t3"], "metadata": {"blocked_reason": "waiting_on:t3"}},
 {"task_id": "t3", "title": "finished", "status": "completed", "metadata": {}},
 {"task_id": "t4", "title": "manual", "status": "blocked", "blocked_by": [], "metadata": {"blocked_reason": "needs a decision"}},
 {"task_id": "t5", "title": "stale list", "status": "blocked", "blocked_by": ["t0", "t9"], "metadata": {"blocked_reason": "waiting_on:t0,t9,t3"}},
 {"task_id": "t6", "title": "only unknown", "status": "in_progress", "blocked_by": ["ghost"], "metadata": {}},
 {"task_id": "t7", "title": "stuck on a ghost", "status": "blocked", "blocked_by": ["ghost"], "metadata": {"blocked_reason": "waiting_on:ghost"}}
]}`

func TestLoadNormalisesALegacyFileOnceAndIdempotently(t *testing.T) {
	e := newEnv(t, writeAll())
	file := e.put(".port_sessions/"+taskFileName, legacyDependencyFile)
	e.ok("task_list", nil) // the first load normalises and persists
	want := map[string][2]string{ // id -> status, blocked_reason
		"t0": {"pending", ""}, "t1": {"blocked", "waiting_on:t0"}, "t2": {"pending", ""}, "t3": {"completed", ""},
		"t4": {"blocked", "needs a decision"}, "t5": {"blocked", "waiting_on:t0"}, "t6": {"in_progress", ""}, "t7": {"pending", ""},
	}
	for id, w := range want {
		got := taskByID(t, e, id)
		reason, _ := got.Metadata["blocked_reason"].(string)
		if got.Status != w[0] || reason != w[1] {
			t.Errorf("%s = %s %q, want %s %q", id, got.Status, reason, w[0], w[1])
		}
	}
	// second pass: nothing changes and the file is not rewritten at all
	normalized, _ := os.ReadFile(file)
	old := time.Now().Add(-48 * time.Hour)
	os.Chtimes(file, old, old)
	e.ok("task_list", nil)
	e.ok("task_list", map[string]any{"actionable_only": true})
	again, _ := os.ReadFile(file)
	st, _ := os.Stat(file)
	if string(again) != string(normalized) || st.ModTime().After(old.Add(time.Minute)) {
		t.Fatalf("an already-normal file was rewritten (mtime %v)", st.ModTime())
	}
	ts := loadTasks(file)
	if normalizeTasks(ts) {
		t.Fatal("normalisation is not idempotent")
	}
}

func TestLoadDoesNotWriteWithoutWritePermission(t *testing.T) {
	e := newEnv(t, config.Permissions{})
	file := e.put(".port_sessions/"+taskFileName, legacyDependencyFile)
	r := e.ok("task_list", map[string]any{"task_id": "t1"})
	if !strings.Contains(r.Content, "status: blocked") || !strings.Contains(r.Content, "waiting_on: t0 (pending)") {
		t.Fatalf("the view is normalised in memory: %q", r.Content)
	}
	if got, _ := os.ReadFile(file); string(got) != legacyDependencyFile {
		t.Fatal("a read-only session rewrote the file")
	}
}

func TestLegacyFileKeepsItsFormatAfterNormalisation(t *testing.T) {
	e := newEnv(t, writeAll())
	file := e.put(".port_sessions/"+taskFileName, legacyDependencyFile)
	e.ok("task_list", nil)
	raw, _ := os.ReadFile(file)
	for _, k := range []string{`"task_id"`, `"blocked_by"`, `"blocks"`, `"metadata"`, `"created_at"`, `"updated_at"`, `"active_form"`} {
		if !strings.Contains(string(raw), k) {
			t.Errorf("record key %s missing", k)
		}
	}
	if strings.Contains(string(raw), `"waiting_on"`) {
		t.Fatal("waiting_on must stay inside metadata.blocked_reason, as before")
	}
}

func TestUsageGuidesInsteadOfErrors(t *testing.T) {
	e := newEnv(t, writeAll())
	// update_plan with no plan yet
	r := e.ok("update_plan", map[string]any{})
	if !strings.Contains(r.Content, "Current plan: none.") || !strings.Contains(r.Content, "items") {
		t.Fatalf("guide: %q", r.Content)
	}
	if _, err := os.Stat(filepath.Join(e.root, ".port_sessions", planFileName)); err == nil {
		t.Fatal("the guide created a plan file")
	}
	// clear:false with nothing else is also a guide; an empty items list stays a valid replace
	if r = e.ok("update_plan", map[string]any{"clear": false}); !strings.Contains(r.Content, "Two modes:") {
		t.Fatalf("clear false: %q", r.Content)
	}
	if r = e.ok("update_plan", map[string]any{"items": []any{}}); r.Content != "The plan now has 0 steps." {
		t.Fatalf("empty items: %q", r.Content)
	}

	// task_update with only a task_id
	e.ok("task_create", map[string]any{"title": "thing", "task_id": "t", "priority": "high"})
	file := filepath.Join(e.root, ".port_sessions", taskFileName)
	before, _ := os.ReadFile(file)
	r = e.ok("task_update", map[string]any{"task_id": "t"})
	for _, want := range []string{"nothing was changed", "add_blocked_by", `{"task_id":"t1","status":"in_progress"}`, "id: t", "status: pending", "title: thing"} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("task_update guide lacks %q:\n%s", want, r.Content)
		}
	}
	if after, _ := os.ReadFile(file); string(after) != string(before) {
		t.Fatal("the guide rewrote the task file")
	}
	e.fails("task_update", map[string]any{"task_id": "nope"}, "tool_execution_error") // an unknown id is still an error
	// any change field means a real update, even an empty-string one
	e.ok("task_update", map[string]any{"task_id": "t", "owner": ""})
	if strings.Contains(e.ok("task_update", map[string]any{"task_id": "t", "priority": "low"}).Content, "nothing was changed") {
		t.Fatal("a field was given")
	}
}
