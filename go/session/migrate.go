package session

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
)

// Seven tool names were folded into task_list, task_update and update_plan.
// Saved sessions may still carry calls and results under the old names; a
// model that sees them in its history tends to call them again and gets
// "unknown tool". MigrateRetiredTools rewrites such entries to the equivalent
// current calls. It is idempotent and leaves anything it does not recognise
// (an unparseable argument string, an unknown name) untouched.

// retiredResultName maps a retired tool name to the tool that replaced it.
var retiredResultName = map[string]string{
	"task_get": "task_list", "task_next": "task_list",
	"task_start": "task_update", "task_complete": "task_update", "task_cancel": "task_update", "task_block": "task_update",
	"plan_clear": "update_plan",
}

// IsRetiredTool reports whether name is a tool that was folded into another one.
func IsRetiredTool(name string) bool { _, ok := retiredResultName[name]; return ok }

// Migration counts what MigrateRetiredTools changed.
type Migration struct {
	Calls   int // assistant tool calls rewritten
	Results int // tool result messages renamed
}

// Changed reports whether anything was rewritten.
func (m Migration) Changed() bool { return m.Calls > 0 || m.Results > 0 }

// Add accumulates another migration.
func (m *Migration) Add(o Migration) { m.Calls += o.Calls; m.Results += o.Results }

// rewriteRetiredCall returns the replacement name and arguments for a retired
// call, or ok=false when the call is not retired or its arguments cannot be read.
func rewriteRetiredCall(name, arguments string) (newName, newArgs string, ok bool) {
	newName, retired := retiredResultName[name]
	if !retired {
		return "", "", false
	}
	old := map[string]any{}
	if strings.TrimSpace(arguments) != "" {
		if err := json.Unmarshal([]byte(arguments), &old); err != nil || old == nil {
			return "", "", false
		}
	}
	out := map[string]any{}
	copyKeys := func(keys ...string) {
		for _, k := range keys {
			if v, has := old[k]; has {
				out[k] = v
			}
		}
	}
	taskID := func() {
		if v, has := old["task_id"]; has {
			out["task_id"] = v
		} else if v, has := old["id"]; has {
			out["task_id"] = v
		}
	}
	switch name {
	case "task_get":
		taskID()
	case "task_next":
		out["actionable_only"] = true
		copyKeys("max_tasks")
	case "task_start":
		taskID()
		out["status"] = "in_progress"
		copyKeys("owner", "active_form")
	case "task_complete":
		taskID()
		out["status"] = "completed"
	case "task_cancel":
		taskID()
		out["status"] = "cancelled"
		copyKeys("reason")
	case "task_block":
		taskID()
		out["status"] = "blocked"
		copyKeys("reason")
		if v, has := old["blocked_by"]; has {
			out["add_blocked_by"] = v
		}
	case "plan_clear":
		out["clear"] = true
		if b, isBool := old["sync_tasks"].(bool); isBool && b {
			out["prune_tasks"] = true
		}
	}
	b, err := marshalRaw(out)
	if err != nil {
		return "", "", false
	}
	return newName, string(b), true
}

// renameResultBody renames the "tool" field at the head of a serialised tool
// result body (ToolResultBody's shape); other content is returned unchanged.
func renameResultBody(content, from, to string) string {
	for _, sep := range []string{"{\n  \"tool\": \"", "{\"tool\": \"", "{\"tool\":\""} {
		prefix := sep + from + "\""
		if strings.HasPrefix(content, prefix) {
			return sep + to + "\"" + content[len(prefix):]
		}
	}
	return content
}

// MigrateRetiredTools rewrites retired tool names in the history, in place.
func (s *Session) MigrateRetiredTools() Migration {
	var m Migration
	skipped := map[string]bool{} // ids of retired calls that could not be rewritten
	for i := range s.Messages {
		msg := &s.Messages[i]
		for j := range msg.ToolCalls {
			c := &msg.ToolCalls[j]
			if _, retired := retiredResultName[c.Function.Name]; !retired {
				continue
			}
			if n, a, ok := rewriteRetiredCall(c.Function.Name, c.Function.Arguments); ok {
				c.Function.Name, c.Function.Arguments = n, a
				m.Calls++
			} else {
				skipped[c.ID] = true
			}
		}
	}
	for i := range s.Messages {
		msg := &s.Messages[i]
		if msg.Role != "tool" || skipped[msg.ToolCallID] {
			continue
		}
		changed := false
		if to, retired := retiredResultName[msg.Name]; retired {
			msg.Content = renameResultBody(msg.Content, msg.Name, to)
			msg.Name = to
			changed = true
		}
		if msg.Metadata != nil {
			if to, retired := retiredResultName[strAt(msg.Metadata, "tool_name")]; retired {
				msg.Metadata["tool_name"] = to
				changed = true
			}
			switch a := strAt(msg.Metadata, "action"); a {
			case "task_get", "task_next", "task_start", "task_complete", "task_cancel", "task_block":
				msg.Metadata["action"] = retiredResultName[a]
				changed = true
			}
		}
		if changed {
			m.Results++
		}
	}
	return m
}

func strAt(m map[string]any, k string) string { s, _ := m[k].(string); return s }

// MigrateReport is what MigrateDir found.
type MigrateReport struct {
	Scanned, Changed, Skipped int
	Calls, Results            int
	Files                     []string // names of changed files
}

// MigrateDir refreshes every <id>.json in dir. With dryRun nothing is written.
// The first time a file changes, its original is copied to <id>.json.bak.
func MigrateDir(dir string, dryRun bool) (MigrateReport, error) {
	var r MigrateReport
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return r, nil
		}
		return r, err
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		raw, err := os.ReadFile(path)
		if err != nil {
			r.Skipped++
			continue
		}
		s, err := parse(raw)
		if err != nil {
			r.Skipped++
			continue
		}
		r.Scanned++
		m := s.MigrateRetiredTools()
		if !m.Changed() {
			continue
		}
		r.Changed++
		r.Calls += m.Calls
		r.Results += m.Results
		r.Files = append(r.Files, e.Name())
		if dryRun {
			continue
		}
		bak := path + ".bak"
		if _, statErr := os.Stat(bak); os.IsNotExist(statErr) {
			if err := os.WriteFile(bak, raw, 0o600); err != nil {
				return r, err
			}
		}
		data, err := s.Marshal()
		if err != nil {
			return r, err
		}
		if err := writeAtomic(path, s.ID, data); err != nil {
			return r, err
		}
	}
	return r, nil
}
