package tools

import (
	"context"
	"errors"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// The to-do list ("tasks") and the plan live in two small JSON files under
// <workspace>/.port_sessions/. They are about the agent's own work items and
// have nothing to do with background processes (see TaskOutput / TaskStop).
//
// Differences from the original design, on purpose: update_plan merges into the
// task list by id and never deletes tasks (it used to replace the whole list);
// update_plan with clear=true (and prune_tasks) removes only tasks the plan itself created; task_create refuses a
// duplicate id; task_list with an unknown task_id is an error; completing a task
// releases only tasks that were blocked on something.

const (
	taskFileName = "task_runtime.json"
	planFileName = "plan_runtime.json"
)

var storeMu sync.Mutex // one process-wide lock; these files are tiny

// Task is one to-do item.
type Task struct {
	TaskID      string         `json:"task_id"`
	Title       string         `json:"title"`
	Status      string         `json:"status"`
	Description *string        `json:"description"`
	Priority    *string        `json:"priority"`
	ActiveForm  *string        `json:"active_form"`
	Owner       *string        `json:"owner"`
	Blocks      []string       `json:"blocks"`
	BlockedBy   []string       `json:"blocked_by"`
	Metadata    map[string]any `json:"metadata"`
	CreatedAt   string         `json:"created_at"`
	UpdatedAt   string         `json:"updated_at"`
}

// PlanStep is one step of the plan.
type PlanStep struct {
	Step        string   `json:"step"`
	Status      string   `json:"status"`
	TaskID      string   `json:"task_id"`
	Description *string  `json:"description"`
	Priority    *string  `json:"priority"`
	ActiveForm  *string  `json:"active_form"`
	Owner       *string  `json:"owner"`
	DependsOn   []string `json:"depends_on"`
}

type planFile struct {
	Explanation *string    `json:"explanation"`
	UpdatedAt   *string    `json:"updated_at"`
	Steps       []PlanStep `json:"steps"`
}

func nowStamp() string { return time.Now().UTC().Format(time.RFC3339) }

func strPtr(s string) *string {
	s = strings.TrimSpace(s)
	if s == "" {
		return nil
	}
	return &s
}

func deref(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

func normStatus(s string) string {
	s = strings.ToLower(strings.TrimSpace(s))
	s = strings.NewReplacer("-", "_", " ", "_").Replace(s)
	switch s {
	case "pending", "in_progress", "completed", "blocked", "cancelled":
		return s
	case "done", "complete":
		return "completed"
	case "todo", "open":
		return "pending"
	case "blocked_on":
		return "blocked"
	}
	return "pending"
}

func dedupe(ids []string) []string {
	seen := map[string]bool{}
	out := []string{}
	for _, id := range ids {
		id = strings.TrimSpace(id)
		if id != "" && !seen[id] {
			seen[id] = true
			out = append(out, id)
		}
	}
	return out
}

func randHex(n int) string {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)[:n]
}

// ---- storage ----------------------------------------------------------------

func storePath(tc *Context, name string) string {
	return filepath.Join(tc.Root, ".port_sessions", name)
}

func writeFileAtomic(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".tmp-*")
	if err != nil {
		return err
	}
	name := tmp.Name()
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(name)
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(name)
		return err
	}
	if err := os.Rename(name, path); err != nil {
		os.Remove(name)
		return err
	}
	return nil
}

func loadTasks(path string) []Task {
	data, err := os.ReadFile(path)
	if err != nil {
		return []Task{}
	}
	var in struct {
		Tasks []struct {
			Task
			ID string `json:"id"`
		} `json:"tasks"`
	}
	if json.Unmarshal(data, &in) != nil {
		return []Task{}
	}
	out := []Task{}
	for _, t := range in.Tasks {
		task := t.Task
		if task.TaskID == "" {
			task.TaskID = t.ID
		}
		task.TaskID, task.Title = strings.TrimSpace(task.TaskID), strings.TrimSpace(task.Title)
		if task.TaskID == "" || task.Title == "" {
			continue
		}
		task.Status = normStatus(task.Status)
		task.Description, task.Priority = strPtr(deref(task.Description)), strPtr(deref(task.Priority))
		task.ActiveForm, task.Owner = strPtr(deref(task.ActiveForm)), strPtr(deref(task.Owner))
		task.Blocks, task.BlockedBy = dedupe(task.Blocks), dedupe(task.BlockedBy)
		if task.Metadata == nil {
			task.Metadata = map[string]any{}
		}
		out = append(out, task)
	}
	return out
}

func encodeTasks(tasks []Task) []byte {
	if tasks == nil {
		tasks = []Task{}
	}
	b, _ := json.MarshalIndent(struct {
		Tasks []Task `json:"tasks"`
	}{tasks}, "", "  ")
	return append(b, '\n')
}

func loadPlan(path string) planFile {
	data, err := os.ReadFile(path)
	p := planFile{Steps: []PlanStep{}}
	if err != nil || json.Unmarshal(data, &p) != nil {
		return planFile{Steps: []PlanStep{}}
	}
	steps := []PlanStep{}
	for _, s := range p.Steps {
		s.Step = strings.TrimSpace(s.Step)
		if s.Step == "" {
			continue
		}
		s.Status, s.DependsOn = normStatus(s.Status), dedupe(s.DependsOn)
		steps = append(steps, s)
	}
	p.Steps = steps
	return p
}

