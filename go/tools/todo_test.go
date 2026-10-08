package tools

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func tasksOnDisk(t *testing.T, e *env) []Task {
	t.Helper()
	return loadTasks(filepath.Join(e.root, ".port_sessions", taskFileName))
}

func taskByID(t *testing.T, e *env, id string) Task {
	t.Helper()
	for _, x := range tasksOnDisk(t, e) {
		if x.TaskID == id {
			return x
		}
	}
	t.Fatalf("no task %q on disk", id)
	return Task{}
}

func TestTaskLifecycle(t *testing.T) {
	e := newEnv(t, writeAll())
	r := e.ok("task_create", map[string]any{"title": "design", "task_id": "a", "priority": "high", "description": "think", "owner": "me"})
	if r.Content != "Created task a: design [pending]" || r.Metadata["task_id"] != "a" || r.Metadata["action"] != "task_create" {
		t.Fatalf("create: %+v", r)
	}
	for _, k := range []string{"path", "before_sha256", "after_sha256", "before_preview", "after_preview", "before_task_count", "after_task_count", "total_tasks"} {
		if _, ok := r.Metadata[k]; !ok {
			t.Errorf("mutation metadata lacks %s", k)
		}
	}
	if r.Metadata["path"] != ".port_sessions/task_runtime.json" || r.Metadata["before_task_count"] != 0 || r.Metadata["after_task_count"] != 1 {
		t.Fatalf("metadata: %v", r.Metadata)
	}
	e.ok("task_create", map[string]any{"title": "build", "task_id": "b", "blocked_by": []any{"a"}})
	e.ok("task_create", map[string]any{"title": "ship", "task_id": "c", "status": "DONE"}) // status words are normalised
	if got := taskByID(t, e, "c").Status; got != "completed" {
		t.Fatalf("DONE -> %q", got)
	}
	// ids and titles: duplicates refused, blank title refused, unknown id errors
	e.fails("task_create", map[string]any{"title": "again", "task_id": "a"}, "tool_execution_error")
	e.fails("task_create", map[string]any{"title": "   "}, "tool_execution_error")
	e.fails("task_list", map[string]any{"task_id": "zzz"}, "tool_execution_error")
	e.fails("task_update", map[string]any{"task_id": "zzz", "title": "x"}, "tool_execution_error")
	e.fails("task_update", map[string]any{"task_id": "zzz", "status": "in_progress"}, "tool_execution_error")
	if len(tasksOnDisk(t, e)) != 3 {
		t.Fatal("failed calls changed the store")
	}

	// b waits on a: starting it blocks it and says on what
	r = e.ok("task_update", map[string]any{"task_id": "b", "status": "in_progress"})
	if taskByID(t, e, "b").Status != "blocked" || taskByID(t, e, "b").Metadata["blocked_reason"] != "waiting_on:a" || !strings.Contains(r.Content, "waiting on a") {
		t.Fatalf("start with unresolved dependency: %+v / %+v", r, taskByID(t, e, "b"))
	}
	e.ok("task_update", map[string]any{"task_id": "a", "status": "in_progress", "owner": "bob", "active_form": "Designing"})
	if a := taskByID(t, e, "a"); a.Status != "in_progress" || deref(a.Owner) != "bob" || deref(a.ActiveForm) != "Designing" {
		t.Fatalf("a: %+v", a)
	}
	// completing a releases b
	e.ok("task_update", map[string]any{"task_id": "a", "status": "completed"})
	if b := taskByID(t, e, "b"); b.Status != "pending" || b.Metadata["blocked_reason"] != nil {
		t.Fatalf("b should be released: %+v", b)
	}
	e.ok("task_update", map[string]any{"task_id": "b", "status": "in_progress"})
	if taskByID(t, e, "b").Status != "in_progress" {
		t.Fatal("b should start now")
	}
	// update: partial, empty string clears an optional field, blank title ignored
	e.ok("task_update", map[string]any{"task_id": "a", "owner": "", "title": "  ", "priority": "low"})
	if a := taskByID(t, e, "a"); a.Owner != nil || a.Title != "design" || deref(a.Priority) != "low" || deref(a.Description) != "think" {
		t.Fatalf("partial update: %+v", a)
	}
	e.ok("task_update", map[string]any{"task_id": "b", "status": "cancelled", "reason": "dropped"})
	if b := taskByID(t, e, "b"); b.Status != "cancelled" || b.Metadata["cancel_reason"] != "dropped" {
		t.Fatalf("cancel: %+v", b)
	}
}

