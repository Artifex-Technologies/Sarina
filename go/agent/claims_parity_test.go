package agent

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// claimScen is one task as the answer checks see it: the question, this task's messages (all of them for inventedHistory,
// from msgStart for the rest), the file history, the tools offered, and whether files can be read and written.
type claimScen struct {
	name     string
	prompt   string
	msgs     []session.Message
	msgStart int
	hist     []map[string]any
	offered  []string
	canRead  bool
	canWrite bool
}

func pcall(id, name, args string) session.Message {
	return session.Message{Role: "assistant", ToolCalls: []llm.WireToolCall{{ID: id, Type: "function", Function: llm.FunctionCall{Name: name, Arguments: args}}}}
}

func ptool(name, id, content string, meta map[string]any) session.Message {
	return session.Message{Role: "tool", Name: name, ToolCallID: id, Content: content, Metadata: meta}
}

// claimScens are the Go tests' own tasks (claims_test.go, secrets_test.go, acceptance_test.go), plus a few more shapes.
func claimScens() []claimScen {
	all := []string{"read_file", "list_dir", "write_file", "edit_file", "check_code", "system_status", "crypto_identify", "crypto_classic",
		"crypto_hash", "stego_clean", "stego_inspect", "agent", "reverse_engineer", "limbo_validate", "glob_search", "grep_search", "bash"}
	cut := `{"tool":"read_file","ok":true,"content":"use std::fs;\n[... lines 214-604 of 690 omitted: read_file start_line/end_line, or grep_search ...]\n[defined in the omitted lines, first line only: read a definition's lines (start_line/end_line) before you say what it does or quote it]\n  515: pub fn protected(pkg: &str) -> bool {\n  520: fn protected_with(pkg: &str, os_list: &str) -> bool {\n  530: def other_one(x):\n[...]\nmod tests {}"}`
	list := `{"tool":"limbo_validate","ok":true,"content":"validation run r18aa5d10 on profile decompile: done, exit code 0\n--- stdout ---\n-- functions (address, size, name; first 400)\nINFO: Analyze all flags starting with sym. and entry0 (aa)\n0x00001030    1      6 sym.imp.puts\n0x00001040    1      6 sym.imp.strlen\n0x00001050    1     37 entry0\n0x00001080    4     34 sym.deregister_tm_clones\n0x000010f0    5     55 entry.fini0\n0x00001149    6     70 sym.check_code\n0x000011e8    1     13 sym._fini\n0x0000118f    5     88 main\n0x00001200    2     20 sym.validate_input\n"}`
	prefetch := "Already computed for you (crypto_identify on what the question contains; call the crypto tools for anything else):\n\n" +
		"Looked at a string in the question (64 characters):\nWhat the string looks like, most likely first:\n" +
		"1. a hex digest: SHA-256, SHA3-256, BLAKE2s-256 or a Keccak-256 (Ethereum) hash (fit 85/100). Evidence: 64 hex digits = 256 bits.\n" +
		"   -> a hash is one-way: it says nothing about its input; it can be compared against the hash of a candidate (crypto_hash with expected)\n" +
		"\nWhat does this hash say? e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
	report := "### Summary\n- Package lua51: severity LOW\n- HELD /x: confirm it (sudo heimdalld immunity confirm 5f5f5f5f5f5f)\n\nCommands to copy exactly, whole:\n- kvm-world-writable:\n  " +
		`printf 'KERNEL=="kvm"\n' | sudo tee /etc/udev/rules.d/65-kvm.rules && sudo usermod -aG kvm $USER && sudo udevadm control --reload`
	sys := session.Message{Role: "system", Content: "You are Sarina. Curated memory: the owner likes strong coffee."}
	return []claimScen{
		{name: "nothing", prompt: "p"},
		{name: "plain tool", prompt: "what does it say?", offered: all, canRead: true, canWrite: true,
			msgs: []session.Message{ptool("", "", "check_code: if (len == 6 && s[0] == 'L' && s[5] == '7') puts(\"access granted: labyrinth\")", nil)}},
		{name: "wrote todo", prompt: "Add an item to the todo list in todo.md", canRead: true, canWrite: true,
			hist: []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/todo.md", "changed_paths": []string{"/w/todo.md"}}},
			msgs: []session.Message{pcall("a", "task_create", `{}`), ptool("task_create", "a", "created", nil)}},
		{name: "planned only", prompt: "Add an item to the todo list in todo.md and check notes.txt and dir.md and link.txt and dangling.md",
			canRead: true, canWrite: true, msgs: []session.Message{pcall("a", "todo_write", `{}`), ptool("todo_write", "a", "ok", nil)},
			hist: []map[string]any{{"tool_name": "read_file", "ok": true, "path": "/w/todo.md"}, {"tool_name": "write_file", "ok": false, "path": "/w/notes.txt"}}},
		{name: "history kinds", prompt: "fix it", canWrite: true, hist: []map[string]any{
			{"tool_name": "stego_clean", "ok": true, "path": "agent.go", "copy": "agent.clean.go", "changed_paths": []string{"agent.go"}},
			{"tool_name": "write_file", "ok": true, "path": "/w/dir with space/my notes.txt"},
			{"tool_name": "vault_write", "ok": true, "path": "Notes/Plan.md"},
			{"tool_name": "write_file", "ok": true, "path": "/w/notes.txt.bak", "changed_paths": []string{"/w/also.txt"}},
			{"tool_name": "edit_file", "ok": true, "path": "/w/stack.go", "verify": "ok"},
			{"tool_name": "stego_clean", "ok": false, "path": "x.go", "copy": "x.clean.go"}}},
		{name: "build ok", prompt: "write stack.go", canWrite: true, hist: []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/stack.go", "verify": "ok"}}},
		{name: "tests ok", prompt: "write stack.go", canWrite: true, hist: []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/stack.go", "verify": "ok", "verify_tests": true}}},
		{name: "build failed", prompt: "Add Double to util.go", canWrite: true, hist: []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/util.go", "verify": "failed"}}},
		{name: "skipped", prompt: "write stack.go", canWrite: true, hist: []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/stack.go", "verify": "skipped"},
			{"tool_name": "write_file", "ok": true, "path": "/w/notes.txt"}}},
		{name: "failed then ok", prompt: "fix slug.py", canWrite: true, hist: []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/slug.py", "verify": "failed"},
			{"tool_name": "edit_file", "ok": true, "path": "/w/slug.py", "verify": "ok"}}},
		{name: "own test failed", prompt: "test rule11.py", canWrite: true, hist: []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/test_rule11.py", "verify": "failed", "verify_kind": "tests"}}},
		{name: "code test failed", prompt: "fix slug.py", canWrite: true, hist: []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/slug.py", "verify": "failed", "verify_kind": "tests"},
			{"tool_name": "edit_file", "ok": true, "path": "/w/README.md", "verify": "failed"}}},
		{name: "report", prompt: report, canRead: true},
		{name: "memory", prompt: "What did I have for breakfast this morning?", msgStart: 1,
			msgs: []session.Message{sys, {Role: "user", Content: "What did I have for breakfast this morning?"}}},
		{name: "false premise", prompt: "You told me yesterday that my processor was running at 300 degrees. Is that still true?", msgStart: 1,
			msgs: []session.Message{sys, {Role: "user", Content: "You told me yesterday that my processor was running at 300 degrees. Is that still true?"}}},
		{name: "earlier turn", prompt: "Any tips?", msgStart: 3, msgs: []session.Message{sys, {Role: "user", Content: "I'm learning to draw with charcoal."},
			{Role: "assistant", Content: "Nice."}, {Role: "user", Content: "Any tips?"}, {Role: "user", MessageID: "claim_check_2", Content: "Lisbon trams skiing"}}},
		{name: "recall", prompt: "Do you remember what I told you about my trip to Lisbon?",
			msgs: []session.Message{{Role: "user", Content: "Do you remember what I told you about my trip to Lisbon?"}}},
		{name: "said now", prompt: "Firefox plays no sound but Spotify does",
			msgs: []session.Message{{Role: "user", Content: "My trip to Lisbon was lovely, the trams were great. Firefox plays no sound but Spotify does."}}},
		{name: "quote marks", prompt: "which prefixes?",
			msgs: []session.Message{ptool("read_file", "", `{"tool":"read_file","ok":true,"content":"assert!(protected(\"yggdrasil-minos\"))"}`, nil)}},
		{name: "identifiers", prompt: "what does the report say? is 3.4.5 bad?",
			msgs: []session.Message{ptool("", "", "kernel 6.18.55-2-cachyos-lts has CVE-2026-72493 (9.9), fixed in 6.18.57. sha 5f5f5f5f5f5f5f5f5f5f line 42: port 8080 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08", nil)}},
		{name: "read notes", prompt: "Summarize notes.txt", canRead: true, msgs: []session.Message{
			pcall("r", "read_file", `{"path":"notes.txt"}`), ptool("read_file", "r", `{"tool":"read_file","ok":true,"content":"Team sync."}`, map[string]any{"ok": true})}},
		{name: "read failed", prompt: "what is in secret.txt?", canRead: true, offered: all, msgs: []session.Message{
			ptool("", "", `{"tool":"read_file","ok":false,"content":"cannot read secret.txt: permission denied"}`, map[string]any{"ok": false})}},
		{name: "ok then fail", prompt: "p", msgs: []session.Message{ptool("", "", `{"ok":true}`, map[string]any{"ok": true}), ptool("", "", `{"ok":false}`, map[string]any{"ok": false})}},
		{name: "fail without meta", prompt: "p", msgs: []session.Message{ptool("", "", `{"tool": "crash_detail", "ok": false, "content": "no such program"}`, nil)}},
		{name: "not run", prompt: "p", msgs: []session.Message{ptool("x", "", `{"tool":"x","ok":false,"content":"no","metadata":{"error_kind": "not_run"}}`, nil)}},
		{name: "duplicate meta", prompt: "p", msgs: []session.Message{ptool("x", "", `{"ok":false}`, map[string]any{"ok": false, "error_kind": "duplicate_call"})}},
		{name: "look around", prompt: "How do I install Spotify?", msgs: []session.Message{
			ptool("list_dir", "", `{"tool":"list_dir","ok":false,"content":"/usr/share/applications is outside the workspace"}`, map[string]any{"ok": false})}},
		{name: "config.env", prompt: "List the setting names in config.env and what each is for. Do not print any secret values.", canRead: true, offered: []string{"read_file"}},
		{name: "config.env refused", prompt: "List the setting names in config.env and what each is for. Do not print any secret values.", canRead: true, msgs: []session.Message{
			pcall("r", "read_file", `{"path":"./config.env"}`), ptool("read_file", "r", `{"tool":"read_file","ok":false,"content":"refused"}`, nil)}},
		{name: "made-up path", prompt: "greeting.txt has a typo. Fix only the typo and change nothing else in the file. Crash report: crashy.c line 6. Read docs/DOES_NOT_EXIST.md", canRead: true, canWrite: true, msgs: []session.Message{
			pcall("r", "read_file", `{"path":"/path/to/greeting.txt"}`),
			ptool("read_file", "r", `{"tool":"read_file","ok":false,"content":"cannot access /path/to/greeting.txt: lstat /path: no such file or directory"}`, nil),
			pcall("s", "read_file", `{"Path": 5, "file_path": "crashy.c"}`), pcall("t", "read_file", `{"FILE_PATH": "notes.txt"} trailing`)}},
		{name: "fix ask", prompt: "go test fails in this folder. Fix the bug in the code so the test passes. Do not change the test file. Say what was wrong.", canRead: true, canWrite: true,
			offered: []string{"read_file", "list_dir", "write_file", "edit_file", "check_code"},
			msgs: []session.Message{pcall("1", "list_dir", "{}"), ptool("", "", "file\tcalc.go\nfile\tcalc_test.go\nfile\tgo.mod", nil),
				pcall("2", "read_file", "{}"), ptool("", "", "package calc\n\n// Add returns the sum of a and b.\nfunc Add(a, b int) int {\n\treturn a - b\n}\n", nil)}},
		{name: "write refused", prompt: "write ../escape.txt with hello", canWrite: true, msgs: []session.Message{
			ptool("write_file", "", `{"tool": "write_file", "ok": false, "content": "../escape.txt is outside the workspace", "metadata": {"error_kind": "tool_execution_error"}}`, nil),
			ptool("edit_file", "", `{"tool": "edit_file", "ok": false, "content": "old_string not found in calc.go"}`, nil)}},
		{name: "write hidden", prompt: "Create locked/result.txt", canWrite: true, msgs: []session.Message{
			{Role: "user", Content: "<system-reminder>\n# scratchpad\nScratch space: /tmp/w/.port_sessions/scratchpad/5eb32d01efa94a66bc2c656d72f13fad\n</system-reminder>"},
			ptool("write_file", "", `{"tool":"write_file","ok":false,"content":"cannot write /w/locked/result.txt: open /w/locked/result.txt: permission denied"}`, map[string]any{"ok": false}),
			ptool("write_file", "", `{"tool":"write_file","ok":true,"content":"Wrote result.txt (4 characters)."}`, map[string]any{"ok": true})}},
		{name: "quoted code", prompt: "Which function decides it? Quote the line.", offered: all, msgs: []session.Message{pcall("r", "read_file", `{"path":"appmgr.rs"}`), ptool("", "r", cut, nil),
			ptool("", "", `{"tool":"read_file","ok":true,"content":"use std::fs;\n[... lines 214-604 of 690 omitted: read_file start_line/end_line, or grep_search ...]\n    fn protected() {\n        assert!(protected(\"yggdrasil-minos\") && !protected(\"firefox\"));\n    }"}`, nil)}},
		{name: "read its lines", prompt: "Which function? Quote the line.", msgs: []session.Message{pcall("r", "read_file", `{"path":"appmgr.rs"}`), ptool("", "r", cut, nil),
			pcall("s", "read_file", `{"path":"appmgr.rs","start_line":510,"end_line":540}`), pcall("u", "read_file", `{"path":"appmgr.rs","start_line":528}`),
			pcall("v", "read_file", `{"path":"appmgr.rs","start_line":1.5}`), ptool("", "", "520: fn protected_with...", nil)}},
		{name: "wrote code", prompt: "Which function? Quote the line.", msgs: []session.Message{pcall("e", "edit_file", "{}"), ptool("", "", cut, nil)}},
		{name: "decompile list", prompt: "explain demo.bin", offered: all, msgs: []session.Message{
			pcall("climbo_validate", "limbo_validate", `{"command":"limbo-decompile functions demo.bin","profile":"decompile","timeout_seconds":900}`),
			ptool("limbo_validate", "climbo_validate", list, nil),
			pcall("c2", "limbo_validate", `{"command":"limbo-decompile decompile demo.bin validate_input main","profile":"default","timeout_seconds":300}`),
			ptool("limbo_validate", "c2", `{"tool":"limbo_validate","ok":false,"content":"profile_unknown: there is no profile named \"default\"."}`, nil)}},
		{name: "decompile done", prompt: "explain demo.bin", msgs: []session.Message{
			pcall("c1", "limbo_validate", `{"command":"limbo-decompile functions demo.bin","profile":"decompile"}`), ptool("", "c1", list, nil),
			pcall("c3", "limbo_validate", `{"command":"limbo-decompile decompile demo.bin check_code main","profile":"decompile"}`),
			ptool("limbo_validate", "c3", `{"tool":"limbo_validate","ok":true,"content":"-- decompiled C (Ghidra)\n=== check_code ===\n"}`, nil)}},
		{name: "reverse list", prompt: "explain demo.bin", offered: all, msgs: []session.Message{
			pcall("creverse_engineer", "reverse_engineer", `{"command":"analyze","path":"demo.bin"}`),
			ptool("reverse_engineer", "creverse_engineer", "## subject.name\n\ndemo.bin\n\n## functions (the program's own)\n\ncheck_code (1149), main (118f), sym.load_profile (1200), _start (1000)\n\n0000000000001149 T verify_pin\n\nNext: decompile the function", nil)}},
		{name: "reverse decompiled", prompt: "explain demo.bin", msgs: []session.Message{
			pcall("c1", "reverse_engineer", `{"command":"analyze","path":"demo.bin"}`),
			ptool("reverse_engineer", "c1", "## functions (the program's own)\n\ncheck_code (1149), main (118f)\n", nil),
			pcall("c2", "reverse_engineer", `{"command": "decompile", "path":"demo.bin","target":"sym.check_code"}`),
			ptool("reverse_engineer", "c2", "undefined8 check_code(char *param_1)", nil)}},
		{name: "jwt", prompt: "what is in it?", msgs: []session.Message{
			ptool("", "", "JWT (JSON Web Token)\n  Header: alg: \"HS256\", typ: \"JWT\"\n  Payload: iat: 1516239022, name: \"John Doe\", sub: \"1234567890\"\n", nil)}},
		{name: "quotes that claim nothing", prompt: "what is it?", msgs: []session.Message{
			pcall("g", "glob", `{"pattern":"limbo*"}`),
			ptool("", "g", "Hidden text: PWNED\nsocket: /run/limbo/agent.sock\nSIGSEGV in parse_config: return line[0];\n{\n  \"sub\": \"1234567890\"\n}\n3452 × 11 = 37972\n// stack_test.go\nt.Run(\"Pop from empty\", func(t *testing.T) { var s Stack; s.Push(1); if v, ok := s.Pop(); ok {\n1. a shifted alphabet (Caesar/ROT), shift 13\n   -> The station is closed until Monday\nDefault apps: PDF document. LOG_LEVEL=info\nAgora: no match for spotify\ncrash: parse_config line 6", nil)}},
		{name: "prefetch digest", prompt: prefetch},
		{name: "identify call", prompt: "What does this hash say? e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", msgs: []session.Message{
			ptool("crypto_identify", "", `{"tool":"crypto_identify","ok":true,"content":"What the string looks like, most likely first:\n1. a hex digest: SHA-256, SHA3-256, BLAKE2s-256 or a Keccak-256 (Ethereum) hash (fit 85/100)."}`, nil)}},
		{name: "offered tools", prompt: "find the marks", offered: []string{"stego_inspect", "stego_clean", "crypto_identify", "crypto_classic", "agent", "read_file", "crypto_hash"}},
		{name: "called tool", prompt: "clean agent.go", offered: []string{"stego_clean", "stego_inspect"}, msgs: []session.Message{pcall("s", "stego_clean", "{}")}},
		{name: "refused tool", prompt: "back up my keys", offered: []string{"stego_clean", "agent", "crypto_identify"}, msgs: []session.Message{
			ptool("", "", `{"tool": "write_file", "ok": false, "content": "refused: the content would take keys off this computer"}`, nil)}},
		{name: "secrets", prompt: "what is in the file?", msgs: []session.Message{
			ptool("", "", "SERVICE_URL=http://127.0.0.1:9000\npassword=hunter2-correct-horse-9\nexport API_KEY=\"abcd1234efgh\"\nLOG_LEVEL=info\nDB_PASSWORD: s3cr3t!x\nTOKEN=$HOME\nauth=true\nsecret=[REDACTED:x]\n", nil),
			ptool("", "", `{"tool":"read_file","ok":true,"content":"user = ada\npassword = \"hunter2-correct-horse\"\n","more":{"results":[{"text":"api_token: abcd1234efgh"},{"text":"pass: wxyz"}]}}`, nil),
			ptool("", "", `{"a":"secret=abcd","b":"token=bcde","c":"key=zzzz"}`, nil)}},
		{name: "edit loop", prompt: "fix calc.go", canWrite: true, msgs: []session.Message{ptool("edit_file", "", "refusing attempt 4 at calc.go", nil)},
			hist: []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/calc.go", "verify": "failed"}}},
		{name: "edges", prompt: "Quote the line.", offered: all, msgs: []session.Message{
			pcall("r1", "read_file", `{"path":"x.rs"}`),
			ptool("read_file", "r1", "[defined in the omitted lines, first line only]\n  540: fn foo(x) {\n  600: fn barbaz(y) {\n", nil),
			pcall("r2", "read_file", `{"path":"x.rs","start_line":598,"end_line":602}`),
			ptool("", "", "let alpha_value = compute_first();\nlet beta_value = compute_second();\npassword=null\nsecret: NULL\ntoken=Nulls", nil),
			pcall("l1", "reverse_engineer", `{"command":"analyze"}`),
			ptool("reverse_engineer", "l1", "## functions (the program's own)\n\ncheck_code (1149), main (118f)\n", nil),
			pcall("d1", "reverse_engineer", `{"command":"decompile","target":"check_code"}`),
			ptool("reverse_engineer", "d1", `{"ok":false,"ok":"str"}`, nil),
			ptool("write_file", "", `{"tool":"write_file","ok":true,"content":"refused: but ok"}`, nil)}},
		{name: "long failure", prompt: "write it", msgs: []session.Message{
			ptool("write_file", "", `{"tool":"write_file","ok":false,"content":"cannot write /w/`+strings.Repeat("deep/", 40)+`x.txt: permission denied"}`, nil)}},
		{name: "deep json", prompt: "p", msgs: []session.Message{ptool("", "", `[{"a":[{"b":"line 77 here"}]}, "x", 5, null, true]`, nil),
			ptool("", "", ` {"broken": `, nil), ptool("", "", "[\"tab\\there\", \"\\u00e9\\ud800\"]", nil)}},
	}
}

// claimAnswers are every string in the Go tests of the checks (the first n), the same with Unicode around and between the
// words (up to base), then changed in case, line breaks and length.
func claimAnswers(t *testing.T) (out []string, n, base int) {
	lits := parity.StringLiterals(t, "claims_test.go", "secrets_test.go", "acceptance_test.go")
	seen := map[string]bool{}
	add := func(s string) {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	for _, s := range lits {
		add(s)
	}
	// the limits of the checks, from both sides
	for n := 76; n <= 81; n++ {
		add("I read " + strings.Repeat("y", n) + " unseen.md today.")
		add(strings.Repeat("A", n) + ". Use `crypto_identify` on it if needed.")
	}
	for _, a := range []string{
		strings.Repeat("B", 90) + ". Use `crypto_identify` to see if it is encoded.",
		"foo checks it.", "barbaz checks it.", "deregister_tm_clones checks the input.",
		"The relevant lines:\n```rust\nlet alpha_value = compute_first();\nlet beta_value = compute_second();\nlet gamma_value = compute_third_one();\n```",
		"**Examples**:\n- item\nThe value `qwerty12` here.",
		"The values null, NULL and Nulls.",
	} {
		add(a)
	}
	n = len(out)
	for _, s := range out[:n] {
		add("\u0130 \u00c4 " + strings.ReplaceAll(s, " ", "\u00a0"))
	}
	base = len(out)
	for _, s := range out[:n] {
		add(strings.ToUpper(s))
		add(strings.ReplaceAll(s, ".", ".\n"))
		if utf8.RuneCountInString(s) > 20 {
			add(string([]rune(s)[:utf8.RuneCountInString(s)/2]))
		}
	}
	return out, n, base
}

func scenJSON(s claimScen) any {
	var msgs []any
	for _, m := range s.msgs {
		var calls []any
		for _, c := range m.ToolCalls {
			calls = append(calls, []string{c.ID, c.Function.Name, c.Function.Arguments})
		}
		msgs = append(msgs, map[string]any{"role": m.Role, "content": m.Content, "name": m.Name, "tool_call_id": m.ToolCallID,
			"message_id": m.MessageID, "calls": calls, "meta": m.Metadata})
	}
	return map[string]any{"name": s.name, "prompt": s.prompt, "msgs": msgs, "msg_start": s.msgStart, "hist": s.hist,
		"offered": s.offered, "can_read": s.canRead, "can_write": s.canWrite}
}

// TestParityClaims records the answer checks (testdata/parity/claims.json): every check of claims.go and the secret
// redactor of secrets.go, on every answer of their Go tests (and those changed in case, spacing and length), in every task
// of those tests. Only answers a check does not let pass are recorded, by their number. A task whose tool output is a JSON
// object is run several times: toolTexts walks a map in Go's random order, so every distinct answer is kept.
func TestParityClaims(t *testing.T) {
	ws := t.TempDir()
	for name, body := range map[string]string{"config.env": "LOG_LEVEL=info\n", "todo.md": "- [ ] milk\n", "greeting.txt": "Hello wrold\n",
		"crashy.c": "int main(){}\n", "notes.txt": "Team sync.\n", "calc.go": "package calc\n"} {
		if err := os.WriteFile(filepath.Join(ws, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.Mkdir(filepath.Join(ws, "dir.md"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(ws, "docs"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("notes.txt", filepath.Join(ws, "link.txt")); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("missing.md", filepath.Join(ws, "dangling.md")); err != nil {
		t.Fatal(err)
	}
	// isCommand looks programs up on PATH: a made-up one, the same in Go's test and Rust's
	bin := filepath.Join(ws, "bin")
	if err := os.Mkdir(bin, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{"pacman", "chmod", "wpctl", "ls", "gpg", "systemctl", "go", "python3"} {
		if err := os.WriteFile(filepath.Join(bin, p), []byte("#!/bin/sh\n"), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	if err := os.WriteFile(filepath.Join(bin, "paru"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(bin, "yay"), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin)

	answers, lits, base := claimAnswers(t)
	var cases []parity.Case
	add := func(fn string, in, out any) { cases = append(cases, parity.Case{Fn: fn, In: in, Out: out}) }
	add("answers", map[string]int{"literals": lits, "base": base}, parity.Texts(answers))
	clean := func(s string) string { return strings.ReplaceAll(s, ws, "$W") }

	// the checks of the answer alone
	only := map[string]func(string) any{
		"suggestsSecurityOff": func(a string) any { return suggestsSecurityOff(a) },
		"announcedNotDone":    func(a string) any { return announcedNotDone(a) },
		"unwrapReminder":      func(a string) any { return unwrapReminder(a) },
		"exfilInAnswer":       func(a string) any { return exfilInAnswer(a) },
		"notAQuote":           func(a string) any { return notAQuote(strings.ToLower(a)) },
		"markedExample":       func(a string) any { return markedExample(a) },
		// a first word with a slash is looked up on the real file system, which differs between machines
		"isCommand": func(a string) any {
			if f := strings.Fields(a); len(f) > 0 && strings.Contains(f[0], "/") {
				return false
			}
			return isCommand(a)
		},
		"ownToolNamed":   func(a string) any { return ownToolNamed(a, []string{"system_status", "read_file", "crypto_hash"}) },
		"upToDateUnseen": func(a string) any { return upToDateUnseen(a, "crash records") },
		"defaultOutput":  func(a string) any { return defaultOutputForOneApp(a, "Firefox plays no sound but Spotify does") },
		"sentences": func(a string) any {
			b, _ := json.Marshal(sentenceEnd.Split(a, -1))
			return string(b)
		},
		"sentenceAt": func(a string) any {
			var out []string
			for i := 0; i <= len(a); i += 17 {
				out = append(out, sentenceAt(a, i))
			}
			b, _ := json.Marshal(out)
			return string(b)
		},
		"closingAdvice": func(a string) any {
			var out []string
			for i := 0; i <= len(a); i += 11 {
				if i == len(a) || utf8.RuneStart(a[i]) {
					s := sentenceAround(a, i, i)
					if closingAdvice(a, i, s) {
						out = append(out, strconv.Itoa(i)+":"+s)
					}
				}
			}
			b, _ := json.Marshal(out)
			return string(b)
		},
		// the question's side of the checks that read it, with every answer as the question
		"asPrompt": func(p string) any {
			r := []any{claimsToolWork("I checked it.", p, nil), unreproducedFailure(p, nil, []string{"check_code"}),
				fixDescribedNotMade(p, "The fix: change a to b.", true, nil), defaultOutputForOneApp("`wpctl set-default 52`", p),
				unsourcedCommand("Run `sudo pacman -Syu lua51` and `sudo pacman -Syu`.", p, nil), digestNotOneWay("The hash is a digest.", p, nil)}
			b, _ := json.Marshal(r)
			return string(b)
		},
	}
	names := make([]string, 0, len(only))
	for n := range only {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		hits := map[string]any{}
		for i, a := range answers {
			if (n == "sentenceAt" || n == "closingAdvice") && i >= lits {
				break
			}
			v := only[n](a)
			switch x := v.(type) {
			case bool:
				if !x {
					continue
				}
			case string:
				if x == "" || (n == "unwrapReminder" && x == a) {
					continue
				}
				v = parity.Text(x)
			}
			hits[strconv.Itoa(i)] = v
		}
		add(n, nil, hits)
	}

	// the checks of an answer in a task
	for _, s := range claimScens() {
		msgs := s.msgs[min(s.msgStart, len(s.msgs)):]
		withRem := s.prompt + "\n" + reminders(s.msgs)
		ev, _ := evidenceOf(withRem, msgs)
		checks := []struct {
			fn string
			f  func(a string) any
		}{
			{"unbackedFileClaim", func(a string) any { return unbackedFileClaim(a, s.hist) }},
			{"unreportedChange", func(a string) any { return unreportedChange(a, s.hist) }},
			{"ungroundedQuote", func(a string) any { return ungroundedQuote(a, s.prompt, msgs) }},
			{"claimsToolWork", func(a string) any { return claimsToolWork(a, s.prompt, msgs) }},
			{"unsourcedCommand", func(a string) any { return unsourcedCommand(a, s.prompt, msgs) }},
			{"inventedHistory", func(a string) any { return inventedHistory(a, s.msgs, s.msgStart) }},
			{"upToDateUnseen", func(a string) any { return upToDateUnseen(a, ev) }},
			{"ungroundedIdentifier", func(a string) any { return ungroundedIdentifier(a, withRem, msgs) }},
			{"digestNotOneWay", func(a string) any { return digestNotOneWay(a, withRem, msgs) }},
			{"checkedUnseenFile", func(a string) any { return checkedUnseenFile(a, msgs) }},
			{"toolErrorIgnored", func(a string) any { return toolErrorIgnored(a, msgs) }},
			{"codeWorksUnverified", func(a string) any { return codeWorksUnverified(a, s.hist) }},
			{"deniedReadable", func(a string) any { return clean(deniedReadable(a, s.prompt, ws, msgs, s.canRead)) }},
			{"fixDescribedNotMade", func(a string) any { return fixDescribedNotMade(s.prompt, a, s.canWrite, s.hist) }},
			{"inventedCode", func(a string) any {
				l, o := inventedCode(a, s.prompt, msgs)
				if l == "" && o == "" {
					return ""
				}
				return l + "\x00" + o
			}},
			{"decompileClaimed", func(a string) any { return decompileClaimed(a, msgs) }},
			{"unreadDefinition", func(a string) any {
				n, l := unreadDefinition(a, msgs)
				if n == "" {
					return ""
				}
				return n + ":" + strconv.Itoa(l)
			}},
			{"undecompiledFunction", func(a string) any {
				n, h := undecompiledFunction(a, msgs)
				if n == "" {
					return ""
				}
				return n + "\x00" + h
			}},
			{"claimsNoTools", func(a string) any { return claimsNoTools(a, msgs, s.offered) }},
			{"fixPending", func(a string) any { return fixPendingAfterFailedCheck(a, s.hist) }},
			{"failedWriteHidden", func(a string) any { return failedWriteHidden(a, msgs) }},
			{"toolNamedNotCalled", func(a string) any { return toolNamedNotCalled(a, msgs, s.offered) }},
			{"hideSecretsRead", func(a string) any {
				o, ch := hideSecretsRead(a, msgs)
				if !ch {
					return ""
				}
				return o
			}},
		}
		runs := 1
		for _, m := range msgs {
			if m.Role == "tool" && strings.Count(m.Content, `":`) > 1 {
				runs = 4
			}
		}
		for _, c := range checks {
			hits := map[string]any{}
			for i, a := range answers[:base] {
				var alts []string
				for r := 0; r < runs; r++ {
					v := c.f(a)
					var str string
					switch x := v.(type) {
					case bool:
						if x {
							str = "true"
						}
					case string:
						str = x
					}
					if !slicesContains(alts, str) {
						alts = append(alts, str)
					}
				}
				if len(alts) == 1 && alts[0] == "" {
					continue
				}
				sort.Strings(alts)
				hits[strconv.Itoa(i)] = parity.Texts(alts)
			}
			add(c.fn, s.name, hits)
		}
		// what the task alone gives
		var texts []string
		for _, m := range msgs {
			if m.Role == "tool" {
				texts = append(texts, toolTexts(m.Content)...)
			}
		}
		sort.Strings(texts)
		vals := secretValues(texts)
		redacted := []string{}
		if red := newStreamRedactor(msgs); red != nil {
			for _, step := range []int{1, 3, 7} {
				for _, a := range []string{"The password is hunter2-correct-horse-9 and the key abcd1234efgh, token bcde, pass: wxyz; é", "plain text without secrets"} {
					var parts []string
					r := []rune(a) // a streamed delta is always whole characters
					for i := 0; i < len(r); i += step {
						parts = append(parts, red.push(string(r[i:min(i+step, len(r))])))
					}
					parts = append(parts, red.flush())
					redacted = append(redacted, strings.Join(parts, "|"))
				}
			}
		}
		add("task", scenJSON(s), map[string]any{
			"taskInsteadOfFile":   taskInsteadOfFile(s.prompt, ws, msgs, s.hist),
			"writeRefused":        writeRefused(msgs),
			"editLoopStopped":     editLoopStopped(msgs),
			"reminders":           reminders(s.msgs),
			"unreproducedFailure": unreproducedFailure(s.prompt, msgs, s.offered),
			"toolTexts":           texts,
			"secretValues":        sortedLenGroups(vals),
			"redacted":            redacted,
		})
	}
	add("texts", nil, []string{claimCheckText("a.md"), unreportedText("b.md"), ungroundedText("q"), taskInsteadOfFileText("t.md"),
		securityOffText("ufw"), unsourcedText("c"), inventedHistoryText("h"), ungroundedIdentifierText("1.2.3"), checkedUnseenText("f.txt"),
		codeWorksText("why"), deniedReadableText("/w/x/y.txt"), inventedCodeText("l", ""), inventedCodeText("l", "o"), unreadDefinitionText("fn", 520),
		undecompiledFunctionText("n", "how"), claimsNoToolsText([]string{"a", "b"}), fixPendingText("f.py"), failedWriteHiddenText("why"),
		toolWorkText, oneAppSilentText, digestNotOneWayText, toolErrorIgnoredText, fixDescribedText, decompileClaimedText, announcedText,
		upToDateText, exfilInAnswerText, unreproducedFailureText, clipQuote("ééééé", 3), clipQuote("abc", 3)})
	parity.Fixture(t, "claims", cases)
}

func slicesContains(ss []string, s string) bool {
	for _, x := range ss {
		if x == s {
			return true
		}
	}
	return false
}

// sortedLenGroups is secretValues' answer with values of the same length in sorted order: Go's sort.Slice is not stable,
// so their order is not part of the answer.
func sortedLenGroups(vals []string) []string {
	out := append([]string{}, vals...)
	sort.SliceStable(out, func(i, j int) bool {
		if len(out[i]) != len(out[j]) {
			return len(out[i]) > len(out[j])
		}
		return out[i] < out[j]
	})
	return out
}
