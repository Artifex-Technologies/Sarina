package agent

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

func probeTool(counter *int) toolT {
	return customTool("probe", func(context.Context, map[string]any, *toolCtx) (string, map[string]any, error) {
		*counter++
		return "probed", nil, nil
	})
}

// DO NOT REPLICATE 5.5.3: a tool-budget stop used to leave unanswered calls.
func TestBudgetStopAnswersEveryToolCall(t *testing.T) {
	runs := 0
	f := newFakeServer(t, reply("", "tool_calls", use(1, 1),
		tcall{"a", "probe", `{}`}, tcall{"b", "probe", `{}`}, tcall{"c", "probe", `{}`}))
	a := newTestAgent(t, f, func(o *Options) {
		o.Runtime.BudgetConfig.MaxToolCalls = intp(1)
		o.Tools.Extra = []toolT{probeTool(&runs)}
	})
	res := mustRun(t, a, "go")
	if res.StopReason != StopBudgetExceeded || runs != 1 {
		t.Fatalf("stop=%s runs=%d", res.StopReason, runs)
	}
	assertPaired(t, res.Transcript)
	last := res.Transcript[len(res.Transcript)-1]
	contains(t, last.Content, "not run")
	if !hasEvent(res, "task_budget_exceeded") {
		t.Fatalf("events = %v", eventTypes(res))
	}
	// the stored file must also be resumable
	stored, _ := session.Load(res.SessionPath)
	assertPaired(t, stored.Messages)
}

func TestTokenBudgetStopWithToolCallsStaysPaired(t *testing.T) {
	f := newFakeServer(t, reply("", "tool_calls", use(50, 50), tcall{"a", "read_file", `{"path":"x"}`}))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.BudgetConfig.MaxTotalTokens = intp(10) })
	res := mustRun(t, a, "go")
	if res.StopReason != StopBudgetExceeded {
		t.Fatalf("stop = %s", res.StopReason)
	}
	assertPaired(t, res.Transcript)
	if res.ToolCalls != 0 {
		t.Fatalf("no call should have run, got %d", res.ToolCalls)
	}
}

// DO NOT REPLICATE 5.9: counters double-counted on resume.
func TestResumeCountersAreCumulativeOnce(t *testing.T) {
	runs := 0
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"a", "probe", `{}`}), reply("one", "stop", use(1, 1)),
		reply("", "tool_calls", use(1, 1), tcall{"b", "probe", `{}`}), reply("two", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Tools.Extra = []toolT{probeTool(&runs)} })
	first := mustRun(t, a, "x")
	stored, _ := session.Load(first.SessionPath)
	second, err := a.Resume(t.Context(), "y", stored)
	if err != nil {
		t.Fatal(err)
	}
	if second.ToolCalls != 2 {
		t.Fatalf("result tool calls = %d", second.ToolCalls)
	}
	after, _ := session.Load(second.SessionPath)
	if after.ToolCalls != 2 || after.BudgetState.ToolCalls != 2 {
		t.Fatalf("persisted tool calls = %d / %d, want 2", after.ToolCalls, after.BudgetState.ToolCalls)
	}
	if after.BudgetState.ModelCalls != 4 || after.Turns != 4 {
		t.Fatalf("model calls=%d turns=%d, want 4 and 4", after.BudgetState.ModelCalls, after.Turns)
	}
	if second.Turns != 2 {
		t.Fatalf("result turns are per call, got %d", second.Turns)
	}
	if after.Usage.Total() != 8 {
		t.Fatalf("usage = %+v", after.Usage)
	}
}

func TestInterruptAnswersRemainingCalls(t *testing.T) {
	runs := 0
	ctx, cancel := context.WithCancel(t.Context())
	f := newFakeServer(t, reply("", "tool_calls", use(1, 1), tcall{"a", "probe", `{}`}, tcall{"b", "probe", `{}`}))
	a := newTestAgent(t, f, func(o *Options) {
		o.Tools.Extra = []toolT{customTool("probe", func(context.Context, map[string]any, *toolCtx) (string, map[string]any, error) {
			runs++
			cancel() // Ctrl-C arrives while the first call runs
			return "partial", nil, nil
		})}
	})
	res, err := a.Run(ctx, "go")
	if err != nil {
		t.Fatal(err)
	}
	if res.StopReason != StopInterrupted || runs != 1 {
		t.Fatalf("stop=%s runs=%d", res.StopReason, runs)
	}
	assertPaired(t, res.Transcript)
	if _, err := os.Stat(res.SessionPath); err != nil {
		t.Fatalf("interrupted run must still be saved: %v", err)
	}
}