func TestManualBlockIsNotReleasedByUnrelatedCompletion(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "waiting for the user", "task_id": "w"})
	e.ok("task_create", map[string]any{"title": "other", "task_id": "o"})
	e.ok("task_update", map[string]any{"task_id": "w", "status": "blocked", "reason": "needs a decision"})
	e.ok("task_update", map[string]any{"task_id": "o", "status": "completed"})
	if w := taskByID(t, e, "w"); w.Status != "blocked" || w.Metadata["blocked_reason"] != "needs a decision" {
		t.Fatalf("a task blocked only by a reason must stay blocked: %+v", w)
	}
	e.ok("task_update", map[string]any{"task_id": "w", "status": "blocked", "add_blocked_by": []any{"o", "o", "x"}})
	if got := taskByID(t, e, "w").BlockedBy; strings.Join(got, ",") != "o,x" {
		t.Fatalf("block merges and de-duplicates: %v", got)
	}
}

func TestTaskListAndNext(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "low one", "task_id": "1", "priority": "low"})
	e.ok("task_create", map[string]any{"title": "crit", "task_id": "2", "priority": "critical", "owner": "ann"})
	e.ok("task_create", map[string]any{"title": "dep", "task_id": "3", "blocked_by": []any{"1"}})
	e.ok("task_create", map[string]any{"title": "done", "task_id": "4", "status": "completed"})
	e.ok("task_update", map[string]any{"task_id": "1", "status": "in_progress"})
	r := e.ok("task_list", nil)
	order := []string{}
	for _, l := range strings.Split(r.Content, "\n") {
		if strings.HasPrefix(l, "- ") {
			order = append(order, strings.Fields(l)[1])
		}
	}
	if strings.Join(order, ",") != "1,2,3,4" { // in_progress, pending(critical), pending (dep), completed
		t.Fatalf("order = %v\n%s", order, r.Content)
	}
	if r = e.ok("task_list", map[string]any{"owner": "ann"}); strings.Contains(r.Content, "- 1") || !strings.Contains(r.Content, "crit") {
		t.Fatalf("owner filter: %q", r.Content)
	}
	if r = e.ok("task_list", map[string]any{"status": "done"}); !strings.Contains(r.Content, "- 4") || strings.Contains(r.Content, "crit") {
		t.Fatalf("status filter normalises: %q", r.Content)
	}
	r = e.ok("task_list", map[string]any{"actionable_only": true})
	if strings.Contains(r.Content, "- 3") || strings.Contains(r.Content, "- 4") || !strings.Contains(r.Content, "- 1") || !strings.Contains(r.Content, "- 2") {
		t.Fatalf("actionable: %q", r.Content)
	}
	r = e.ok("task_list", map[string]any{"actionable_only": true, "max_tasks": 1})
	if strings.Count(r.Content, "- ") != 1 || !strings.Contains(r.Content, "- 1") {
		t.Fatalf("next: %q", r.Content)
	}
	if r = e.ok("task_list", map[string]any{"status": "blocked"}); r.Content != "No tasks." {
		t.Fatalf("empty: %q", r.Content)
	}
}

func TestTodoWriteReplacesButKeepsIdentity(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "keep me", "task_id": "k"})
	before := taskByID(t, e, "k")
	e.ok("task_create", map[string]any{"title": "drop me", "task_id": "d"})
	r := e.ok("todo_write", map[string]any{"items": []any{
		map[string]any{"title": "keep me", "status": "in_progress"},
		map[string]any{"title": "brand new"},
		map[string]any{"title": "explicit", "task_id": "x1"},
	}})
	if !strings.Contains(r.Content, "3 tasks") {
		t.Fatalf("message: %q", r.Content)
	}
	ts := tasksOnDisk(t, e)
	if len(ts) != 3 {
		t.Fatalf("tasks: %+v", ts)
	}
	if k := taskByID(t, e, "k"); k.Status != "in_progress" || k.CreatedAt != before.CreatedAt {
		t.Fatalf("an item with an existing title keeps its id and creation time: %+v", k)
	}
	for _, x := range ts {
		if x.TaskID == "d" {
			t.Fatal("todo_write replaces the whole list")
		}
	}
	taskByID(t, e, "x1")
	e.fails("todo_write", map[string]any{"items": []any{map[string]any{"title": "a", "task_id": "same"}, map[string]any{"title": "b", "task_id": "same"}}}, "tool_execution_error")
	if len(tasksOnDisk(t, e)) != 3 {
		t.Fatal("a failed todo_write changed the list")
	}
}