func encodePlan(p planFile) []byte {
	if p.Steps == nil {
		p.Steps = []PlanStep{}
	}
	b, _ := json.MarshalIndent(p, "", "  ")
	return append(b, '\n')
}

// ---- ordering and dependency helpers -----------------------------------------

func statusRank(s string) int {
	switch s {
	case "in_progress":
		return 0
	case "pending":
		return 1
	case "blocked":
		return 2
	case "completed":
		return 3
	}
	return 4
}

func priorityRank(p *string) int {
	switch strings.ToLower(deref(p)) {
	case "critical":
		return 0
	case "high":
		return 1
	case "medium":
		return 2
	case "low":
		return 3
	}
	return 4
}

func sortTasks(ts []Task) {
	sort.SliceStable(ts, func(i, j int) bool {
		a, b := ts[i], ts[j]
		if statusRank(a.Status) != statusRank(b.Status) {
			return statusRank(a.Status) < statusRank(b.Status)
		}
		if priorityRank(a.Priority) != priorityRank(b.Priority) {
			return priorityRank(a.Priority) < priorityRank(b.Priority)
		}
		if la, lb := strings.ToLower(a.Title), strings.ToLower(b.Title); la != lb {
			return la < lb
		}
		return a.TaskID < b.TaskID
	})
}

func findTask(ts []Task, id string) int {
	for i := range ts {
		if ts[i].TaskID == id {
			return i
		}
	}
	return -1
}

// Dependency rules. A dependency blocks while it exists and is not completed:
// a cancelled one still blocks (the dependant must be told to drop or replace it),
// while an id that names no task is reported and ignored, never a permanent block.

const waitingPrefix = "waiting_on:"

func isFinished(status string) bool { return status == "completed" || status == "cancelled" }

// unresolved lists dependencies that exist and are not completed.
func unresolved(ts []Task, t Task) []string {
	var out []string
	for _, id := range t.BlockedBy {
		if i := findTask(ts, id); i >= 0 && ts[i].Status != "completed" {
			out = append(out, id)
		}
	}
	return out
}

// unknownDeps lists dependencies that name no task.
func unknownDeps(ts []Task, t Task) []string {
	var out []string
	for _, id := range t.BlockedBy {
		if findTask(ts, id) < 0 {
			out = append(out, id)
		}
	}
	return out
}

func actionable(ts []Task, t Task) bool {
	return t.Status == "in_progress" || (t.Status == "pending" && len(unresolved(ts, t)) == 0)
}

func blockedReason(t Task) string {
	r, _ := t.Metadata["blocked_reason"].(string)
	return r
}

// normalizeTasks applies the dependency rules to a whole list and reports
// whether anything changed. Idempotent: a second call changes nothing.
//   - in_progress with an unfinished dependency becomes blocked, waiting_on:<ids>
//   - blocked with dependencies that are all completed goes back to pending; so does a
//     task whose only reason was waiting_on <ids> that no longer exist or were removed.
//     A task blocked by a reason alone (no dependencies) is never released.
//   - a stale waiting_on:<ids> reason is recomputed.
func normalizeTasks(ts []Task) bool {
	changed := false
	for i := range ts {
		t := &ts[i]
		if t.Metadata == nil {
			t.Metadata = map[string]any{}
		}
		un := unresolved(ts, *t)
		reason := blockedReason(*t)
		switch {
		case t.Status == "in_progress" && len(un) > 0:
			t.Status = "blocked"
			t.Metadata["blocked_reason"] = waitingPrefix + strings.Join(un, ",")
		case t.Status == "blocked" && len(un) == 0 && (strings.HasPrefix(reason, waitingPrefix) ||
			(len(t.BlockedBy) > 0 && len(unknownDeps(ts, *t)) < len(t.BlockedBy))):
			// nothing left to wait on: either every known dependency is completed, or the
			// block was only ever "waiting_on" and the dependency was removed or never existed
			t.Status = "pending"
			delete(t.Metadata, "blocked_reason")
		case t.Status == "blocked" && strings.HasPrefix(reason, waitingPrefix):
			want := waitingPrefix + strings.Join(un, ",")
			if reason == want {
				continue
			}
			t.Metadata["blocked_reason"] = want
		default:
			continue
		}
		t.UpdatedAt = nowStamp()
		changed = true
	}
	return changed
}

// cyclePath returns the dependency path that leads from start back to start, or nil.
func cyclePath(start string, deps func(id string) []string) []string {
	seen := map[string]bool{}
	var walk func(id string, path []string) []string
	walk = func(id string, path []string) []string {
		for _, d := range deps(id) {
			if d == start {
				return append(append([]string{}, path...), d)
			}
			if !seen[d] {
				seen[d] = true
				if p := walk(d, append(path, d)); p != nil {
					return p
				}
			}
		}
		return nil
	}
	return walk(start, []string{start})
}

