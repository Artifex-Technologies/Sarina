package cli

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/flagx"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// TestParityChatOptions records chat's and resume's flags and how they become the model and runtime configuration
// (testdata/parity/chatopts.json): opts.build on a fresh run and on a stored session (one's own and another account's),
// with the environment, the access tier and every flag that changes something; the usage on -h and the parse errors.
// "$BASE" is the test folder. The API key is recorded apart (it is never in the JSON forms).
func TestParityChatOptions(t *testing.T) {
	t.Setenv("SARINA_ACCESS_TIER", "")
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	hide := func(s string) string { return strings.ReplaceAll(s, top, "$BASE") }
	t.Chdir(top) // "." is the test folder (the workspace a bare run gets)
	schema := filepath.Join(top, "schema.json")
	os.WriteFile(schema, []byte(`{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}`), 0o644)
	os.WriteFile(filepath.Join(top, "array.json"), []byte(`[1,2]`), 0o644)
	os.WriteFile(filepath.Join(top, "broken.json"), []byte(`{"a":`), 0o644)
	ws := filepath.Join(top, "ws")
	os.MkdirAll(ws, 0o755)

	stored := map[string]string{
		"plain": `{"session_id":"s1","model_config":{"model":"stored-model","base_url":"http://127.0.0.1:9/v1","temperature":0.7,` +
			`"timeout_seconds":45,"pricing":{"input_cost_per_million_tokens_usd":1.5}},"runtime_config":{"cwd":"` + ws + `","max_turns":7,` +
			`"command_timeout_seconds":30,"max_output_chars":12000,"stream_model_responses":true,"compact_preserve_messages":2,` +
			`"permissions":{"allow_file_write":true,"allow_shell_commands":false},"additional_working_directories":["/tmp/x"],` +
			`"budget_config":{"max_total_tokens":5000},"session_directory":"/stored/sessions","scratchpad_root":"/stored/scratch",` +
			`"context_management":{"disable_notes":true}},"system_prompt_parts":["s"],"user_context":{},"system_context":{},` +
			`"messages":[],"turns":0,"tool_calls":0}`,
		"wide": `{"session_id":"s2","model_config":{"model":"m","base_url":"http://elsewhere.invalid/v1","fallbacks":["a","b"]},` +
			`"runtime_config":{"cwd":"/somewhere/else","max_turns":12,"permissions":{"allow_file_write":true,"allow_shell_commands":true,` +
			`"allow_destructive_shell_commands":true},"additional_working_directories":["/"],"escalation_config":{"enabled":true,` +
			`"cli_path":"/tmp/evil"},"session_directory":"/tmp/out","scratchpad_root":"/tmp/out2"},"system_prompt_parts":[],` +
			`"user_context":{},"system_context":{},"messages":[],"turns":0,"tool_calls":0}`,
	}
	type run struct {
		name    string
		args    []string
		env     map[string]string
		stored  string
		foreign bool
		tier    string
	}
	W := ws
	runs := []run{
		{name: "defaults"},
		{name: "env model", env: map[string]string{"OPENAI_MODEL": "env-model", "OPENAI_BASE_URL": "http://127.0.0.1:1/v1", "OPENAI_API_KEY": "sk-env",
			"SARINA_TEMPERATURE": "0.3", "SARINA_FALLBACK_MODELS": "x, y"}},
		{name: "model flags", args: []string{"--model", "m2", "--base-url", "http://127.0.0.1:2/v1", "--api-key", "k2", "--temperature", "1.25",
			"--timeout-seconds", "7.5", "--input-cost-per-million", "3", "--output-cost-per-million", "4e-1"}},
		{name: "runtime flags", args: []string{"--cwd", W, "--max-turns", "3", "--stream", "--auto-snip-threshold", "100",
			"--auto-compact-threshold", "0", "--compact-preserve-messages", "-5", "--allow-write", "--allow-shell", "--unsafe",
			"--add-dir", "/a", "--add-dir", "/b", "--disable-claude-md", "--session-dir", "/sd", "--scratchpad-root", "/sr"}},
		{name: "budget flags", args: []string{"--max-total-tokens", "10", "--max-input-tokens", "0", "--max-output-tokens", "20",
			"--max-tool-calls", "3", "--max-model-calls", "4", "--max-session-turns", "5", "--max-cost-usd", "0.25"}},
		{name: "context flags", args: []string{"--no-session-notes", "--no-clear-tool-outputs", "--no-llm-compaction", "--no-context-discovery", "--notes-every", "3"}},
		{name: "notes every zero", args: []string{"--notes-every", "0"}},
		{name: "tools list", args: []string{"--tools", " read_file, ,bash ,"}},
		{name: "tools empty", args: []string{"--tools", ""}},
		{name: "no tools", args: []string{"--no-tools", "--tools", "bash"}},
		{name: "schema", args: []string{"--response-schema-file", schema, "--response-schema-name", "ans", "--response-schema-strict"}},
		{name: "schema missing", args: []string{"--response-schema-file", filepath.Join(top, "nope.json")}},
		{name: "schema array", args: []string{"--response-schema-file", filepath.Join(top, "array.json")}},
		{name: "schema broken", args: []string{"--response-schema-file", filepath.Join(top, "broken.json")}},
		{name: "positional among flags", args: []string{"hello", "--allow-write", "there", "--", "--model", "x"}},
		{name: "bad float", args: []string{"--temperature", "warm"}},
		{name: "float out of range", args: []string{"--max-cost-usd", "1e999"}},
		{name: "float forms", args: []string{"--temperature", "0x1p-2", "--timeout-seconds", "1_000", "--input-cost-per-million", "Inf"}},
		{name: "bad int", args: []string{"--max-turns", "many"}},
		{name: "unknown flag", args: []string{"--frobnicate"}},
		{name: "help", args: []string{"-h"}},
		{name: "help with env defaults", args: []string{"--help"}, env: map[string]string{"OPENAI_MODEL": "env-model", "SARINA_TEMPERATURE": "0.5"}},
		{name: "escalation flags", args: []string{"--escalate-to-claude-code", "--escalation-trigger", "backend_error", "--disable-resource-check"}},
		{name: "tier 1", tier: "1"},
		{name: "tier 3", tier: "3"},
		{name: "tier 9", tier: "9"},
		{name: "tier junk", tier: "x"},
		{name: "stored plain", stored: "plain"},
		{name: "stored with env key", stored: "plain", env: map[string]string{"OPENAI_API_KEY": "sk-later", "OPENAI_BASE_URL": "http://127.0.0.1:3/v1"}},
		{name: "stored flags override", stored: "plain", args: []string{"--model", "new", "--max-turns", "2", "--stream=false", "--temperature", "0",
			"--allow-shell", "--session-dir", "/new/sd", "--notes-every", "4"}},
		{name: "stored new cwd", stored: "plain", args: []string{"--cwd", W}},
		{name: "stored new cwd keeps given dirs", stored: "plain", args: []string{"--cwd", W, "--session-dir", "/kept", "--scratchpad-root", "/kept2"}},
		{name: "stored defaults not applied", stored: "plain", args: []string{"--compact-preserve-messages", "4"}},
		{name: "stored wide own", stored: "wide"},
		{name: "stored wide foreign", stored: "wide", foreign: true, env: map[string]string{"OPENAI_BASE_URL": "http://127.0.0.1:4/v1"}},
		{name: "stored foreign flags", stored: "wide", foreign: true, args: []string{"--allow-write", "--add-dir", "/ok", "--session-dir", "/mine"}},
		{name: "stored tier", stored: "plain", tier: "3"},
	}
	var cases []parity.Case
	for _, r := range runs {
		env := map[string]string{}
		for k, v := range r.env {
			env[k] = v
		}
		t.Setenv("SARINA_ACCESS_TIER", r.tier)
		var errb bytes.Buffer
		e := &Env{Stderr: &errb, Getenv: func(k string) string { return env[k] }}
		o := newOpts("chat", e)
		positional, perr := flagx.Parse(o.fs, r.args)
		in := map[string]any{"args": r.args, "env": r.env, "stored": stored[r.stored], "foreign": r.foreign,
			"tier": config.AccessTierFromEnv(nil)}
		out := map[string]any{}
		if perr != nil {
			out["parse_error"] = perr.Error()
		} else {
			out["positional"] = positional
			var st *session.Session
			if r.stored != "" {
				s, err := session.Parse([]byte(stored[r.stored]))
				if err != nil {
					t.Fatal(r.name, err)
				}
				st = s
			}
			o.foreignLoaded = r.foreign
			m, rt, allow, err := o.build(e, st)
			if err != nil {
				out["error"] = hide(err.Error())
			} else {
				mj, _ := json.Marshal(m)
				rj, _ := json.Marshal(rt.Normalized()) // as a session file holds it
				out["model"] = parity.Text(hide(string(mj)))
				out["api_key"] = m.APIKey
				out["runtime"] = parity.Text(hide(string(rj)))
				if allow == nil {
					out["allow"] = nil
				} else {
					out["allow"] = allow
				}
				if eo := o.esc.Options(); eo != nil {
					out["escalation"] = map[string]any{"cli_path": eo.CLIPath, "triggers": eo.Triggers, "disable_resource_check": eo.DisableResourceCheck}
				}
			}
		}
		out["stderr"] = parity.Text(hide(errb.String()))
		in["args"] = func() []string {
			var a []string
			for _, x := range r.args {
				a = append(a, hide(x))
			}
			return a
		}()
		in["stored"] = hide(stored[r.stored])
		cases = append(cases, parity.Case{Fn: r.name, In: in, Out: out})
	}
	parity.Fixture(t, "chatopts", cases)
}
