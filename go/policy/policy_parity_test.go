package policy

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

type refuseNames map[string]bool

func (r refuseNames) Refuse(p string) string {
	if r[p] {
		return "not approved (made up)"
	}
	return ""
}

// policyFile writes a fixture file: its text, or "@dir" (a folder), "@big" (one byte over the limit), "@fill:<text>" (the
// text padded with spaces to the limit), "@unreadable:<text>" (mode 000).
func policyFile(t *testing.T, path, spec string) {
	t.Helper()
	os.MkdirAll(filepath.Dir(path), 0o755)
	switch {
	case spec == "@dir":
		os.MkdirAll(path, 0o755)
	case spec == "@big":
		os.WriteFile(path, []byte(strings.Repeat(" ", maxFileBytes+1)), 0o644)
	case strings.HasPrefix(spec, "@fill:"):
		body := spec[len("@fill:"):]
		os.WriteFile(path, []byte(body+strings.Repeat(" ", maxFileBytes-len(body))), 0o644)
	case strings.HasPrefix(spec, "@unreadable:"):
		os.WriteFile(path, []byte(spec[len("@unreadable:"):]), 0o000)
	default:
		os.WriteFile(path, []byte(spec), 0o644)
	}
}

func budgetOut(b config.BudgetConfig) []any {
	out := []any{}
	for _, f := range budgetFields {
		if f.i != nil {
			if v := *f.i(&b); v != nil {
				out = append(out, strconv.Itoa(*v))
			} else {
				out = append(out, nil)
			}
		} else if v := *f.f(&b); v != nil {
			out = append(out, strconv.FormatFloat(*v, 'g', -1, 64))
		} else {
			out = append(out, nil)
		}
	}
	return out
}

// policyTools are the names every case asks Denied about.
var policyTools = []string{"bash", "BASH", "Bash", "write_file", "Write_File", "mcp__x", "mcp__", "MCP__Y__z", "mcp_", "read_file", "", "x", "y", "a", "b",
	"web_fetch", "web", "İstanbul", "ıx", "Kelvin", "kelvin", "Kelvin", "denied", "ß", "SS"}

