package llm

import (
	"context"
	"crypto/tls"
	"math"
	"net"
	"net/http"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// anthropicRequests are the requests BuildBody is recorded on (the Rust test builds the same list).
func anthropicRequests() []Request {
	calls := []WireToolCall{
		{ID: "call.1/é", Type: "function", Function: FunctionCall{Name: "list_dir", Arguments: `{"path":"<a> & b","n":1.5e3,"z":null}`}},
		{ID: "", Type: "function", Function: FunctionCall{Name: "read_file", Arguments: `not json`}},
		{ID: "c3", Type: "function", Function: FunctionCall{Name: "x", Arguments: `null`}},
		{ID: "c4", Type: "function", Function: FunctionCall{Name: "y", Arguments: `[1,2]`}},
		{ID: "c5", Type: "function", Function: FunctionCall{Name: "z", Arguments: `{"a":1,"a":2,"b":"é\ud800"}`}},
	}
	schema := &config.OutputSchema{Name: "out<1>", Schema: map[string]any{"type": "object", "properties": map[string]any{"a": map[string]any{"type": "string", "pattern": "<&>"}}}}
	tools := []ToolSpec{{Name: "list_dir", Description: "lists <dirs> & more", Parameters: map[string]any{"type": "object", "properties": map[string]any{"path": map[string]any{"type": "string"}}}},
		{Name: "plain"}}
	return []Request{
		{},
		{Messages: []Message{{Role: "system", Content: "sys one"}, {Role: "system", Content: "  "}, {Role: "user", Content: "list <files>"},
			{Role: "assistant", Content: "sure", ToolCalls: calls}, {Role: "tool", ToolCallID: "call.1/é", Content: "a.txt"},
			{Role: "tool", ToolCallID: "", Content: " \n"}, {Role: "user", Content: "thanks"}, {Role: "system", Content: "late note"}},
			Tools: tools},
		{Messages: []Message{{Role: "assistant", Content: "I start"}, {Role: "assistant", Content: "  "}, {Role: "developer", Content: "odd role"},
			{Role: "user", Content: ""}}, MaxTokens: 100, Schema: schema},
		{Messages: []Message{{Role: "tool", ToolCallID: "t!", Content: "r"}, {Role: "user", Content: "q"}}, MaxTokens: -5, Tools: tools[1:]},
		{Messages: []Message{{Role: "user", Content: "a /no_think"}}, Stream: true, NoThink: true},
	}
}

// anthropicAnswers are bodies the Messages API (or something pretending) may send.
var anthropicAnswers = []string{
	`{"type":"message","role":"assistant","stop_reason":"tool_use","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"checking"},` +
		`{"type":"tool_use","id":"toolu_1","name":"read_file","input":{"path":"a.go","é":"<x>"}}],"usage":{"input_tokens":11,"output_tokens":4,"cache_read_input_tokens":2}}`,
	`{"content":[{"type":"text","text":"a"},{"type":"text"},5,null,{"type":7},{"type":"tool_use","name":"x","input":[1]},{"type":"redacted_thinking","data":"z"}],"stop_reason":"end_turn"}`,
	`{"content":[{"type":"tool_use","id":"t","input":{}}]}`, `{"content":[],"stop_reason":"max_tokens"}`, `{"content":[],"stop_reason":"stop_sequence"}`,
	`{"content":[],"stop_reason":"pause_turn"}`, `{"content":[],"stop_reason":"refusal"}`, `{"content":[],"stop_reason":"model_context_window_exceeded"}`,
	`{"content":[],"stop_reason":""}`, `{"content":[],"stop_reason":5}`, `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}`,
	`{"type":"error","error":"plain"}`, `{"type":"error"}`, `{"content":"text"}`, `{}`, `null`, `[]`, `"x"`, `not json`, ``, `{"content":[]} junk`,
	`{"content":[{"type":"text","text":"x"}],"usage":{"input_tokens":3,"output_tokens":"4"}}`,
	`{"type":"message","content":[{"type":"text","text":"é\ud800"}],"usage":{"prompt_tokens":3}}`,
}

func anthropicEndpointBases() []string {
	return []string{"", "https://api.anthropic.com", "https://proxy.example/v1/", "https://proxy.example/v1//", "http://h:1/api", "http://h:1/v1x", "http://h:1/apiv1", "/"}
}

// TestParityAnthropic records the Anthropic client (testdata/parity/anthropic.json): BuildBody, parseAnthropic, the
// endpoints, anthropicID, and Complete against scripted servers (the retry, the key scrubbed from every error, the timer,
// cancellation, the replay of a finished answer as events, TLS against an authority made in the test).
func TestParityAnthropic(t *testing.T) {
	if testing.Short() {
		t.Skip("timed cases")
	}
	var cases []parity.Case
	for i, r := range anthropicRequests() {
		for _, temp := range []float64{0, 0.7, -1, 1e-7} {
			c := &AnthropicClient{Config: config.ModelConfig{Model: "claude <x>", Temperature: temp}}
			b, err := c.BuildBody(r)
			cases = append(cases, parity.Case{Fn: "body", In: []any{i, strconv.FormatUint(math.Float64bits(temp), 16)}, Out: []any{parity.Text(string(b)), parity.Err(err)}})
		}
	}
	for _, a := range anthropicAnswers {
		turn, err := parseAnthropic([]byte(a))
		cases = append(cases, parity.Case{Fn: "parse", In: parity.Text(a), Out: []any{turnFields(turn), errOut(err)}})
	}
	for _, b := range anthropicEndpointBases() {
		c := &AnthropicClient{Config: config.ModelConfig{BaseURL: b}}
		cases = append(cases, parity.Case{Fn: "endpoints", In: b, Out: []string{c.endpoint(), c.ModelsEndpoint()}})
	}
	for _, id := range []string{"", "call_1", "call.1/é", "a-b_C9", "\xff\xfex", "日本"} {
		cases = append(cases, parity.Case{Fn: "id", In: parity.Text(id), Out: anthropicID(id)})
	}

	ok := `{"type":"message","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":5,"output_tokens":2}}`
	echo := `{"type":"error","error":{"message":"invalid x-api-key ` + anthKey + `"}}`
	ca := parity.NewTestCA(t, "Sarina parity test CA")
	type acase struct {
		name              string
		script            [][]S
		tls, base, apiKey string
		timeout           float64
		stream            bool
		maxTokens         int
		cancelMs, retryMs int
		maxResp           int64
	}
	for _, tc := range []acase{
		{name: "ok", script: [][]S{okJSON(ok)}},
		{name: "okReplay", script: [][]S{okJSON(ok)}, stream: true},
		{name: "replayNoUsage", script: [][]S{okJSON(`{"content":[],"stop_reason":"tool_use"}`)}, stream: true},
		{name: "baseV1", script: [][]S{okJSON(ok)}, base: "/v1"},
		{name: "basePath", script: [][]S{okJSON(ok)}, base: "/api//"},
		{name: "retry529", script: [][]S{statusJSON(529, `{"type":"error"}`), okJSON(ok)}},
		{name: "retry429Twice", script: [][]S{statusJSON(429, "slow"), statusJSON(429, "slow")}},
		{name: "noRetry401Scrubbed", script: [][]S{statusJSON(401, echo)}},
		{name: "errorIn200Scrubbed", script: [][]S{okJSON(echo)}},
		{name: "notJSON", script: [][]S{okJSON("nope " + anthKey)}},
		{name: "eofTwice", script: [][]S{{}, {}}},
		{name: "refusedKeyInURL", base: "refused"},
		{name: "badKey", apiKey: "k\nX: 1", script: [][]S{okJSON(ok)}},
		{name: "timeoutKeyInURL", script: [][]S{{{Wait: 1500}, {Send: okJSON(ok)[0].Send}}}, timeout: 0.3, base: "/" + anthKey},
		{name: "maxTokensDoNotExtend", script: [][]S{{{Wait: 1500}, {Send: okJSON(ok)[0].Send}}}, timeout: 0.3, maxTokens: 4000},
		{name: "bodyTimeout", script: [][]S{{{Send: "HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n{\"con"}, {Wait: 1500}}}, timeout: 0.3},
		{name: "cancel", script: [][]S{{{Hold: true}}}, cancelMs: 250},
		{name: "cancelInRetryWait", script: [][]S{statusJSON(503, "a")}, cancelMs: 250, retryMs: 3000},
		{name: "tooLarge", script: [][]S{okJSON(ok)}, maxResp: 64},
		{name: "bodyCutShort", script: [][]S{{{Send: "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"content\":[]}"}}}},
		{name: "tlsOK", script: [][]S{okJSON(ok)}, tls: "trusted"},
		{name: "tlsUntrusted", script: [][]S{okJSON(ok), okJSON(ok)}, tls: "untrusted"},
	} {
		var srvCfg *tls.Config
		clientCfg := &tls.Config{RootCAs: ca.Pool}
		scheme := "http"
		switch tc.tls {
		case "trusted":
			srvCfg, scheme = ca.Server(t, []string{"127.0.0.1"}, nil), "https"
		case "untrusted":
			srvCfg, scheme = ca.Server(t, []string{"127.0.0.1"}, nil), "https"
			clientCfg = &tls.Config{RootCAs: parity.NewTestCA(t, "Another test CA").Pool}
		}
		addr, seen := "", func() []map[string]any { return nil }
		var base string
		if tc.base == "refused" {
			ln, _ := net.Listen("tcp", "127.0.0.1:0")
			addr = ln.Addr().String()
			ln.Close()
			base = "http://" + addr + "/" + anthKey
		} else {
			addr, seen = parity.RawSeq(t, srvCfg, tc.script)
			base = scheme + "://" + addr + tc.base
		}
		key := tc.apiKey
		if key == "" {
			key = anthKey
		}
		c := &AnthropicClient{Config: config.ModelConfig{Model: "claude-test", BaseURL: base, APIKey: key, Temperature: 0.3, TimeoutSeconds: tc.timeout},
			RetryDelay: time.Duration(max(tc.retryMs, 30)) * time.Millisecond,
			HTTPClient: &http.Client{Transport: &http.Transport{DisableKeepAlives: true, TLSClientConfig: clientCfg}}}
		ctx, cancel := context.Background(), context.CancelFunc(func() {})
		if tc.cancelMs > 0 {
			ctx, cancel = context.WithCancel(ctx)
			time.AfterFunc(time.Duration(tc.cancelMs)*time.Millisecond, cancel)
		}
		oldR := maxResponseBytes
		if tc.maxResp > 0 {
			maxResponseBytes = tc.maxResp
		}
		var evs []any
		turn, err := c.Complete(ctx, Request{Messages: []Message{{Role: "user", Content: "hi"}}, Stream: tc.stream, MaxTokens: tc.maxTokens},
			func(e StreamEvent) {
				var u any
				if e.Usage != nil {
					u = []int{e.Usage.InputTokens, e.Usage.OutputTokens}
				}
				evs = append(evs, []any{e.Type, parity.Text(e.Delta), e.FinishReason, u})
			})
		maxResponseBytes = oldR
		cancel()
		time.Sleep(150 * time.Millisecond)
		in := map[string]any{"name": tc.name, "script": parity.Steps(tc.script), "tls": tc.tls, "base": tc.base, "timeout": tc.timeout,
			"stream": tc.stream, "maxTokens": tc.maxTokens, "cancelMs": tc.cancelMs, "retryMs": tc.retryMs, "maxResp": tc.maxResp, "apiKey": tc.apiKey}
		out := map[string]any{"turn": turnFields(turn), "err": errShort(err, addr), "events": evs, "requests": seen()}
		if err != nil && strings.Contains(err.Error(), anthKey) {
			t.Errorf("%s: the key is in the error: %v", tc.name, err)
		}
		cases = append(cases, parity.Case{Fn: "complete", In: in, Out: out})
	}
	cases = append(cases, parity.Case{Fn: "key", Out: anthKey})
	parity.Fixture(t, "anthropic", cases)
}
