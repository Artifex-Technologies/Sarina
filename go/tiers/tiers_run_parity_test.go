package tiers

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/heimdall"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// claudeStandIn stands in for the claude CLI, in bash's builtins only (the first line names the bash to run it): it writes
// its arguments and exported variables to .standin-call in its folder, then answers as STANDIN_MODE says.
const claudeStandIn = `#!BASH
{ printf '[%s]\n' "$@"; export -p; } > .standin-call
case "$STANDIN_MODE" in
  ok) printf '{"type":"result","is_error":false,"result":"done by claude","session_id":"cli-1","total_cost_usd":0.25}'; printf 'a warning\n' >&2;;
  is_error) printf '{"is_error":true,"result":"nope","session_id":"cli-2"}';;
  is_error_num) printf '{"is_error":1,"result":"x"}';;
  not_error) printf '{"is_error":0,"result":"fine","total_cost_usd":"x","session_id":5}';;
  is_error_str) printf '{"is_error":"","result":"empty string is false"}';;
  badjson) printf 'not json at all';;
  null) printf 'null';;
  array) printf '[1]';;
  trailing) printf '{"result":"x"} {}';;
  exit1) printf '{"result":"x"}'; exit 1;;
  empty) ;;
  secret) printf '{"result":"key local-key-0123456789 here","session_id":"s"}'; printf 'token ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa and local-key-0123456789\n' >&2;;
  bigerr) printf '%05000d' 7 >&2; printf '{"result":"after noise"}';;
  bigout) printf '{"result":"'; printf '%03000d' 1; printf '"}';;
  huge) printf '{"result":"'; printf '%016777216d' 0; printf '"}';;
  sleep) read -t 5 <> <(:);;
  tree) ( read -t 5 <> <(:) ) & read -t 5 <> <(:);;
  bgholds) ( read -t 4 <> <(:) ) & printf '{"result":"quick"}';;
esac
`

