package agent

import (
	"context"
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// Spec 01 section 14, scenario 1.
func TestScenario01_ToolRoundTrip(t *testing.T) {
	f := newFakeServer(t,
		reply("Let me write it.", "tool_calls", use(10, 5), tcall{"call_a", "write_file", `{"path":"hello.txt","content":"hello world\n"}`}),
		reply("It says hello.", "stop", use(20, 6)))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.Permissions.AllowFileWrite = true })
	res := mustRun(t, a, "write hello.txt")

	if res.FinalOutput != "It says hello." || res.ToolCalls != 1 || res.StopReason != "stop" {
		t.Fatalf("unexpected result: %+v", res)
	}
	if len(res.Transcript) < 6 {
		t.Fatalf("want >= 6 transcript entries (system, context, user, assistant, tool, assistant), got %d", len(res.Transcript))
	}
	roles := []string{}
	for _, m := range res.Transcript {
		roles = append(roles, m.Role)
	}
	if got := strings.Join(roles, ","); got != "system,user,user,assistant,tool,assistant" {
		t.Fatalf("roles = %s", got)
	}
	tool := res.Transcript[4]
	contains(t, tool.Content, "hello.txt")
	if tool.ToolCallID != "call_a" || tool.Name != "write_file" {
		t.Fatalf("tool message not linked: %+v", tool)
	}
	// the second request must carry the tool result back
	second := reqMessages(f.requests()[1])
	if last := second[len(second)-1]; last["role"] != "tool" || last["tool_call_id"] != "call_a" {
		t.Fatalf("second request did not end with the tool result: %v", last)
	}
	assertPaired(t, res.Transcript)
	if len(res.FileHistory) != 1 || res.FileHistory[0]["history_kind"] != "file_change" {
		t.Fatalf("file history = %v", res.FileHistory)
	}
}

// Scenario 2: run then resume.
func TestScenario02_RunThenResume(t *testing.T) {
	f := newFakeServer(t, reply("first answer", "stop", use(5, 3)), reply("second answer", "stop", use(9, 4)))
	a := newTestAgent(t, f)
	first := mustRun(t, a, "first prompt")
	if _, err := os.Stat(first.SessionPath); err != nil {
		t.Fatal(err)
	}
	if want := session.Path(a.opts.Runtime.SessionDirectory, first.SessionID); first.SessionPath != want {
		t.Fatalf("session path %s, want %s", first.SessionPath, want)
	}
	if len(first.SessionID) != 32 || strings.ToLower(first.SessionID) != first.SessionID || strings.Contains(first.SessionID, "-") {
		t.Fatalf("session id %q is not 32 lowercase hex", first.SessionID)
	}
	stored, err := session.Load(first.SessionPath)
	if err != nil {
		t.Fatal(err)
	}
	second, err := a.Resume(t.Context(), "second prompt", stored)
	if err != nil {
		t.Fatal(err)
	}
	if second.SessionID != first.SessionID || second.FinalOutput != "second answer" {
		t.Fatalf("resume result: %+v", second)
	}
	var texts []string
	for _, m := range reqMessages(f.requests()[1]) {
		texts = append(texts, m["content"].(string))
	}
	all := strings.Join(texts, "\n")
	for _, want := range []string{"first prompt", "first answer", "second prompt"} {
		contains(t, all, want)
	}
	loaded, err := session.LoadByID(a.opts.Runtime.SessionDirectory, first.SessionID)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.Turns != 2 || len(loaded.Messages) != len(second.Transcript) {
		t.Fatalf("stored turns=%d messages=%d", loaded.Turns, len(loaded.Messages))
	}
}

