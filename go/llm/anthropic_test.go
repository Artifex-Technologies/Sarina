package llm

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

const anthKey = "sk-ant-api03-UNIT-TEST-KEY-0123456789"

func decodeBody(t *testing.T, b []byte) map[string]any {
	t.Helper()
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatalf("%v: %s", err, b)
	}
	return m
}

func TestAnthropicBodyTranslatesToolUseBothWays(t *testing.T) {
	c := &AnthropicClient{Config: config.ModelConfig{Model: "claude-test", APIKey: anthKey}}
	req := Request{
		Messages: []Message{
			{Role: "system", Content: "sys one"},
			{Role: "user", Content: "list files"},
			{Role: "assistant", Content: "sure", ToolCalls: []WireToolCall{
				{ID: "call.1", Type: "function", Function: FunctionCall{Name: "list_dir", Arguments: `{"path":"."}`}},
				{ID: "call/2", Type: "function", Function: FunctionCall{Name: "read_file", Arguments: `not json`}}}},
			{Role: "tool", Name: "list_dir", ToolCallID: "call.1", Content: "a.txt"},
			{Role: "tool", Name: "read_file", ToolCallID: "call/2", Content: ""},
			{Role: "user", Content: "thanks"},
			{Role: "system", Content: "late system note"},
		},
		Tools:     []ToolSpec{{Name: "list_dir", Description: "lists", Parameters: map[string]any{"type": "object", "properties": map[string]any{"path": map[string]any{"type": "string"}}}}, {Name: "plain"}},
		MaxTokens: 0,
	}
	b, err := c.BuildBody(req)
	if err != nil {
		t.Fatal(err)
	}
	body := decodeBody(t, b)
	if body["system"] != "sys one\n\nlate system note" {
		t.Errorf("system = %v", body["system"])
	}
	if body["max_tokens"].(float64) != defaultAnthropicMaxTokens {
		t.Errorf("max_tokens = %v", body["max_tokens"])
	}
	if _, has := body["temperature"]; has {
		t.Error("zero temperature must be omitted")
	}
	msgs := body["messages"].([]any)
	if len(msgs) != 3 {
		t.Fatalf("want user/assistant/user, got %d: %s", len(msgs), b)
	}
	asst := msgs[1].(map[string]any)["content"].([]any)
	if len(asst) != 3 || asst[0].(map[string]any)["type"] != "text" {
		t.Fatalf("assistant blocks: %v", asst)
	}
	use1, use2 := asst[1].(map[string]any), asst[2].(map[string]any)
	if use1["type"] != "tool_use" || use1["id"] != "call_1" || use1["input"].(map[string]any)["path"] != "." {
		t.Errorf("tool_use 1: %v", use1)
	}
	if use2["id"] != "call_2" || len(use2["input"].(map[string]any)) != 0 {
		t.Errorf("unparseable arguments must become an empty object: %v", use2)
	}
	// both tool results and the follow-up user text share ONE user message, results first.
	last := msgs[2].(map[string]any)
	if last["role"] != "user" {
		t.Fatal(last)
	}
	blocks := last["content"].([]any)
	if len(blocks) != 3 || blocks[0].(map[string]any)["type"] != "tool_result" || blocks[1].(map[string]any)["type"] != "tool_result" || blocks[2].(map[string]any)["type"] != "text" {
		t.Fatalf("user blocks: %v", blocks)
	}
	if blocks[0].(map[string]any)["tool_use_id"] != "call_1" || blocks[1].(map[string]any)["content"] != "(no output)" {
		t.Errorf("results: %v", blocks)
	}
	tools := body["tools"].([]any)
	if len(tools) != 2 || tools[0].(map[string]any)["input_schema"] == nil || tools[1].(map[string]any)["input_schema"] == nil {
		t.Errorf("tools: %v", tools)
	}
	if body["tool_choice"].(map[string]any)["type"] != "auto" {
		t.Error("tool_choice")
	}
	if strings.Contains(string(b), anthKey) {
		t.Error("the key must never be in the body")
	}
}

func TestAnthropicBodyOmitsToolsWhenNoneAndStartsWithUser(t *testing.T) {
	c := &AnthropicClient{Config: config.ModelConfig{Model: "m", Temperature: 0.4}}
	b, _ := c.BuildBody(Request{Messages: []Message{{Role: "assistant", Content: "earlier answer"}}, MaxTokens: 100})
	body := decodeBody(t, b)
	if _, has := body["tools"]; has {
		t.Error("tools must be omitted when there are none")
	}
	if _, has := body["tool_choice"]; has {
		t.Error("tool_choice must be omitted when there are no tools")
	}
	msgs := body["messages"].([]any)
	if msgs[0].(map[string]any)["role"] != "user" || body["max_tokens"].(float64) != 100 || body["temperature"].(float64) != 0.4 {
		t.Errorf("%s", b)
	}
}

type anthServer struct {
	*httptest.Server
	mu      sync.Mutex
	headers []http.Header
	bodies  [][]byte
}

func newAnthServer(t *testing.T, handler func(n int, w http.ResponseWriter)) *anthServer {
	s := &anthServer{}
	s.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		s.mu.Lock()
		s.headers = append(s.headers, r.Header.Clone())
		s.bodies = append(s.bodies, b)
		n := len(s.bodies)
		s.mu.Unlock()
		if r.URL.Path != "/v1/messages" {
			w.WriteHeader(404)
			return
		}
		handler(n, w)
	}))
	t.Cleanup(s.Close)
	return s
}