func TestInterruptDuringModelCall(t *testing.T) {
	f := newFakeServer(t, scripted{delay: 300 * time.Millisecond, body: map[string]any{}})
	a := newTestAgent(t, f)
	ctx, cancel := context.WithTimeout(t.Context(), 50*time.Millisecond)
	defer cancel()
	res, err := a.Run(ctx, "go")
	if err != nil {
		t.Fatal(err)
	}
	if res.StopReason != StopInterrupted {
		t.Fatalf("stop = %s (%s)", res.StopReason, res.FinalOutput)
	}
}

// Contract 1: tool_allowlist [] means no tools, and the request omits them.
func TestEmptyAllowlistMeansNoTools(t *testing.T) {
	f := newFakeServer(t, reply("advice", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Tools.Allowlist = []string{} })
	if a.ToolCount() != 0 {
		t.Fatalf("tool count = %d", a.ToolCount())
	}
	mustRun(t, a, "advise")
	req := f.requests()[0]
	if _, ok := req["tools"]; ok {
		t.Fatal("tools must be omitted")
	}
	if _, ok := req["tool_choice"]; ok {
		t.Fatal("tool_choice must be omitted")
	}
	if err := a.rebuildTools(a.opts.Runtime.CWD); err != nil || a.ToolCount() != 0 {
		t.Fatalf("rebuild resurrected tools: %d (%v)", a.ToolCount(), err)
	}
	sys := reqMessages(req)[0]["content"].(string)
	contains(t, sys, "No tools are available")
}

func TestToolsAndToolChoiceSentWhenPresent(t *testing.T) {
	f := newFakeServer(t, reply("hi", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	mustRun(t, a, "hi")
	req := f.requests()[0]
	if req["tool_choice"] != "auto" {
		t.Fatalf("tool_choice = %v", req["tool_choice"])
	}
	specs := req["tools"].([]any)
	fn := specs[0].(map[string]any)
	if fn["type"] != "function" || fn["function"].(map[string]any)["name"] != "read_file" {
		t.Fatalf("tool spec = %v", fn)
	}
	if f.auths[0] != "Bearer "+testKey {
		t.Fatalf("auth header = %q", f.auths[0])
	}
}

func TestAllowlistRestrictsAndSurvivesCwdUpdate(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"a", "cd", `{}`}),
		reply("moved", "stop", use(1, 1)))
	sub := ""
	a := newTestAgent(t, f, func(o *Options) {
		o.Tools.Allowlist = []string{"cd", "does_not_exist"}
		o.Tools.Extra = []toolT{customTool("cd", func(_ context.Context, _ map[string]any, tc *toolCtx) (string, map[string]any, error) {
			sub = filepath.Join(tc.Root, "sub")
			if err := os.MkdirAll(sub, 0o755); err != nil {
				return "", nil, err
			}
			return "moved", map[string]any{"cwd_update": sub}, nil
		})}
	})
	res := mustRun(t, a, "move")
	if res.FinalOutput != "moved" {
		t.Fatalf("final = %q", res.FinalOutput)
	}
	if names := a.Registry().Names(); len(names) != 1 || names[0] != "cd" {
		t.Fatalf("after rebuild the allowlist was lost: %v", names)
	}
	if !strings.HasSuffix(filepath.ToSlash(a.toolCtx.Root), "/sub") {
		t.Fatalf("root did not switch: %s", a.toolCtx.Root)
	}
	specs := f.requests()[1]["tools"].([]any)
	if len(specs) != 1 {
		t.Fatalf("second request advertised %d tools", len(specs))
	}
}