// Scenario 3: streaming text, usage on the last event, mutation totals.
func TestScenario03_StreamingText(t *testing.T) {
	f := newFakeServer(t, scripted{sse: []any{
		map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"role": "assistant", "content": "Streaming "}}}},
		map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"content": "works."}}}},
		map[string]any{"choices": []any{map[string]any{"delta": map[string]any{}, "finish_reason": "stop"}}, "usage": use(12, 7)},
	}})
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.StreamModelResponses = true })
	res := mustRun(t, a, "stream please")
	if res.FinalOutput != "Streaming works." {
		t.Fatalf("final = %q", res.FinalOutput)
	}
	if res.Usage.InputTokens != 12 || res.Usage.OutputTokens != 7 {
		t.Fatalf("usage = %+v", res.Usage)
	}
	if !hasEvent(res, "content_delta") || !hasEvent(res, "message_start") || !hasEvent(res, "message_stop") {
		t.Fatalf("events = %v", eventTypes(res))
	}
	if st, err := os.Stat(res.ScratchpadDirectory); err != nil || !st.IsDir() {
		t.Fatalf("scratchpad missing: %v", err)
	}
	if req := f.requests()[0]; req["stream"] != true || req["stream_options"].(map[string]any)["include_usage"] != true {
		t.Fatalf("stream flags missing: %v", req)
	}
	var assistants []session.Message
	for _, m := range res.Transcript {
		if m.Role == "assistant" {
			assistants = append(assistants, m)
		}
	}
	if len(assistants) != 1 {
		t.Fatalf("want one assistant message, got %d", len(assistants))
	}
	totals := assistants[0].Metadata["mutation_totals"].(map[string]any)
	if n, _ := config.AsInt(totals["assistant_delta_append"]); n < 2 {
		t.Fatalf("delta appends = %v", totals)
	}
	if n, _ := config.AsInt(totals["assistant_finalize"]); n != 1 {
		t.Fatalf("finalize = %v", totals)
	}
	if assistants[0].State != session.StateFinal {
		t.Fatalf("state = %s", assistants[0].State)
	}
}

// Scenario 4: streamed tool call split in fragments; usage summed across calls.
func TestScenario04_StreamingToolCallFragments(t *testing.T) {
	frag := func(idx int, id, name, args string) map[string]any {
		fn := map[string]any{"arguments": args}
		if name != "" {
			fn["name"] = name
		}
		tc := map[string]any{"index": idx, "function": fn}
		if id != "" {
			tc["id"] = id
		}
		return map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"tool_calls": []any{tc}}}}}
	}
	f := newFakeServer(t,
		scripted{sse: []any{
			frag(0, "call_1", "read_file", `{"path":`),
			frag(0, "", "", `"a.txt"}`),
			map[string]any{"choices": []any{map[string]any{"delta": map[string]any{}, "finish_reason": "tool_calls"}}, "usage": use(9, 4)},
		}},
		scripted{sse: []any{
			map[string]any{"choices": []any{map[string]any{"delta": map[string]any{"content": "done"}, "finish_reason": "stop"}}, "usage": use(11, 2)},
		}})
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.StreamModelResponses = true })
	writeFile(t, a, "a.txt", "alpha")
	res := mustRun(t, a, "read a.txt")
	if res.FinalOutput != "done" || res.ToolCalls != 1 {
		t.Fatalf("result %+v", res)
	}
	if res.Usage.InputTokens != 20 || res.Usage.OutputTokens != 6 {
		t.Fatalf("usage should sum over model calls (9+11, 4+2): %+v", res.Usage)
	}
	asst := res.Transcript[3]
	if len(asst.ToolCalls) != 1 || asst.ToolCalls[0].Function.Arguments != `{"path":"a.txt"}` {
		t.Fatalf("assistant tool calls: %+v", asst.ToolCalls)
	}
	totals := asst.Metadata["mutation_totals"].(map[string]any)
	if n, _ := config.AsInt(totals["assistant_tool_call_delta"]); n < 2 {
		t.Fatalf("totals = %v", totals)
	}
	contains(t, res.Transcript[4].Content, "alpha")
}