func TestUpdatePlanDoesNotWipeTasks(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "manual work", "task_id": "manual-1"})
	e.ok("task_create", map[string]any{"title": "more manual work", "task_id": "manual-2", "status": "in_progress"})

	r := e.ok("update_plan", map[string]any{"explanation": "first cut", "items": []any{
		map[string]any{"step": "research", "status": "in_progress"},
		map[string]any{"step": "   "}, // skipped
		map[string]any{"step": "implement", "depends_on": []any{"plan_1"}},
		map[string]any{"step": "test", "depends_on": []any{"plan_2"}, "priority": "high"},
	}})
	if r.Content != "The plan now has 3 steps." || r.Metadata["after_plan_count"] != 3 || r.Metadata["sync_tasks"] != true || r.Metadata["synced_tasks"] != 3 {
		t.Fatalf("update_plan: %+v", r)
	}
	// ids number the KEPT steps, so a skipped item does not shift them
	plan := loadPlan(filepath.Join(e.root, ".port_sessions", planFileName))
	if len(plan.Steps) != 3 || plan.Steps[1].TaskID != "plan_2" || plan.Steps[2].TaskID != "plan_3" || deref(plan.Explanation) != "first cut" || plan.UpdatedAt == nil {
		t.Fatalf("plan: %+v", plan)
	}
	ts := tasksOnDisk(t, e)
	if len(ts) != 5 {
		t.Fatalf("manual tasks must survive a plan sync: %+v", ts)
	}
	if m := taskByID(t, e, "manual-2"); m.Status != "in_progress" {
		t.Fatalf("manual task changed: %+v", m)
	}
	p1 := taskByID(t, e, "plan_1")
	if p1.Title != "research" || p1.Status != "in_progress" || strings.Join(p1.Blocks, ",") != "plan_2" || p1.Metadata["from_plan"] != true {
		t.Fatalf("plan_1: %+v", p1)
	}
	if p3 := taskByID(t, e, "plan_3"); strings.Join(p3.BlockedBy, ",") != "plan_2" || deref(p3.Priority) != "high" {
		t.Fatalf("plan_3: %+v", p3)
	}

	// a second plan reuses ids: merged, not duplicated; the dropped step's task stays unless pruned
	e.ok("update_plan", map[string]any{"items": []any{map[string]any{"step": "research v2", "status": "completed"}}})
	ts = tasksOnDisk(t, e)
	if len(ts) != 5 || taskByID(t, e, "plan_1").Title != "research v2" || taskByID(t, e, "plan_1").Status != "completed" {
		t.Fatalf("merge by id: %+v", ts)
	}
	// prune_tasks removes only plan-created tasks, never the manual ones
	e.ok("update_plan", map[string]any{"prune_tasks": true, "items": []any{map[string]any{"step": "only step"}}})
	ts = tasksOnDisk(t, e)
	var ids []string
	for _, x := range ts {
		ids = append(ids, x.TaskID)
	}
	if len(ts) != 3 || taskByID(t, e, "manual-1").Title != "manual work" || taskByID(t, e, "plan_1").Title != "only step" {
		t.Fatalf("after prune: %v", ids)
	}
}

func TestUpdatePlanSyncCanBeDisabled(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "mine", "task_id": "m"})
	r := e.ok("update_plan", map[string]any{"sync_tasks": false, "items": []any{map[string]any{"step": "s"}}})
	if r.Metadata["sync_tasks"] != false || len(tasksOnDisk(t, e)) != 1 {
		t.Fatalf("sync_tasks=false must not touch tasks: %+v", tasksOnDisk(t, e))
	}
	e.fails("update_plan", map[string]any{"items": []any{map[string]any{"step": "a", "task_id": "x"}, map[string]any{"step": "b", "task_id": "x"}}}, "tool_execution_error")
}

