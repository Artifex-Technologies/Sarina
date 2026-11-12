package tools

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

var (
	todoTimeRE = regexp.MustCompile(`[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z`)
	todoSHARE  = regexp.MustCompile(`[0-9a-f]{64}`)
	todoIDRE   = regexp.MustCompile(`task_([0-9]+_)?[0-9a-f]{6}([0-9a-f]{4})?\b`)
)

// todoHide writes what changes from run to run as placeholders: the time, content hashes and random task ids.
func todoHide(s, top string) string {
	s = strings.ReplaceAll(s, top, "$BASE")
	s = todoTimeRE.ReplaceAllString(s, "TIME")
	s = todoSHARE.ReplaceAllString(s, "SHA")
	return todoIDRE.ReplaceAllString(s, "task_RAND")
}

// todoScenarios: the files the store starts with ("" for none, else "task=..." or "plan=..." or "portfile" for a
// .port_sessions that is a file), then the calls [tool, arguments as JSON].
func todoScenarios() [][2][][2]string {
	return [][2][][2]string{
		{{}, {
			{"task_list", `{}`}, {"task_create", `{"title":"Write tests","task_id":"t1","priority":"high"}`},
			{"task_create", `{"title":"Fix bug","task_id":"t2","blocked_by":["t1"],"description":"  the bug  "}`},
			{"task_create", `{"title":"dup","task_id":"t1"}`}, {"task_create", `{"title":"   "}`}, {"task_create", `{"title":"Anonymous"}`},
			{"task_update", `{"task_id":"t2","status":"in_progress"}`}, {"task_update", `{"task_id":"t2"}`},
			{"task_update", `{"task_id":"nope","status":"done"}`}, {"task_update", `{"task_id":"t1","status":"done"}`},
			{"task_list", `{}`}, {"task_list", `{"actionable_only":true}`}, {"task_list", `{"task_id":"t2"}`}, {"task_list", `{"task_id":"zzz"}`},
			{"task_update", `{"task_id":"t1","add_blocked_by":["t2"]}`}, {"task_update", `{"task_id":"t1","blocked_by":["t1"]}`},
			{"task_create", `{"title":"Ship","task_id":"t3","blocked_by":["t2","ghost"],"owner":"me","metadata":{"k":1.5,"n":1e21,"list":[1,"a",true,null],"m":{"z":1,"a":"b<&>"}}}`},
			{"task_update", `{"task_id":"t2","status":"cancelled","reason":"no longer needed"}`}, {"task_list", `{"task_id":"t3"}`},
			{"task_update", `{"task_id":"t3","remove_blocked_by":["t2"],"status":"blocked","reason":"waiting for review"}`},
			{"task_list", `{"status":"blocked"}`}, {"task_list", `{"owner":"me"}`}, {"task_list", `{"max_tasks":1}`},
			{"task_update", `{"task_id":"t3","status":"in_progress","active_form":"Shipping","priority":"critical","title":" Ship it "}`},
			{"task_list", `{}`}, {"task_update", `{"task_id":"t2","status":"blocked"}`}, {"task_update", `{"task_id":"t2","status":"pending","blocks":["t3"],"metadata":{}}`},
			{"todo_write", `{"items":[{"title":"Ship it"},{"title":"New one","status":"todo"},{"task_id":"t1","title":"Write tests"},{"title":"Later","task_id":"t4","blocked_by":["t1"]}]}`},
			{"task_list", `{}`}, {"todo_write", `{"items":[{"title":"A","task_id":"x"},{"title":"B","task_id":"x"}]}`},
			{"todo_write", `{"items":[{"title":"A","task_id":"x","blocked_by":["y"]},{"title":"B","task_id":"y","blocked_by":["x"]}]}`},
			{"todo_write", `{"items":[{"title":""}]}`}, {"todo_write", `{"items":[]}`}, {"task_list", `{}`},
		}},
		{{}, {
			{"plan_get", `{}`}, {"update_plan", `{}`}, {"update_plan", `{"clear":true,"items":[{"step":"x"}]}`},
			{"update_plan", `{"items":[{"step":"Write tests","status":"in_progress"},{"step":"Fix the bug","depends_on":["plan_1"]}],"explanation":"Two steps"}`},
			{"plan_get", `{}`}, {"task_list", `{}`},
			{"update_plan", `{"items":[{"step":"A","task_id":"a","depends_on":["b"]},{"step":"B","task_id":"b","depends_on":["a"]}]}`},
			{"update_plan", `{"items":[{"step":"A","task_id":"a"},{"step":"A2","task_id":"a"}]}`},
			{"update_plan", `{"items":[{"step":"Only","priority":"low","owner":"o","description":"d","active_form":"af"},{"step":"   "}],"sync_tasks":false}`},
			{"plan_get", `{}`}, {"update_plan", `{"clear":true}`}, {"task_list", `{}`}, {"update_plan", `{"clear":true,"prune_tasks":true}`},
			{"task_list", `{}`}, {"task_create", `{"title":"Mine","task_id":"t9","metadata":{"keep":"yes"}}`},
			{"update_plan", `{"items":[{"step":"Mine, planned","task_id":"t9","status":"done"},{"step":"After","depends_on":["t9"]}]}`},
			{"task_list", `{"task_id":"t9"}`}, {"task_list", `{"task_id":"plan_2"}`}, {"update_plan", `{"items":[{"step":"Self","task_id":"s","depends_on":["s"]}]}`},
		}},
		{{{"portfile", ""}}, {{"task_create", `{"title":"t"}`}, {"task_list", `{}`}, {"update_plan", `{"items":[{"step":"s"}]}`}, {"plan_get", `{}`}}},
		{{{"link", ""}}, {{"task_create", `{"title":"t"}`}, {"task_list", `{}`}, {"update_plan", `{"clear":true}`}, {"plan_get", `{}`}}},
		{{{"task", `{"tasks":[{"id":"a","title":" A ","status":"DONE"},{"task_id":"b","title":"B","status":"in-progress","blocked_by":["a","a"," ","c"]},{"task_id":"","title":"no id"},{"task_id":"c","title":""},{"task_id":"d","title":"D","status":"blocked","blocked_by":["gone"],"metadata":{"blocked_reason":"waiting_on:gone"}},{"task_id":"e","title":"E","status":"blocked","metadata":{"blocked_reason":"waiting_on:x"},"description":"   ","priority":" high "}]}`}},
			{{"task_list", `{}`}, {"task_list", `{"task_id":"d"}`}}},
		{{{"task", `{"tasks":[`}}, {{"task_list", `{}`}}},
		{{{"task", `{"tasks":[{"task_id":5,"title":"x"}]}`}}, {{"task_list", `{}`}}},
		{{{"task", `{"Tasks":[{"Task_ID":"x","TITLE":"Upper","Status":"Blocked","BLOCKED_BY":["y"]},{"task_id":"y","title":"Y","status":"in progress","metadata":null}]}`}}, {{"task_list", `{}`}, {"task_list", `{"task_id":"x"}`}}},
		{{{"task", `null`}}, {{"task_list", `{}`}}},
		{{{"task", `{"tasks":null,"extra":1}`}}, {{"task_list", `{}`}}},
		{{{"task", `{"tasks":[{"task_id":"z","title":"Z","metadata":{"b":[{"c":1}],"a":{"x":[]},"n":null,"f":0.000001,"g":100000000000000000000000}}]}`}}, {{"task_list", `{"task_id":"z"}`}}},
		{{{"plan", `{"explanation":"E","steps":[{"step":" S1 ","status":"Complete","task_id":"s1","depends_on":["s0","s0"]},{"step":"","task_id":"x"},{"step":"S2","priority":"high","owner":"me","description":"d","active_form":"a"}]}`}}, {{"plan_get", `{}`}}},
		{{{"plan", `{"steps":`}}, {{"plan_get", `{}`}}},
		{{}, {{"update_plan", `{"items":[{"step":"A","task_id":"a"},{"step":"Z","task_id":"z","depends_on":["a"]},{"step":"B","task_id":"b","depends_on":["a"]}]}`}, {"task_list", `{"task_id":"a"}`}}},
		{{{"task", `{"tasks":[{"task_id":"x","title":"X","blocked_by":["y"]},{"task_id":"y","title":"Y","blocked_by":["x"]}]}`}},
			{{"task_create", `{"title":"T","task_id":"t","blocked_by":["x"]}`}, {"task_list", `{}`}}},
		{{{"plan", `{"steps":[{"step":"x","depends_on":"notalist"}]}`}}, {{"plan_get", `{}`}}},
	}
}

