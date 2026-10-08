package policy

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

type env struct {
	home, ws, extra string
}

func newEnv(t *testing.T) *env {
	t.Helper()
	root := t.TempDir()
	e := &env{home: filepath.Join(root, "home"), ws: filepath.Join(root, "repo", "work")}
	for _, d := range []string{filepath.Join(e.home, ".sarina"), e.ws} {
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	e.extra = filepath.Join(root, "extra.json")
	return e
}

func write(t *testing.T, path, body string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func (e *env) operator(t *testing.T, body string) {
	write(t, filepath.Join(e.home, ".sarina", "policy.json"), body)
}
func (e *env) workspace(t *testing.T, body string) {
	write(t, filepath.Join(e.ws, WorkspaceFile), body)
}

func (e *env) load() *Policy {
	return Load(Options{Home: e.home, Workspace: e.ws, Getenv: func(k string) string {
		if k == EnvFile {
			return e.extra
		}
		return ""
	}})
}

func ip(n int) *int { return &n }

func TestOperatorDenyListExactAndPrefixCaseInsensitive(t *testing.T) {
	e := newEnv(t)
	e.operator(t, `{"denyTools":["Bash","write_file"],"denyToolPrefixes":["mcp__evil__"]}`)
	p := e.load()
	for _, name := range []string{"bash", "BASH", "write_file", "mcp__evil__x", "MCP__EVIL__y"} {
		if p.Denied(name) == "" {
			t.Errorf("%s should be denied", name)
		}
	}
	for _, name := range []string{"read_file", "mcp__good__x", "bash2"} {
		if p.Denied(name) != "" {
			t.Errorf("%s should be allowed: %s", name, p.Denied(name))
		}
	}
	if !strings.Contains(p.Denied("bash"), "policy.json") {
		t.Fatalf("the refusal should name the file: %s", p.Denied("bash"))
	}
	if (*Policy)(nil).Denied("bash") != "" {
		t.Fatal("a nil policy denies nothing")
	}
}

func TestSnakeAndCamelKeysAndEnvFile(t *testing.T) {
	e := newEnv(t)
	write(t, e.extra, `{"deny_tools":["a"],"deny_tool_prefixes":"pre_","budget":{"max_tool_calls":3}}`)
	p := e.load()
	if p.Denied("a") == "" || p.Denied("pre_x") == "" {
		t.Fatal("snake_case keys and a string prefix were not honoured")
	}
	b := config.BudgetConfig{}
	p.ApplyBudget(&b)
	if b.MaxToolCalls == nil || *b.MaxToolCalls != 3 {
		t.Fatalf("budget = %+v", b)
	}
}

func TestWorkspacePolicyCanOnlyTighten(t *testing.T) {
	e := newEnv(t)
	e.operator(t, `{"denyTools":["bash"],"trusted":true,"budget":{"toolCalls":10,"totalTokens":1000}}`)
	e.workspace(t, `{"denyTools":["write_file"],"denyToolPrefixes":["mcp__"],"trusted":true,
		"safeEnv":["HOME","PATH"],"budget":{"toolCalls":50,"totalTokens":500},"hooks":{"beforeTool":{"*":"x"}}}`)
	p := e.load()
	// loosening attempts did nothing
	if p.Denied("bash") == "" {
		t.Fatal("the workspace removed an operator denial")
	}
	if !p.Trusted {
		t.Fatal("trust must stay at the operator's value")
	}
	if len(p.SafeEnv) != 0 {
		t.Fatalf("a workspace file exported environment variables: %v", p.SafeEnv)
	}
	b := config.BudgetConfig{}
	p.ApplyBudget(&b)
	if *b.MaxToolCalls != 10 {
		t.Fatalf("tool-call limit = %d; the workspace raised it", *b.MaxToolCalls)
	}
	if *b.MaxTotalTokens != 500 {
		t.Fatalf("token limit = %d; the workspace may lower it", *b.MaxTotalTokens)
	}
	// tightening worked
	if p.Denied("write_file") == "" || p.Denied("mcp__s__t") == "" {
		t.Fatal("workspace denials were not added")
	}
	notes := strings.Join(p.Notices, "\n")
	for _, want := range []string{"trusted=true ignored", "safeEnv ignored", "hooks are not supported", "tightens"} {
		if !strings.Contains(notes, want) {
			t.Errorf("notices lack %q:\n%s", want, notes)
		}
	}
}

func TestWorkspaceCanTurnTrustOffButNeverOn(t *testing.T) {
	e := newEnv(t)
	e.workspace(t, `{"trusted":false}`)
	if e.load().Trusted {
		t.Fatal("a workspace file may withdraw trust")
	}
	e.operator(t, `{"trusted":false}`)
	e.workspace(t, `{"trusted":true}`)
	if e.load().Trusted {
		t.Fatal("a workspace file must not grant trust")
	}
	e.operator(t, `{}`)
	if !e.load().Trusted {
		t.Fatal("trusted is the default")
	}
}

func TestWorkspaceBudgetWithoutAnyOtherLimitActsAsACap(t *testing.T) {
	e := newEnv(t)
	e.workspace(t, `{"budget":{"modelCalls":7,"totalCostUsd":1.5}}`)
	b := config.BudgetConfig{MaxModelCalls: ip(100)}
	e.load().ApplyBudget(&b)
	if *b.MaxModelCalls != 7 || *b.MaxTotalCostUSD != 1.5 {
		t.Fatalf("budget = %d / %v", *b.MaxModelCalls, *b.MaxTotalCostUSD)
	}
	// a smaller command-line limit is kept
	b = config.BudgetConfig{MaxModelCalls: ip(3)}
	e.load().ApplyBudget(&b)
	if *b.MaxModelCalls != 3 {
		t.Fatalf("limit = %d", *b.MaxModelCalls)
	}
}

func TestOperatorBudgetOverridesTheCommandLine(t *testing.T) {
	e := newEnv(t)
	e.operator(t, `{"budget":{"sessionTurns":4}}`)
	b := config.BudgetConfig{MaxSessionTurns: ip(99)}
	e.load().ApplyBudget(&b)
	if *b.MaxSessionTurns != 4 {
		t.Fatalf("turns = %d", *b.MaxSessionTurns)
	}
}

func TestBudgetRejectsBooleansStringsAndNegatives(t *testing.T) {
	e := newEnv(t)
	e.operator(t, `{"budget":{"toolCalls":true,"modelCalls":"5","sessionTurns":-1,"bogus":3,"totalTokens":9}}`)
	p := e.load()
	b := config.BudgetConfig{}
	p.ApplyBudget(&b)
	if b.MaxToolCalls != nil || b.MaxModelCalls != nil || b.MaxSessionTurns != nil {
		t.Fatalf("invalid values were accepted: %+v", b)
	}
	if b.MaxTotalTokens == nil || *b.MaxTotalTokens != 9 {
		t.Fatal("the valid value should still apply")
	}
	if len(p.Notices) != 4 {
		t.Fatalf("notices = %v", p.Notices)
	}
}

func TestParentDirectoriesAreNeverRead(t *testing.T) {
	e := newEnv(t)
	write(t, filepath.Join(filepath.Dir(e.ws), WorkspaceFile), `{"denyTools":["read_file"],"trusted":false}`)
	p := e.load()
	if p.Denied("read_file") != "" || !p.Trusted || len(p.Sources) != 0 {
		t.Fatalf("an ancestor's policy file was used: %v", p.Sources)
	}
}

func TestSafeEnvFromOperatorOnly(t *testing.T) {
	e := newEnv(t)
	e.operator(t, `{"safeEnv":["ZETA","ALPHA"]}`)
	if got := e.load().SafeEnv; !slices.Equal(got, []string{"ALPHA", "ZETA"}) {
		t.Fatalf("safeEnv = %v", got)
	}
}

func TestBrokenOperatorFileIsFatalButBrokenWorkspaceFileIsNot(t *testing.T) {
	e := newEnv(t)
	e.operator(t, `{not json`)
	if p := e.load(); len(p.Fatal) != 1 {
		t.Fatalf("a malformed operator policy must stop the run, Fatal = %v", p.Fatal)
	}
	e.operator(t, `{}`)
	e.workspace(t, `{not json`)
	p := e.load()
	if len(p.Fatal) != 0 || len(p.Notices) != 1 {
		t.Fatalf("fatal = %v notices = %v", p.Fatal, p.Notices)
	}
}

func TestNoFilesMeansAnOpenDefault(t *testing.T) {
	e := newEnv(t)
	p := e.load()
	if !p.Trusted || p.DenyRules() != 0 || len(p.Fatal) != 0 || len(p.Notices) != 0 {
		t.Fatalf("%+v", p)
	}
}
