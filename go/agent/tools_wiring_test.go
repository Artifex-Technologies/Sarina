package agent

import (
	"os/exec"
	"strings"
	"testing"
)

func toolCallSSE(id, name, args string) scripted {
	frag := map[string]any{"index": 0, "id": id, "function": map[string]any{"name": name, "arguments": args}}
	return scripted{sse: []any{
		map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"tool_calls": []any{frag}}}}},
		map[string]any{"choices": []any{map[string]any{"delta": map[string]any{}, "finish_reason": "tool_calls"}}, "usage": use(5, 5)},
	}}
}

func finalSSE(text string) scripted {
	return scripted{sse: []any{
		map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"content": text}, "finish_reason": "stop"}}, "usage": use(5, 5)},
	}}
}

func denials(res *Result) []map[string]any {
	var out []map[string]any
	for _, e := range res.Events {
		if e["type"] == "tool_permission_denial" {
			out = append(out, e)
		}
	}
	return out
}

// The permission denial for bash must be reported the same way whether the
// model is streamed or not (the original dropped the error kind when streaming).
func TestBashDenialEmitsPermissionEventOnBothPaths(t *testing.T) {
	for _, stream := range []bool{false, true} {
		var f *fakeServer
		if stream {
			f = newFakeServer(t, toolCallSSE("c1", "bash", `{"command":"echo hi"}`), finalSSE("ok"))
		} else {
			f = newFakeServer(t, reply("", "tool_calls", use(1, 1), tcall{"c1", "bash", `{"command":"echo hi"}`}), reply("ok", "stop", use(1, 1)))
		}
		a := newTestAgent(t, f, func(o *Options) { o.Runtime.StreamModelResponses = stream })
		res := mustRun(t, a, "run it")
		d := denials(res)
		if len(d) != 1 || d[0]["tool_name"] != "bash" || !strings.Contains(d[0]["reason"].(string), "--allow-shell") {
			t.Fatalf("stream=%v: denial events = %v", stream, d)
		}
		tool := res.Transcript[4]
		if !strings.Contains(tool.Content, "permission_denied") || !strings.Contains(tool.Content, `"ok": false`) {
			t.Fatalf("stream=%v: tool message = %s", stream, tool.Content)
		}
	}
}

func TestBashStreamsThroughTheAgent(t *testing.T) {
	if _, err := exec.LookPath("bash"); err != nil {
		t.Skip("bash not available")
	}
	f := newFakeServer(t, toolCallSSE("c1", "bash", `{"command":"echo from-bash"}`), finalSSE("ok"))
	a := newTestAgent(t, f, func(o *Options) {
		o.Runtime.StreamModelResponses = true
		o.Runtime.Permissions.AllowShellCommands = true
	})
	res := mustRun(t, a, "run it")
	var live []string
	for _, e := range res.Events {
		if e["type"] == "tool_delta" && e["stream"] == "stdout" {
			live = append(live, e["delta"].(string))
		}
	}
	if !strings.Contains(strings.Join(live, ""), "from-bash") {
		t.Fatalf("expected live stdout deltas, events: %v", res.Events)
	}
	if res.ToolCalls != 1 || !strings.Contains(res.Transcript[4].Content, "from-bash") {
		t.Fatalf("result: %+v", res.Transcript[4])
	}
}

func TestDefaultRegistryHasTheCoreToolsAndAllowlistStillWins(t *testing.T) {
	f := newFakeServer(t, reply("hi", "stop", use(1, 1)))
	a := newTestAgent(t, f, withWrite, func(o *Options) { o.Runtime.Permissions.AllowShellCommands = true })
	for _, name := range []string{"read_file", "write_file", "edit_file", "glob_search", "grep_search", "bash", "TaskOutput", "TaskStop", "task_create", "todo_write", "update_plan"} {
		if _, ok := a.Registry().Get(name); !ok {
			t.Errorf("default registry lacks %s", name)
		}
	}
	none := newTestAgent(t, f, func(o *Options) { o.Tools.Allowlist = []string{} })
	if none.ToolCount() != 0 {
		t.Fatalf("an empty allowlist must still mean zero tools, got %d", none.ToolCount())
	}
}