func TestReactiveCompactionRetriesOnce(t *testing.T) {
	f := newFakeServer(t,
		scripted{status: 400, body: map[string]any{"error": map[string]any{"message": "This model's maximum context length is 4096 tokens"}}},
		reply("recovered", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	s, err := a.newSession()
	if err != nil {
		t.Fatal(err)
	}
	big := strings.Repeat("filler text ", 80)
	for i := 0; i < 6; i++ {
		s.Append(session.Message{Role: "user", Content: fmt.Sprintf("question %d", i), MessageID: s.NextMessageID("user")})
		id := fmt.Sprintf("call_%d", i)
		s.Append(session.Message{Role: "assistant", Content: big, MessageID: s.NextMessageID("assistant"),
			ToolCalls: []llm.WireToolCall{{ID: id, Type: "function", Function: llm.FunctionCall{Name: "read_file", Arguments: "{}"}}}})
		s.Append(session.Message{Role: "tool", Name: "read_file", ToolCallID: id, Content: big, MessageID: s.NextMessageID("tool")})
	}
	res, err := a.Resume(t.Context(), "continue", s)
	if err != nil {
		t.Fatal(err)
	}
	if res.FinalOutput != "recovered" || !hasEvent(res, "reactive_compact_retry") {
		t.Fatalf("final=%q events=%v", res.FinalOutput, eventTypes(res))
	}
	reqs := f.requests()
	if len(reqs) != 2 {
		t.Fatalf("requests = %d", len(reqs))
	}
	size := func(r map[string]any) int { b, _ := json.Marshal(r["messages"]); return len(b) }
	if size(reqs[1]) >= size(reqs[0]) {
		t.Fatalf("retry was not smaller: %d vs %d", size(reqs[1]), size(reqs[0]))
	}
	assertPairedWire(t, reqMessages(reqs[1]))
	assertPaired(t, res.Transcript)
}

func TestReactiveCompactionGivesUpAfterOneRetry(t *testing.T) {
	tooLong := scripted{status: 400, body: map[string]any{"error": "prompt is too long"}}
	f := newFakeServer(t, tooLong, tooLong, tooLong)
	a := newTestAgent(t, f)
	s, _ := a.newSession()
	for i := 0; i < 8; i++ {
		s.Append(session.Message{Role: "user", Content: fmt.Sprintf("q%d %s", i, strings.Repeat("x", 300)), MessageID: s.NextMessageID("user")})
		s.Append(session.Message{Role: "assistant", Content: "a", MessageID: s.NextMessageID("assistant")})
	}
	res, _ := a.Resume(t.Context(), "go", s)
	if res.StopReason != StopBackendError {
		t.Fatalf("stop = %s", res.StopReason)
	}
	if n := len(f.requests()); n != 2 {
		t.Fatalf("expected exactly one retry (2 requests), got %d", n)
	}
}

// DO NOT REPLICATE 7.4: compaction must not split call/result pairs.
func TestCompactionNeverSplitsPairs(t *testing.T) {
	build := func() *session.Session {
		s := &session.Session{}
		add := func(m session.Message) { m.MessageID = s.NextMessageID(m.Role); s.Append(m) }
		add(session.Message{Role: "system", Content: "sys"})
		add(session.Message{Role: "user", Content: session.ReminderOpen + " ctx " + session.ReminderClose})
		add(session.Message{Role: "user", Content: "q1"})
		add(session.Message{Role: "assistant", Content: "calling", ToolCalls: []llm.WireToolCall{
			{ID: "c1", Type: "function", Function: llm.FunctionCall{Name: "t", Arguments: "{}"}},
			{ID: "c2", Type: "function", Function: llm.FunctionCall{Name: "t", Arguments: "{}"}}}})
		add(session.Message{Role: "tool", Name: "t", ToolCallID: "c1", Content: "r1"})
		add(session.Message{Role: "tool", Name: "t", ToolCallID: "c2", Content: "r2"})
		add(session.Message{Role: "user", Content: "q2"})
		add(session.Message{Role: "assistant", Content: "calling again", ToolCalls: []llm.WireToolCall{
			{ID: "c3", Type: "function", Function: llm.FunctionCall{Name: "t", Arguments: "{}"}}}})
		add(session.Message{Role: "tool", Name: "t", ToolCallID: "c3", Content: "r3"})
		add(session.Message{Role: "assistant", Content: "done"})
		return s
	}
	for preserve := 0; preserve <= 9; preserve++ {
		s := build()
		_, ok := compactPass(s, reduceOpts{preserve: preserve, turn: 1})
		assertPaired(t, s.Messages)
		if ok && s.Messages[0].Role != "system" {
			t.Fatalf("preserve=%d: system message lost", preserve)
		}
	}
	// preserve=2 would naively start the tail at c3's tool result; the cut must move back to its call
	s := build()
	ev, ok := compactPass(s, reduceOpts{preserve: 2, turn: 1})
	if !ok {
		t.Fatal("expected compaction")
	}
	var tailHasCall bool
	for _, m := range s.Messages {
		for _, c := range m.ToolCalls {
			if c.ID == "c3" {
				tailHasCall = true
			}
		}
	}
	if !tailHasCall {
		t.Fatalf("assistant call c3 was compacted away from its result; event=%v", ev)
	}
}

func TestSnipKeepsPairing(t *testing.T) {
	s := &session.Session{}
	big := strings.Repeat("payload ", 200)
	s.Append(session.Message{Role: "system", Content: "sys", MessageID: "system_0"})
	s.Append(session.Message{Role: "user", Content: "go", MessageID: "user_1"})
	s.Append(session.Message{Role: "assistant", Content: "", MessageID: "assistant_2", ToolCalls: []llm.WireToolCall{
		{ID: "c1", Type: "function", Function: llm.FunctionCall{Name: "t", Arguments: "{}"}}}})
	s.Append(session.Message{Role: "tool", Name: "t", ToolCallID: "c1", Content: big, MessageID: "tool_3"})
	s.Append(session.Message{Role: "user", Content: "next", MessageID: "user_4"})
	ev, ok := snipPass(s, reduceOpts{target: 0, preserve: 1, turn: 2})
	if !ok || ev["type"] != "snip_boundary" {
		t.Fatalf("snip failed: %v", ev)
	}
	assertPaired(t, s.Messages)
	tomb := s.Messages[3]
	if tomb.State != session.StateTombstoned || tomb.ToolCallID != "c1" || tomb.Kind() != session.KindSnipped {
		t.Fatalf("tombstone = %+v", tomb)
	}
	if _, again := snipPass(s, reduceOpts{target: 0, preserve: 1, turn: 3}); again {
		t.Fatal("a tombstone must not be snipped again")
	}
}

func TestMalformedArgumentsGoBackToModel(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"a", "read_file", `not json at all`}),
		reply("retrying", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	res := mustRun(t, a, "go")
	if res.StopReason != "stop" || res.FinalOutput != "retrying" {
		t.Fatalf("a bad argument string must not abort the run: %s %q", res.StopReason, res.FinalOutput)
	}
	tool := res.Transcript[4]
	contains(t, tool.Content, "could not be used")
	if tool.Metadata["error_kind"] != "invalid_arguments" {
		t.Fatalf("metadata = %v", tool.Metadata)
	}
	if res.Transcript[3].ToolCalls[0].Function.Arguments != "{}" {
		t.Fatalf("stored arguments must stay valid JSON: %q", res.Transcript[3].ToolCalls[0].Function.Arguments)
	}
	assertPaired(t, res.Transcript)
}

func TestRepairedArgumentsAreExecuted(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"a", "read_file", "{'path': 'r.txt',}"}),
		reply("read it", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	writeFile(t, a, "r.txt", "repaired content")
	res := mustRun(t, a, "go")
	contains(t, res.Transcript[4].Content, "repaired content")
}

