package runkit

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// TestParityRunkit records what the service and the GUI send out (testdata/parity/runkit.json): Scrubber over texts
// with and without literals, Transcript over a session's messages (tool results with and without an ok, metadata,
// calls) through a scrubber, and Result for run results (every stop reason, an empty one). The messages come from a
// session file's JSON, which the Rust test parses the same way.
func TestParityRunkit(t *testing.T) {
	texts := []string{
		"", "plain text", "Authorization: Bearer abcdefghijklmnop.qrs", "bearer short", "BEARER abcdefghijkl12345",
		"api_key=sk-1234567890abcdef", "API-KEY: 'supersecretvalue'", "password = \"hunter2hunter2\"", "token:abc",
		"my_secret_token=xyzxyzxyz rest", "passwd=12345 and passwd=1234567", "AKIAIOSFODNN7EXAMPLE in text",
		"-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----", "the literal sk-LITERAL-123 appears",
		"ghp_abcdefghijklmnopqrstuvwxyz0123456789", "xoxb-1234-5678-abcdefghij", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abc123def456",
		"secretive=no", "a key=value pair", "tokenizer=bpe settings", "é unicode token=ünïcödé123",
	}
	literals := [][]string{nil, {"sk-LITERAL-123"}, {"short"}, {"plain text", "hunter2hunter2"}, {""}}
	var cases []parity.Case
	for _, lits := range literals {
		scrub := Scrubber(lits...)
		var out []any
		for _, s := range texts {
			out = append(out, scrub(s))
		}
		cases = append(cases, parity.Case{Fn: "scrubber", In: map[string]any{"literals": lits, "texts": texts}, Out: out})
	}

	sess := `{"session_id":"s","model_config":{"model":"m"},"runtime_config":{"cwd":"/w"},"system_prompt_parts":[],"user_context":{},` +
		`"system_context":{},"turns":0,"tool_calls":0,"messages":[` +
		`{"role":"system","content":"sys"},` +
		`{"role":"user","content":"hello token=abcdefgh","message_id":"u1"},` +
		`{"role":"assistant","content":"","tool_calls":[{"id":"c1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a\",\"api_key\":\"sk-12345678\"}"}}],"message_id":"a1"},` +
		`{"role":"tool","name":"read_file","tool_call_id":"c1","content":"{\"ok\":true,\"content\":\"body\"}","metadata":{"path":"a","nested":{"password":"pw=secretsecret"},"list":["bearer abcdefghijklmn",1,true]}},` +
		`{"role":"tool","name":"bash","tool_call_id":"c2","content":"{\"ok\":false,\"content\":\"no\"}"},` +
		`{"role":"tool","name":"x","tool_call_id":"c3","content":"not json"},` +
		`{"role":"tool","name":"y","tool_call_id":"c4","content":"not json","metadata":{"ok":true}},` +
		`{"role":"tool","name":"z","tool_call_id":"c5","content":"{\"content\":\"no ok field\"}","metadata":{"ok":"yes"}},` +
		`{"role":"assistant","content":"done, password: hunter2hunter2","metadata":{"empty":{}}}]}`
	s, err := session.Parse([]byte(sess))
	if err != nil {
		t.Fatal(err)
	}
	for _, lits := range [][]string{nil, {"hunter2hunter2"}} {
		var scrub func(string) string
		if lits != nil {
			scrub = Scrubber(lits...)
		}
		b, _ := json.Marshal(Transcript(s.Messages, scrub))
		cases = append(cases, parity.Case{Fn: "transcript", In: map[string]any{"session": sess, "literals": lits}, Out: parity.Text(string(b))})
	}
	for _, stop := range []string{"", "stop", agent.StopMaxTurns, agent.StopInterrupted, agent.StopPlanPending} {
		res := &agent.Result{FinalOutput: "the answer, api_key=sk-1234567890ab", Turns: 2, ToolCalls: 3, Transcript: s.Messages[1:3],
			Usage: llm.Usage{InputTokens: 10, OutputTokens: 5, CacheReadInputTokens: 2, ReasoningTokens: 1}, TotalCostUSD: 0.125,
			StopReason: stop, SessionID: "sid"}
		for _, lits := range [][]string{nil, {"the answer"}} {
			var scrub func(string) string
			if lits != nil {
				scrub = Scrubber(lits...)
			}
			b, _ := json.Marshal(Result(res, scrub))
			cases = append(cases, parity.Case{Fn: "result", In: map[string]any{"session": sess, "stop": stop, "literals": lits},
				Out: parity.Text(strings.TrimSpace(string(b)))})
		}
	}
	parity.Fixture(t, "runkit", cases)
}
