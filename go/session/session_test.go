package session

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

func sample(t *testing.T) *Session {
	t.Helper()
	dir := t.TempDir()
	rt := config.DefaultRuntime(dir)
	m := config.ModelConfig{Model: "qwen", BaseURL: "http://127.0.0.1:8000/v1", APIKey: "super-secret-key-9999", TimeoutSeconds: 120}
	s := &Session{ID: NewID(), Model: m, Runtime: rt, SystemPromptParts: []string{"part one", "part two"},
		ScratchpadDirectory: filepath.Join(dir, "scratch")}
	s.UserContext.Set("currentDate", "Today is café day.")
	s.UserContext.Set("alpha", "1")
	s.SystemContext.Set("zeta", "last")
	s.SystemContext.Set("beta", "first")
	s.Append(Message{Role: "system", Content: "sys", MessageID: "system_0"})
	s.Append(Message{Role: "user", Content: "héllo <b> & \U0001F600", MessageID: "user_1"})
	s.Append(Message{Role: "assistant", Content: "calling", MessageID: "assistant_2", Usage: llm.Usage{InputTokens: 3, OutputTokens: 2},
		StopReason: "tool_calls", ToolCalls: []llm.WireToolCall{{ID: "c1", Type: "function", Function: llm.FunctionCall{Name: "read_file", Arguments: `{"path":"a"}`}}}})
	s.Append(Message{Role: "tool", Name: "read_file", ToolCallID: "c1", MessageID: "tool_3", Content: "result",
		Metadata: map[string]any{"path": "a", "phase": "completed"}})
	s.Turns, s.ToolCalls = 2, 1
	s.Usage = llm.Usage{InputTokens: 10, OutputTokens: 4, ReasoningTokens: 1}
	s.TotalCostUSD = 0.25
	s.FileHistory = []map[string]any{{"tool_name": "read_file", "path": "a"}}
	s.BudgetState = BudgetState{ModelCalls: 2, SessionTurns: 2, ToolCalls: 1}
	return s
}

func topLevelKeys(t *testing.T, data []byte) []string {
	t.Helper()
	dec := json.NewDecoder(strings.NewReader(string(data)))
	if tok, _ := dec.Token(); tok != json.Delim('{') {
		t.Fatal("not an object")
	}
	var keys []string
	for dec.More() {
		k, _ := dec.Token()
		keys = append(keys, k.(string))
		var skip json.RawMessage
		if err := dec.Decode(&skip); err != nil {
			t.Fatal(err)
		}
	}
	return keys
}

func TestFileFormatKeysAndSecrecy(t *testing.T) {
	s := sample(t)
	dir := t.TempDir()
	path, err := s.Save(dir)
	if err != nil {
		t.Fatal(err)
	}
	if path != filepath.Join(dir, s.ID+".json") {
		t.Fatalf("path = %s", path)
	}
	data, _ := os.ReadFile(path)
	want := "session_id,model_config,runtime_config,system_prompt_parts,user_context,system_context,messages,turns,tool_calls,usage,total_cost_usd,file_history,budget_state,plugin_state,scratchpad_directory"
	if got := strings.Join(topLevelKeys(t, data), ","); got != want {
		t.Fatalf("keys:\n got %s\nwant %s", got, want)
	}
	if strings.Contains(string(data), "super-secret-key-9999") || strings.Contains(string(data), "api_key") {
		t.Fatal("the API key must never be written")
	}
	for _, r := range string(data) {
		if r > 0x7e {
			t.Fatalf("file must be ASCII-only, found %q", r)
		}
	}
	text := string(data)
	for _, want := range []string{"\\u00e9", "\\ud83d\\ude00", "<b> & "} {
		if !strings.Contains(text, want) {
			t.Fatalf("missing %q in file", want)
		}
	}
	if !strings.Contains(text, "\n  \"session_id\"") {
		t.Fatal("expected 2-space indentation")
	}
	entries, _ := os.ReadDir(dir)
	if len(entries) != 1 {
		t.Fatalf("temp files left behind: %v", entries)
	}
	var raw map[string]any
	json.Unmarshal(data, &raw)
	if _, isObj := raw["messages"].([]any)[0].(map[string]any); !isObj {
		t.Fatal("messages must be objects")
	}
	mc := raw["model_config"].(map[string]any)
	if mc["model"] != "qwen" || mc["pricing"].(map[string]any)["input_cost_per_million_tokens_usd"] == nil {
		t.Fatalf("model_config = %v", mc)
	}
	usage := raw["usage"].(map[string]any)
	if usage["total_tokens"] != float64(14) || usage["reasoning_tokens"] != float64(1) {
		t.Fatalf("usage = %v", usage)
	}
	if raw["plugin_state"].(map[string]any) == nil {
		t.Fatal("plugin_state must be an object")
	}
}