func cycleError(p []string) error {
	if len(p) == 2 {
		return fmt.Errorf("task %s cannot depend on itself; nothing was changed", p[0])
	}
	return fmt.Errorf("dependency cycle %s; nothing was changed. Remove one of those dependencies first", strings.Join(p, " -> "))
}

// checkCycles refuses a cycle through any of the given task ids.
func checkCycles(ts []Task, ids ...string) error {
	deps := func(id string) []string {
		if i := findTask(ts, id); i >= 0 {
			return ts[i].BlockedBy
		}
		return nil
	}
	for _, id := range ids {
		if p := cyclePath(id, deps); p != nil {
			return cycleError(p)
		}
	}
	return nil
}

// depNotes describes why a task waits and which dependencies are unusable.
func depNotes(ts []Task, t Task) []string {
	var notes []string
	if !isFinished(t.Status) {
		var parts []string
		for _, id := range unresolved(ts, t) {
			st := ts[findTask(ts, id)].Status
			if st == "cancelled" {
				parts = append(parts, id+" (cancelled - remove or replace it)")
			} else {
				parts = append(parts, id+" ("+st+")")
			}
		}
		if len(parts) > 0 {
			notes = append(notes, "waiting_on: "+strings.Join(parts, ", "))
		}
	}
	for _, id := range unknownDeps(ts, t) {
		notes = append(notes, "depends on unknown task "+id+" (ignored, not blocking)")
	}
	if r := blockedReason(t); t.Status == "blocked" && r != "" && !strings.HasPrefix(r, waitingPrefix) {
		notes = append(notes, "blocked_reason: "+r)
	}
	return notes
}

// ---- rendering ----------------------------------------------------------------

func renderTaskLine(b *strings.Builder, t Task) {
	fmt.Fprintf(b, "- %s [%s]", t.TaskID, t.Status)
	if t.Priority != nil {
		fmt.Fprintf(b, " (%s)", *t.Priority)
	}
	if t.Owner != nil {
		fmt.Fprintf(b, " @%s", *t.Owner)
	}
	fmt.Fprintf(b, " %s\n", t.Title)
}

func renderTaskDetails(b *strings.Builder, ts []Task, t Task) {
	if t.Description != nil {
		fmt.Fprintf(b, "    description: %s\n", *t.Description)
	}
	if t.ActiveForm != nil {
		fmt.Fprintf(b, "    active: %s\n", *t.ActiveForm)
	}
	if len(t.BlockedBy) > 0 {
		fmt.Fprintf(b, "    blocked_by: %s\n", strings.Join(t.BlockedBy, ", "))
	}
	for _, n := range depNotes(ts, t) {
		fmt.Fprintf(b, "    %s\n", n)
	}
	if len(t.Blocks) > 0 {
		fmt.Fprintf(b, "    blocks: %s\n", strings.Join(t.Blocks, ", "))
	}
}

func renderTaskFull(ts []Task, t Task) string {
	var b strings.Builder
	fmt.Fprintf(&b, "id: %s\nstatus: %s\ntitle: %s\n", t.TaskID, t.Status, t.Title)
	for _, kv := range [][2]string{{"priority", deref(t.Priority)}, {"owner", deref(t.Owner)}, {"active form", deref(t.ActiveForm)}, {"description", deref(t.Description)}} {
		if kv[1] != "" {
			fmt.Fprintf(&b, "%s: %s\n", kv[0], kv[1])
		}
	}
	if len(t.BlockedBy) > 0 {
		fmt.Fprintf(&b, "blocked_by: %s\n", strings.Join(t.BlockedBy, ", "))
	}
	for _, n := range depNotes(ts, t) {
		fmt.Fprintf(&b, "%s\n", n)
	}
	if len(t.Blocks) > 0 {
		fmt.Fprintf(&b, "blocks: %s\n", strings.Join(t.Blocks, ", "))
	}
	keys := make([]string, 0, len(t.Metadata))
	for k := range t.Metadata {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		fmt.Fprintf(&b, "%s=%v\n", k, t.Metadata[k])
	}
	fmt.Fprintf(&b, "updated_at: %s", t.UpdatedAt)
	return b.String()
}

func taskMutationMD(action, path string, before, after []byte, nb, na int) map[string]any {
	md := map[string]any{
		"action": action, "path": path, "before_task_count": nb, "after_task_count": na, "total_tasks": na,
		"after_sha256": sha256Hex(string(after)), "after_preview": preview(string(after), 140),
	}
	if before == nil {
		md["before_sha256"], md["before_preview"] = nil, nil
	} else {
		md["before_sha256"], md["before_preview"] = sha256Hex(string(before)), preview(string(before), 140)
	}
	return md
}

// unchanged is returned by a mutation that deliberately did nothing and wants
// its message delivered as a normal result (a usage guide).
type unchanged struct{ msg string }

func (u *unchanged) Error() string { return u.msg }

