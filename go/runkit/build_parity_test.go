package runkit

import (
	"context"
	"fmt"
	"math/rand/v2"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

type buildScen struct {
	Name       string            `json:"name"`
	Allow      []string          `json:"allow"`
	AllowSet   bool              `json:"allow_set"`
	Files      map[string]string `json:"files"`
	UseTiers   bool              `json:"use_tiers"`
	Escalate   bool              `json:"escalate"`
	Subagents  bool              `json:"subagents"`
	Override   string            `json:"override"`
	Append     string            `json:"append"`
	Answers    []string          `json:"answers"`
	Env        map[string]string `json:"env"`
	AllowWrite bool              `json:"allow_write"`
}

func chatAnswer(content string) string {
	b := `{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":` +
		strconv.Quote(content) + `},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":3,"total_tokens":14}}`
	return "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: " + strconv.Itoa(len(b)) + "\r\nConnection: close\r\n\r\n" + b
}

var (
	dateRE = regexp.MustCompile(`\d{4}-\d{2}-\d{2}`)
	hexRE  = regexp.MustCompile(`[0-9a-f]{32}`)
)

// TestParityBuild records runkit.Build (testdata/parity/runkitbuild.json): agents built from plain settings (the
// allowlist nil, empty and named, a policy denying a tool and capping the budget, a damaged policy, compute tiers, a
// damaged tiers file, the claude CLI escalation, sub-agents, the prompt options), each run once against a scripted model:
// the notices, whether a Router was made, the error, the answer and every request body. The tools are this test binary's
// built-ins (the default tools and memory's), which the Rust test passes as Spec.builtins. The workspace's path is in the
// prompt, so both tests use paths of the same shape, /tmp/sarina-runkit-XXXXXXXX/wNN.
func TestParityBuild(t *testing.T) {
	t.Setenv("SARINA_LESSONS_FILE", "off")
	t.Setenv("SARINA_TRUST_DIR", t.TempDir())
	ok := []string{chatAnswer("hi there")}
	models := "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 30\r\nConnection: close\r\n\r\n" + `{"data":[{"id":"test-model"}]}`
	routed := []string{models, chatAnswer("from the first tier"), models, chatAnswer("again")}
	scens := []buildScen{
		{Name: "plain", Answers: ok},
		{Name: "no tools", AllowSet: true, Answers: ok},
		{Name: "named tools", AllowSet: true, Allow: []string{"read_file", "memory_add", "not_a_tool"}, Answers: ok},
		{Name: "policy", Files: map[string]string{".sarina/policy.json": `{"denyTools":["bash","write_file"],"budget":{"maxToolCalls":1},"safeEnv":["PARITY_SAFE"]}`},
			Env: map[string]string{"PARITY_SAFE": "visible"}, AllowWrite: true, Answers: ok},
		{Name: "damaged policy", Files: map[string]string{".sarina/policy.json": `{`}},
		{Name: "tiers", UseTiers: true, Files: map[string]string{".sarina/tiers.json": `{"tiers":[{"name":"local","kind":"openai_compat","allow_data":"all"},` +
			`{"name":"big","kind":"openai_compat","base_url":"http://$ADDR/big/v1","model":"big","allow_data":["no_secrets"]}]}`}, Answers: routed},
		{Name: "damaged tiers", UseTiers: true, Files: map[string]string{".sarina/tiers.json": `{"tiers": 5}`}, Answers: ok},
		{Name: "tiers unused", Files: map[string]string{".sarina/tiers.json": `{"tiers":[{"name":"a","kind":"openai_compat"},{"name":"b","kind":"openai_compat","base_url":"http://127.0.0.1:9/v1","model":"x"}]}`}, Answers: ok},
		{Name: "escalation", Escalate: true, AllowWrite: true, Answers: routed},
		{Name: "subagents", Subagents: true, Answers: ok},
		{Name: "prompt", Override: "You only say hi.", Append: "Be brief.", Answers: ok},
	}
	top := fmt.Sprintf("/tmp/sarina-runkit-%08x", rand.Uint32())
	if err := os.Mkdir(top, 0o700); err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(top)
	var cases []parity.Case
	for n, sc := range scens {
		var script [][]parity.Step
		for _, a := range sc.Answers {
			script = append(script, []parity.Step{{Send: a}})
		}
		addr, seen := parity.RawSeq(t, nil, script)
		ws := fmt.Sprintf("%s/w%02d", top, n)
		os.MkdirAll(ws, 0o700)
		for name, c := range sc.Files {
			p := filepath.Join(ws, name)
			os.MkdirAll(filepath.Dir(p), 0o755)
			os.WriteFile(p, []byte(strings.ReplaceAll(c, "$ADDR", addr)), 0o644)
		}
		rt := config.DefaultRuntime(ws)
		rt.DisableClaudeMdDiscovery = true
		rt.Permissions.AllowFileWrite = sc.AllowWrite
		var allow []string
		if sc.AllowSet {
			allow = append([]string{}, sc.Allow...)
		}
		var esc *tiers.EscalationOptions
		if sc.Escalate {
			esc = &tiers.EscalationOptions{Enabled: true, CLIPath: filepath.Join(ws, "no-claude"), Triggers: []string{"backend_error"}}
		}
		env := sc.Env
		b, err := Build(Spec{Model: config.ModelConfig{Model: "test-model", BaseURL: "http://" + addr + "/v1", APIKey: "parity-key", TimeoutSeconds: 5},
			Runtime: rt, Allowlist: allow, Prompt: agent.PromptOptions{Override: sc.Override, Append: sc.Append}, Home: ws,
			Getenv: func(k string) string { return env[k] }, UseTiers: sc.UseTiers, Escalation: esc, Subagents: sc.Subagents})
		clean := func(s string) string {
			s = strings.ReplaceAll(s, addr, "$ADDR")
			s = strings.ReplaceAll(s, ws, "$W")
			s = hexRE.ReplaceAllString(s, "SID")
			return dateRE.ReplaceAllString(s, "DATE")
		}
		out := map[string]any{"err": clean(parity.Err(err))}
		if err == nil {
			var notices []string
			for _, x := range b.Notices {
				notices = append(notices, clean(x))
			}
			out["notices"] = notices
			out["router"] = b.Router != nil
			out["tools"] = b.Agent.ToolCount()
			res, rerr := b.Agent.Run(context.Background(), "hello")
			out["run_err"] = clean(parity.Err(rerr))
			if res != nil {
				out["final"], out["stop"] = clean(res.FinalOutput), res.StopReason
			}
			var reqs []string
			for _, r := range parity.Settle(seen) {
				body, _ := r["body"].(string)
				reqs = append(reqs, clean(fmt.Sprint(r["uri"]))+" "+clean(body))
			}
			out["requests"] = reqs
		}
		cases = append(cases, parity.Case{Fn: "build", In: sc, Out: out})
	}
	parity.Fixture(t, "runkitbuild", cases)
}
