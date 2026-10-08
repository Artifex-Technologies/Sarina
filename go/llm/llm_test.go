package llm

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

func newTestClient(url string) *Client {
	return &Client{Config: config.ModelConfig{Model: "m", BaseURL: url, APIKey: "k", TimeoutSeconds: 5}, RetryDelay: time.Millisecond}
}

func readTool() ToolSpec { return ToolSpec{Name: "read_file", Description: "d"} }

func serve(t *testing.T, h http.HandlerFunc) *httptest.Server {
	t.Helper()
	s := httptest.NewServer(h)
	t.Cleanup(s.Close)
	return s
}

func TestBuildBodyOmitsToolsWhenEmpty(t *testing.T) {
	c := newTestClient("http://x")
	b, err := c.BuildBody(Request{Messages: []Message{{Role: "user", Content: "hi"}}})
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	json.Unmarshal(b, &m)
	for _, k := range []string{"tools", "tool_choice", "stream_options", "response_format", "max_tokens"} {
		if _, ok := m[k]; ok {
			t.Fatalf("%s must be omitted: %s", k, b)
		}
	}
	if m["temperature"] != float64(0) || m["stream"] != false || m["model"] != "m" {
		t.Fatalf("body = %s", b)
	}
	b, _ = c.BuildBody(Request{Tools: []ToolSpec{readTool()}, Stream: true, MaxTokens: 77,
		Schema: &config.OutputSchema{Name: "n", Schema: map[string]any{"type": "object"}, Strict: true}})
	json.Unmarshal(b, &m)
	if m["tool_choice"] != "auto" || m["max_tokens"] != float64(77) || m["stream_options"].(map[string]any)["include_usage"] != true {
		t.Fatalf("body = %s", b)
	}
	if m["response_format"].(map[string]any)["json_schema"].(map[string]any)["strict"] != true {
		t.Fatalf("schema missing: %s", b)
	}
}

func TestEndpointJoinWithAndWithoutSlash(t *testing.T) {
	for _, base := range []string{"http://h/v1", "http://h/v1/", "http://h/v1//"} {
		if got := newTestClient(base).endpoint(); got != "http://h/v1/chat/completions" {
			t.Fatalf("%s -> %s", base, got)
		}
	}
}

func TestParseResponseShapes(t *testing.T) {
	parse := func(s string) (*Turn, error) { return parseResponse([]byte(s)) }

	t1, err := parse(`{"choices":[{"message":{"content":null},"finish_reason":null}]}`)
	if err != nil || t1.Content != "" || t1.HasFinishReason {
		t.Fatalf("null content/finish: %+v %v", t1, err)
	}
	t2, _ := parse(`{"choices":[{"message":{"content":["a",{"type":"text","text":"b"},{"x":1}]},"finish_reason":"stop"}]}`)
	if t2.Content != `ab{"x":1}` {
		t.Fatalf("flattened = %q", t2.Content)
	}
	t3, _ := parse(`{"choices":[{"message":{"content":"x","reasoning_content":"because"},"finish_reason":7}]}`)
	if t3.Reasoning != "because" || t3.FinishReason != "7" {
		t.Fatalf("%+v", t3)
	}
	for _, bad := range []string{`nope`, `[]`, `{"choices":[]}`, `{"choices":[1]}`, `{"choices":[{}]}`} {
		if _, err := parse(bad); err == nil {
			t.Fatalf("%s should fail", bad)
		} else if _, ok := err.(*BackendError); !ok {
			t.Fatalf("error type = %T", err)
		}
	}
	_, err = parse(`{"error":"model not loaded"}`)
	if err == nil || !strings.Contains(err.Error(), "model not loaded") {
		t.Fatalf("ollama style error: %v", err)
	}
	_, err = parse(`{"choices":[{"message":{"tool_calls":[{"function":{"arguments":"{}"}}]}}]}`)
	if err == nil || !strings.Contains(err.Error(), "missing function name") {
		t.Fatalf("missing name: %v", err)
	}
}