// mutateTasks loads the store, runs fn, saves the result, and returns the
// mutation metadata. A failed fn leaves the file untouched.
func mutateTasks(tc *Context, action string, fn func(ts *[]Task) (string, *Task, error)) (string, map[string]any, error) {
	storeMu.Lock()
	defer storeMu.Unlock()
	path := storePath(tc, taskFileName)
	var before []byte
	if raw, err := os.ReadFile(path); err == nil {
		before = raw
	}
	ts := loadTasks(path)
	nb := len(ts)
	msg, touched, err := fn(&ts)
	var u *unchanged
	if errors.As(err, &u) {
		return u.msg, map[string]any{"guide": true, "changed": false}, nil
	}
	if err != nil {
		return "", nil, err
	}
	normalizeTasks(ts)
	if touched != nil {
		if i := findTask(ts, touched.TaskID); i >= 0 {
			touched = &ts[i]
		}
	}
	data := encodeTasks(ts)
	if err := writeFileAtomic(path, data); err != nil {
		return "", nil, fmt.Errorf("cannot save the task list: %v", err)
	}
	md := taskMutationMD(action, relPath(tc.Root, path), before, data, nb, len(ts))
	if touched != nil {
		md["task_id"], md["task_status"] = touched.TaskID, touched.Status
	}
	return msg, md, nil
}

// ---- argument handling ----------------------------------------------------------

type taskPatch struct {
	id                                       string
	title, status                            *string
	description, priority, activeForm, owner *string
	blocks, blockedBy                        *[]string
	metadata                                 map[string]any
}

func patchOf(m map[string]any) taskPatch {
	var p taskPatch
	str := func(k string) *string {
		if v, ok := m[k].(string); ok {
			return &v
		}
		return nil
	}
	list := func(k string) *[]string {
		if l, ok := m[k].([]any); ok {
			out := []string{}
			for _, x := range l {
				if s, ok := x.(string); ok {
					out = append(out, s)
				}
			}
			out = dedupe(out)
			return &out
		}
		return nil
	}
	p.id = strings.TrimSpace(argStr(m, "task_id"))
	if p.id == "" {
		p.id = strings.TrimSpace(argStr(m, "id"))
	}
	p.title, p.status = str("title"), str("status")
	p.description, p.priority, p.activeForm, p.owner = str("description"), str("priority"), str("active_form"), str("owner")
	p.blocks, p.blockedBy = list("blocks"), list("blocked_by")
	if md, ok := m["metadata"].(map[string]any); ok {
		p.metadata = md
	}
	return p
}

func (p taskPatch) apply(t *Task) {
	if p.title != nil && strings.TrimSpace(*p.title) != "" {
		t.Title = strings.TrimSpace(*p.title)
	}
	if p.status != nil {
		t.Status = normStatus(*p.status)
	}
	if p.description != nil {
		t.Description = strPtr(*p.description)
	}
	if p.priority != nil {
		t.Priority = strPtr(*p.priority)
	}
	if p.activeForm != nil {
		t.ActiveForm = strPtr(*p.activeForm)
	}
	if p.owner != nil {
		t.Owner = strPtr(*p.owner)
	}
	if p.blocks != nil {
		t.Blocks = *p.blocks
	}
	if p.blockedBy != nil {
		t.BlockedBy = *p.blockedBy
	}
	if p.metadata != nil {
		t.Metadata = p.metadata
	}
	t.UpdatedAt = nowStamp()
}

var taskProps = map[string]any{
	"title":       prop("string", "", "minLength", 1),
	"description": prop("string", ""),
	"status":      prop("string", "pending|in_progress|completed|blocked|cancelled"),
	"priority":    prop("string", "critical|high|medium|low"),
	"active_form": prop("string", ""),
	"owner":       prop("string", ""),
	"blocks":      map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
	"blocked_by":  map[string]any{"type": "array", "items": map[string]any{"type": "string"}, "description": "Ids that must finish first."},
	"metadata":    map[string]any{"type": "object"},
}

// withoutValueHints is withProps minus the value lists for status and priority,
// for tools whose callers already saw them on task_create.
func withoutValueHints(extra map[string]any) map[string]any {
	m := withProps(extra)
	for _, k := range []string{"status", "priority"} {
		m[k] = map[string]any{"type": "string"}
	}
	return m
}

func withProps(extra map[string]any) map[string]any {
	m := map[string]any{}
	for k, v := range taskProps {
		m[k] = v
	}
	for k, v := range extra {
		m[k] = v
	}
	return m
}

func taskIDProp() map[string]any {
	return prop("string", "", "minLength", 1)
}

// ---- the tools --------------------------------------------------------------------

// TodoTools returns the task list and plan tools.
func TodoTools() []Tool {
	return []Tool{taskList(), taskCreate(), taskUpdate(), todoWrite(), planGet(), updatePlan()}
}

// readTasks loads the list and applies the dependency rules once. The result is
// written back only if the rules changed something and the session may write.
func readTasks(tc *Context) []Task {
	storeMu.Lock()
	defer storeMu.Unlock()
	path := storePath(tc, taskFileName)
	ts := loadTasks(path)
	if normalizeTasks(ts) && tc.Permissions.AllowFileWrite {
		_ = writeFileAtomic(path, encodeTasks(ts))
	}
	return ts
}