// shStandIn is the sh a wake command runs with: bash itself.
func writeStandIns(t *testing.T, dir, bash string) string {
	t.Helper()
	p := filepath.Join(dir, "claude")
	if err := os.WriteFile(p, []byte(strings.Replace(claudeStandIn, "BASH", bash, 1)), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(bash, filepath.Join(dir, "sh")); err != nil {
		t.Fatal(err)
	}
	return p
}

func httpAns(status, ctype, body string) []parity.Step {
	return []parity.Step{{Send: "HTTP/1.1 " + status + "\r\nContent-Type: " + ctype + "\r\nContent-Length: " + strconv.Itoa(len(body)) +
		"\r\nConnection: close\r\n\r\n" + body}}
}

func ansModels() []parity.Step { return httpAns("200 OK", "application/json", `{"data":[]}`) }
func ansChat(text string) []parity.Step {
	return httpAns("200 OK", "application/json", `{"choices":[{"message":{"role":"assistant","content":"`+text+
		`"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}`)
}
func ansClaude(text string) []parity.Step {
	return httpAns("200 OK", "application/json", `{"content":[{"type":"text","text":"`+text+
		`"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}`)
}
func ansStatus(status, body string) []parity.Step { return httpAns(status, "application/json", body) }
func ansSSE(parts ...string) []parity.Step {
	var b strings.Builder
	for _, p := range parts {
		b.WriteString("data: " + p + "\n\n")
	}
	b.WriteString("data: [DONE]\n\n")
	return []parity.Step{{Send: "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n" + b.String()}}
}
func hold() []parity.Step { return []parity.Step{{Hold: true}} }

// routerScenario is one Router's life: its tiers.json (with $S<n> for the scripted servers' base URLs, $A<n> for their
// bare addresses, $DEAD for an address nothing answers on, $CLI for the stand-in), the resource sample, the operator's
// answers, the wake runner's answers, Heimdall's grants file and answers, and the operations in order.
type routerScenario struct {
	Name     string
	Doc      string
	Servers  [][][]parity.Step
	Heimdall [][]parity.Step
	Sample   Sample
	Confirm  []bool // nil: no one can be asked
	Wake     []string
	NoWake   bool
	Grants   string
	Mode     string // the stand-in's STANDIN_MODE
	Timeout  float64
	Ops      []string
}

const longText = "0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789" +
	"0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789"

func routerScenarios() []routerScenario {
	secretText := "my token ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa and local-key-0123456789 please"
	claudeTierDoc := `{"name":"claude","kind":"claude","base_url":"$A1","model":"claude-test","api_key_env":"ANTHROPIC_TEST_KEY"}`
	return []routerScenario{
		{Name: "local", Doc: `{"tiers":[{"name":"local","base_url":"$S0","model":"local-model"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("hi there"), ansModels()}}, Sample: Sample{MemPercent: 20, MemOK: true},
			Ops: []string{"begin", "complete:hello", "record", "current", "pressure", "status"}},
		{Name: "escalate", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desktop","base_url":"$S0","model":"big-model"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("ok one"), ansChat("ok two")}},
			Ops:     []string{"begin", "complete:" + secretText, "record", "begin", "complete:again", "record", "current"}},
		{Name: "alldown", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desktop","base_url":"$DEAD"},` + strings.Replace(claudeTierDoc, `"model"`, `"enabled":false,"model"`, 1) + `]}`,
			Servers: [][][]parity.Step{{}, {}}, Ops: []string{"begin", "complete:hello", "record0"}},
		{Name: "captures", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},` + strings.Replace(claudeTierDoc, "$A1", "$A0", 1) + `]}`,
			Servers: [][][]parity.Step{{ansModels(), ansClaude("from claude")}},
			Ops:     []string{"begin", "complete:dump:\nFiletype: Flipper NFC device\nUID: 04 A1 B2 C3 ", "complete:plain question", "record"}},
		{Name: "errors", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desktop","base_url":"$S0"},` + claudeTierDoc + `]}`,
			Servers: [][][]parity.Step{{ansModels(), ansStatus("400 Bad Request", `{"error":{"message":"bad request"}}`),
				ansStatus("503 Service Unavailable", "busy"), ansStatus("503 Service Unavailable", "busy")},
				{ansModels(), ansClaude("claude answered"),
					ansStatus("500 Internal Server Error", `{"error":{"message":"key sk-ant-api03-TESTKEY-do-not-leak-0123456789 bad"}}`),
					ansStatus("500 Internal Server Error", `{"error":{"message":"key sk-ant-api03-TESTKEY-do-not-leak-0123456789 bad"}}`)}},
			Ops: []string{"begin", "complete:one", "begin", "complete:two", "begin", "complete:three", "record"}},
		{Name: "toobig", Doc: `{"tiers":[{"name":"local","base_url":"$S0","max_context":50},{"name":"desktop","base_url":"$S1"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("small")}, {ansModels(), ansChat("big")}},
			Ops:     []string{"begin", "complete:" + longText, "record", "begin", "complete:hi", "record"}},
		{Name: "pressure-stay", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"desktop","base_url":"$DEAD"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("stayed")}}, Sample: Sample{MemPercent: 95, MemOK: true, GPUPercent: 99, GPUOK: true},
			Ops: []string{"begin", "complete:hi", "pressure", "record"}},
		{Name: "pressure-up", Doc: `{"resource":{"max_cpu_percent":50},"tiers":[{"name":"local","base_url":"$S0"},{"name":"desktop","base_url":"$S1"}]}`,
			Servers: [][][]parity.Step{{ansModels()}, {ansModels(), ansChat("moved")}}, Sample: Sample{CPUPercent: 75, CPUOK: true},
			Ops: []string{"begin", "complete:hi", "pressure", "record"}},
		{Name: "timeouts", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"desktop","base_url":"$S1"}]}`, Timeout: 0.4,
			Servers: [][][]parity.Step{{ansModels(), hold(), hold()}, {ansModels(), ansChat("after timeouts")}},
			Ops:     []string{"begin", "complete:hi", "record"}},
		{Name: "updown", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"desktop","base_url":"$S1"},{"name":"cloud","base_url":"$S2","enabled":false}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("local again")}, {ansModels(), ansChat("on desktop")}, {}},
			Ops:     []string{"up", "up", "begin", "current", "complete:hi", "record", "begin", "up", "down", "current", "begin", "complete:back", "record"}},
		{Name: "auto-no", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desk","base_url":"$S0","auto":false}]}`,
			Servers: [][][]parity.Step{{ansModels()}}, Confirm: []bool{false}, Ops: []string{"begin", "complete:hi", "begin", "complete:again", "record"}},
		{Name: "auto-yes", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desk","base_url":"$S0","auto":false}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("yes one"), ansChat("yes two")}}, Confirm: []bool{true},
			Ops: []string{"begin", "complete:hi", "begin", "complete:again", "record"}},
		{Name: "auto-none", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desk","base_url":"$S0","auto":false}]}`,
			Servers: [][][]parity.Step{{ansModels()}}, Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "up-confirms", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"desk","base_url":"$S1","auto":false}]}`,
			Servers: [][][]parity.Step{{}, {ansModels(), ansChat("asked by up")}}, Ops: []string{"up", "begin", "complete:hi", "record"}},
		{Name: "wake", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD","wake":" wol x ","wake_wait_seconds":3},{"name":"desktop","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("woke one"), ansChat("woke two")}}, Wake: []string{"ok"},
			Ops: []string{"begin", "complete:hi", "begin", "complete:again", "record"}},
		{Name: "wake-up", Doc: `{"tiers":[{"name":"local","base_url":"$S0","wake":"wol x"}]}`,
			Servers: [][][]parity.Step{{ansStatus("503 Service Unavailable", ""), ansModels(), ansChat("up after wake")}}, Wake: []string{"ok"},
			Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "wake-fail", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD","wake":"wol x"},{"name":"desktop","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("desk")}}, Wake: []string{"no route to host"}, Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "wake-none", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD","wake":"wol x"},{"name":"desktop","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("desk")}}, NoWake: true, Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "grants-request", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},` + strings.Replace(claudeTierDoc, "$A1", "$A0", 1) + `]}`,
			Servers: [][][]parity.Step{{ansModels()}}, Grants: `{"access_tier": 0, "escalations": []}`,
			Heimdall: [][]parity.Step{ansStatus("200 OK", `{"id":"ab12","status":"pending","approve":"sudo heimdalld grant approve ab12"}`)},
			Ops:      []string{"begin", "complete:hi", "begin", "complete:again", "record"}},
		{Name: "grants-approved", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},` + strings.Replace(claudeTierDoc, "$A1", "$A0", 1) + `]}`,
			Servers: [][][]parity.Step{{ansModels(), ansClaude("approved")}},
			Grants:  `{"access_tier": 0, "escalations": [{"tier": "claude", "until": "2026-10-05T10:00:00Z"}, {"tier": "other", "until": "2030-01-01T00:00:00Z"}]}`,
			Ops:     []string{"begin", "complete:hi", "record"}},
		{Name: "grants-expired", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},` + strings.Replace(claudeTierDoc, "$A1", "$A0", 1) + `]}`,
			Servers: [][][]parity.Step{{ansModels()}}, Grants: `{"escalations": [{"tier": "claude", "until": "2026-10-05T09:00:03Z"}]}`,
			Heimdall: [][]parity.Step{ansStatus("403 Forbidden", "  no  ")}, Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "grants-bad-answer", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},` + strings.Replace(claudeTierDoc, "$A1", "$A0", 1) + `]}`,
			Servers: [][][]parity.Step{{ansModels()}}, Grants: `{}`, Heimdall: [][]parity.Step{ansStatus("200 OK", `{"approve":""}`)},
			Ops: []string{"begin", "complete:hi"}},
		{Name: "grants-loopback", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desktop","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("loopback is this computer")}}, Grants: `{}`, Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "cli", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"claude-cli","kind":"claude_cli","cli_path":"$CLI","extra_cli_args":["--model","x"]}]}`,
			Servers: [][][]parity.Step{}, Mode: "ok", Ops: []string{"begin", "stream:refactor the parser please", "record", "begin", "complete:again", "record"}},
		{Name: "cli-trigger", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"claude-cli","kind":"claude_cli","cli_path":"$CLI","trigger_stop_reasons":["resource_exhausted"]}]}`,
			Mode: "ok", Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "cli-pressure", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"claude-cli","kind":"claude_cli","cli_path":"$CLI","trigger_stop_reasons":["resource_exhausted"],"accept_edits":true,"add_dirs":["/extra"]}]}`,
			Servers: [][][]parity.Step{{ansModels()}}, Sample: Sample{MemPercent: 95, MemOK: true}, Mode: "ok", Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "cli-fail", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"claude-cli","kind":"claude_cli","cli_path":"$CLI","max_escalations":0}]}`,
			Mode: "is_error", Ops: []string{"begin", "complete:hi", "begin", "complete:" + secretText, "record"}},
		{Name: "cli-missing", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"claude-cli","kind":"claude_cli","cli_path":"/nonexistent/claude"}]}`,
			Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "cli-grants", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"claude-cli","kind":"claude_cli","cli_path":"$CLI"}]}`,
			Grants: `{"escalations":[]}`, Heimdall: [][]parity.Step{ansStatus("200 OK", `{"approve":"sudo heimdalld grant approve cd34"}`)}, Mode: "ok",
			Ops: []string{"begin", "complete:hi", "record"}},
		{Name: "cancelled", Doc: `{"tiers":[{"name":"local","base_url":"$S0"}]}`, Servers: [][][]parity.Step{{}},
			Ops: []string{"begin", "cancelled:hi", "record"}},
		{Name: "stream", Doc: `{"tiers":[{"name":"local","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansSSE(`{"choices":[{"delta":{"content":"str"}}]}`, `{"choices":[{"delta":{"content":"eamed"},"finish_reason":"stop"}]}`)}},
			Ops:     []string{"begin", "stream:hello", "record"}},
		{Name: "http504", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"desktop","base_url":"$S1"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansStatus("504 Gateway Timeout", ""), ansStatus("504 Gateway Timeout", "")}, {ansModels(), ansChat("after 504")}},
			Ops:     []string{"begin", "complete:hi", "record"}},
		{Name: "ttl-29", Doc: `{"tiers":[{"name":"local","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("one"), ansModels(), ansChat("two"), ansChat("three")}},
			Ops:     []string{"begin", "complete:hi", "advance:29", "begin", "complete:again", "record"}},
		{Name: "ttl-30", Doc: `{"tiers":[{"name":"local","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("one"), ansModels(), ansChat("two"), ansChat("three")}},
			Ops:     []string{"begin", "complete:hi", "advance:30", "begin", "complete:again", "record"}},
		{Name: "ttl-28", Doc: `{"tiers":[{"name":"local","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("one"), ansModels(), ansChat("two"), ansChat("three")}},
			Ops:     []string{"begin", "complete:hi", "advance:28", "begin", "complete:again", "record"}},
		{Name: "status", Doc: `{"tiers":[{"name":"local","base_url":"$S0"},{"name":"desk","base_url":"http://x","enabled":false},` +
			`{"name":"nokey","kind":"claude","model":"m","api_key_env":"NOPE_KEY"},{"name":"cli1","kind":"claude_cli","cli_path":"/nonexistent/claude"},` +
			`{"name":"cli2","kind":"claude_cli","cli_path":"$CLI"},{"name":"claude","kind":"claude","base_url":"$A1","model":"claude-test","api_key_env":"ANTHROPIC_TEST_KEY","allow_data":"all"},` +
			`{"name":"sick","base_url":"$S2","wake":"w","max_context":9},{"name":"bad","base_url":"http://[::1"}]}`,
			Servers: [][][]parity.Step{{ansModels()}, {ansStatus("401 Unauthorized", "")}, {ansStatus("500 Internal Server Error", "")}}, Ops: []string{"status"}},
		{Name: "record-merge", Doc: `{"tiers":[{"name":"local","base_url":"$DEAD"},{"name":"desktop","base_url":"$S0"}]}`,
			Servers: [][][]parity.Step{{ansModels(), ansChat("merged")}},
			Ops: []string{"begin", "complete:" + secretText, `recordprev:{"escalations":[{"type":"old"}],"highest_tier_index":1.9,"redactions":{"desktop":2,"x":"y","z":1.5},"other":1}`,
				`recordprev:{"highest_tier_index":7}`, `recordprev:"not a map"`, "recordprev:{}"}},
	}
}

func eventOut(hide func(string) string, e map[string]any) any {
	b, _ := json.Marshal(e)
	return hide(string(b))
}

// TestParityTiersRun records the parts of compute tiers that run something (testdata/parity/tiersrun.json): the claude
// CLI hand-off against a stand-in in bash's builtins (its arguments and environment, every kind of answer, the bounds,
// the timeout and a cancel killing its process group), the wake runner (URLs against scripted servers, the shell
// command under the permission model, with only bash as sh on PATH), and the Router's whole life in 31 scenarios
// against scripted model servers: health probes, escalations, refusals, redactions, confirmation, wake, Heimdall's
// grant gate with a scripted Heimdall, the CLI tier, the status, the session record. Never the real model.
func TestParityTiersRun(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil {
		t.Skip("no bash")
	}
	if r, err := filepath.EvalSymlinks(bash); err == nil {
		bash = r
	}
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	bin := filepath.Join(top, "bin")
	os.MkdirAll(bin, 0o755)
	cli := writeStandIns(t, bin, bash)
	t.Setenv("PATH", bin)
	t.Setenv("SAFE_VAR", "visible")
	t.Setenv("MY_TOKEN", "hidden")
	var cases []parity.Case
	add := func(fn string, in, out any) { cases = append(cases, parity.Case{Fn: fn, In: in, Out: out}) }
	baseHide := func(s string) string { return strings.ReplaceAll(strings.ReplaceAll(s, bash, "$BASH"), top, "$BASE") }

	// ---- the claude CLI client on its own
	environ := []string{"PATH=" + bin, "HOME=/h", "OPENAI_API_KEY=x", "ANTHROPIC_TEST_KEY=" + claudeKey, "GITHUB_TOKEN=ghp_x", "MY_SECRET_THING=hush",
		"SAFE_VAR=visible", "WRAPPED_VALUE=prefix-" + localKey + "-suffix", "CUSTOM_KEYNAME_VAR=1", "LANG=C"}
	msgs := []llm.Message{{Role: "system", Content: "be brief"}, {Role: "user", Content: "refactor the parser"},
		{Role: "assistant", Content: "working on it"}, {Role: "tool", Content: "secret local-key-0123456789 in output"}}
	type cliCase struct {
		mode, path, workdir, reason string
		timeout                     float64
		cancelAfter                 int // milliseconds; 0: never
		stream                      bool
		tier                        Tier
	}
	ccases := []cliCase{{mode: "ok", stream: true}, {mode: "ok", reason: "backend unreachable", tier: Tier{ExtraDirs: []string{"/a"}, AcceptEdits: true, ExtraArgs: []string{"--x"}, APIKeyEnv: "custom_keyname_var"}},
		{mode: "is_error"}, {mode: "is_error_num"}, {mode: "not_error"}, {mode: "is_error_str"}, {mode: "badjson"}, {mode: "null"}, {mode: "array"},
		{mode: "trailing"}, {mode: "exit1"}, {mode: "empty"}, {mode: "secret"}, {mode: "bigerr"}, {mode: "bigout"}, {mode: "huge"},
		{mode: "sleep", timeout: 0.5}, {mode: "tree", timeout: 0.5}, {mode: "sleep", cancelAfter: 300}, {mode: "bgholds"},
		{mode: "ok", path: "/nonexistent/claude"}, {mode: "ok", path: "claude-nope"}, {mode: "ok", path: "claude"}, {mode: "ok", workdir: "/nonexistent-dir"}}
	for i, cc := range ccases {
		work := filepath.Join(top, "cli", strconv.Itoa(i))
		os.MkdirAll(work, 0o755)
		if cc.workdir != "" {
			work = cc.workdir
		}
		tr := cc.tier
		tr.Kind, tr.CLIPath, tr.TimeoutSeconds = KindClaudeCLI, cli, cc.timeout
		if cc.path != "" {
			tr.CLIPath = cc.path
		}
		c := &cliClient{tier: tr, workdir: work, environ: func() []string { return append(environ, "STANDIN_MODE="+cc.mode) },
			secrets: []string{localKey, claudeKey, "short"}}
		ctx, cancel := context.WithCancel(context.Background())
		if cc.reason != "" {
			ctx = withReason(ctx, TriggerBackendError, cc.reason)
		}
		if cc.cancelAfter > 0 {
			time.AfterFunc(time.Duration(cc.cancelAfter)*time.Millisecond, cancel)
		}
		var events []any
		var on func(llm.StreamEvent)
		if cc.stream {
			on = func(e llm.StreamEvent) { events = append(events, []string{e.Type, e.Delta, e.FinishReason}) }
		}
		start := time.Now()
		turn, err := c.Complete(ctx, llm.Request{Messages: msgs}, on)
		took := time.Since(start)
		cancel()
		var tout any
		if turn != nil {
			raw, _ := json.Marshal(turn.Raw)
			tout = []any{turn.Content, turn.FinishReason, turn.HasFinishReason, string(raw)}
		}
		var cliErr *CLIError
		isCLI := errors.As(err, &cliErr)
		call, _ := os.ReadFile(filepath.Join(work, ".standin-call"))
		os.Remove(filepath.Join(work, ".standin-call"))
		l := c.last
		in := []any{cc.mode, baseHide(cc.path), baseHide(cc.workdir), cc.reason, cc.timeout, cc.cancelAfter, cc.stream, tr.ExtraDirs, tr.AcceptEdits, tr.ExtraArgs, tr.APIKeyEnv}
		add("cli", in, []any{tout, baseHide(parity.Err(err)), isCLI, []any{l.OK, baseHide(l.Err), l.SessionID, fmt.Sprint(l.CostUSD), len(l.Stdout), l.Stdout[:min(len(l.Stdout), 200)], l.Stderr[:min(len(l.Stderr), 200)], len(l.Stderr)},
			events, baseHide(string(call)), took < 4*time.Second, took < 1800*time.Millisecond})
	}

	// ---- the wake runner
	wakeAddr, wakeSeen := parity.RawSeq(t, nil, [][]parity.Step{ansStatus("200 OK", ""), ansStatus("404 Not Found", ""), {}})
	deadAddr := deadAddress(t)
	out := filepath.Join(top, "wake.out")
	wakes := []struct {
		perms config.Permissions
		wake  string
	}{
		{config.Permissions{}, "http://" + wakeAddr + "/wake"}, {config.Permissions{}, " http://" + wakeAddr + "/again "}, {config.Permissions{}, "http://" + wakeAddr + "/eof"},
		{config.Permissions{}, "http://" + deadAddr + "/x"}, {config.Permissions{}, "   "}, {config.Permissions{}, "echo hi > " + out},
		{config.Permissions{AllowShellCommands: true}, "rm -rf /"}, {config.Permissions{AllowShellCommands: true}, "sudo true"},
		{config.Permissions{AllowShellCommands: true}, `printf '%s|%s|%s' "$SAFE_VAR" "${MY_TOKEN-unset}" "$PWD" > ` + out},
		{config.Permissions{AllowShellCommands: true}, "exit 3"}, {config.Permissions{AllowShellCommands: true, AllowDestructiveShellCommands: true}, "printf x >> " + out},
		{config.Permissions{AllowShellCommands: true}, "ftp://example.invalid/x"},
	}
	cwd, _ := os.Getwd()
	for _, w := range wakes {
		err := NewWake(w.perms, top, nil)(context.Background(), Tier{Name: "desk", Wake: w.wake})
		b, _ := os.ReadFile(out)
		add("wake", []any{w.perms.AllowShellCommands, w.perms.AllowDestructiveShellCommands, baseHide(strings.ReplaceAll(strings.ReplaceAll(w.wake, wakeAddr, "$W"), deadAddr, "$DEAD"))},
			[]any{baseHide(strings.ReplaceAll(strings.ReplaceAll(strings.ReplaceAll(parity.Err(err), wakeAddr, "$W"), deadAddr, "$DEAD"), cwd, "$CWD")),
				strings.ReplaceAll(baseHide(string(b)), cwd, "$CWD")})
	}
	var wreqs []any
	for _, r := range parity.Settle(wakeSeen) {
		wreqs = append(wreqs, []any{r["method"], r["uri"]})
	}
	add("wakeRequests", nil, wreqs)

	// ---- the Router
	for _, sc := range routerScenarios() {
		var servers []any
		for _, script := range sc.Servers {
			servers = append(servers, parity.Steps(script))
		}
		in := map[string]any{"name": sc.Name, "doc": sc.Doc, "servers": servers, "heimdall": parity.Steps(sc.Heimdall),
			"sample":  []any{sc.Sample.MemPercent, sc.Sample.MemOK, sc.Sample.CPUPercent, sc.Sample.CPUOK, sc.Sample.GPUPercent, sc.Sample.GPUOK},
			"confirm": sc.Confirm, "wake": sc.Wake, "nowake": sc.NoWake, "grants": sc.Grants, "mode": sc.Mode, "timeout": sc.Timeout, "ops": sc.Ops}
		add("router", in, runRouterScenario(t, sc, top, cli, baseHide))
	}
	parity.Fixture(t, "tiersrun", cases)
}

func deadAddress(t *testing.T) string {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	a := ln.Addr().String()
	ln.Close()
	return a
}

func runRouterScenario(t *testing.T, sc routerScenario, top, cli string, baseHide func(string) string) any {
	var addrs []string
	var seens []func() []map[string]any
	for _, script := range sc.Servers {
		a, s := parity.RawSeq(t, nil, script)
		addrs, seens = append(addrs, a), append(seens, s)
	}
	dead := deadAddress(t)
	hAddr, hSeen := parity.RawSeq(t, nil, sc.Heimdall)
	t.Setenv(heimdall.EnvVar, "http://"+hAddr+"/v1/review")
	hide := func(s string) string {
		for i, a := range addrs {
			s = strings.ReplaceAll(s, a, fmt.Sprintf("$A%d", i))
		}
		s = strings.ReplaceAll(strings.ReplaceAll(s, dead, "$DEAD"), hAddr, "$H")
		return baseHide(s)
	}
	doc := sc.Doc
	for i, a := range addrs {
		doc = strings.ReplaceAll(doc, fmt.Sprintf("$S%d", i), "http://"+a+"/v1")
		doc = strings.ReplaceAll(doc, fmt.Sprintf("$A%d", i), "http://"+a)
	}
	doc = strings.ReplaceAll(strings.ReplaceAll(doc, "$DEAD", "http://"+dead+"/v1"), "$CLI", cli)
	cfg, err := Parse([]byte(doc))
	if err != nil {
		t.Fatalf("%s: %v", sc.Name, err)
	}
	if sc.Grants != "" {
		dir := filepath.Join(top, "grants-"+sc.Name)
		os.MkdirAll(dir, 0o755)
		os.Chmod(dir, 0o755)
		p := filepath.Join(dir, "grants.json")
		os.WriteFile(p, []byte(sc.Grants), 0o644)
		oldP, oldO := heimdall.GrantsPath, heimdall.GrantsOwner
		heimdall.GrantsPath, heimdall.GrantsOwner = p, uint32(os.Getuid())
		defer func() { heimdall.GrantsPath, heimdall.GrantsOwner = oldP, oldO }()
	}
	var mu sync.Mutex
	clock := time.Date(2026, 10, 5, 9, 0, 0, 0, time.UTC)
	now := func() time.Time {
		mu.Lock()
		defer mu.Unlock()
		c := clock
		clock = clock.Add(time.Second)
		return c
	}
	var log []any
	work := filepath.Join(top, "router-"+sc.Name)
	os.MkdirAll(work, 0o755)
	opt := Options{Config: cfg, Base: config.ModelConfig{Model: "local-model", BaseURL: cfg.Tiers[0].BaseURL, APIKey: localKey, TimeoutSeconds: 5},
		Getenv: envOf(map[string]string{"ANTHROPIC_TEST_KEY": claudeKey}), Sampler: &fakeSampler{sc.Sample}, Now: now,
		HealthTimeout: time.Second, WakePoll: 5 * time.Millisecond, Workdir: work,
		Environ: func() []string {
			return []string{"PATH=/nowhere", "STANDIN_MODE=" + sc.Mode, "OPENAI_API_KEY=" + localKey, "ANTHROPIC_TEST_KEY=" + claudeKey, "LANG=C"}
		}}
	if sc.Timeout > 0 {
		opt.Base.TimeoutSeconds = sc.Timeout
	}
	if sc.Confirm != nil {
		answers := append([]bool{}, sc.Confirm...)
		opt.Confirm = func(p string) bool {
			log = append(log, []any{"confirm", p})
			if len(answers) == 0 {
				return false
			}
			a := answers[0]
			answers = answers[1:]
			return a
		}
	}
	if sc.Wake != nil || !sc.NoWake {
		answers := append([]string{}, sc.Wake...)
		opt.Wake = func(ctx context.Context, tr Tier) error {
			log = append(log, []any{"wake", tr.Name, tr.Wake})
			if len(answers) == 0 || answers[0] == "ok" {
				if len(answers) > 0 {
					answers = answers[1:]
				}
				return nil
			}
			a := answers[0]
			answers = answers[1:]
			return errors.New(a)
		}
	}
	if sc.NoWake {
		opt.Wake = nil
	}
	r, err := New(opt)
	if err != nil {
		t.Fatal(err)
	}
	emit := func(e map[string]any) { log = append(log, []any{"event", eventOut(hide, e)}) }
	var prev any
	for _, op := range sc.Ops {
		name, arg, _ := strings.Cut(op, ":")
		switch name {
		case "begin":
			r.BeginTurn(emit)
		case "complete", "stream", "cancelled":
			ctx, cancel := context.WithCancel(context.Background())
			if name == "cancelled" {
				cancel()
			}
			req := llm.Request{Messages: []llm.Message{{Role: "system", Content: "be brief"}, {Role: "user", Content: arg}}}
			var on func(llm.StreamEvent)
			if name == "stream" {
				req.Stream = true
				on = func(e llm.StreamEvent) {
					log = append(log, []any{"stream", e.Type, e.Delta, e.FinishReason})
				}
			}
			turn, err := r.Complete(ctx, req, on)
			cancel()
			var tout any
			if turn != nil {
				tout = []any{turn.Content, turn.FinishReason, turn.Model}
			}
			var be *llm.BackendError
			var ce *CLIError
			kind, status := "", 0
			switch {
			case errors.As(err, &be):
				kind, status = "backend", be.Status
			case errors.As(err, &ce):
				kind = "cli"
			case err != nil:
				kind = "other"
			}
			log = append(log, []any{op[:min(len(op), 40)], tout, hide(parity.Err(err)), kind, status})
		case "advance":
			n, _ := strconv.Atoi(arg)
			mu.Lock()
			clock = clock.Add(time.Duration(n) * time.Second)
			mu.Unlock()
		case "up":
			n, err := r.RequestUp()
			log = append(log, []any{"up", n, parity.Err(err)})
		case "down":
			log = append(log, []any{"down", r.RequestDown()})
		case "current":
			log = append(log, []any{"current", r.Current()})
		case "pressure":
			s, p := r.Pressure(context.Background())
			th := r.Thresholds()
			log = append(log, []any{"pressure", fmt.Sprint(s.MemPercent), s.MemOK, p, fmt.Sprint(th.MaxMemoryPercent), fmt.Sprint(th.MaxCPUPercent)})
		case "record", "record0", "recordprev":
			var p any
			switch name {
			case "record":
				p = prev
			case "recordprev":
				json.Unmarshal([]byte(arg), &p)
			}
			rec := r.Record(p)
			b, _ := json.Marshal(rec)
			var back any
			json.Unmarshal(b, &back)
			if rec != nil {
				prev = back
			}
			log = append(log, []any{name, hide(string(b))})
		case "status":
			for _, s := range r.Status(context.Background()) {
				log = append(log, []any{"status", s.Name, s.Kind, hide(s.BaseURL), s.Model, s.APIKeyEnv, s.KeySet, s.AllowData, s.MaxContext, s.Wake,
					s.Enabled, s.Auto, s.Healthy, hide(s.Why), s.Current, hide(s.CLIPath), s.CLIFound})
			}
		default:
			t.Fatalf("unknown op %q", op)
		}
	}
	var served []any
	for _, seen := range seens {
		var reqs []any
		for _, rq := range parity.Settle(seen) {
			reqs = append(reqs, []any{rq["method"], rq["uri"], hide(fmt.Sprint(rq["auth"])), hide(fmt.Sprint(rq["apiKey"])), hide(fmt.Sprint(rq["body"]))})
		}
		served = append(served, reqs)
	}
	var hreqs []any
	for _, rq := range parity.Settle(hSeen) {
		hreqs = append(hreqs, []any{rq["method"], rq["uri"], rq["body"]})
	}
	var calls []string
	if b, err := os.ReadFile(filepath.Join(work, ".standin-call")); err == nil {
		calls = strings.Split(hide(string(b)), "\n")
	}
	return []any{log, served, hreqs, calls}
}