func TestAnthropicCompleteParsesTextAndToolUse(t *testing.T) {
	s := newAnthServer(t, func(n int, w http.ResponseWriter) {
		io.WriteString(w, `{"type":"message","role":"assistant","stop_reason":"tool_use",
			"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"checking"},
			{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"a.go"}}],
			"usage":{"input_tokens":11,"output_tokens":4,"cache_read_input_tokens":2}}`)
	})
	c := &AnthropicClient{Config: config.ModelConfig{Model: "claude-test", BaseURL: s.URL, APIKey: anthKey, TimeoutSeconds: 5}}
	var events []string
	turn, err := c.Complete(context.Background(), Request{Messages: []Message{{Role: "user", Content: "go"}}, Stream: true},
		func(e StreamEvent) { events = append(events, e.Type) })
	if err != nil {
		t.Fatal(err)
	}
	if turn.Content != "checking" || turn.Reasoning != "hmm" || turn.FinishReason != "tool_calls" || !turn.HasFinishReason {
		t.Fatalf("%+v", turn)
	}
	if len(turn.ToolCalls) != 1 || turn.ToolCalls[0].ID != "toolu_1" || turn.ToolCalls[0].Name != "read_file" || turn.ToolCalls[0].Arguments["path"] != "a.go" {
		t.Fatalf("%+v", turn.ToolCalls)
	}
	if turn.Usage.InputTokens != 11 || turn.Usage.OutputTokens != 4 || turn.Usage.CacheReadInputTokens != 2 || !turn.UsageReported {
		t.Fatalf("%+v", turn.Usage)
	}
	if strings.Join(events, ",") != "message_start,content_delta,usage,message_stop" {
		t.Fatalf("events: %v", events)
	}
	h := s.headers[0]
	if h.Get("x-api-key") != anthKey || h.Get("anthropic-version") != "2023-06-01" || h.Get("Authorization") != "" {
		t.Fatalf("headers: %v", h)
	}
}

func TestAnthropicStopReasonMapping(t *testing.T) {
	for in, want := range map[string]string{"end_turn": "stop", "max_tokens": "length", "tool_use": "tool_calls", "refusal": "content_filter", "stop_sequence": "stop"} {
		turn, err := parseAnthropic([]byte(`{"content":[{"type":"text","text":"x"}],"stop_reason":"` + in + `"}`))
		if err != nil || turn.FinishReason != want {
			t.Errorf("%s -> %v %v", in, turn, err)
		}
	}
}

func TestAnthropicErrorsScrubTheKeyAndRetryOnce(t *testing.T) {
	s := newAnthServer(t, func(n int, w http.ResponseWriter) {
		w.WriteHeader(401)
		io.WriteString(w, `{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key `+anthKey+`"}}`)
	})
	c := &AnthropicClient{Config: config.ModelConfig{Model: "m", BaseURL: s.URL, APIKey: anthKey, TimeoutSeconds: 5}}
	_, err := c.Complete(context.Background(), Request{Messages: []Message{{Role: "user", Content: "x"}}}, nil)
	var be *BackendError
	if err == nil || !errors.As(err, &be) || be.Status != 401 {
		t.Fatalf("%v", err)
	}
	if strings.Contains(err.Error(), anthKey) || !strings.Contains(err.Error(), "[redacted-key]") {
		t.Fatalf("key leaked in error: %v", err)
	}
	if len(s.bodies) != 1 {
		t.Fatalf("a 401 must not be retried (%d calls)", len(s.bodies))
	}

	s2 := newAnthServer(t, func(n int, w http.ResponseWriter) {
		if n == 1 {
			w.WriteHeader(529)
			return
		}
		io.WriteString(w, `{"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}`)
	})
	c2 := &AnthropicClient{Config: config.ModelConfig{Model: "m", BaseURL: s2.URL, APIKey: anthKey, TimeoutSeconds: 5}, RetryDelay: time.Millisecond}
	turn, err := c2.Complete(context.Background(), Request{Messages: []Message{{Role: "user", Content: "x"}}}, nil)
	if err != nil || turn.Content != "ok" || len(s2.bodies) != 2 {
		t.Fatalf("%v %v", turn, err)
	}

	dead := &AnthropicClient{Config: config.ModelConfig{Model: "m", BaseURL: "http://127.0.0.1:1", APIKey: anthKey, TimeoutSeconds: 2}, RetryDelay: time.Millisecond}
	_, err = dead.Complete(context.Background(), Request{Messages: []Message{{Role: "user", Content: "x"}}}, nil)
	if err == nil || strings.Contains(err.Error(), anthKey) {
		t.Fatalf("%v", err)
	}
}

func TestAnthropicEndpoints(t *testing.T) {
	c := &AnthropicClient{}
	if c.endpoint() != "https://api.anthropic.com/v1/messages" || c.ModelsEndpoint() != "https://api.anthropic.com/v1/models" {
		t.Error(c.endpoint(), c.ModelsEndpoint())
	}
	c.Config.BaseURL = "https://proxy.example/v1/"
	if c.endpoint() != "https://proxy.example/v1/messages" {
		t.Error(c.endpoint())
	}
}