func taskList() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "task_list",
			Description: "List to-do tasks, most urgent first. Pass task_id to show one task in full. " +
				"actionable_only=true lists only what can be worked on now (in progress, or pending with every blocked_by task finished); use it to pick the next task.",
			Parameters: obj(map[string]any{
				"task_id":         prop("string", "Show only this task, with all its fields."),
				"status":          prop("string", ""),
				"owner":           prop("string", ""),
				"actionable_only": prop("boolean", ""),
				"max_tasks":       prop("integer", "", "minimum", 1, "maximum", 500),
			}),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			all := readTasks(tc)
			if id := strings.TrimSpace(argStr(args, "task_id")); id != "" {
				i := findTask(all, id)
				if i < 0 {
					return "", nil, fmt.Errorf("unknown task id %q", id)
				}
				return renderTaskFull(all, all[i]), map[string]any{"total_tasks": len(all)}, nil
			}
			sortTasks(all)
			var show []Task
			for _, t := range all {
				if s := argStr(args, "status"); s != "" && t.Status != normStatus(s) {
					continue
				}
				if o := argStr(args, "owner"); o != "" && deref(t.Owner) != o {
					continue
				}
				if argBool(args, "actionable_only", false) && !actionable(all, t) {
					continue
				}
				show = append(show, t)
			}
			if len(show) == 0 {
				if argBool(args, "actionable_only", false) {
					return "Nothing is ready to work on.", map[string]any{"total_tasks": len(all)}, nil
				}
				return "No tasks.", map[string]any{"total_tasks": len(all)}, nil
			}
			limit := argInt(args, "max_tasks", 50)
			var b strings.Builder
			b.WriteString("Tasks:\n")
			for i, t := range show {
				if i >= limit {
					fmt.Fprintf(&b, "(+%d more)\n", len(show)-limit)
					break
				}
				renderTaskLine(&b, t)
				renderTaskDetails(&b, all, t)
			}
			return TruncateOutput(strings.TrimRight(b.String(), "\n"), tc.MaxOutputChars), map[string]any{"total_tasks": len(all)}, nil
		},
	}
}

func taskCreate() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "task_create",
			Description: "Add a to-do task.",
			Parameters:  obj(withProps(map[string]any{"task_id": prop("string", "")}), "title"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			return mutateTasks(tc, "task_create", func(ts *[]Task) (string, *Task, error) {
				p := patchOf(args)
				if p.title == nil || strings.TrimSpace(*p.title) == "" {
					return "", nil, fmt.Errorf("title is required")
				}
				if p.id == "" {
					p.id = "task_" + randHex(10)
				}
				if findTask(*ts, p.id) >= 0 {
					return "", nil, fmt.Errorf("a task with id %q already exists; use task_update to change it", p.id)
				}
				now := nowStamp()
				t := Task{TaskID: p.id, Status: "pending", Blocks: []string{}, BlockedBy: []string{}, Metadata: map[string]any{}, CreatedAt: now}
				p.apply(&t)
				*ts = append(*ts, t)
				if err := checkCycles(*ts, t.TaskID); err != nil {
					return "", nil, err
				}
				return withNotes(fmt.Sprintf("Created task %s: %s [%s]", t.TaskID, t.Title, t.Status), notesFor(*ts, t, "")), &t, nil
			})
		},
	}
}

func taskUpdate() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "task_update",
			Description: "Change the given fields of a to-do task; omitted fields stay as they are. " +
				"Move it through its life with status: in_progress starts it (it becomes blocked instead, waiting on them, if a blocked_by task is unfinished); " +
				"completed finishes it and releases tasks that were blocked only on it; cancelled drops it (reason is recorded); " +
				"blocked marks it blocked (reason is recorded; a task blocked by a reason alone is only released by setting its status again). " +
				"Edit dependencies with add_blocked_by / remove_blocked_by; blocked_by replaces the whole list. " +
				"Cycles are refused; an unknown dependency id is reported, not blocking; a cancelled one keeps dependants waiting until removed or replaced.",
			Parameters: obj(map[string]any{
				"task_id":           taskIDProp(),
				"title":             prop("string", "", "minLength", 1),
				"description":       prop("string", ""),
				"status":            prop("string", "pending|in_progress|completed|blocked|cancelled"),
				"priority":          prop("string", "critical|high|medium|low"),
				"active_form":       prop("string", ""),
				"owner":             prop("string", ""),
				"reason":            prop("string", "Why it is blocked or cancelled."),
				"blocks":            map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
				"blocked_by":        map[string]any{"type": "array", "items": map[string]any{"type": "string"}, "description": "Ids that must finish first (replaces the list)."},
				"add_blocked_by":    map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
				"remove_blocked_by": map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
				"metadata":          map[string]any{"type": "object"},
			}, "task_id"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			return mutateTasks(tc, "task_update", func(ts *[]Task) (string, *Task, error) {
				p := patchOf(args)
				i := findTask(*ts, p.id)
				if i < 0 {
					return "", nil, fmt.Errorf("unknown task id %q", p.id)
				}
				if !hasAnyArg(args, taskChangeFields) {
					return "", nil, &unchanged{taskUpdateGuide(renderTaskFull(*ts, (*ts)[i]))}
				}
				t := &(*ts)[i]
				p.apply(t)
				add, rm := argStrList(args, "add_blocked_by"), argStrList(args, "remove_blocked_by")
				if len(add) > 0 {
					t.BlockedBy = dedupe(append(t.BlockedBy, add...))
				}
				if len(rm) > 0 {
					drop := map[string]bool{}
					for _, id := range rm {
						drop[strings.TrimSpace(id)] = true
					}
					kept := []string{}
					for _, id := range t.BlockedBy {
						if !drop[id] {
							kept = append(kept, id)
						}
					}
					t.BlockedBy = kept
				}
				if p.blockedBy != nil || len(add) > 0 {
					if err := checkCycles(*ts, t.TaskID); err != nil {
						return "", nil, err
					}
				}
				if t.Metadata == nil {
					t.Metadata = map[string]any{}
				}
				waiting := ""
				if p.status != nil && t.Status == "in_progress" {
					if un := unresolved(*ts, *t); len(un) > 0 {
						t.Status = "blocked"
						t.Metadata["blocked_reason"] = waitingPrefix + strings.Join(un, ",")
						waiting = strings.Join(un, ", ")
					} else {
						delete(t.Metadata, "blocked_reason")
					}
				}
				if r := strings.TrimSpace(argStr(args, "reason")); r != "" {
					switch t.Status {
					case "blocked":
						t.Metadata["blocked_reason"] = r
					case "cancelled":
						t.Metadata["cancel_reason"] = r
					}
				}
				normalizeTasks(*ts)
				out := *t
				if waiting != "" {
					return withNotes(fmt.Sprintf("Task %s is blocked, waiting on %s: %s [%s]", out.TaskID, waiting, out.Title, out.Status), notesFor(*ts, out, waiting)), &out, nil
				}
				return withNotes(fmt.Sprintf("Updated task %s: %s [%s]", out.TaskID, out.Title, out.Status), notesFor(*ts, out, "")), &out, nil
			})
		},
		}
		}

