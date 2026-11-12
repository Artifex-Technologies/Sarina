package llm

import (
	"math/rand"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// repairSeeds are model outputs the repair and the normaliser see: tool calls in every shape local servers write them,
// with the damage small models do.
var repairSeeds = []string{
	`{"name":"read_file","arguments":{"path":"a.txt"}}`, `{"name": "bash", "arguments": "{\"command\": \"ls\"}"}`,
	`{"function":{"name":"bash","arguments":{"command":"ls -la"}}}`, `{"function":{"name":"bash","parameters":{"command":"pwd"}}}`,
	`[{"name":"read_file","arguments":{"path":"a"}},{"name":"bash","arguments":{"command":"x"}}]`, `[{"name":"read_file"},"x"]`,
	`{"name":"bash","parameters":{"command":"echo 'hi'"}}`, `{"name":"bash","arguments":5}`, `{"name":"bash","arguments":null,"parameters":{"a":1}}`,
	`{'name': 'bash', 'arguments': {'command': 'echo "x"'}}`, `{"a": 1,}`, `[1, 2, ]`, `{"a": "line one
line two"}`, "{\"a\": \"tab\there\"}", `{"a": "cut`, `{"a": [1, 2`, `{"a": {"b":`, `{"a": {"b": 1,`, `{"a"`, `{`, `[`, `'x`,
	"```json\n{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}\n```", "```\n[1]\n```", "```{\"a\":1}```", "```", "``` ",
	`{"a": 'it\'s'}`, `{"a": 'say "hi"'}`, `{"a": 'back\\slash'}`, `{"a": "esc\"aped"}`, `{"a": 1} trailing words`, `{"a": 1}{"b": 2}`,
	`{"a": 1e400}`, `{"a": tru`, `{"a": [}`, `{"a": ]}`, `}{`, `][`, `{"a": 1]}`, `[{"a": 1}}`, `{"a":"é"}`, `{"a":"\ud83d"}`,
	`null`, `true`, `"just a string"`, `42`, ``, `   `, "\n\t", `{"name":"unknown_tool","arguments":{}}`, `{"name":"","arguments":{}}`,
	`{"name":"bash","function":{"name":"read_file","arguments":{"path":"p"}}}`, `{"function":"bash","name":"bash","arguments":{"c":1}}`,
	`{"name":"bash","arguments":"{\"command\": \"ls\""}`, `{"name":"bash","arguments":"not json"}`, `{"name":"bash","arguments":"[1]"}`,
	`{"function":{"name":"bash"},"arguments":{"command":"ls"}}`, `{"function":{"name":"bash"},"parameters":{"c":1}}`,
	`{"function":{"name":"bash","arguments":null},"arguments":{"x":1}}`, `{"function":{"name":"bash","parameters":null},"parameters":"{\"y\":2}"}`,
	`{"name":"bash","arguments":""}`, `{"name":"bash","arguments":"  "}`, `{"name":"bash","arguments":[1]}`, `{"name":"bash","arguments":"{'a': 'b',}"}`,
	"{\"a\": \"\r\n\"}", `{"a": "x\`, `{"a": 'x\`, `{"a": 'x`, `['a', 'b'`, `{"a": 1, "b": [1, {"c": "d`, "{\"a\":\n", `{"a":  `,
}

// TestParityRepair records RepairJSON, ParseArguments and Normalize (testdata/parity/repair.json): the Go tests' inputs, the
// seeds above, every cut of each seed, and generated damage.
func TestParityRepair(t *testing.T) {
	texts := parity.StringLiterals(t, "llm_test.go")
	texts = append(texts, repairSeeds...)
	// every cut at a character boundary (what a model sends is JSON-decoded text, always UTF-8)
	for _, s := range repairSeeds {
		for i := 1; i < len(s); i++ {
			if utf8.ValidString(s[:i]) {
				texts = append(texts, s[:i])
			}
		}
	}
	rng := rand.New(rand.NewSource(20261112))
	pool := []string{"{", "}", "[", "]", ",", ":", `"`, "'", "\\", "\n", " ", "a", "1", "null", `"k"`, "'v'", "```", "\t"}
	for i := 0; i < 600; i++ {
		var b strings.Builder
		for j := rng.Intn(14); j >= 0; j-- {
			b.WriteString(pool[rng.Intn(len(pool))])
		}
		texts = append(texts, b.String())
	}
	var cases []parity.Case
	seen := map[string]bool{}
	for _, s := range texts {
		if seen[s] || !utf8.ValidString(s) {
			continue
		}
		seen[s] = true
		v, trunc, ok := RepairJSON(s)
		args, atrunc, aerr := ParseArguments(s)
		var a any
		if args != nil {
			a = parity.Typed(args)
		}
		cases = append(cases, parity.Case{Fn: "repair", In: parity.Text(s), Out: []any{parity.Typed(v), trunc, ok, a, atrunc, aerr}})
	}
	// Normalize: the content as a model turn, with and without tool names, finish reasons, calls already there
	names := []string{"bash", "read_file"}
	contents := append([]string{}, texts[:0]...)
	for _, s := range repairSeeds {
		contents = append(contents, s, "<tool_call>"+s+"</tool_call>", "Let me look.\n<tool_call>"+s+"</tool_call>\nDone.",
			"<tool_call>"+s, "<|python_tag|>"+s+"<|eom_id|>", "text <|python_tag|> "+s+" <|eot_id|>", "<think>plan</think>"+s)
	}
	contents = append(contents, "<think>a</think>b", "<think>a", "a</think>b", "</think>x<think>y</think>z", "x <think> y </think> <think>z</think>w",
		"no tags", "think>", "<think></think>", "  <think> r </think>  ", "<tool_call>{\"name\":\"bash\",\"arguments\":{}}</tool_call><tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"x\"}}</tool_call>",
		"<tool_call>{\"name\":\"nope\"}</tool_call><tool_call>{\"name\":\"bash\"}</tool_call>", "<tool_call></tool_call>", "<tool_call><tool_call>{\"name\":\"bash\"}</tool_call>",
		"<|python_tag|>", "<|python_tag|>{\"name\":\"bash\",\"parameters\":{\"command\":\"x\"}}",
		"<|python_tag|>{\"name\":\"bash\" <|eot_id|>\n", "<|python_tag|>{\"name\":\"bash\"<|eom_id|> ", "<|python_tag|>{\"name\":\"bash\"<|eom_id|><|eot_id|>",
		"<|python_tag|>{\"name\":\"bash\"<|eot_id|><|eom_id|>", "<tool_call>{\"name\":\"bash\"}</tool_call><|python_tag|>{\"name\":\"read_file\"}")
	for _, s := range contents {
		for _, finish := range []string{"stop", "length"} {
			for _, tn := range [][]string{nil, names} {
				turn := &Turn{Content: s, Reasoning: "earlier", FinishReason: finish}
				Normalize(turn, tn)
				cases = append(cases, parity.Case{Fn: "normalize", In: []any{parity.Text(s), finish, tn != nil}, Out: turnOut(turn)})
			}
		}
	}
	// calls the server sent: ids made unique, cut-off arguments refused on a length stop
	for _, finish := range []string{"length", "max_tokens", "stop", ""} {
		turn := &Turn{Content: "<think>x</think>", FinishReason: finish, ToolCalls: []ToolCall{
			{ID: "", Name: "a"}, {ID: "call_1", Name: "b", truncated: true, Arguments: map[string]any{"x": 1.0}}, {ID: "call_1", Name: "c"},
			{ID: "dup", Name: "d", truncated: true, ParseError: "already"}, {ID: "dup", Name: "e"}, {ID: "call_4", Name: "f"}, {ID: "", Name: "g"},
			{ID: "call_6_1", Name: "h"}, {ID: "call_6", Name: "i"}, {ID: "", Name: "j"}}}
		Normalize(turn, names)
		cases = append(cases, parity.Case{Fn: "ids", In: finish, Out: turnOut(turn)})
	}
	// an id that a later rename would take: call_1 then an empty one at index 1 (call_1_1), and on
	turn := &Turn{ToolCalls: []ToolCall{{ID: "call_1", Name: "a"}, {ID: "", Name: "b"}, {ID: "call_1_1", Name: "c"}, {ID: "call_1", Name: "d"},
		{ID: "call_3", Name: "e"}, {ID: "call_3_1", Name: "f"}, {ID: "", Name: "g"}}}
	Normalize(turn, nil)
	cases = append(cases, parity.Case{Fn: "ids2", Out: turnOut(turn)})
	parity.Fixture(t, "repair", cases)
}

func turnOut(t *Turn) any {
	var calls []any
	for _, c := range t.ToolCalls {
		var a any
		if c.Arguments != nil {
			a = parity.Typed(c.Arguments)
		}
		calls = append(calls, []any{c.ID, c.Name, a, parity.Text(c.RawArguments), c.ParseError, c.truncated})
	}
	return map[string]any{"content": parity.Text(t.Content), "reasoning": parity.Text(t.Reasoning), "calls": calls}
}