func TestTextEmbeddedToolCallIsExecuted(t *testing.T) {
	f := newFakeServer(t,
		reply("I will read it.\n<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"e.txt\"}}\n</tool_call>", "stop", use(1, 1)),
		reply("done", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	writeFile(t, a, "e.txt", "embedded works")
	res := mustRun(t, a, "go")
	if res.ToolCalls != 1 || res.FinalOutput != "done" {
		t.Fatalf("tool calls=%d final=%q", res.ToolCalls, res.FinalOutput)
	}
	asst := res.Transcript[3]
	if strings.Contains(asst.Content, "tool_call") || len(asst.ToolCalls) != 1 {
		t.Fatalf("assistant = %+v", asst)
	}
	contains(t, res.Transcript[4].Content, "embedded works")
}

func TestEmptyAndDuplicateIDsInOneMessageAllRun(t *testing.T) {
	runs := 0
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"", "probe", `{}`}, tcall{"", "probe", `{}`},
			tcall{"x", "probe", `{}`}, tcall{"x", "probe", `{}`}),
		reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Tools.Extra = []toolT{probeTool(&runs)} })
	res := mustRun(t, a, "go")
	if runs != 4 || res.ToolCalls != 4 {
		t.Fatalf("runs=%d toolCalls=%d: legitimate calls were swallowed", runs, res.ToolCalls)
	}
	seen := map[string]bool{}
	for _, c := range res.Transcript[3].ToolCalls {
		if c.ID == "" || seen[c.ID] {
			t.Fatalf("ids not unique: %+v", res.Transcript[3].ToolCalls)
		}
		seen[c.ID] = true
	}
	assertPaired(t, res.Transcript)
}