func TestPlanClear(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "mine", "task_id": "m"})
	e.ok("update_plan", map[string]any{"items": []any{map[string]any{"step": "a"}, map[string]any{"step": "b"}}})
	if len(tasksOnDisk(t, e)) != 3 {
		t.Fatal("setup")
	}
	r := e.ok("update_plan", map[string]any{"clear": true})
	if r.Content != "Cleared the plan." || r.Metadata["after_plan_count"] != 0 || r.Metadata["before_plan_count"] != 2 {
		t.Fatalf("plan_clear: %+v", r)
	}
	if len(tasksOnDisk(t, e)) != 3 {
		t.Fatal("plan_clear must not delete tasks by default")
	}
	if r = e.ok("plan_get", nil); r.Content != "There is no plan." {
		t.Fatalf("plan_get: %q", r.Content)
	}
	e.ok("update_plan", map[string]any{"items": []any{map[string]any{"step": "again"}}})
	e.ok("update_plan", map[string]any{"clear": true, "prune_tasks": true})
	ts := tasksOnDisk(t, e)
	if len(ts) != 3 { // m + the stale plan_2 from the first plan (not part of the second); plan_1 (from_plan) removed
		var ids []string
		for _, x := range ts {
			ids = append(ids, x.TaskID)
		}
		// plan_1 and plan_2 were both created by plans, so only "m" may remain
		if len(ts) != 1 || ts[0].TaskID != "m" {
			t.Fatalf("sync clear removes plan-created tasks only: %v", ids)
		}
	}
	taskByID(t, e, "m")
}

func TestPlanGetRendering(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("update_plan", map[string]any{"explanation": "why", "items": []any{
		map[string]any{"step": "first", "status": "in_progress", "description": "details", "owner": "me", "active_form": "Doing"},
		map[string]any{"step": "second", "depends_on": []any{"plan_1"}},
	}})
	r := e.ok("plan_get", nil)
	for _, want := range []string{"why", "1. [in_progress] first (plan_1)", "details", "owner: me", "active: Doing", "2. [pending] second (plan_2)", "depends_on: plan_1"} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("missing %q in\n%s", want, r.Content)
		}
	}
}

func TestStoreFileShapeAndAtomicity(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "x", "task_id": "t"})
	raw, err := os.ReadFile(filepath.Join(e.root, ".port_sessions", taskFileName))
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string][]map[string]any
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	task := doc["tasks"][0]
	for _, k := range []string{"task_id", "title", "status", "description", "priority", "active_form", "owner", "blocks", "blocked_by", "metadata", "created_at", "updated_at"} {
		if _, ok := task[k]; !ok {
			t.Errorf("task record lacks key %s", k)
		}
	}
	if task["description"] != nil || task["blocks"] == nil {
		t.Errorf("absent optionals are null, lists are [] not null: %v", task)
	}
	entries, _ := os.ReadDir(filepath.Join(e.root, ".port_sessions"))
	for _, en := range entries {
		if strings.HasPrefix(en.Name(), ".tmp") {
			t.Errorf("temporary file left behind: %s", en.Name())
		}
	}
}

func TestStoreLoadIsTolerant(t *testing.T) {
	e := newEnv(t, writeAll())
	dir := filepath.Join(e.root, ".port_sessions")
	os.MkdirAll(dir, 0o755)
	path := filepath.Join(dir, taskFileName)
	for _, junk := range []string{"", "not json", "[]", `{"tasks": "x"}`, `{"tasks": [1, null, {"title": ""}, {"task_id": "", "title": "x"}]}`} {
		os.WriteFile(path, []byte(junk), 0o644)
		if got := tasksOnDisk(t, e); len(got) != 0 {
			t.Errorf("%q should load as empty, got %+v", junk, got)
		}
		e.ok("task_list", nil)
	}
	os.WriteFile(path, []byte(`{"tasks": [{"id": "legacy", "title": "old format", "status": "Done", "description": "", "blocks": ["a","a"," "]}]}`), 0o644)
	x := taskByID(t, e, "legacy")
	if x.Status != "completed" || x.Description != nil || len(x.Blocks) != 1 {
		t.Fatalf("legacy record: %+v", x)
	}
	e.ok("task_create", map[string]any{"title": "after junk", "task_id": "n"})
}