func TestParseToolCalls(t *testing.T) {
	body := `{"choices":[{"message":{"content":"","tool_calls":[
		{"id":"c1","function":{"name":"a","arguments":"{\"x\": 1}"}},
		{"function":{"name":"b","arguments":{"y":"z"}}},
		{"id":"c3","function":{"name":"c"}},
		{"id":"c4","function":{"name":"d","arguments":"   "}},
		{"id":"c5","function":{"name":"e","arguments":"[1,2]"}},
		{"id":"c6","function":{"name":"f","arguments":42}}]},"finish_reason":"tool_calls"}]}`
	turn, err := parseResponse([]byte(body))
	if err != nil {
		t.Fatal(err)
	}
	Normalize(turn, []string{"a", "b", "c", "d", "e", "f"})
	calls := turn.ToolCalls
	if len(calls) != 6 {
		t.Fatalf("calls = %d", len(calls))
	}
	if calls[0].Arguments["x"] != float64(1) || calls[1].Arguments["y"] != "z" || len(calls[2].Arguments) != 0 || len(calls[3].Arguments) != 0 {
		t.Fatalf("arguments wrong: %+v", calls)
	}
	if calls[1].ID != "call_1" {
		t.Fatalf("synthesised id = %q", calls[1].ID)
	}
	if calls[4].ParseError == "" || calls[5].ParseError == "" {
		t.Fatalf("non-object arguments must be flagged, not fatal: %+v", calls[4:])
	}
	legacy, _ := parseResponse([]byte(`{"choices":[{"message":{"function_call":{"name":"a","arguments":"{}"}}}]}`))
	if len(legacy.ToolCalls) != 1 || legacy.ToolCalls[0].ID != "call_0" {
		t.Fatalf("legacy function_call: %+v", legacy.ToolCalls)
	}
}

func TestUsageMapping(t *testing.T) {
	cases := []struct {
		in   string
		want Usage
	}{
		{`{"prompt_tokens":10,"completion_tokens":4}`, Usage{InputTokens: 10, OutputTokens: 4}},
		{`{"input_tokens":3,"output_tokens":2,"cache_creation_input_tokens":5,"cache_read_input_tokens":6}`,
			Usage{3, 2, 5, 6, 0}},
		{`{"prompt_eval_count":8,"eval_count":9}`, Usage{InputTokens: 8, OutputTokens: 9}},
		{`{"input_tokens":0,"prompt_tokens":7}`, Usage{InputTokens: 7}},
		{`{"completion_tokens":5,"completion_tokens_details":{"reasoning_tokens":3}}`, Usage{OutputTokens: 5, ReasoningTokens: 3}},
		{`{"reasoning_tokens":2,"completion_tokens_details":{"reasoning_tokens":9}}`, Usage{ReasoningTokens: 2}},
		{`{"prompt_tokens":"12","completion_tokens":4.9}`, Usage{InputTokens: 12, OutputTokens: 4}},
		{`{"prompt_tokens":true,"completion_tokens":"abc"}`, Usage{}},
		{`{"prompt_tokens":20,"prompt_tokens_details":{"cached_tokens":15}}`, Usage{InputTokens: 20, CacheReadInputTokens: 15}},
		{`"not an object"`, Usage{}},
	}
	for _, c := range cases {
		var v any
		json.Unmarshal([]byte(c.in), &v)
		if got := UsageFromProvider(v); got != c.want {
			t.Errorf("%s: got %+v want %+v", c.in, got, c.want)
		}
	}
	u := Usage{1, 2, 3, 4, 100}
	if u.Total() != 10 {
		t.Fatalf("total must exclude reasoning: %d", u.Total())
	}
	b, _ := json.Marshal(u)
	if !strings.Contains(string(b), `"total_tokens":10`) {
		t.Fatalf("marshal = %s", b)
	}
	var back Usage
	json.Unmarshal(b, &back)
	if back != u {
		t.Fatalf("round trip: %+v", back)
	}
}

