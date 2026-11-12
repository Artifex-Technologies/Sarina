package llm

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

func turnFields(t *Turn) any {
	if t == nil {
		return nil
	}
	var calls []any
	for _, c := range t.ToolCalls {
		var a any
		if c.Arguments != nil {
			a = parity.Typed(c.Arguments)
		}
		calls = append(calls, []any{c.ID, c.Name, a, parity.Text(c.RawArguments), c.ParseError, c.truncated})
	}
	var raw any
	if t.Raw != nil {
		raw = parity.Typed(t.Raw)
	}
	u := t.Usage
	return map[string]any{"content": parity.Text(t.Content), "reasoning": parity.Text(t.Reasoning), "finish": t.FinishReason, "hasFinish": t.HasFinishReason,
		"usage": []int{u.InputTokens, u.OutputTokens, u.ReasoningTokens, u.CacheCreationInputTokens, u.CacheReadInputTokens}, "usageReported": t.UsageReported,
		"calls": calls, "raw": raw}
}

func errOut(err error) any {
	if err == nil {
		return nil
	}
	if be, ok := err.(*BackendError); ok {
		return []any{parity.Text(be.Message), be.Status}
	}
	return []any{err.Error(), -1}
}

// openAIAnswers are bodies an OpenAI-compatible server may send for a non-streamed call.
var openAIAnswers = []string{
	`{"choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}`,
	`{"choices":[{"message":{"content":[{"type":"text","text":"a"},"b",{"x":"<é>"},5,null],"reasoning_content":"think","reasoning":"other"},"finish_reason":7}]}`,
	`{"choices":[{"message":{"content":null,"reasoning":"r","tool_calls":[{"id":"c1","function":{"name":"read_file","arguments":"{\"path\":\"a\"}"}},` +
		`{"function":{"name":"bash","arguments":{"command":"ls"}}},{"id":"c3","function":{"name":"x","arguments":"{\"a\":"}},{"function":{"name":"y","arguments":5}},` +
		`{"function":{"name":"z"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"completion_tokens_details":{"reasoning_tokens":0}}}`,
	`{"choices":[{"message":{"content":"x","tool_calls":[{"function":{"arguments":"{}"}}]}}]}`, `{"choices":[{"message":{"content":"x","tool_calls":[5]}}]}`,
	`{"choices":[{"message":{"content":"x","function_call":{"name":"bash","arguments":"{\"command\":\"pwd\"}"}}}]}`,
	`{"choices":[{"message":{"content":"x","function_call":{"arguments":"{}"}}}]}`, `{"choices":[{"message":{"content":"x","tool_calls":"no"}}]}`,
	`{"choices":[]}`, `{"choices":[],"error":"overloaded"}`, `{"error":{"message":"model not found","code":404}}`, `{"error":{"code":5}}`, `{"error":[1,"<"]}`,
	`{"choices":[5]}`, `{"choices":[{}]}`, `{"choices":[{"message":"text"}]}`, `[]`, `"x"`, `null`, `not json`, ``, `{"choices":[{"message":{"content":"x"}}]} junk`,
	`{"choices":[{"message":{"content":{"a":1}},"finish_reason":null}]}`, `{"choices":[{"message":{"content":"x"},"finish_reason":{"a":1}}],"usage":{"input_tokens":3}}`,
	`{"choices":[{"message":{"content":"<think>plan</think>answer"}}]}`, `{"choices":[{"message":{"content":"x","reasoning":""}}],"usage":{"total_tokens":9}}`,
	`{"choices":[{"message":{"content":"x","reasoning":"\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9"}}]}`,
}

// openAIStreams are server-sent event streams.
var openAIStreams = []string{
	strings.Join([]string{": keep-alive", "event: ignored", `data: {"choices":[{"delta":{"content":"Hel"}}]}`, "", "id: 7", "data:" + `{"choices":[{"delta":{"content":"lo"}}]}`, "",
		`data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}`, "", "data: [DONE]", ""}, "\r\n"),
	"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}",
	"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n" +
		"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"read_file\",\"arguments\":\"th\\\":\\\"x\\\"}\"}}]}}]}\n\n" +
		"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"b\",\"function\":{\"name\":\"bash\",\"arguments\":\"{}\"}}]}}]}\n\n" +
		"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":true,\"function\":{\"name\":\"ba\"}},{\"index\":\"1\",\"function\":{\"name\":\"q\",\"arguments\":\"{\"}}]}}]}\n\n" +
		"data: {\"choices\":[{\"delta\":{\"reasoning\":\"r1\",\"reasoning_content\":\"r0\"}},5,{\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
	"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{}\"}}]}}]}\n\n",
	"data: {\"usage\":{\"prompt_tokens\":1}}\n\ndata: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}\n\n",
	"data: {\"error\":{\"message\":\"rate limited\"}}\n\n", "data: {\"error\":\"x\",\"choices\":[]}\n\n",
	"data: not json\n\n", "data: [1]\n\n", "data: [DONE]\n\n", "", ": only comments\n\n", "data: {\"a\":1}\ndata: \n\n",
	"data: {\"choices\":[{\"delta\":\n\"content\":\"split\"}}]}\n\n", "data: {\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":5}]}\n\n",
	"data:{\"choices\":[{\"delta\":{\"content\":\"no space\"}}]}\r\n\r\ndata: [DONE]\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"after\"}}]}\n\n",
	"data: {\"choices\":[{\"delta\":{\"content\":\"\\u00e9 <b>\"}}]}\n\n\n\n",
}