func TestSaveOverwritesAtomically(t *testing.T) {
	s := sample(t)
	dir := t.TempDir()
	s.Save(dir)
	s.Turns = 9
	if _, err := s.Save(dir); err != nil {
		t.Fatal(err)
	}
	got, err := LoadByID(dir, s.ID)
	if err != nil || got.Turns != 9 {
		t.Fatalf("%v %v", got, err)
	}
	if entries, _ := os.ReadDir(dir); len(entries) != 1 {
		t.Fatalf("entries = %v", entries)
	}
}

func TestRoundTripPreservesEverything(t *testing.T) {
	s := sample(t)
	dir := t.TempDir()
	path, _ := s.Save(dir)
	got, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if got.ID != s.ID || got.Turns != 2 || got.ToolCalls != 1 || got.TotalCostUSD != 0.25 || got.Usage != s.Usage {
		t.Fatalf("scalars: %+v", got)
	}
	if got.Model.Model != "qwen" || got.Model.APIKey != "" {
		t.Fatalf("model = %+v", got.Model)
	}
	if got.Runtime.CWD != s.Runtime.CWD || got.Runtime.MaxTurns != 12 {
		t.Fatalf("runtime = %+v", got.Runtime)
	}
	if len(got.Messages) != 4 || got.Messages[1].Content != s.Messages[1].Content {
		t.Fatalf("messages = %+v", got.Messages)
	}
	if got.Messages[2].ToolCalls[0].Function.Arguments != `{"path":"a"}` || got.Messages[2].Usage.Total() != 5 {
		t.Fatalf("assistant = %+v", got.Messages[2])
	}
	if got.Messages[3].ToolCallID != "c1" || got.Messages[3].Metadata["path"] != "a" || got.Messages[3].State != StateFinal {
		t.Fatalf("tool = %+v", got.Messages[3])
	}
	var keys []string
	for _, kv := range got.UserContext {
		keys = append(keys, kv.Key)
	}
	if strings.Join(keys, ",") != "currentDate,alpha" {
		t.Fatalf("user_context order lost: %v", keys)
	}
	if v, _ := got.SystemContext.Get("beta"); v != "first" || got.SystemContext[0].Key != "zeta" {
		t.Fatalf("system_context = %+v", got.SystemContext)
	}
	if got.BudgetState != s.BudgetState || len(got.FileHistory) != 1 || got.ScratchpadDirectory != s.ScratchpadDirectory {
		t.Fatalf("state = %+v", got)
	}
}

func TestLoaderTolerance(t *testing.T) {
	doc := `{
	  "session_id": "abc",
	  "model_config": {"model": "m", "api_key": "legacy-key-present", "temperature": "0.5", "timeout_seconds": true},
	  "runtime_config": {"cwd": "/w", "max_turns": "7", "max_output_chars": false, "permissions": "oops",
	     "budget_config": {"max_total_tokens": "100", "max_tool_calls": true}, "escalation_config": 5,
	     "output_schema": {"name": "", "schema": {}}, "additional_working_directories": ["/a", 3]},
	  "system_prompt_parts": ["p"], "user_context": {}, "system_context": {},
	  "messages": [{"role":"user"}, "junk", 4, {"role":"weird","content":"x","state":"streaming"}],
	  "turns": "3", "tool_calls": 2
	}`
	s, err := Parse([]byte(doc))
	if err != nil {
		t.Fatal(err)
	}
	if s.Model.APIKey != "" || s.Model.Temperature != 0.5 || s.Model.TimeoutSeconds != config.DefaultTimeoutSeconds || s.Model.BaseURL != config.DefaultBaseURL {
		t.Fatalf("model = %+v", s.Model)
	}
	r := s.Runtime
	if r.MaxTurns != 7 || r.MaxOutputChars != 12000 || r.Permissions.AllowFileWrite || r.OutputSchema != nil {
		t.Fatalf("runtime = %+v", r)
	}
	if r.BudgetConfig.MaxTotalTokens == nil || *r.BudgetConfig.MaxTotalTokens != 100 || r.BudgetConfig.MaxToolCalls != nil {
		t.Fatalf("budget = %+v", r.BudgetConfig)
	}
	if len(r.AdditionalWorkingDirectories) != 1 || r.EscalationConfig.MaxEscalations != 1 {
		t.Fatalf("runtime = %+v", r)
	}
	if !strings.HasSuffix(filepath.ToSlash(r.SessionDirectory), "/.port_sessions/agent") {
		t.Fatalf("session dir default = %s", r.SessionDirectory)
	}
	if len(s.Messages) != 2 || s.Messages[0].Content != "" || s.Messages[0].State != StateFinal || s.Messages[1].State != StateStreaming {
		t.Fatalf("messages = %+v", s.Messages)
	}
	if s.Turns != 3 || s.ToolCalls != 2 || s.PluginState != nil || s.ScratchpadDirectory != "" || len(s.FileHistory) != 0 {
		t.Fatalf("defaults wrong: %+v", s)
	}
	if _, err := s.Marshal(); err != nil {
		t.Fatal(err)
	}
}