func TestMissingUsageIsEstimated(t *testing.T) {
	f := newFakeServer(t, reply(strings.Repeat("word ", 100), "stop", nil))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.BudgetConfig.MaxTotalTokens = intp(5) })
	res := mustRun(t, a, "hello there")
	if res.Usage.Total() == 0 {
		t.Fatal("usage must fall back to an estimate")
	}
	if res.StopReason != StopBudgetExceeded {
		t.Fatalf("token budgets must still trip without server usage; stop=%s", res.StopReason)
	}
	if res.Transcript[len(res.Transcript)-1].Metadata["usage_estimated"] != true {
		t.Fatal("estimated usage should be flagged")
	}
}

// DO NOT REPLICATE 12.6: bad streamed arguments crashed the run.
func TestStreamingMalformedArgumentsDoNotCrash(t *testing.T) {
	f := newFakeServer(t,
		scripted{sse: []any{
			map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"tool_calls": []any{
				map[string]any{"index": 0, "id": "c1", "function": map[string]any{"name": "read_file", "arguments": "{{{"}}}}}}},
			map[string]any{"choices": []any{map[string]any{"delta": map[string]any{}, "finish_reason": "tool_calls"}}},
		}},
		scripted{sse: []any{map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"content": "recovered"}, "finish_reason": "stop"}}}}})
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.StreamModelResponses = true })
	res := mustRun(t, a, "go")
	if res.FinalOutput != "recovered" {
		t.Fatalf("final = %q stop=%s", res.FinalOutput, res.StopReason)
	}
	contains(t, res.Transcript[4].Content, "could not be used")
}

func TestToolResultsAreRedacted(t *testing.T) {
	secret := "ghp_" + strings.Repeat("a1B2", 6)
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"a", "read_file", `{"path":"s.txt"}`}),
		reply("seen", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	writeFile(t, a, "s.txt", "token="+secret)
	res := mustRun(t, a, "go")
	if strings.Contains(res.Transcript[4].Content, secret) || !strings.Contains(res.Transcript[4].Content, "[REDACTED:github]") {
		t.Fatalf("tool content = %s", res.Transcript[4].Content)
	}
	b, _ := json.Marshal(f.requests()[1])
	if strings.Contains(string(b), secret) {
		t.Fatal("the secret reached the second model request")
	}
}

// DO NOT REPLICATE 12.5: timeouts escaped the error type.
func TestBackendTimeoutBecomesBackendError(t *testing.T) {
	f := newFakeServer(t, scripted{delay: 400 * time.Millisecond, body: map[string]any{}})
	a := newTestAgent(t, f, func(o *Options) {
		o.Model.TimeoutSeconds = 0.05
		o.Client = &llm.Client{Config: o.Model, RetryDelay: time.Millisecond}
	})
	res := mustRun(t, a, "go")
	if res.StopReason != StopBackendError {
		t.Fatalf("stop = %s", res.StopReason)
	}
	contains(t, res.FinalOutput, "timed out")
}