// TestParityOpenAIWire records the OpenAI-compatible client's wire format (testdata/parity/openai.json): BuildBody for
// requests of every shape, parseResponse on the answers above, readStream on the streams above (the turn and every event)
// and the bounds on what is read.
func TestParityOpenAIWire(t *testing.T) {
	var cases []parity.Case
	f := 0.7
	schema := &config.OutputSchema{Name: "out", Schema: map[string]any{"type": "object", "properties": map[string]any{"a": map[string]any{"type": "string"}}}, Strict: true}
	msgs := []Message{{Role: "system", Content: "be <brief> & kind"}, {Role: "user", Content: "hi é /no_think  "},
		{Role: "assistant", Content: "", ToolCalls: []WireToolCall{{ID: "c1", Type: "function", Function: FunctionCall{Name: "read_file", Arguments: `{"path":"<a>"}`}}}},
		{Role: "tool", Content: "result", ToolCallID: "c1", Name: "read_file"}}
	tools := []ToolSpec{{Name: "read_file", Description: "Reads <a> file & more", Parameters: map[string]any{"type": "object", "properties": map[string]any{"path": map[string]any{"type": "string"}}}},
		{Name: "noop", Description: "nothing"}}
	for i, r := range []Request{{}, {Messages: msgs, Tools: tools, Stream: true, MaxTokens: 512}, {Messages: msgs[:1], Schema: schema},
		{Messages: []Message{{Role: "user", Content: "a /no_think"}, {Role: "assistant", Content: "b"}}}, {Messages: msgs[:2], NoThink: true, Tools: tools[:1]},
		{Messages: []Message{{Role: "user", Content: "/no_think x"}}}} {
		for _, temp := range []float64{0, f, 1e-7, 2} {
			c := newTestClient("http://x")
			c.Config.Temperature = temp
			c.Config.Model = "qwen <3>"
			b, err := c.BuildBody(r)
			cases = append(cases, parity.Case{Fn: "body", In: []any{i, strconv.FormatUint(math.Float64bits(temp), 16)}, Out: []any{parity.Text(string(b)), parity.Err(err)}})
		}
	}
	for _, a := range openAIAnswers {
		turn, err := parseResponse([]byte(a))
		cases = append(cases, parity.Case{Fn: "response", In: parity.Text(a), Out: []any{turnFields(turn), errOut(err)}})
	}
	streamCase := func(fn string, s string, resp int64, line int) {
		oldR, oldL := maxResponseBytes, maxStreamLine
		maxResponseBytes, maxStreamLine = resp, line
		defer func() { maxResponseBytes, maxStreamLine = oldR, oldL }()
		touches := 0
		var evs []any
		turn, err := readStream(strings.NewReader(s), func() { touches++ }, func(e StreamEvent) {
			var u any
			if e.Usage != nil {
				u = []int{e.Usage.InputTokens, e.Usage.OutputTokens, e.Usage.ReasoningTokens}
			}
			evs = append(evs, []any{e.Type, parity.Text(e.Delta), e.ToolCallIndex, e.ToolCallID, e.ToolName, parity.Text(e.ArgumentsDelta), e.FinishReason, u})
		})
		cases = append(cases, parity.Case{Fn: fn, In: []any{parity.Text(s), resp, line}, Out: []any{turnFields(turn), errOut(err), evs, touches}})
	}
	for _, s := range openAIStreams {
		streamCase("stream", s, 256<<20, 16<<20)
	}
	ev := "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n"
	for _, b := range []int64{int64(len(ev)*3 - 1), int64(len(ev) * 3), int64(len(ev)*3 + 1)} {
		streamCase("streamBound", strings.Repeat(ev, 3), b, 1<<20)
	}
	for _, l := range []int{len(ev) - 2, len(ev) - 1, len(ev)} {
		streamCase("streamBound", ev+ev, 1<<20, l)
	}
	_ = fmt.Sprint
	parity.Fixture(t, "openai", cases)
}