func TestRepairJSON(t *testing.T) {
	good := map[string]any{
		`{"a": 1}`:                    map[string]any{"a": 1.0},
		"```json\n{\"a\": 1}\n```":    map[string]any{"a": 1.0},
		`{"a": 1,}`:                   map[string]any{"a": 1.0},
		`{'a': 'it\'s', "b": [1,2,]}`: map[string]any{"a": "it's", "b": []any{1.0, 2.0}},
		"{\"a\": \"line1\nline2\"}":   map[string]any{"a": "line1\nline2"},
		`{"a": 1} trailing words`:     map[string]any{"a": 1.0},
		`{"a": {"b": [1, 2`:           map[string]any{"a": map[string]any{"b": []any{1.0, 2.0}}},
		`{"a": "cut o`:                map[string]any{"a": "cut o"},
		`{"a": 1, "b":`:               map[string]any{"a": 1.0, "b": nil},
		`{"quote": 'say "hi"'}`:       map[string]any{"quote": `say "hi"`},
	}
	for in, want := range good {
		got, _, ok := RepairJSON(in)
		if !ok || !reflect.DeepEqual(got, want) {
			t.Errorf("%q -> %v (ok=%v), want %v", in, got, ok, want)
		}
	}
	for _, in := range []string{``, `not json`, `{"a" "b"}`, `{{{`} {
		if _, _, ok := RepairJSON(in); ok {
			t.Errorf("%q should not repair", in)
		}
	}
	if _, trunc, _ := RepairJSON(`{"a": "cut`); !trunc {
		t.Error("truncation not reported")
	}
	if _, trunc, _ := RepairJSON(`{"a": 1,}`); trunc {
		t.Error("trailing comma is not truncation")
	}
}

func TestNormalizeThinkBlocks(t *testing.T) {
	cases := map[string][2]string{
		"<think>plan</think>Answer":                   {"Answer", "plan"},
		"Before <think>x</think> after":               {"Before  after", "x"},
		"<think>unfinished reasoning that never ends": {"", "unfinished reasoning that never ends"},
		"reasoning only</think>The answer":            {"The answer", "reasoning only"},
		"no thinking here":                            {"no thinking here", ""},
		"<think>a</think>x<think>b</think>y":          {"xy", "a" + "b"},
	}
	for in, want := range cases {
		turn := &Turn{Content: in}
		Normalize(turn, nil)
		if turn.Content != want[0] {
			t.Errorf("%q: content %q, want %q", in, turn.Content, want[0])
		}
		if strings.ReplaceAll(turn.Reasoning, "\n", "") != want[1] {
			t.Errorf("%q: reasoning %q, want %q", in, turn.Reasoning, want[1])
		}
	}
}

func TestNormalizeEmbeddedToolCalls(t *testing.T) {
	known := []string{"read_file", "grep"}
	check := func(name, content string, wantCalls int, wantRest string) {
		t.Helper()
		turn := &Turn{Content: content}
		Normalize(turn, known)
		if len(turn.ToolCalls) != wantCalls || turn.Content != wantRest {
			t.Errorf("%s: calls=%d rest=%q (%+v)", name, len(turn.ToolCalls), turn.Content, turn.ToolCalls)
		}
	}
	check("qwen block", "Reading.\n<tool_call>\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}\n</tool_call>", 1, "Reading.")
	check("two blocks", `<tool_call>{"name":"read_file","arguments":{}}</tool_call><tool_call>{"name":"grep","arguments":{"q":"x"}}</tool_call>`, 2, "")
	check("unterminated block", `<tool_call>{"name":"grep","arguments":{"q":"x"}}`, 1, "")
	check("python tag", `<|python_tag|>{"name":"grep","parameters":{"q":"y"}}<|eom_id|>`, 1, "")
	check("bare json", `{"name":"read_file","arguments":{"path":"p"}}`, 1, "")
	check("fenced json", "```json\n{\"name\":\"grep\",\"arguments\":\"{\\\"q\\\":\\\"z\\\"}\"}\n```", 1, "")
	check("array", `[{"name":"grep","arguments":{}},{"name":"read_file","arguments":{}}]`, 2, "")
	check("openai nested", `{"type":"function","function":{"name":"grep","arguments":{"q":"n"}}}`, 1, "")
	check("unknown tool stays text", `{"name":"rm_rf","arguments":{}}`, 0, `{"name":"rm_rf","arguments":{}}`)
	check("prose with braces", `Use {"name": "x"} style.`, 0, `Use {"name": "x"} style.`)
	check("garbage block stays", `<tool_call>oops</tool_call>`, 0, `<tool_call>oops</tool_call>`)

	// not applied when there are no tools, or when real tool calls exist
	turn := &Turn{Content: `{"name":"read_file","arguments":{}}`}
	Normalize(turn, nil)
	if len(turn.ToolCalls) != 0 {
		t.Error("converted without tools")
	}
	turn = &Turn{Content: `<tool_call>{"name":"grep","arguments":{}}</tool_call>`, ToolCalls: []ToolCall{{ID: "r", Name: "read_file"}}}
	Normalize(turn, known)
	if len(turn.ToolCalls) != 1 || turn.ToolCalls[0].Name != "read_file" {
		t.Errorf("structured calls must win: %+v", turn.ToolCalls)
	}
	// embedded args: parsed value reaches the call
	turn = &Turn{Content: `<tool_call>{"name":"grep","arguments":{"q":"x"}}</tool_call>`}
	Normalize(turn, known)
	if turn.ToolCalls[0].Arguments["q"] != "x" || turn.ToolCalls[0].ID == "" {
		t.Errorf("call = %+v", turn.ToolCalls[0])
	}
}