// Scenario 5: blocks in the stored transcript.
func TestScenario05_TranscriptBlocks(t *testing.T) {
	f := newFakeServer(t,
		reply("checking", "tool_calls", use(1, 1), tcall{"c1", "read_file", `{"path":"x.txt"}`}),
		reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	writeFile(t, a, "x.txt", "x")
	res := mustRun(t, a, "go")
	b, err := json.Marshal(res.Transcript)
	if err != nil {
		t.Fatal(err)
	}
	var raw []map[string]any
	if err := json.Unmarshal(b, &raw); err != nil {
		t.Fatal(err)
	}
	asst := raw[3]["blocks"].([]any)
	if asst[0].(map[string]any)["type"] != "text" || asst[1].(map[string]any)["type"] != "tool_call" {
		t.Fatalf("assistant blocks = %v", asst)
	}
	if asst[1].(map[string]any)["arguments"] != `{"path":"x.txt"}` {
		t.Fatalf("tool_call block arguments = %v", asst[1])
	}
	toolBlocks := raw[4]["blocks"].([]any)
	if toolBlocks[0].(map[string]any)["type"] != "tool_result" {
		t.Fatalf("tool blocks = %v", toolBlocks)
	}
}

// Scenario 6: token budget.
func TestScenario06_TokenBudget(t *testing.T) {
	f := newFakeServer(t, reply("big answer", "stop", use(30, 12)))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.BudgetConfig.MaxTotalTokens = intp(20) })
	res := mustRun(t, a, "hi")
	if res.StopReason != StopBudgetExceeded || res.Usage.Total() != 42 {
		t.Fatalf("stop=%s usage=%+v", res.StopReason, res.Usage)
	}
	contains(t, res.FinalOutput, "token")
}

func intp(n int) *int { return &n }

type neverCalled struct{ t *testing.T }

func (n neverCalled) Complete(context.Context, llm.Request, func(llm.StreamEvent)) (*llm.Turn, error) {
	n.t.Fatal("the model must not be called")
	return nil, nil
}

// Scenario 7: hard context overflow stops before any model call.
func TestScenario07_PromptTooLong(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f, func(o *Options) {
		o.Client = neverCalled{t}
		o.ContextWindow = 1100 // hard limit 76 tokens: smaller than the system prompt itself
	})
	res := mustRun(t, a, "anything")
	if res.StopReason != StopPromptTooLong || res.Turns != 0 {
		t.Fatalf("stop=%s turns=%d", res.StopReason, res.Turns)
	}
	contains(t, res.FinalOutput, "hard limit")
	contains(t, res.FinalOutput, "context window of 1100 tokens (set with --context-window)")
	contains(t, res.FinalOutput, "num_ctx 16384")
	if !hasEvent(res, "prompt_length_check") {
		t.Fatalf("events = %v", eventTypes(res))
	}
}

// Scenario 8: truncation continuation.
func TestScenario08_Continuation(t *testing.T) {
	f := newFakeServer(t, reply("Part 1 ", "length", use(5, 5)), reply("Part 2", "stop", use(8, 3)))
	a := newTestAgent(t, f)
	res := mustRun(t, a, "write a lot")
	if res.FinalOutput != "Part 1 Part 2" {
		t.Fatalf("final = %q", res.FinalOutput)
	}
	if countEvents(res, "continuation_request") != 1 {
		t.Fatalf("events = %v", eventTypes(res))
	}
	contains(t, lastUserText(f.requests()[1]), "cut off")
	if res.Turns != 2 {
		t.Fatalf("turns = %d", res.Turns)
	}
}

// Scenario 9: the model-call budget carries across a resume.
func TestScenario09_ModelCallBudgetAcrossResume(t *testing.T) {
	f := newFakeServer(t, reply("one", "stop", use(1, 1)), reply("two", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Runtime.BudgetConfig.MaxModelCalls = intp(1) })
	first := mustRun(t, a, "a")
	if first.StopReason != "stop" {
		t.Fatalf("first run should answer normally, got %s", first.StopReason)
	}
	stored, _ := session.Load(first.SessionPath)
	if stored.BudgetState.ModelCalls != 1 {
		t.Fatalf("stored model calls = %d", stored.BudgetState.ModelCalls)
	}
	second, err := a.Resume(t.Context(), "b", stored)
	if err != nil {
		t.Fatal(err)
	}
	if second.StopReason != StopBudgetExceeded {
		t.Fatalf("stop = %s", second.StopReason)
	}
	contains(t, second.FinalOutput, "model-call")
}

