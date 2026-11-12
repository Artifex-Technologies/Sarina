package session

import (
	"encoding/json"
	"fmt"
	"hash/fnv"
	"math/rand"
	"sort"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// paritySession is a made-up session with every part the file has: contexts, each kind of message, retired tool calls and
// their results, usage, file history, budget, plan state.
func paritySession() *Session {
	env := map[string]string{"OPENAI_MODEL": "made-up-model", "SARINA_FALLBACK_MODELS": "b, c", "SARINA_TEMPERATURE": "0.6"}
	m := config.ModelFromEnv(func(k string) string { return env[k] })
	m.Pricing = config.Pricing{InputPerMillion: 0.5, OutputPerMillion: 1.25}
	rt := config.DefaultRuntime("/nonexistent-sarina/ws")
	five := 5
	rt.AutoCompactThresholdTokens = &five
	rt.ContextManagement = &config.ContextConfig{NotesEveryTurns: 3, DisableClearing: true}
	s := &Session{ID: "0123456789abcdef0123456789abcdef", Model: m, Runtime: rt, SystemPromptParts: []string{"part one", "two <&>"},
		ScratchpadDirectory: "/nonexistent-sarina/ws/.port_sessions/scratchpad/x"}
	s.UserContext.Set("currentDate", "Today is caf\u00e9 day.")
	s.UserContext.Set("alpha", "1")
	s.SystemContext.Set("zeta", "last")
	s.Append(Message{Role: "system", Content: "sys", MessageID: "system_0"})
	s.Append(Message{Role: "user", Content: "<system-reminder>ctx</system-reminder>", MessageID: "user_1",
		Metadata: map[string]any{"kind": "file_history_replay"}})
	s.Append(Message{Role: "user", Content: "h\u00e9llo <b> & \U0001F600 \u2028", MessageID: "user_2"})
	s.Append(Message{Role: "assistant", Content: "calling", MessageID: "assistant_3", Usage: llm.Usage{InputTokens: 3, OutputTokens: 2},
		StopReason: "tool_calls", ToolCalls: []llm.WireToolCall{
			{ID: "c1", Type: "function", Function: llm.FunctionCall{Name: "read_file", Arguments: `{"path":"a"}`}},
			{ID: "c2", Type: "function", Function: llm.FunctionCall{Name: "task_start", Arguments: `{"id":3,"owner":"me","x":1}`}},
			{ID: "c3", Type: "function", Function: llm.FunctionCall{Name: "plan_clear", Arguments: `{"sync_tasks":true}`}},
			{ID: "c4", Type: "function", Function: llm.FunctionCall{Name: "task_block", Arguments: `{"task_id":"a","reason":"r","blocked_by":["b"]}`}},
			{ID: "c5", Type: "function", Function: llm.FunctionCall{Name: "task_next", Arguments: ``}},
			{ID: "c6", Type: "function", Function: llm.FunctionCall{Name: "task_cancel", Arguments: `{`}},
		}})
	s.Append(Message{Role: "tool", Name: "read_file", ToolCallID: "c1", MessageID: "tool_4", Content: "result",
		Metadata: map[string]any{"path": "a", "phase": "completed", "n": 3}})
	s.Append(Message{Role: "tool", Name: "task_start", ToolCallID: "c2", MessageID: "tool_5",
		Content:  ToolResultBody("task_start", true, "started", map[string]any{"action": "task_start"}),
		Metadata: map[string]any{"tool_name": "task_start", "action": "task_start"}})
	s.Append(Message{Role: "tool", Name: "plan_clear", ToolCallID: "c3", MessageID: "tool_6", Content: `{"tool": "plan_clear", "ok": true}`})
	s.Append(Message{Role: "tool", Name: "task_cancel", ToolCallID: "c6", MessageID: "tool_7", Content: "x"})
	s.Append(Message{Role: "assistant", Content: "done", MessageID: "assistant_8", State: StateStreaming, StopReason: "stop"})
	s.Messages[len(s.Messages)-1].Mutate("snip")
	s.Messages[len(s.Messages)-1].Mutate("snip")
	s.Turns, s.ToolCalls = 2, 6
	s.Usage = llm.Usage{InputTokens: 10, OutputTokens: 4, ReasoningTokens: 1, CacheReadInputTokens: 2}
	s.TotalCostUSD = 0.25
	s.FileHistory = []map[string]any{{"tool_name": "read_file", "path": "a"}}
	s.BudgetState = BudgetState{ModelCalls: 2, SessionTurns: 2, ToolCalls: 1}
	s.SetPlan(PlanState{Mode: PlanActive, Pending: "the plan"})
	return s
}

// parityValues are what a key of a hand-edited or damaged file may hold instead of the right type.
var parityValues = []string{`null`, `"x"`, `""`, `0`, `-1`, `1.5`, `-2.7`, `"7"`, `" 8 "`, `"1e3"`, `"0x10"`, `"NaN"`, `"inf"`,
	`"-Inf"`, `"1e400"`, `1e300`, `-1e300`, `9223372036854775807`, `9223372036854775808`, `"99999999999999999999"`, `true`,
	`false`, `[]`, `{}`, `["a",1]`, `{"a":1}`, `1e400`, `"1_000"`, `"+5"`, `"9007199254740993"`, `9007199254740993`}

// edit returns doc (a JSON object) with the value at path replaced by raw (or removed when raw is ""), written back by Go.
func edit(doc map[string]any, path []string, raw string) []byte {
	var cp map[string]any
	b, _ := json.Marshal(doc)
	_ = json.Unmarshal(b, &cp)
	m := cp
	for _, k := range path[:len(path)-1] {
		next, ok := m[k].(map[string]any)
		if !ok {
			next = map[string]any{}
			m[k] = next
		}
		m = next
	}
	last := path[len(path)-1]
	if raw == "" {
		delete(m, last)
	} else {
		m[last] = json.RawMessage(raw)
	}
	out, err := json.Marshal(cp)
	if err != nil {
		panic(err)
	}
	return out
}

// sessionOut is everything recorded for one file: Parse's error, or the file written back before and after the retired-tool
// migration, the migration's counts, the plan state, what RepairPairing makes of the history, each message's kind and its
// wire form.
func sessionOut(b []byte, full bool) map[string]any {
	// a long text is kept as its FNV-1a hash and length (the base file's answers are kept whole, to show a difference)
	text := func(s string) any {
		if full || len(s) < 1200 {
			return parity.Text(s)
		}
		return digest([]byte(s))
	}
	s, err := parse(b)
	if err != nil {
		return map[string]any{"err": err.Error()}
	}
	out := map[string]any{}
	marshal := func() any {
		data, err := s.Marshal()
		if err != nil {
			return map[string]any{"err": err.Error()}
		}
		return text(string(data))
	}
	out["file"] = marshal()
	mig := s.MigrateRetiredTools()
	out["migration"] = []int{mig.Calls, mig.Results}
	if mig.Changed() {
		out["migrated"] = marshal()
	}
	p := s.Plan()
	out["plan"] = []any{p.Mode, parity.Text(p.Pending), parity.Text(p.Note), p.Active()}
	repaired, _ := EncodeJSON(RepairPairing(s.Messages))
	out["repaired"] = text(string(repaired))
	// each message's kind, whether it is a reminder, and its wire form (json.Marshal of Wire()): whole for the base file,
	// else the hash of "kind NUL 0|1 NUL wire SOH" for each message in turn
	var msgs []any
	var joined []byte
	for _, m := range s.Messages {
		wire, _ := json.Marshal(m.Wire())
		msgs = append(msgs, []any{parity.Text(m.Kind()), m.IsReminder(), parity.Text(string(wire))})
		rem := "0"
		if m.IsReminder() {
			rem = "1"
		}
		joined = append(joined, m.Kind()+"\x00"+rem+"\x00"+string(wire)+"\x01"...)
	}
	if full {
		out["messages"] = msgs
	} else {
		out["messages"] = digest(joined)
	}
	return out
}

// digest is a long text as the fixture keeps it: its FNV-1a (64-bit) hash and its length.
func digest(b []byte) map[string]any {
	h := fnv.New64a()
	h.Write(b)
	return map[string]any{"fnv": fmt.Sprintf("%016x", h.Sum64()), "len": len(b)}
}

// TestParitySession records the session file (testdata/parity/session.json): made-up files, each key of each level
// removed or given every wrong type, damaged messages, tool calls and usage, the package's own test inputs; Redact,
// ToolResultBody, NotRunMessage, SetPlan, Mutate, FileMode.
func TestParitySession(t *testing.T) {
	base, err := paritySession().Marshal()
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(base, &doc); err != nil {
		t.Fatal(err)
	}
	var cases []parity.Case
	// a file is kept whole when short; an edit of the made-up file as the edit (the Rust side makes the same one, checked by
	// the hash); a damaged copy as the damage
	add := func(name string, b []byte) {
		cases = append(cases, parity.Case{Fn: "parse", In: map[string]any{"what": name, "file": parity.Text(string(b))},
			Out: sessionOut(b, false)})
	}
	addEdit := func(path []string, raw string) {
		b := edit(doc, path, raw)
		cases = append(cases, parity.Case{Fn: "parse", In: map[string]any{"edit": path, "raw": raw, "doc": digest(b)},
			Out: sessionOut(b, false)})
	}
	cases = append(cases, parity.Case{Fn: "parse", In: map[string]any{"what": "base", "file": parity.Text(string(base))},
		Out: sessionOut(base, true)})
	// every key at every level of the made-up file, removed or given each wrong value
	var paths [][]string
	var walk func(m map[string]any, prefix []string)
	walk = func(m map[string]any, prefix []string) {
		keys := make([]string, 0, len(m))
		for k := range m {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			p := append(append([]string(nil), prefix...), k)
			if len(prefix) > 0 && prefix[0] == "messages" {
				continue
			}
			paths = append(paths, p)
			if sub, ok := m[k].(map[string]any); ok && len(prefix) < 3 {
				walk(sub, p)
			}
		}
	}
	walk(doc, []string{})
	extra := [][]string{{"model_config", "api_key"}, {"runtime_config", "output_schema"}, {"runtime_config", "context_management"},
		{"usage", "prompt_tokens"}, {"usage", "completion_tokens_details"}, {"usage", "prompt_tokens_details"},
		{"runtime_config", "escalation_config", "resource_thresholds", "max_cpu_percent"}}
	paths = append(paths, extra...)
	for _, p := range paths {
		addEdit(p, "")
		for _, v := range parityValues {
			addEdit(p, v)
		}
	}
	// values that only make sense at one place
	special := map[string][]string{
		"runtime_config.cwd":                            {`"rel/dir"`, `"/"`, `"/a/../b/"`},
		"runtime_config.session_directory":              {`""`},
		"runtime_config.scratchpad_root":                {`""`},
		"runtime_config.output_schema":                  {`{"name":"n","schema":{"type":"object"},"strict":true}`, `{"name":"","schema":{}}`, `{"name":"n","schema":[]}`, `{"name":"n","schema":{"b":1,"a":[1e21,0.000001]},"strict":"yes"}`},
		"runtime_config.context_management":             {`{"disable_notes":true,"notes_every_turns":"4","clear_min_tokens":2.9}`, `{"disable_notes":false}`, `{"x":1}`, `{"notes_window_percent":-3}`},
		"runtime_config.additional_working_directories": {`["/a",2,null,"b"]`, `[]`},
		"user_context":                                  {`{"b":"1","a":2,"c":null,"d":[1,"x"],"e":{"z":true,"y":1.5},"b":"again","f":1e21,"g":1e400,"h":"after"}`, `{"k":0.1,"l":-0.0,"m":100000000000000000000}`, `[1]`, `"s"`},
		"system_context":                                {`{"\ud800":"\udc00x","<":"&"}`},
		"messages": {`[]`, `[1,"x",null,[],{}]`, `[{"role":"user","content":"hi"}]`,
			`[{"role":"assistant","content":"","tool_calls":[{"id":"a","function":{"name":"n","arguments":"{}"}},{"function":{}},"x",{"id":5,"function":{"name":7,"arguments":{"a":1}}}]},{"role":"tool","tool_call_id":"zz","content":"orphan"}]`,
			`[{"role":"user","content":"a","state":"tombstoned","stop_reason":"s","name":"n","message_id":"m","usage":{"prompt_tokens":"5","completion_tokens":2.9,"reasoning_tokens":true},"metadata":{"kind":"compact_boundary","x":[1,{"y":null}]}}]`,
			`[{"role":"user","content":"a","usage":{"input_tokens":0},"metadata":{}},{"role":"user","content":5,"state":""}]`,
			`[{"role":"assistant","content":"x","tool_calls":[{"id":"q","type":"other","function":{"name":"task_complete","arguments":"{\"task_id\":1}"}}]},{"role":"tool","name":"task_complete","tool_call_id":"q","content":"{\"tool\":\"task_complete\",\"ok\":true}","metadata":{"tool_name":"task_get","action":"task_next"}}]`,
			`[{"role":"assistant","tool_calls":[{"id":"r","function":{"name":"task_get","arguments":"null"}},{"id":"s","function":{"name":"task_get","arguments":"[]"}},{"id":"t","function":{"name":"task_get","arguments":" "}}]},{"role":"tool","name":"task_get","tool_call_id":"t","content":"{\n  \"tool\": \"task_get\""}]`,
			`[{"role":"assistant","tool_calls":[{"id":"p","function":{"name":"plan_clear","arguments":"{\"sync_tasks\":false}"}},{"id":"q","function":{"name":"plan_clear","arguments":"{\"sync_tasks\":\"yes\"}"}},{"id":"a","function":{"name":"x","arguments":"{}"}}]},{"role":"tool","tool_call_id":"a","content":"one"},{"role":"tool","tool_call_id":"a","content":"two"},{"role":"tool","tool_call_id":"p","name":"plan_clear","content":"{\"tool\": \"plan_clear\"}"}]`,
			`[{"role":"user","content":"\b\f\u0000\u001f\u007f\u0080\ud83d\ude00\ud800 <>&\u2028\u2029"}]`},
		"plugin_state": {`{"plan_mode":{"mode":"plan","pending":"p","note":"n"}}`, `{"plan_mode":{"mode":"normal","note":5}}`, `{"plan_mode":"x"}`, `{"z":1,"a":{"b":[]}}`},
		"file_history": {`[{"a":1},2,"x",{"b":{"c":1e-7}}]`},
		"usage":        {`{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":3,"cache_read_input_tokens":4,"reasoning_tokens":5,"total_tokens":99}`, `{"prompt_eval_count":7,"eval_count":8}`, `{"completion_tokens_details":{"reasoning_tokens":"3"},"prompt_tokens_details":{"cached_tokens":4}}`},
	}
	keys := make([]string, 0, len(special))
	for k := range special {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		for _, v := range special[k] {
			addEdit(strings.Split(k, "."), v)
		}
	}
	// whole documents that are not session files, and the package's own test inputs
	minimal := `"model_config":{"model":"m"},"runtime_config":{"cwd":"/w"},"system_prompt_parts":[],"user_context":{},"system_context":{},"messages":[],"turns":0,"tool_calls":0`
	add("doc duplicate keys", []byte(`{"session_id":"x",`+minimal+`,"session_id":"y","turns":1,"turns":"2"}`))
	add("doc minimal", []byte(`{"session_id":"x",`+minimal+`}`))
	for _, d := range []string{``, `null`, `[]`, `"x"`, `1`, `true`, `{}`, `{"session_id":"a"}`, `{`, `{"a":1} x`} {
		add("doc "+d, []byte(d))
	}
	for _, lit := range parity.StringLiterals(t, "session_test.go", "migrate_test.go") {
		if strings.HasPrefix(strings.TrimSpace(lit), "{") {
			add("literal", []byte(lit))
		}
	}
	// generated damage: one byte changed or the file cut, fixed seed
	rng := rand.New(rand.NewSource(1112))
	for i := 0; i < 150; i++ {
		b := append([]byte(nil), base...)
		at := rng.Intn(len(b))
		in := map[string]any{"cut": at}
		if rng.Intn(2) == 0 {
			b = b[:at]
		} else {
			b[at] = `"{}[],:0a\ `[rng.Intn(11)]
			in = map[string]any{"at": at, "byte": string(b[at])}
		}
		cases = append(cases, parity.Case{Fn: "parse", In: in, Out: sessionOut(b, false)})
	}

	// Redact on the package's own literals and made-up credentials
	redactIn := parity.StringLiterals(t, "session_test.go")
	redactIn = append(redactIn, "sk-ant-abcdefgh", "sk-ant-abcdefg", "sk-proj-AAAAAAAA_-x", "xsk-live-12345678", "sk_live_0123456789abcdef",
		"ask_live_0123456789abcdef", "ghp_01234567890123456789", "xghp_01234567890123456789", "AKIA0123456789ABCDEF",
		"xoxb-0123456789-abcdefghij", "AIza"+strings.Repeat("a", 35), "AIza"+strings.Repeat("a", 34),
		"eyJabcde.eyJabcde.abcde", "-----BEGIN PRIVATE KEY-----", "-----BEGIN RSA OPENSSH PRIVATE KEY-----",
		"-----BEGIN rsa PRIVATE KEY-----", "a sk-or-xxxxxxxx b ghp_aaaaaaaaaaaaaaaaaaaa", "caf\xe9 sk-test-12345678", "é_AKIA0123456789ABCDEF",
		"sk-ant-sk-ant-12345678", "pk_test_aaaaaaaaaaaaaaaa", "rk_live_AAAAAAAAAAAAAAA")
	for _, s := range redactIn {
		cases = append(cases, parity.Case{Fn: "Redact", In: parity.Text(s), Out: parity.Text(Redact(s))})
	}
	for _, c := range []struct {
		tool, content string
		ok            bool
		md            map[string]any
	}{{"read_file", "x", true, nil}, {"bash", "a\nb <&> \u00e9", false, map[string]any{"exit_code": 1, "z": "y", "f": 1.5}},
		{"t", "", true, map[string]any{}}, {"t", "bad", true, map[string]any{"c": make(chan int)}}} {
		md := parity.Typed(c.md)
		if _, bad := c.md["c"]; bad {
			md = "a value JSON cannot write" // the encoder's error makes ToolResultBody drop the metadata
		}
		cases = append(cases, parity.Case{Fn: "ToolResultBody", In: []any{c.tool, c.ok, c.content, md},
			Out: parity.Text(ToolResultBody(c.tool, c.ok, c.content, c.md))})
	}
	nr, _ := json.Marshal(NotRunMessage("id1", "bash", "denied <x>", "tool_9"))
	cases = append(cases, parity.Case{Fn: "NotRunMessage", In: []string{"id1", "bash", "denied <x>", "tool_9"}, Out: string(nr)})
	for _, p := range []PlanState{{}, {Mode: PlanActive}, {Mode: "other", Pending: "p"}, {Note: "n"}, {Mode: PlanNormal, Pending: "a", Note: "b"}} {
		for _, start := range []func() map[string]any{func() map[string]any { return nil },
			func() map[string]any { return map[string]any{"x": 1} }, func() map[string]any { return map[string]any{"plan_mode": "old"} }} {
			s := &Session{PluginState: start()}
			s.SetPlan(p)
			got, _ := json.Marshal(s.PluginState)
			cases = append(cases, parity.Case{Fn: "SetPlan", In: []any{p.Mode, p.Pending, p.Note, parity.Typed(start())}, Out: string(got)})
		}
	}
	for _, kinds := range [][]string{{"a"}, {"a", "a", "b"}, {"x", "y", "x", "x"}} {
		// each start made afresh: Mutate changes the map it is given
		for _, start := range []func() map[string]any{func() map[string]any { return nil },
			func() map[string]any {
				return map[string]any{"lineage_id": "L", "revision": 4.0, "mutation_totals": map[string]any{"a": 2.0}}
			},
			func() map[string]any { return map[string]any{"revision": "3", "mutation_totals": "bad"} },
			func() map[string]any { return map[string]any{"revision": true} }} {
			for _, id := range []string{"", "m_1"} {
				m := Message{Role: "assistant", MessageID: id, Metadata: start()}
				for _, k := range kinds {
					m.Mutate(k)
				}
				got, _ := json.Marshal(m.Metadata)
				cases = append(cases, parity.Case{Fn: "Mutate", In: []any{kinds, parity.Typed(start()), id}, Out: string(got)})
			}
		}
	}
	for _, v := range []string{"", "0600", "0640", "640", " 0644 ", "644", "0666", "0400", "x"} {
		cases = append(cases, parity.Case{Fn: "FileMode", In: v, Out: uint32(FileMode(func(string) string { return v }))})
	}
	parity.Fixture(t, "session", cases)
}