func TestNormalizeIDsAndTruncation(t *testing.T) {
	turn := &Turn{ToolCalls: []ToolCall{{ID: "", Name: "a"}, {ID: "", Name: "a"}, {ID: "x", Name: "a"}, {ID: "x", Name: "a"}, {ID: "call_0", Name: "a"}}}
	Normalize(turn, nil)
	seen := map[string]bool{}
	for _, c := range turn.ToolCalls {
		if c.ID == "" || seen[c.ID] {
			t.Fatalf("ids = %+v", turn.ToolCalls)
		}
		seen[c.ID] = true
	}
	if turn.ToolCalls[2].ID != "x" {
		t.Fatalf("first occurrence keeps its id: %+v", turn.ToolCalls)
	}

	body := `{"choices":[{"message":{"tool_calls":[{"id":"t","function":{"name":"w","arguments":"{\"path\": \"a.txt\", \"content\": \"half"}}]},"finish_reason":"length"}]}`
	cut, _ := parseResponse([]byte(body))
	Normalize(cut, nil)
	if cut.ToolCalls[0].ParseError == "" {
		t.Fatal("arguments cut off by a length stop must not be executed")
	}
	ok, _ := parseResponse([]byte(strings.Replace(body, `"length"`, `"tool_calls"`, 1)))
	Normalize(ok, nil)
	if ok.ToolCalls[0].ParseError != "" {
		t.Fatalf("repair should be accepted on a normal stop: %+v", ok.ToolCalls[0])
	}
}

func completeStream(t *testing.T, lines string) (*Turn, []StreamEvent, error) {
	t.Helper()
	s := serve(t, func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, lines)
	})
	var evs []StreamEvent
	turn, err := newTestClient(s.URL).Complete(context.Background(), Request{Stream: true, Tools: []ToolSpec{readTool()}},
		func(e StreamEvent) { evs = append(evs, e) })
	return turn, evs, err
}

func TestStreamParsing(t *testing.T) {
	turn, evs, err := completeStream(t, strings.Join([]string{
		": keep-alive comment",
		"event: ignored",
		`data: {"choices":[{"delta":{"content":"Hel"}}]}`,
		"",
		"id: 7",
		"data:" + `{"choices":[{"delta":{"content":"lo"}}]}`,
		"",
		`data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}`,
		"",
		"data: [DONE]",
		"",
	}, "\r\n"))
	if err != nil {
		t.Fatal(err)
	}
	if turn.Content != "Hello" || turn.FinishReason != "stop" || turn.Usage.InputTokens != 5 || !turn.UsageReported {
		t.Fatalf("turn = %+v", turn)
	}
	var types []string
	for _, e := range evs {
		types = append(types, e.Type)
	}
	if got := strings.Join(types, ","); got != "message_start,content_delta,content_delta,usage,message_stop" {
		t.Fatalf("events = %s", got)
	}
}