// Scenario 10: structured output request.
func TestScenario10_ResponseSchema(t *testing.T) {
	f := newFakeServer(t, reply(`{"ok":true}`, "stop", use(1, 1)))
	schema := map[string]any{"type": "object", "properties": map[string]any{"ok": map[string]any{"type": "boolean"}}}
	a := newTestAgent(t, f, func(o *Options) {
		o.Runtime.OutputSchema = &config.OutputSchema{Name: "verdict", Schema: schema, Strict: true}
	})
	mustRun(t, a, "judge")
	rf := f.requests()[0]["response_format"].(map[string]any)
	js := rf["json_schema"].(map[string]any)
	if rf["type"] != "json_schema" || js["name"] != "verdict" || js["strict"] != true {
		t.Fatalf("response_format = %v", rf)
	}
	if js["schema"].(map[string]any)["type"] != "object" {
		t.Fatalf("schema = %v", js["schema"])
	}
}

// Scenario 11: a re-emitted tool-call id is executed once.
func TestScenario11_DuplicateToolCallID(t *testing.T) {
	runs := 0
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"same", "probe", `{}`}),
		reply("", "tool_calls", use(1, 1), tcall{"same", "probe", `{}`}),
		reply("finished", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) {
		o.Tools.Extra = []toolT{customTool("probe", func(context.Context, map[string]any, *toolCtx) (string, map[string]any, error) {
			runs++
			return "ran", nil, nil
		})}
	})
	res := mustRun(t, a, "go")
	if runs != 1 || res.ToolCalls != 1 || res.FinalOutput != "finished" {
		t.Fatalf("runs=%d toolCalls=%d final=%q", runs, res.ToolCalls, res.FinalOutput)
	}
	assertPaired(t, res.Transcript)
	contains(t, res.Transcript[len(res.Transcript)-2].Content, "already executed")
}

// Scenario 14: compaction at a threshold, preserving the tail.
func TestScenario14_CompactionAtThreshold(t *testing.T) {
	f := newFakeServer(t, reply("ok", "stop", use(1, 1)))
	long := strings.Repeat("alpha beta gamma ", 20)
	a := newTestAgent(t, f, func(o *Options) {
		o.Runtime.AutoCompactThresholdTokens = intp(80)
		o.Runtime.CompactPreserveMessages = 1
	})
	s, err := a.newSession()
	if err != nil {
		t.Fatal(err)
	}
	for i, txt := range []string{"first question " + long, "first answer " + long, "second question " + long} {
		role := []string{"user", "assistant", "user"}[i]
		s.Append(session.Message{Role: role, Content: txt, MessageID: s.NextMessageID(role)})
	}
	res, err := a.Resume(t.Context(), "newest request", s)
	if err != nil {
		t.Fatal(err)
	}
	if countEvents(res, "compact_boundary") != 1 {
		t.Fatalf("events = %v", eventTypes(res))
	}
	notes := 0
	for _, m := range reqMessages(f.requests()[0]) {
		if strings.Contains(m["content"].(string), "was compacted") {
			notes++
		}
	}
	if notes != 1 {
		t.Fatalf("request carries %d compaction notes", notes)
	}
	var boundary *session.Message
	for i := range res.Transcript {
		if res.Transcript[i].Kind() == session.KindCompactBoundary {
			boundary = &res.Transcript[i]
		}
	}
	if boundary == nil {
		t.Fatal("no boundary in the transcript")
	}
	if d, _ := config.AsInt(boundary.Metadata["compaction_depth"]); d != 1 {
		t.Fatalf("depth = %v", boundary.Metadata["compaction_depth"])
	}
	if n, _ := config.AsInt(boundary.Metadata["nested_compaction_count"]); n != 0 {
		t.Fatalf("nested = %v", boundary.Metadata["nested_compaction_count"])
	}
	if _, ok := boundary.Metadata["preserved_tail_ids"]; !ok {
		t.Fatal("preserved_tail_ids missing")
	}
}
