package llm

import (
	"encoding/json"
	"math/rand"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// typesDocs are values a provider's "usage", a model's tool arguments or a tool's parameters may hold.
var typesDocs = []string{`null`, `true`, `1`, `"x"`, `[]`, `{}`, `{"input_tokens":3,"output_tokens":4}`,
	`{"prompt_tokens":"5","completion_tokens":6.9,"prompt_eval_count":7,"eval_count":8}`,
	`{"input_tokens":0,"prompt_tokens":0,"prompt_eval_count":9}`, `{"input_tokens":true,"prompt_tokens":2}`,
	`{"output_tokens":"x","completion_tokens":" 12 ","eval_count":1}`, `{"cache_creation_input_tokens":1,"cache_read_input_tokens":2,"reasoning_tokens":3}`,
	`{"completion_tokens_details":{"reasoning_tokens":4},"prompt_tokens_details":{"cached_tokens":"5"}}`,
	`{"reasoning_tokens":1,"completion_tokens_details":{"reasoning_tokens":4},"cache_read_input_tokens":2,"prompt_tokens_details":{"cached_tokens":5}}`,
	`{"completion_tokens_details":[],"prompt_tokens_details":"x"}`, `{"input_tokens":1e300,"output_tokens":-1e300}`,
	`{"input_tokens":9223372036854775807,"output_tokens":9223372036854775807}`, `{"input_tokens":"NaN","output_tokens":"1e400"}`,
	`{"input_tokens":1e400}`, `{"total_tokens":99}`, `{"input_tokens":-3,"output_tokens":3}`, `{"input_tokens":2.5,"reasoning_tokens":true}`,
	`{"path":"a<b>&c","n":1e21,"m":0.000001,"x":[1,"é",null,{"z":"\u2028😀"}]}`, `{"":"","k":"\u0000\u001f"}`,
	`{"type":"object","properties":{"path":{"type":"string","description":"x <y>"}},"required":["path"]}`,
	`{"a":{"b":{"c":[[[]]]}}}`, `{"a":1,"a":2}`, `{"A":"\ud800"}`, `{"f":-0.0,"g":1e-7,"h":123456789.125}`, `[1,2]`, `{`}

func genArgs(rng *rand.Rand, depth int) string {
	switch n := rng.Intn(7); {
	case n == 0 && depth < 3:
		var parts []string
		for i := rng.Intn(4); i > 0; i-- {
			parts = append(parts, genArgs(rng, depth+1))
		}
		return "[" + strings.Join(parts, ",") + "]"
	case n == 1 && depth < 3:
		var parts []string
		for i := rng.Intn(4); i > 0; i-- {
			parts = append(parts, `"`+[]string{"input_tokens", "prompt_tokens", "eval_count", "a", "é", "<k>", "reasoning_tokens", "cached_tokens"}[rng.Intn(8)]+`":`+genArgs(rng, depth+1))
		}
		return "{" + strings.Join(parts, ",") + "}"
	case n < 5:
		return []string{`0`, `2`, `-3.5`, `1e21`, `7`, `"8"`, `"x"`, `true`, `null`, `1e400`, `0.1`}[rng.Intn(11)]
	}
	return `"` + []string{"a", "é😀", "<&>", `\n`, "\u2028", " "}[rng.Intn(6)] + `"`
}

// TestParityLlmTypes records go/llm's types (testdata/parity/llmtypes.json): UsageFromProvider and Usage's JSON (both
// ways), a tool call's arguments as replayed, a tool definition's JSON, a wire message's JSON.
func TestParityLlmTypes(t *testing.T) {
	docs := append([]string(nil), typesDocs...)
	rng := rand.New(rand.NewSource(1113))
	for i := 0; i < 300; i++ {
		d := genArgs(rng, 0)
		if i%2 == 0 {
			d = "{" + `"input_tokens":` + genArgs(rng, 1) + `,"x":` + d + "}"
		}
		docs = append(docs, d)
	}
	var cases []parity.Case
	for _, d := range docs {
		var v any
		_ = json.Unmarshal([]byte(d), &v)
		u := UsageFromProvider(v)
		fromProvider, _ := json.Marshal(u)
		var back Usage
		_ = back.UnmarshalJSON([]byte(d))
		unmarshalled, _ := json.Marshal(back)
		out := map[string]any{"provider": string(fromProvider), "unmarshal": string(unmarshalled), "zero": u.IsZero(),
			"sum": u.Add(back).Total()}
		args, _ := v.(map[string]any)
		out["args"] = parity.Text(ToolCall{Arguments: args}.ArgumentsJSON())
		spec, err := json.Marshal(ToolSpec{Name: "n<", Description: "d & é", Parameters: args})
		out["spec"] = parity.Text(string(spec))
		if err != nil {
			out["spec"] = err.Error()
		}
		cases = append(cases, parity.Case{Fn: "doc", In: parity.Text(d), Out: out})
	}
	msgs := []Message{{}, {Role: "user", Content: "hi <b> é 😀"}, {Role: "tool", Content: "x", Name: "n", ToolCallID: "c"},
		{Role: "assistant", ToolCalls: []WireToolCall{{ID: "a", Type: "function", Function: FunctionCall{Name: "f", Arguments: `{"x":"<"}`}}}},
		{Role: "assistant", ToolCalls: []WireToolCall{}}}
	for _, m := range msgs {
		b, _ := json.Marshal(m)
		calls := []any{}
		for _, c := range m.ToolCalls {
			calls = append(calls, []string{c.ID, c.Type, c.Function.Name, c.Function.Arguments})
		}
		cases = append(cases, parity.Case{Fn: "Message", In: []any{m.Role, m.Content, m.Name, m.ToolCallID, calls}, Out: string(b)})
	}
	for _, c := range []ToolCall{{ID: "1", Name: "a"}, {ID: "2", Name: "b<", Arguments: map[string]any{"k": "é"}}} {
		b, _ := json.Marshal(c.Wire())
		cases = append(cases, parity.Case{Fn: "Wire", In: []any{c.ID, c.Name, parity.Typed(c.Arguments)}, Out: string(b)})
	}
	parity.Fixture(t, "llmtypes", cases)
}