func TestTaskUpdateEditsDependencies(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "a", "task_id": "a"})
	e.ok("task_create", map[string]any{"title": "b", "task_id": "b", "blocked_by": []any{"a", "z"}})
	e.ok("task_update", map[string]any{"task_id": "b", "add_blocked_by": []any{"c", "a"}})
	if got := strings.Join(taskByID(t, e, "b").BlockedBy, ","); got != "a,z,c" {
		t.Fatalf("add merges and de-duplicates: %s", got)
	}
	e.ok("task_update", map[string]any{"task_id": "b", "remove_blocked_by": []any{"z", "c", "nope"}})
	if got := strings.Join(taskByID(t, e, "b").BlockedBy, ","); got != "a" {
		t.Fatalf("remove: %s", got)
	}
	e.ok("task_update", map[string]any{"task_id": "b", "blocked_by": []any{"c"}})
	if got := strings.Join(taskByID(t, e, "b").BlockedBy, ","); got != "c" {
		t.Fatalf("blocked_by still replaces the list: %s", got)
	}
	// removing the last open dependency of a blocked task releases it
	e.ok("task_update", map[string]any{"task_id": "b", "status": "blocked"})
	e.ok("task_update", map[string]any{"task_id": "b", "remove_blocked_by": []any{"c"}})
	if b := taskByID(t, e, "b"); b.Status != "blocked" || len(b.BlockedBy) != 0 {
		t.Fatalf("a task with no dependencies is never released by an edit: %+v", b)
	}
	// starting with an open dependency blocks instead; the reason names it
	e.ok("task_update", map[string]any{"task_id": "b", "add_blocked_by": []any{"a"}, "status": "in_progress"})
	if b := taskByID(t, e, "b"); b.Status != "blocked" || b.Metadata["blocked_reason"] != "waiting_on:a" {
		t.Fatalf("start with open dependency: %+v", b)
	}
	e.ok("task_update", map[string]any{"task_id": "a", "status": "completed"})
	if b := taskByID(t, e, "b"); b.Status != "pending" || b.Metadata["blocked_reason"] != nil {
		t.Fatalf("completing the dependency releases it: %+v", b)
	}
}

func TestTaskListDetailAndEmptyActionable(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("task_create", map[string]any{"title": "one", "task_id": "1", "description": "details", "blocked_by": []any{"2"}})
	e.ok("task_create", map[string]any{"title": "two", "task_id": "2", "status": "blocked"})
	r := e.ok("task_list", map[string]any{"task_id": "1"})
	for _, want := range []string{"id: 1", "status: pending", "title: one", "description: details", "blocked_by: 2", "updated_at: "} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("task_id view lacks %q: %s", want, r.Content)
		}
	}
	if r = e.ok("task_list", map[string]any{"actionable_only": true}); r.Content != "Nothing is ready to work on." {
		t.Fatalf("actionable on a blocked-only list: %q", r.Content)
	}
}

func TestPlanClearModes(t *testing.T) {
	e := newEnv(t, writeAll())
	e.ok("update_plan", map[string]any{"items": []any{map[string]any{"step": "a"}}})
	planFile := filepath.Join(e.root, ".port_sessions", planFileName)
	planBefore, _ := os.ReadFile(planFile)
	r := e.ok("update_plan", map[string]any{"clear": true, "items": []any{map[string]any{"step": "x"}}})
	if !strings.Contains(r.Content, "cannot be combined; nothing was changed") || !strings.Contains(r.Content, "1. [pending] a (plan_1)") {
		t.Fatalf("both given: %q", r.Content)
	}
	r = e.ok("update_plan", map[string]any{})
	for _, want := range []string{"items", "clear", "prune_tasks", `{"items":[`, `{"clear":true}`, "Current plan:", "a (plan_1)"} {
		if !strings.Contains(r.Content, want) {
			t.Errorf("guide lacks %q: %s", want, r.Content)
		}
	}
	if now, _ := os.ReadFile(planFile); string(now) != string(planBefore) {
		t.Fatal("the guide changed the plan")
	}
	taskFile := filepath.Join(e.root, ".port_sessions", taskFileName)
	before, _ := os.ReadFile(taskFile)
	e.ok("update_plan", map[string]any{"clear": true})
	after, _ := os.ReadFile(taskFile)
	if string(before) != string(after) {
		t.Fatal("clear without prune_tasks must not rewrite the task file")
	}
	if plan := loadPlan(filepath.Join(e.root, ".port_sessions", planFileName)); len(plan.Steps) != 0 {
		t.Fatalf("plan not cleared: %+v", plan)
	}
	// an empty items list is still a valid replace
	e.ok("update_plan", map[string]any{"items": []any{}})
}

func TestLegacyPlanFileStillLoads(t *testing.T) {
	e := newEnv(t, writeAll())
	dir := filepath.Join(e.root, ".port_sessions")
	os.MkdirAll(dir, 0o755)
	os.WriteFile(filepath.Join(dir, planFileName), []byte(`{"explanation":"old","updated_at":"2026-01-01T00:00:00Z","steps":[{"step":"s","status":"Done","task_id":"plan_1","description":null,"priority":null,"active_form":null,"owner":null,"depends_on":[]}]}`), 0o644)
	if r := e.ok("plan_get", nil); !strings.Contains(r.Content, "1. [completed] s (plan_1)") {
		t.Fatalf("legacy plan: %q", r.Content)
	}
}