func todoWrite() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "todo_write",
			Description: "Replace the whole to-do list; tasks not in items are removed.",
			Parameters: obj(map[string]any{
				"items": map[string]any{
					"type":  "array",
					"items": obj(withoutValueHints(map[string]any{"task_id": prop("string", "")}), "title"),
				},
			}, "items"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			return mutateTasks(tc, "todo_write", func(ts *[]Task) (string, *Task, error) {
				items, _ := args["items"].([]any)
				old := *ts
				next := []Task{}
				used := map[string]bool{}
				for idx, raw := range items {
					m, ok := raw.(map[string]any)
					if !ok {
						continue
					}
					p := patchOf(m)
					if p.title == nil || strings.TrimSpace(*p.title) == "" {
						continue
					}
					t := Task{Status: "pending", Blocks: []string{}, BlockedBy: []string{}, Metadata: map[string]any{}, CreatedAt: nowStamp()}
					id := p.id
					if id == "" { // keep the id of an existing task with the same title
						for _, o := range old {
							if o.Title == strings.TrimSpace(*p.title) && !used[o.TaskID] {
								id = o.TaskID
								t.CreatedAt = o.CreatedAt
								break
							}
						}
					} else if i := findTask(old, id); i >= 0 {
						t.CreatedAt = old[i].CreatedAt
					}
					if id == "" {
						id = fmt.Sprintf("task_%d_%s", idx+1, randHex(6))
					}
					if used[id] {
						return "", nil, fmt.Errorf("task id %q appears twice in items", id)
					}
					used[id] = true
					t.TaskID = id
					p.apply(&t)
					next = append(next, t)
				}
				*ts = next
				ids := make([]string, 0, len(next))
				for _, t := range next {
					ids = append(ids, t.TaskID)
				}
				if err := checkCycles(*ts, ids...); err != nil {
					return "", nil, err
				}
				return fmt.Sprintf("Replaced the to-do list with %d tasks.", len(next)), nil, nil
			})
		},
	}
}

// ---- plan -----------------------------------------------------------------------

func readPlan(tc *Context) planFile {
	storeMu.Lock()
	defer storeMu.Unlock()
	return loadPlan(storePath(tc, planFileName))
}

func planGet() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "plan_get",
			Description: "Show the plan.",
			Parameters:  obj(map[string]any{}),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			p := readPlan(tc)
			if len(p.Steps) == 0 {
				return "There is no plan.", nil, nil
			}
			return renderPlan(p), nil, nil
		},
	}
}

func renderPlan(p planFile) string {
	var b strings.Builder
	if p.Explanation != nil {
		fmt.Fprintf(&b, "%s\n", *p.Explanation)
	}
	for i, s := range p.Steps {
		fmt.Fprintf(&b, "%d. [%s] %s (%s)", i+1, s.Status, s.Step, s.TaskID)
		if s.Priority != nil {
			fmt.Fprintf(&b, " priority=%s", *s.Priority)
		}
		b.WriteString("\n")
		if s.Description != nil {
			fmt.Fprintf(&b, "    %s\n", *s.Description)
		}
		if s.ActiveForm != nil {
			fmt.Fprintf(&b, "    active: %s\n", *s.ActiveForm)
		}
		if s.Owner != nil {
			fmt.Fprintf(&b, "    owner: %s\n", *s.Owner)
		}
		if len(s.DependsOn) > 0 {
			fmt.Fprintf(&b, "    depends_on: %s\n", strings.Join(s.DependsOn, ", "))
		}
	}
	return strings.TrimRight(b.String(), "\n")
}