func TestBackendErrorCarriesStatusAndBody(t *testing.T) {
	f := newFakeServer(t, scripted{status: 400, body: map[string]any{"error": "model exploded"}})
	a := newTestAgent(t, f)
	res := mustRun(t, a, "go")
	if res.StopReason != StopBackendError || res.Turns != 0 {
		t.Fatalf("stop=%s turns=%d", res.StopReason, res.Turns)
	}
	contains(t, res.FinalOutput, "400")
	contains(t, res.FinalOutput, "model exploded")
	if _, err := os.Stat(res.SessionPath); err != nil {
		t.Fatal("a failed run must still be saved")
	}
}

func TestMaxOutputTokensForwardedAsMaxTokens(t *testing.T) {
	f := newFakeServer(t, reply("a", "stop", use(1, 1)), reply("b", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.BudgetConfig.MaxOutputTokens = intp(300) })
	mustRun(t, a, "go")
	if mt, _ := f.requests()[0]["max_tokens"].(float64); mt != 300 {
		t.Fatalf("max_tokens = %v", f.requests()[0]["max_tokens"])
	}
	f2 := newFakeServer(t, reply("a", "stop", use(1, 1)))
	b := newTestAgent(t, f2)
	mustRun(t, b, "go")
	if _, ok := f2.requests()[0]["max_tokens"]; ok {
		t.Fatal("max_tokens must be absent when unset")
	}
}

func TestResumeRepairsDanglingToolCalls(t *testing.T) {
	f := newFakeServer(t, reply("fine", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	s, _ := a.newSession()
	s.Append(session.Message{Role: "user", Content: "old", MessageID: s.NextMessageID("user")})
	s.Append(session.Message{Role: "assistant", MessageID: s.NextMessageID("assistant"), ToolCalls: []llm.WireToolCall{
		{ID: "dangling", Type: "function", Function: llm.FunctionCall{Name: "read_file", Arguments: "{}"}}}})
	res, err := a.Resume(t.Context(), "again", s)
	if err != nil {
		t.Fatal(err)
	}
	if res.FinalOutput != "fine" {
		t.Fatalf("final = %q", res.FinalOutput)
	}
	assertPairedWire(t, reqMessages(f.requests()[0]))
}

func TestResumeAddsFileHistoryReplayOnce(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"a", "write_file", `{"path":"h.txt","content":"x"}`}), reply("one", "stop", use(1, 1)),
		reply("two", "stop", use(1, 1)), reply("three", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.Permissions.AllowFileWrite = true })
	first := mustRun(t, a, "go")
	count := func(r *Result) int {
		n := 0
		for _, m := range r.Transcript {
			if m.Kind() == session.KindFileHistoryReplay {
				n++
			}
		}
		return n
	}
	s1, _ := session.Load(first.SessionPath)
	r2, _ := a.Resume(t.Context(), "more", s1)
	if count(r2) != 1 {
		t.Fatalf("replay messages = %d", count(r2))
	}
	s2, _ := session.Load(r2.SessionPath)
	r3, _ := a.Resume(t.Context(), "more", s2)
	if count(r3) != 1 {
		t.Fatalf("replay duplicated on the next resume: %d", count(r3))
	}
}