func TestLoaderRequiresCoreKeys(t *testing.T) {
	full := map[string]any{
		"session_id": "x", "model_config": map[string]any{"model": "m"}, "runtime_config": map[string]any{"cwd": "/w"},
		"system_prompt_parts": []any{}, "user_context": map[string]any{}, "system_context": map[string]any{},
		"messages": []any{}, "turns": 0, "tool_calls": 0,
	}
	b, _ := json.Marshal(full)
	if _, err := Parse(b); err != nil {
		t.Fatal(err)
	}
	for k := range full {
		cp := map[string]any{}
		for kk, v := range full {
			if kk != k {
				cp[kk] = v
			}
		}
		b, _ := json.Marshal(cp)
		if _, err := Parse(b); err == nil {
			t.Errorf("missing %s should fail", k)
		}
	}
	cp := map[string]any{}
	for k, v := range full {
		cp[k] = v
	}
	cp["model_config"] = map[string]any{}
	b, _ = json.Marshal(cp)
	if _, err := Parse(b); err == nil {
		t.Error("model_config.model is required")
	}
	cp["model_config"] = full["model_config"]
	cp["runtime_config"] = map[string]any{}
	b, _ = json.Marshal(cp)
	if _, err := Parse(b); err == nil {
		t.Error("runtime_config.cwd is required")
	}
	if _, err := Parse([]byte(`[1]`)); err == nil {
		t.Error("non-object file")
	}
}

func TestMessageStoredShape(t *testing.T) {
	b, _ := json.Marshal(Message{Role: "user", Content: "hi", MessageID: "user_0", State: StateFinal})
	var m map[string]any
	json.Unmarshal(b, &m)
	for _, k := range []string{"name", "tool_call_id", "tool_calls", "state", "stop_reason", "usage", "metadata"} {
		if _, ok := m[k]; ok {
			t.Errorf("%s should be omitted: %s", k, b)
		}
	}
	if m["content"] != "hi" || m["message_id"] != "user_0" || len(m["blocks"].([]any)) != 1 {
		t.Fatalf("shape = %s", b)
	}
	b, _ = json.Marshal(Message{Role: "assistant", MessageID: "a", State: StateStreaming, StopReason: "stop", Usage: llm.Usage{InputTokens: 1}})
	json.Unmarshal(b, &m)
	if m["content"] != "" || m["state"] != "streaming" || m["stop_reason"] != "stop" || m["usage"] == nil {
		t.Fatalf("shape = %s", b)
	}
	if (Message{Role: "tool", ToolCallID: "c", Name: "t", Content: "x"}).Wire().ToolCallID != "c" {
		t.Fatal("wire shape lost tool_call_id")
	}
	w, _ := json.Marshal(Message{Role: "user", Content: "q", MessageID: "x", Metadata: map[string]any{"a": 1}, Usage: llm.Usage{InputTokens: 9}}.Wire())
	if string(w) != `{"role":"user","content":"q"}` {
		t.Fatalf("wire message carries extras: %s", w)
	}
}

func TestMutateTracksRevisionsAndTotals(t *testing.T) {
	m := Message{Role: "assistant", MessageID: "assistant_4"}
	m.Mutate("assistant_delta_append")
	m.Mutate("assistant_delta_append")
	m.Mutate("assistant_finalize")
	totals := m.Metadata["mutation_totals"].(map[string]any)
	if totals["assistant_delta_append"] != 2 || totals["assistant_finalize"] != 1 {
		t.Fatalf("totals = %v", totals)
	}
	if m.Metadata["lineage_id"] != "assistant_4" || m.Metadata["revision"] != 3 {
		t.Fatalf("metadata = %v", m.Metadata)
	}
	// totals survive a JSON round trip (numbers become float64)
	b, _ := json.Marshal(m)
	var back map[string]any
	json.Unmarshal(b, &back)
	m2 := MessageFromMap(back)
	m2.Mutate("assistant_finalize")
	if m2.Metadata["mutation_totals"].(map[string]any)["assistant_finalize"] != 2 {
		t.Fatalf("after reload: %v", m2.Metadata)
	}
}