// TestParityPolicy records Load (operator files, SARINA_POLICY_FILE's list, the workspace file that can only tighten, the
// trust check), Denied, DenyRules and ApplyBudget on made-up folders (testdata/parity/policy.json; {ROOT} is the folder).
// Go reads a file's keys in map order, so each case runs 60 times and every distinct answer is kept: the Rust answer must be
// one of them (docs/RUST_PORT.md "Map order").
func TestParityPolicy(t *testing.T) {
	type pcase struct {
		home, ws *string
		extras   string            // SARINA_POLICY_FILE with {ROOT}
		files    map[string]string // more files under {ROOT}
		refuse   []string          // paths (with {ROOT}) the trust check refuses
		noHome   bool
		wsDir    string // the workspace folder under {ROOT} ("" = ws; "-" = none)
	}
	s := func(x string) *string { return &x }
	full := `{"denyTools":["bash","Write_File"," mcp__x ","İstanbul"],"denyToolPrefixes":["mcp__","WEB"],"trusted":false,"safeEnv":["PATH","HOME"," LANG "],` +
		`"budget":{"maxToolCalls":10,"max_total_cost_usd":1.5,"maxTotalTokens":1000,"input_tokens":200,"OutputTokens":50,"maxReasoningTokens":7,` +
		`"maxDelegatedTasks":2,"maxModelCalls":30,"maxSessionTurns":9}}`
	var cs []pcase
	for _, h := range []string{full, `{}`, `null`, `[]`, `"x"`, `5`, ``, `{"a":1}x`, `{"a":1,}`, `{"budget":{"maxToolCalls":1e400}}`,
		`{"deny_tools":"bash","DENYTOOLS":["x"]}`, `{"trusted":"yes"}`, `{"denyTools":[1,"",null," ","ok"]}`, `{"budget":[]}`, `{"safeEnv":{}}`,
		`{"denyToolPrefixes":5}`, `{"budget":{"maxTotalTokens":1e300}}`, `{"budget":{"inputTokens":-1}}`, `{"budget":{"MAX_OUTPUT_TOKENS":2.9}}`,
		`{"budget":{"reasoning_tokens":true}}`, `{"budget":{"totalCostUSD":"1"}}`, `{"budget":{"toolcalls":0}}`, `{"budget":{"maxmaxToolCalls":1}}`,
		`{"budget":{"max_tool_calls":-0}}`, `{"budget":{"max_tool_calls":1e19}}`, `{"budget":{"max_tool_calls":9.2233720368547758e18}}`,
		`{"budget":{"MaxTotal_Cost_USD":0.1}}`, `{"budget":{"cost":1}}`, `{"budget":{"":1}}`, `{"budget":{"é":1}}`, `{"budget":null}`,
		`{"hooks":{"before":"x"}}`, `{"Hooks":null}`, `{"HOOKS":1,"denyTools":["bash"]}`, `{"trusted":true,"Trusted":false}`, `{"trusted":null}`,
		`{"budget":{"maxToolCalls":5,"tool_calls":7}}`, `{"truſted":false}`, `{"DENY_TOOLS":["a"],"deny­tools":["b"]}`, `{"denytoolprefixes":["","x"]}`,
		`{"denyTools":"  "}`, `{"safeEnv":"ONE"}`, `{"safe_env":["B","A","B"]}`, `{"denyTools":["BASH","bash"]}`, `{"denyToolPrefixes":["mcp__","MCP__"]}`,
		`{"trusted":false,"x":[1,2,{"y":null}]}`, `{"denyTools":["Kelvin"]}`, `{"denyTools":["SS"],"denyToolPrefixes":["ı"]}`,
		"{\"denyTools\":[\"bad \xff\"]}", `{"a":"\ud800"}`, `{"budget":{"maxToolCalls":3,"bad":1,"worse":-2}}`, `{"denyToolPrefİxes":["z"],"budget":{"maxReasonİngTokens":4}}`} {
		cs = append(cs, pcase{home: s(h)})
	}
	for _, w := range []string{`{"trusted":true}`, `{"trusted":false,"safeEnv":["X"],"denyTools":["y"],"budget":{"maxToolCalls":2,"totalCostUsd":0.5}}`,
		`{"budget":{"maxToolCalls":1000,"maxSessionTurns":1}}`, `{"denyToolPrefixes":["b"]}`, `{}`, `not json`, `[]`, `{"hooks":[]}`, `{"trusted":"no"}`,
		`{"budget":{"maxToolCalls":-1}}`, `@big`, `@fill:{"denyTools":["x"]}`, `@dir`, `@unreadable:{"denyTools":["x"]}`, `{"denyTools":["bash"],"trusted":false}`} {
		cs = append(cs, pcase{home: s(full), ws: s(w)}, pcase{ws: s(w), noHome: true})
	}
	cs = append(cs,
		pcase{home: s(`{"denyTools":["a"]}`), extras: "{ROOT}/extra.json", files: map[string]string{"extra.json": `{"denyTools":["b"],"trusted":false}`}},
		pcase{home: s(`{"trusted":false}`), extras: "{ROOT}/extra.json", files: map[string]string{"extra.json": `{"trusted":true,"budget":{"maxToolCalls":4}}`}},
		pcase{extras: "{ROOT}/extra.json:{ROOT}/extra.json::{ROOT}/missing.json:{ROOT}/d:{ROOT}/home/.sarina/policy.json", home: s(`{"denyTools":["a"]}`),
			files: map[string]string{"extra.json": `{"denyTools":["b"]}`, "d": "@dir"}},
		pcase{extras: "{ROOT}/bad.json:{ROOT}/big.json:{ROOT}/locked.json:{ROOT}/f/x.json", files: map[string]string{"bad.json": `{`, "big.json": "@big",
			"locked.json": `@unreadable:{}`, "f": `x`}, noHome: true},
		pcase{extras: "{ROOT}/fill.json", files: map[string]string{"fill.json": `@fill:{"safeEnv":["FILLED"]}`}, noHome: true},
		pcase{extras: "relative-policy-nowhere.json:./also-nowhere.json", noHome: true},
		pcase{extras: "{ROOT}/ws/.sarina-policy.json", ws: s(`{"trusted":true,"safeEnv":["FROM_WS_AS_OPERATOR"]}`), noHome: true},
		pcase{home: s(`{"denyTools":["a"]}`), ws: s(`{"denyTools":["b"]}`), refuse: []string{"{ROOT}/home/.sarina/policy.json"}},
		pcase{home: s(`{"denyTools":["a"]}`), extras: "{ROOT}/extra.json:{ROOT}/missing.json", files: map[string]string{"extra.json": `{"denyTools":["c"]}`},
			refuse: []string{"{ROOT}/extra.json", "{ROOT}/missing.json", "{ROOT}/ws/.sarina-policy.json"}, ws: s(`{"denyTools":["w"]}`)},
		pcase{home: s(`{"denyTools":["a"]}`), wsDir: "-", ws: s(`{"denyTools":["never"]}`)},
		pcase{home: s(`{"denyTools":["a"]}`), wsDir: "home/.sarina/..", ws: s(`{"denyTools":["parent"]}`)},
	)
	starts := []config.BudgetConfig{{}}
	hundred, one, big := 100, 1, 1<<40
	hf, of := 100.0, 0.25
	starts = append(starts, config.BudgetConfig{MaxTotalTokens: &hundred, MaxInputTokens: &hundred, MaxOutputTokens: &hundred, MaxReasoningTokens: &hundred,
		MaxTotalCostUSD: &hf, MaxToolCalls: &hundred, MaxDelegatedTasks: &hundred, MaxModelCalls: &hundred, MaxSessionTurns: &hundred},
		config.BudgetConfig{MaxTotalTokens: &one, MaxToolCalls: &one, MaxTotalCostUSD: &of, MaxSessionTurns: &big})
	var cases []parity.Case
	for _, c := range cs {
		root := t.TempDir()
		home := filepath.Join(root, "home")
		in := map[string]any{}
		if c.home != nil {
			policyFile(t, filepath.Join(home, ".sarina", "policy.json"), *c.home)
			in["home"] = parity.Text(*c.home)
		}
		ws := filepath.Join(root, "ws")
		switch c.wsDir {
		case "-":
			ws = ""
			in["wsDir"] = "-"
		case "":
		default:
			ws = filepath.Join(root, c.wsDir)
			in["wsDir"] = c.wsDir
		}
		if c.ws != nil && ws != "" {
			policyFile(t, filepath.Join(ws, WorkspaceFile), *c.ws)
		}
		if c.ws != nil {
			in["ws"] = parity.Text(*c.ws)
		}
		for name, spec := range c.files {
			policyFile(t, filepath.Join(root, name), spec)
		}
		if len(c.files) > 0 {
			in["files"] = c.files
		}
		if c.extras != "" {
			in["extras"] = c.extras
		}
		if c.noHome {
			in["noHome"] = true
		}
		opts := Options{Workspace: ws, Getenv: func(k string) string {
			if k == EnvFile {
				return strings.ReplaceAll(c.extras, "{ROOT}", root)
			}
			return ""
		}}
		if !c.noHome {
			opts.Home = home
		}
		if c.refuse != nil {
			r := refuseNames{}
			for _, p := range c.refuse {
				r[strings.ReplaceAll(p, "{ROOT}", root)] = true
			}
			opts.Trust = r
			in["refuse"] = c.refuse
		}
		wd, _ := os.Getwd()
		clean := func(x string) string { return strings.ReplaceAll(strings.ReplaceAll(x, root, "{ROOT}"), wd, "{CWD}") }
		cleanAll := func(xs []string) []any {
			out := []any{}
			for _, x := range xs {
				out = append(out, parity.Text(clean(x)))
			}
			return out
		}
		seen := map[string]bool{}
		var outs []any
		for run := 0; run < 60; run++ {
			p := Load(opts)
			var denied []any
			for _, tool := range policyTools {
				denied = append(denied, parity.Text(clean(p.Denied(tool))))
			}
			var budgets []any
			for _, st := range starts {
				b := st
				p.ApplyBudget(&b)
				budgets = append(budgets, budgetOut(b))
			}
			out := map[string]any{"trusted": p.Trusted, "safeEnv": cleanAll(p.SafeEnv), "sources": cleanAll(p.Sources), "notices": cleanAll(p.Notices),
				"fatal": cleanAll(p.Fatal), "rules": p.DenyRules(), "denied": denied, "budgets": budgets}
			key, _ := json.Marshal(out)
			if !seen[string(key)] {
				seen[string(key)] = true
				outs = append(outs, out)
			}
		}
		// the answers in a fixed order, so the fixture does not change from run to run
		sort.Slice(outs, func(i, j int) bool {
			a, _ := json.Marshal(outs[i])
			b, _ := json.Marshal(outs[j])
			return string(a) < string(b)
		})
		os.Chmod(filepath.Join(ws, WorkspaceFile), 0o644)
		for name := range c.files {
			os.Chmod(filepath.Join(root, name), 0o644)
		}
		cases = append(cases, parity.Case{Fn: "load", In: in, Out: outs})
	}
	cases = append(cases, parity.Case{Fn: "tools", In: parity.Texts(policyTools)})
	parity.Fixture(t, "policy", cases)
}
