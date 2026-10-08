package agent

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

const testKey = "test-api-key-do-not-persist-4242"

// scripted is one canned backend reply.
type scripted struct {
	status int
	body   any
	sse    []any // each element becomes a "data:" event
	delay  time.Duration
}

type fakeServer struct {
	*httptest.Server
	mu    sync.Mutex
	reqs  []map[string]any
	auths []string
	left  []scripted
}

func newFakeServer(t *testing.T, script ...scripted) *fakeServer {
	t.Helper()
	f := &fakeServer{left: script}
	f.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		var req map[string]any
		_ = json.Unmarshal(b, &req)
		f.mu.Lock()
		f.reqs = append(f.reqs, req)
		f.auths = append(f.auths, r.Header.Get("Authorization"))
		var s scripted
		if len(f.left) > 0 {
			s, f.left = f.left[0], f.left[1:]
		} else {
			s = scripted{status: 400, body: map[string]any{"error": "script exhausted"}}
		}
		f.mu.Unlock()
		if s.delay > 0 {
			time.Sleep(s.delay)
		}
		if s.sse != nil {
			w.Header().Set("Content-Type", "text/event-stream")
			for _, ev := range s.sse {
				if str, ok := ev.(string); ok {
					fmt.Fprintf(w, "data: %s\n\n", str)
				} else {
					j, _ := json.Marshal(ev)
					fmt.Fprintf(w, "data: %s\n\n", j)
				}
				if fl, ok := w.(http.Flusher); ok {
					fl.Flush()
				}
			}
			fmt.Fprint(w, "data: [DONE]\n\n")
			return
		}
		if s.status != 0 {
			w.WriteHeader(s.status)
		}
		j, _ := json.Marshal(s.body)
		w.Write(j)
	}))
	t.Cleanup(f.Close)
	return f
}

func (f *fakeServer) requests() []map[string]any {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]map[string]any(nil), f.reqs...)
}

type tcall struct{ ID, Name, Args string }

func reply(content, finish string, usage map[string]any, calls ...tcall) scripted {
	msg := map[string]any{"role": "assistant", "content": content}
	if len(calls) > 0 {
		var tcs []any
		for _, c := range calls {
			tcs = append(tcs, map[string]any{"id": c.ID, "type": "function",
				"function": map[string]any{"name": c.Name, "arguments": c.Args}})
		}
		msg["tool_calls"] = tcs
	}
	body := map[string]any{"choices": []any{map[string]any{"message": msg, "finish_reason": finish}}}
	if usage != nil {
		body["usage"] = usage
	}
	return scripted{body: body}
}

func use(prompt, completion int) map[string]any {
	return map[string]any{"prompt_tokens": prompt, "completion_tokens": completion}
}

func reqMessages(req map[string]any) []map[string]any {
	var out []map[string]any
	list, _ := req["messages"].([]any)
	for _, m := range list {
		out = append(out, m.(map[string]any))
	}
	return out
}

func lastUserText(req map[string]any) string {
	ms := reqMessages(req)
	for i := len(ms) - 1; i >= 0; i-- {
		if ms[i]["role"] == "user" {
			s, _ := ms[i]["content"].(string)
			return s
		}
	}
	return ""
}

func fixedNow() time.Time { return time.Date(2026, 10, 4, 12, 0, 0, 0, time.UTC) }

func testOptions(t *testing.T, f *fakeServer) Options {
	t.Helper()
	dir := t.TempDir()
	rt := config.DefaultRuntime(dir)
	rt.DisableClaudeMdDiscovery = true
	m := config.ModelConfig{Model: "test-model", BaseURL: f.URL, APIKey: testKey, TimeoutSeconds: 5}
	return Options{Model: m, Runtime: rt, Client: &llm.Client{Config: m, RetryDelay: time.Millisecond},
		Home: dir, Now: fixedNow}
}

func newTestAgent(t *testing.T, f *fakeServer, mut ...func(*Options)) *Agent {
	t.Helper()
	opts := testOptions(t, f)
	for _, m := range mut {
		m(&opts)
	}
	a, err := New(opts)
	if err != nil {
		t.Fatal(err)
	}
	return a
}

func writeFile(t *testing.T, a *Agent, name, content string) string {
	t.Helper()
	p := filepath.Join(a.opts.Runtime.CWD, name)
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

func mustRun(t *testing.T, a *Agent, prompt string) *Result {
	t.Helper()
	res, err := a.Run(t.Context(), prompt)
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	return res
}

// assertPaired fails unless every assistant tool call is answered by exactly
// one tool message immediately after it, and no tool message is orphaned.
func assertPaired(t *testing.T, msgs []session.Message) {
	t.Helper()
	pending := map[string]bool{}
	for i, m := range msgs {
		if m.Role == "tool" {
			if !pending[m.ToolCallID] {
				t.Fatalf("message %d: tool result %q has no pending call", i, m.ToolCallID)
			}
			delete(pending, m.ToolCallID)
			continue
		}
		if len(pending) > 0 {
			t.Fatalf("message %d (%s): unanswered tool calls %v", i, m.Role, pending)
		}
		for _, c := range m.ToolCalls {
			pending[c.ID] = true
		}
	}
	if len(pending) > 0 {
		t.Fatalf("transcript ends with unanswered tool calls %v", pending)
	}
}

func assertPairedWire(t *testing.T, msgs []map[string]any) {
	t.Helper()
	var sm []session.Message
	for _, m := range msgs {
		sm = append(sm, session.MessageFromMap(m))
	}
	assertPaired(t, sm)
}

func eventTypes(res *Result) []string {
	var out []string
	for _, e := range res.Events {
		out = append(out, e["type"].(string))
	}
	return out
}

func hasEvent(res *Result, typ string) bool {
	for _, e := range res.Events {
		if e["type"] == typ {
			return true
		}
	}
	return false
}

func countEvents(res *Result, typ string) int {
	n := 0
	for _, e := range res.Events {
		if e["type"] == typ {
			n++
		}
	}
	return n
}

// customTool builds a tool for tests.
func customTool(name string, h tools.Handler) tools.Tool {
	return tools.Tool{Spec: llm.ToolSpec{Name: name, Description: "test tool " + name,
		Parameters: map[string]any{"type": "object", "properties": map[string]any{}}}, Handler: h}
}

func contains(t *testing.T, haystack, needle string) {
	t.Helper()
	if !strings.Contains(haystack, needle) {
		t.Fatalf("expected %q to contain %q", haystack, needle)
	}
}

type (
	toolT   = tools.Tool
	toolCtx = tools.Context
)

func perms() config.Permissions { return config.Permissions{} }

func a0() config.RuntimeConfig { return config.DefaultRuntime(".") }