// DO NOT REPLICATE 8.1: the dynamic-boundary sentinel leaked into the prompt.
func TestNoSentinelInSystemPrompt(t *testing.T) {
	f := newFakeServer(t, reply("hi", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	res := mustRun(t, a, "hi")
	sys := res.Transcript[0].Content
	if strings.Contains(strings.ToUpper(sys), "DYNAMIC") || strings.Contains(sys, "BOUNDARY") {
		t.Fatal("sentinel leaked")
	}
	contains(t, sys, "Sarina")
	contains(t, sys, "# Environment")
	ctxMsg := res.Transcript[1].Content
	contains(t, ctxMsg, "Today's date is 2026-10-04.")
	if !strings.HasPrefix(ctxMsg, session.ReminderOpen) {
		t.Fatal("user context must be wrapped in a reminder")
	}
}

func TestPromptOverrides(t *testing.T) {
	env := envInfo{CWD: "/w", Model: "m"}
	if got := BuildSystemPromptParts(PromptOptions{Override: "only", System: "x", Append: "y"}, env, nil, perms()); len(got) != 1 || got[0] != "only" {
		t.Fatalf("override: %v", got)
	}
	if got := BuildSystemPromptParts(PromptOptions{System: "custom", Append: "tail"}, env, nil, perms()); len(got) != 2 || got[0] != "custom" || got[1] != "tail" {
		t.Fatalf("custom+append: %v", got)
	}
}

func TestGuidanceDiscoveryOrderAndDedup(t *testing.T) {
	root := t.TempDir()
	home := filepath.Join(root, "home")
	proj := filepath.Join(root, "proj", "sub")
	for _, d := range []string{filepath.Join(home, ".claude"), proj, filepath.Join(root, "proj", ".claude", "rules")} {
		os.MkdirAll(d, 0o755)
	}
	os.WriteFile(filepath.Join(home, ".claude", "CLAUDE.md"), []byte("global rules"), 0o644)
	os.WriteFile(filepath.Join(root, "proj", "CLAUDE.md"), []byte("project rules"), 0o644)
	os.WriteFile(filepath.Join(root, "proj", ".claude", "rules", "b.md"), []byte("rule b"), 0o644)
	os.WriteFile(filepath.Join(root, "proj", ".claude", "rules", "a.md"), []byte("rule a"), 0o644)
	os.WriteFile(filepath.Join(proj, "CLAUDE.md"), []byte("   "), 0o644) // empty: skipped
	got := discoverGuidance(proj, []string{proj}, home)
	order := []string{"global rules", "project rules", "rule a", "rule b"}
	last := -1
	for _, want := range order {
		i := strings.Index(got, want)
		if i < 0 || i < last {
			t.Fatalf("%q missing or out of order in:\n%s", want, got)
		}
		last = i
	}
	if strings.Count(got, "## ") != 4 {
		t.Fatalf("dedup/empty handling wrong:\n%s", got)
	}
}

func TestTokenHelpers(t *testing.T) {
	if CountTokens("") != 0 || CountTokens("a") != 1 || CountTokens("abcdefgh") != 2 {
		t.Fatal("heuristic wrong")
	}
	for model, want := range map[string]int{"Qwen3-Coder-30B": 256000, "devstral-small": 256000, "qwen2.5": 131072,
		"claude-x": 200000, "gpt-4o": 128000, "llama3": 128000} {
		if got := InferContextWindow(model); got != want {
			t.Fatalf("%s: %d != %d", model, got, want)
		}
	}
	f := newFakeServer(t, reply("x", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) {
		o.Getenv = func(k string) string {
			if k == "SARINA_CONTEXT_WINDOW" {
				return "8192"
			}
			return ""
		}
	})
	if a.window() != 8192 {
		t.Fatalf("env window = %d", a.window())
	}
}

func TestSnapshotArithmetic(t *testing.T) {
	msgs := []session.Message{{Role: "user", Content: strings.Repeat("a", 400)}}
	rt := a0()
	s := TakeSnapshot(128000, msgs, nil, rt)
	if s.ReservedOutput != 4096 || s.Hard != 128000-4096 || s.Buffer != 12800 || s.Soft != s.Hard-12800 {
		t.Fatalf("snapshot = %+v", s)
	}
	if s.Projected != 100+5+3 {
		t.Fatalf("projected = %d", s.Projected)
	}
	rt.BudgetConfig.MaxInputTokens = intp(50)
	if s2 := TakeSnapshot(128000, msgs, nil, rt); s2.Hard != 50 || !s2.ExceedsHard {
		t.Fatalf("max_input_tokens ignored: %+v", s2)
	}
}