func TestStreamTrailingEventWithoutBlankLine(t *testing.T) {
	turn, _, err := completeStream(t, `data: {"choices":[{"delta":{"content":"tail"},"finish_reason":"stop"}]}`)
	if err != nil || turn.Content != "tail" {
		t.Fatalf("%+v %v", turn, err)
	}
}

func TestStreamUsageIsLastNotSummed(t *testing.T) {
	turn, _, err := completeStream(t, strings.Join([]string{
		`data: {"choices":[{"delta":{"content":"a"}}],"usage":{"prompt_tokens":10,"completion_tokens":1}}`, "",
		`data: {"choices":[{"delta":{"content":"b"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}`, "",
		`data: {"choices":[{"delta":{"content":"c"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3}}`, "",
		"data: [DONE]", ""}, "\n"))
	if err != nil {
		t.Fatal(err)
	}
	if turn.Usage.InputTokens != 10 || turn.Usage.OutputTokens != 3 {
		t.Fatalf("cumulative usage chunks were summed: %+v", turn.Usage)
	}
}

func TestStreamToolCallAssembly(t *testing.T) {
	chunk := func(tcs string) string {
		return `data: {"choices":[{"delta":{"tool_calls":[` + tcs + `]}}]}` + "\n\n"
	}
	turn, _, err := completeStream(t,
		chunk(`{"index":0,"id":"A","function":{"name":"read_","arguments":"{\"path\":"}}`)+
			chunk(`{"index":0,"function":{"name":"file","arguments":"\"a\"}"}}`)+
			// a server that reuses index 0 for a second parallel call, distinguished only by id
			chunk(`{"index":0,"id":"B","function":{"name":"read_file","arguments":"{\"path\":\"b\"}"}}`)+
			chunk(`{"index":1,"id":"C","function":{"name":"read_file","arguments":""}}`)+
			"data: [DONE]\n\n")
	if err != nil {
		t.Fatal(err)
	}
	if len(turn.ToolCalls) != 3 {
		t.Fatalf("calls = %+v", turn.ToolCalls)
	}
	a, b := turn.ToolCalls[0], turn.ToolCalls[1]
	if a.ID != "A" || a.Name != "read_file" || a.Arguments["path"] != "a" || b.ID != "B" || b.Arguments["path"] != "b" {
		t.Fatalf("assembly wrong: %+v", turn.ToolCalls)
	}
}

func TestStreamReasoningFieldAndErrors(t *testing.T) {
	turn, _, err := completeStream(t, `data: {"choices":[{"delta":{"reasoning_content":"thinking","content":"ans"},"finish_reason":"stop"}]}`+"\n\ndata: [DONE]\n\n")
	if err != nil || turn.Content != "ans" || turn.Reasoning != "thinking" {
		t.Fatalf("%+v %v", turn, err)
	}
	if _, _, err := completeStream(t, "data: {broken\n\n"); err == nil || !strings.Contains(err.Error(), "invalid JSON") {
		t.Fatalf("err = %v", err)
	}
	if _, _, err := completeStream(t, ": only comments\n\ndata: [DONE]\n\n"); err == nil {
		t.Fatal("an empty stream must be an error")
	}
	if _, _, err := completeStream(t, `data: {"error":{"message":"overloaded"}}`+"\n\n"); err == nil || !strings.Contains(err.Error(), "overloaded") {
		t.Fatalf("err = %v", err)
	}
	if _, _, err := completeStream(t, `data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}`+"\n\n"); err == nil {
		t.Fatal("a streamed call without a name must be an error")
	}
}