// TestParityTodo records the to-do and plan tools (task_list, task_create, task_update, todo_write, plan_get,
// update_plan) through the registry, and the store files after each scenario (testdata/parity/todo.json); the time,
// content hashes and random ids are written as placeholders.
func TestParityTodo(t *testing.T) {
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	var cases []parity.Case
	for n, sc := range todoScenarios() {
		base := filepath.Join(top, string(rune('a'+n)))
		ws, outside := filepath.Join(base, "ws"), filepath.Join(base, "outside")
		for _, d := range []string{ws, outside} {
			if err := os.MkdirAll(d, 0o755); err != nil {
				t.Fatal(err)
			}
		}
		store := filepath.Join(ws, ".port_sessions")
		for _, f := range sc[0] {
			var err error
			switch f[0] {
			case "portfile":
				err = os.WriteFile(store, []byte("x"), 0o644)
			case "link":
				err = os.Symlink(outside, store)
			case "task", "plan":
				os.MkdirAll(store, 0o755)
				err = os.WriteFile(filepath.Join(store, f[0]+"_runtime.json"), []byte(f[1]), 0o644)
			}
			if err != nil {
				t.Fatal(err)
			}
		}
		tc := &Context{Root: ws, MaxOutputChars: 4000, Permissions: config.Permissions{AllowFileWrite: true}}
		reg := NewRegistry(TodoTools()...)
		var out []any
		for _, call := range sc[1] {
			var args map[string]any
			if err := json.Unmarshal([]byte(call[1]), &args); err != nil {
				t.Fatalf("%s: %v", call[1], err)
			}
			r := reg.Execute(context.Background(), call[0], args, tc)
			md, _ := json.Marshal(r.Metadata)
			out = append(out, []any{r.OK, parity.Text(todoHide(r.Content, top)), todoHide(string(md), top)})
		}
		var files []any
		for _, dir := range []string{store, outside} {
			for _, name := range []string{taskFileName, planFileName} {
				if data, err := os.ReadFile(filepath.Join(dir, name)); err == nil {
					rel, _ := filepath.Rel(base, filepath.Join(dir, name))
					files = append(files, []any{rel, parity.Text(todoHide(string(data), top))})
				}
			}
		}
		cases = append(cases, parity.Case{Fn: "scenario", In: []any{n, sc[0], sc[1]}, Out: []any{out, files}})
	}
	parity.Fixture(t, "todo", cases)
}