func TestToolResultBodyFormat(t *testing.T) {
	got := ToolResultBody("read_file", true, "café", nil)
	want := "{\n  \"tool\": \"read_file\",\n  \"ok\": true,\n  \"content\": \"caf\\u00e9\"\n}"
	if got != want {
		t.Fatalf("got:\n%s\nwant:\n%s", got, want)
	}
	with := ToolResultBody("t", false, "x", map[string]any{"error_kind": "permission_denied"})
	if !strings.Contains(with, `"metadata": {`) || !strings.Contains(with, "permission_denied") {
		t.Fatalf("metadata missing: %s", with)
	}
}

func TestRedact(t *testing.T) {
	cases := map[string]string{
		"key sk-ant-abcdefgh12345 here":                                   "key [REDACTED:ant] here",
		"sk-proj-ABCDEFGH_12-34":                                          "[REDACTED:proj]",
		"sk-live-abcdefghij":                                              "[REDACTED:live]",
		"rk_live_" + strings.Repeat("a1", 8):                              "[REDACTED:stripe]",
		"pk_test_" + strings.Repeat("Z9", 9):                              "[REDACTED:stripe]",
		"ghp_" + strings.Repeat("a", 20):                                  "[REDACTED:github]",
		"id AKIA" + strings.Repeat("A1", 8) + " end":                      "id [REDACTED:aws] end",
		"xoxb-" + strings.Repeat("1", 12) + "-abcdefgh":                   "[REDACTED:slack]",
		"AIza" + strings.Repeat("a", 35):                                  "[REDACTED:google]",
		"eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcDEF123_-xyz": "[REDACTED:jwt]",
		"-----BEGIN OPENSSH PRIVATE KEY-----\nabc":                        "[REDACTED:pem]\nabc",
		"-----BEGIN PRIVATE KEY-----":                                     "[REDACTED:pem]",
		"plain text sk-short and ghp_tooshort":                            "plain text sk-short and ghp_tooshort",
	}
	for in, want := range cases {
		if got := Redact(in); got != want {
			t.Errorf("Redact(%q) = %q, want %q", in, got, want)
		}
	}
}

func call(id string) llm.WireToolCall {
	return llm.WireToolCall{ID: id, Type: "function", Function: llm.FunctionCall{Name: "t", Arguments: "{}"}}
}

func TestRepairPairing(t *testing.T) {
	in := []Message{
		{Role: "system", Content: "s"},
		{Role: "user", Content: "u"},
		{Role: "assistant", ToolCalls: []llm.WireToolCall{call("a"), call("b")}},
		{Role: "tool", ToolCallID: "b", Name: "t", Content: "rb"},
		{Role: "tool", ToolCallID: "ghost", Name: "t", Content: "orphan"},
		{Role: "user", Content: "next"},
		{Role: "tool", ToolCallID: "late", Name: "t", Content: "orphan2"},
		{Role: "assistant", ToolCalls: []llm.WireToolCall{call("z")}},
	}
	out := RepairPairing(in)
	var shape []string
	for _, m := range out {
		shape = append(shape, m.Role+":"+m.ToolCallID)
	}
	want := "system:,user:,assistant:,tool:b,tool:a,user:,assistant:,tool:z"
	if strings.Join(shape, ",") != want {
		t.Fatalf("got  %s\nwant %s", strings.Join(shape, ","), want)
	}
	contains := func(m Message, s string) bool { return strings.Contains(m.Content, s) }
	if !contains(out[4], "no recorded result") || !contains(out[7], "no recorded result") || out[3].Content != "rb" {
		t.Fatalf("synthetic results wrong: %+v", out)
	}
	// already valid input is unchanged
	again := RepairPairing(out)
	if len(again) != len(out) {
		t.Fatalf("not idempotent: %d vs %d", len(again), len(out))
	}
}

func TestNewIDShape(t *testing.T) {
	seen := map[string]bool{}
	for i := 0; i < 50; i++ {
		id := NewID()
		if len(id) != 32 || strings.ToLower(id) != id || strings.ContainsAny(id, "-ghijklmnopqrstuvwxyz") || id[12] != '4' {
			t.Fatalf("bad id %q", id)
		}
		if seen[id] {
			t.Fatal("duplicate id")
		}
		seen[id] = true
	}
}