func TestRetryPolicy(t *testing.T) {
	var n atomic.Int32
	s := serve(t, func(w http.ResponseWriter, r *http.Request) {
		if n.Add(1) == 1 {
			w.WriteHeader(503)
			io.WriteString(w, "warming up")
			return
		}
		io.WriteString(w, `{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}`)
	})
	turn, err := newTestClient(s.URL).Complete(context.Background(), Request{}, nil)
	if err != nil || turn.Content != "ok" || n.Load() != 2 {
		t.Fatalf("5xx should retry once: %v %v calls=%d", turn, err, n.Load())
	}

	n.Store(0)
	s2 := serve(t, func(w http.ResponseWriter, r *http.Request) {
		n.Add(1)
		w.WriteHeader(400)
		io.WriteString(w, `{"error":"prompt is too long"}`)
	})
	_, err = newTestClient(s2.URL).Complete(context.Background(), Request{}, nil)
	be, ok := err.(*BackendError)
	if !ok || be.Status != 400 || n.Load() != 1 || !strings.Contains(err.Error(), "prompt is too long") {
		t.Fatalf("4xx must not retry and must carry the body: %v calls=%d", err, n.Load())
	}

	n.Store(0)
	s3 := serve(t, func(w http.ResponseWriter, r *http.Request) { n.Add(1); w.WriteHeader(502) })
	if _, err = newTestClient(s3.URL).Complete(context.Background(), Request{}, nil); err == nil || n.Load() != 2 {
		t.Fatalf("persistent 5xx: err=%v calls=%d", err, n.Load())
	}
}

func TestConnectionFailureAndTimeout(t *testing.T) {
	s := serve(t, func(w http.ResponseWriter, r *http.Request) {})
	url := s.URL
	s.Close()
	_, err := newTestClient(url).Complete(context.Background(), Request{}, nil)
	if _, ok := err.(*BackendError); !ok || !strings.Contains(err.Error(), url) {
		t.Fatalf("connection failure should be a BackendError naming the URL: %v", err)
	}

	slow := serve(t, func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(300 * time.Millisecond)
		io.WriteString(w, `{}`)
	})
	c := newTestClient(slow.URL)
	c.Config.TimeoutSeconds = 0.05
	start := time.Now()
	_, err = c.Complete(context.Background(), Request{}, nil)
	if _, ok := err.(*BackendError); !ok || !strings.Contains(err.Error(), "timed out") {
		t.Fatalf("timeout: %v", err)
	}
	if time.Since(start) > 250*time.Millisecond {
		t.Fatal("timeout was not enforced promptly")
	}
}

func TestContextCancellationReturnsCtxErr(t *testing.T) {
	slow := serve(t, func(w http.ResponseWriter, r *http.Request) { time.Sleep(300 * time.Millisecond) })
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	_, err := newTestClient(slow.URL).Complete(ctx, Request{}, nil)
	if err != context.DeadlineExceeded {
		t.Fatalf("err = %v", err)
	}
}

func TestRequestHeaders(t *testing.T) {
	var auth, ct string
	s := serve(t, func(w http.ResponseWriter, r *http.Request) {
		auth, ct = r.Header.Get("Authorization"), r.Header.Get("Content-Type")
		if r.URL.Path != "/v1/chat/completions" {
			t.Errorf("path = %s", r.URL.Path)
		}
		io.WriteString(w, `{"choices":[{"message":{"content":"x"}}]}`)
	})
	c := newTestClient(s.URL + "/v1/")
	if _, err := c.Complete(context.Background(), Request{}, nil); err != nil {
		t.Fatal(err)
	}
	if auth != "Bearer k" || ct != "application/json" {
		t.Fatalf("headers: %q %q", auth, ct)
	}
}

func TestToolCallArgumentsJSONIsASCII(t *testing.T) {
	c := ToolCall{ID: "i", Name: "n", Arguments: map[string]any{"s": "é<>&😀"}}
	got := c.ArgumentsJSON()
	if got != "{\"s\":\"\\u00e9<>&\\ud83d\\ude00\"}" {
		t.Fatalf("got %s", got)
	}
	var back map[string]any
	if err := json.Unmarshal([]byte(got), &back); err != nil || back["s"] != "é<>&😀" {
		t.Fatalf("not reversible: %v %v", back, err)
	}
	if (ToolCall{}).ArgumentsJSON() != "{}" {
		t.Fatal("nil args should serialise as {}")
	}
	_ = fmt.Sprint
}