func updatePlan() Tool {
	return Tool{
		Requires: PermWrite, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "update_plan",
			Description: "Replace the plan with the given steps; mirrors them into the to-do list unless sync_tasks is false. " +
				"The to-do list is merged by id and never emptied: tasks that are not steps stay (removed only with prune_tasks, and only if an earlier plan created them). " +
				"clear=true empties the plan instead of giving items; it leaves the to-do list alone unless prune_tasks=true, which then removes the tasks plans created. " +
				"With neither or both of items/clear it only returns usage.",
			Parameters: obj(map[string]any{
				"items": map[string]any{
					"type": "array",
					"items": obj(map[string]any{
						"step":        prop("string", "", "minLength", 1),
						"status":      prop("string", "pending|in_progress|completed|blocked|cancelled"),
						"task_id":     prop("string", ""),
						"description": prop("string", ""),
						"priority":    prop("string", "critical|high|medium|low"),
						"active_form": prop("string", ""),
						"owner":       prop("string", ""),
						"depends_on":  map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
					}, "step"),
				},
				"explanation": prop("string", ""),
				"sync_tasks":  prop("boolean", "Default true."),
				"prune_tasks": prop("boolean", "Also remove to-do tasks from earlier plans that are no longer steps."),
				"clear":       prop("boolean", "Empty the plan; no items needed."),
			}),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			storeMu.Lock()
			defer storeMu.Unlock()
			ppath, tpath := storePath(tc, planFileName), storePath(tc, taskFileName)
			var before []byte
			if raw, err := os.ReadFile(ppath); err == nil {
				before = raw
			}
			old := loadPlan(ppath)
			plan := planFile{Steps: []PlanStep{}}
			items, hasItems := args["items"].([]any)
			clearing := argBool(args, "clear", false)
			if clearing && len(items) > 0 {
				return updatePlanGuide("items and clear cannot be combined; nothing was changed.", old), map[string]any{"guide": true, "changed": false}, nil
			}
			if !clearing && !hasItems {
				return updatePlanGuide("", old), map[string]any{"guide": true, "changed": false}, nil
			}
			ids := map[string]bool{}
			for _, raw := range items {
				m, ok := raw.(map[string]any)
				if !ok {
					continue
				}
				step := strings.TrimSpace(argStr(m, "step"))
				if step == "" {
					continue
				}
				id := strings.TrimSpace(argStr(m, "task_id"))
				if id == "" {
					id = fmt.Sprintf("plan_%d", len(plan.Steps)+1) // numbered among the kept steps
				}
				if ids[id] {
					return "", nil, fmt.Errorf("step id %q appears twice", id)
				}
				ids[id] = true
				plan.Steps = append(plan.Steps, PlanStep{
					Step: step, Status: normStatus(argStr(m, "status")), TaskID: id,
					Description: strPtr(argStr(m, "description")), Priority: strPtr(argStr(m, "priority")),
					ActiveForm: strPtr(argStr(m, "active_form")), Owner: strPtr(argStr(m, "owner")),
					DependsOn: dedupe(argStrList(m, "depends_on")),
				})
			}
			stepDeps := map[string][]string{}
			for _, st := range plan.Steps {
				stepDeps[st.TaskID] = st.DependsOn
			}
			for _, st := range plan.Steps {
				if cp := cyclePath(st.TaskID, func(id string) []string { return stepDeps[id] }); cp != nil {
					return "", nil, cycleError(cp)
				}
			}
			if !clearing {
				stamp := nowStamp()
				plan.UpdatedAt = &stamp
				plan.Explanation = strPtr(argStr(args, "explanation"))
			}
			data := encodePlan(plan)
			action := "update_plan"
			if clearing {
				action = "plan_clear"
			}
			md := map[string]any{
				"action": action, "path": relPath(tc.Root, ppath),
				"before_plan_count": len(old.Steps), "after_plan_count": len(plan.Steps), "total_steps": len(plan.Steps),
				"after_sha256": sha256Hex(string(data)), "after_preview": preview(string(data), 140),
			}
			if before == nil {
				md["before_sha256"], md["before_preview"] = nil, nil
			} else {
				md["before_sha256"], md["before_preview"] = sha256Hex(string(before)), preview(string(before), 140)
			}
			sync := argBool(args, "sync_tasks", true)
			md["sync_tasks"] = sync
			if plan.Explanation != nil {
				md["explanation"] = *plan.Explanation
			}
			if sync && (!clearing || argBool(args, "prune_tasks", false)) {
				tasks := loadTasks(tpath)
				mergePlan(&tasks, plan.Steps)
				stepIDs := make([]string, 0, len(plan.Steps))
				for _, st := range plan.Steps {
					stepIDs = append(stepIDs, st.TaskID)
				}
				if err := checkCycles(tasks, stepIDs...); err != nil {
					return "", nil, err
				}
				if argBool(args, "prune_tasks", false) {
					kept := tasks[:0]
					for _, t := range tasks {
						if t.Metadata["from_plan"] == true && !ids[t.TaskID] {
							continue
						}
						kept = append(kept, t)
					}
					tasks = kept
				}
				normalizeTasks(tasks)
				tdata := encodeTasks(tasks)
				if err := writeFileAtomic(tpath, tdata); err != nil {
					return "", nil, fmt.Errorf("cannot save the task list: %v", err)
				}
				md["synced_task_store_path"], md["synced_task_sha256"], md["synced_tasks"] = relPath(tc.Root, tpath), sha256Hex(string(tdata)), len(plan.Steps)
			}
			if err := writeFileAtomic(ppath, data); err != nil {
				return "", nil, fmt.Errorf("cannot save the plan: %v", err)
			}
			if clearing {
				return "Cleared the plan.", md, nil
			}
			return fmt.Sprintf("The plan now has %d steps.", len(plan.Steps)), md, nil
		},
	}
}

// mergePlan upserts plan steps into the task list by id. Tasks it creates are
// marked metadata.from_plan so a later prune can tell them from the agent's own.
func mergePlan(ts *[]Task, steps []PlanStep) {
	dependents := map[string][]string{}
	for _, s := range steps {
		for _, d := range s.DependsOn {
			dependents[d] = append(dependents[d], s.TaskID)
		}
	}
	now := nowStamp()
	for _, s := range steps {
		blocks := dedupe(dependents[s.TaskID])
		sort.Strings(blocks)
		i := findTask(*ts, s.TaskID)
		if i < 0 {
			*ts = append(*ts, Task{TaskID: s.TaskID, Title: s.Step, Status: s.Status, Description: s.Description, Priority: s.Priority,
				ActiveForm: s.ActiveForm, Owner: s.Owner, Blocks: blocks, BlockedBy: s.DependsOn,
				Metadata: map[string]any{"from_plan": true}, CreatedAt: now, UpdatedAt: now})
			continue
		}
		t := &(*ts)[i]
		t.Title, t.Status, t.Description, t.Priority = s.Step, s.Status, s.Description, s.Priority
		t.ActiveForm, t.Owner, t.Blocks, t.BlockedBy, t.UpdatedAt = s.ActiveForm, s.Owner, blocks, s.DependsOn, now
	}
}

// ---- usage guides and result notes ----------------------------------------------

func withNotes(msg string, notes []string) string {
	for _, n := range notes {
		msg += "\n" + n
	}
	return msg
}

// notesFor lists what the model should know about a task's dependencies after a
// change. When waiting is set the message already says what it waits on, so the
// waiting_on line is dropped.
func notesFor(ts []Task, t Task, waiting string) []string {
	var notes []string
	for _, n := range depNotes(ts, t) {
		if waiting != "" && strings.HasPrefix(n, "waiting_on:") {
			continue
		}
		notes = append(notes, n)
	}
	if t.Status == "cancelled" {
		var dependants []string
		for _, o := range ts {
			if isFinished(o.Status) {
				continue
			}
			for _, id := range o.BlockedBy {
				if id == t.TaskID {
					dependants = append(dependants, o.TaskID)
				}
			}
		}
		if len(dependants) > 0 {
			notes = append(notes, "Still waiting on this cancelled task: "+strings.Join(dependants, ", ")+
				". Remove or replace the dependency on each (task_update remove_blocked_by / add_blocked_by).")
		}
	}
	return notes
}

var taskChangeFields = []string{"title", "description", "status", "priority", "active_form", "owner", "reason",
	"blocks", "blocked_by", "add_blocked_by", "remove_blocked_by", "metadata"}

func hasAnyArg(args map[string]any, keys []string) bool {
	for _, k := range keys {
		if args[k] != nil {
			return true
		}
	}
	return false
}

func taskUpdateGuide(current string) string {
	return "task_update changes one task; you gave only task_id, so nothing was changed. Add at least one field to change:\n" +
		"- status: pending, in_progress, completed, blocked or cancelled (reason explains blocked/cancelled). " +
		"Example: {\"task_id\":\"t1\",\"status\":\"in_progress\"}\n" +
		"- dependencies: add_blocked_by, remove_blocked_by (or blocked_by to replace the list). " +
		"Example: {\"task_id\":\"t1\",\"add_blocked_by\":[\"t0\"]}\n" +
		"- other fields: title, description, priority, owner, active_form, blocks, metadata.\n\n" +
		"Current state of the task:\n" + current
}

func updatePlanGuide(note string, current planFile) string {
	var b strings.Builder
	if note == "" {
		note = "update_plan needs items or clear; nothing was changed."
	}
	b.WriteString(note + "\n")
	b.WriteString("Two modes:\n" +
		"1. items: replace the plan with these steps (they are merged into the to-do list by id). " +
		"Example: {\"items\":[{\"step\":\"Write tests\",\"status\":\"in_progress\"},{\"step\":\"Fix the bug\",\"depends_on\":[\"plan_1\"]}]}\n" +
		"2. clear: true resets the plan; the to-do list is left alone unless prune_tasks is true, which also removes the tasks earlier plans created. " +
		"Example: {\"clear\":true} or {\"clear\":true,\"prune_tasks\":true}\n")
	if len(current.Steps) == 0 {
		b.WriteString("Current plan: none.")
	} else {
		b.WriteString("Current plan:\n" + renderPlan(current))
	}
	return b.String()
}
