package agent

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/policy"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

func withSubagents(o *Options) { o.Subagents = &agents.Catalog{Profiles: agents.Builtins()} }

func callAgent(id, profile, prompt string) tcall {
	return tcall{id, "agent", `{"profile":"` + profile + `","prompt":"` + prompt + `"}`}
}

func hasAny(names []string, bad ...string) string {
	for _, n := range names {
		if slices.Contains(bad, n) {
			return n
		}
	}
	return ""
}

func TestSubagentRunsReadOnlyChildAndReturnsItsReport(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(10, 5), callAgent("c1", "explorer", "find the config")),
		reply("the config is in app.toml", "stop", use(7, 3)),
		reply("all done", "stop", use(10, 5)),
	)
	a := newTestAgent(t, f, withSubagents, withWrite, func(o *Options) { o.Runtime.Permissions.AllowShellCommands = true })
	res := mustRun(t, a, "where is the config?")
	if res.StopReason != "stop" || res.FinalOutput != "all done" {
		t.Fatalf("stop=%q out=%q", res.StopReason, res.FinalOutput)
	}
	reqs := f.requests()
	if len(reqs) != 3 {
		t.Fatalf("requests = %d, want 3", len(reqs))
	}
	if !strings.Contains(res.Transcript[len(res.Transcript)-2].Content, "the config is in app.toml") {
		t.Fatalf("the child's report must be the tool result: %s", res.Transcript[len(res.Transcript)-2].Content)
	}

	// The child saw only read-only tools, no agent tool, and its own prompt.
	childTools := reqToolNames(reqs[1])
	if bad := hasAny(childTools, "write_file", "edit_file", "bash", "agent", "todo_write", "update_plan", "TaskStop"); bad != "" {
		t.Fatalf("child was offered %s: %v", bad, childTools)
	}
	if !slices.Contains(childTools, "read_file") || !slices.Contains(childTools, "grep_search") {
		t.Fatalf("child lacks the read tools: %v", childTools)
	}
	sys := reqMessages(reqs[1])[0]["content"].(string)
	if strings.Contains(sys, "You are Sarina") || !strings.Contains(sys, "exploration helper") {
		t.Fatalf("child system prompt = %q", sys)
	}
	// The parent still has everything.
	if !slices.Contains(reqToolNames(reqs[0]), "write_file") || !slices.Contains(reqToolNames(reqs[0]), "agent") {
		t.Fatalf("parent tools: %v", reqToolNames(reqs[0]))
	}

	// Linkage: parent history entry, child file with a parent link, counters.
	parent := mustLoad(t, res)
	var childID string
	for _, e := range parent.FileHistory {
		if e["history_kind"] == "delegation" {
			childID, _ = e["child_session_id"].(string)
		}
	}
	if childID == "" {
		t.Fatalf("no delegation entry in %v", parent.FileHistory)
	}
	child, err := session.LoadByID(filepath.Dir(res.SessionPath), childID)
	if err != nil {
		t.Fatalf("child session not saved next to the parent: %v", err)
	}
	link, _ := child.PluginState[SubagentStateKey].(map[string]any)
	if link["parent_session_id"] != parent.ID || link["parent_tool_call_id"] != "c1" || link["profile"] != "explorer" {
		t.Fatalf("link = %v", link)
	}
	if parent.BudgetState.DelegatedTasks != 1 {
		t.Fatalf("delegated tasks = %d", parent.BudgetState.DelegatedTasks)
	}
	// The child's tokens count against the parent: 10+5 + 7+3 + 10+5.
	if res.Usage.Total() != 40 {
		t.Fatalf("usage total = %d, want 40 (child rolled up)", res.Usage.Total())
	}
	if !hasEvent(res, "subagent_start") || !hasEvent(res, "subagent_end") {
		t.Fatalf("events: %v", eventTypes(res))
	}
}

func TestChildCannotWidenTheToolSet(t *testing.T) {
	f := newFakeServer(t)
	greedy := &agents.Profile{Name: "greedy", Description: "asks for everything", Prompt: "p",
		Tools: []string{"write_file", "edit_file", "bash", "agent", "read_file", "task_create", "enter_plan_mode", "no_such_tool"}}
	// parent with everything allowed
	a := newTestAgent(t, f, withSubagents, withWrite, func(o *Options) { o.Runtime.Permissions.AllowShellCommands = true })
	if got := a.childTools(greedy); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("greedy profile (no shell flag) got %v", got)
	}
	greedy.Shell = true
	if got := a.childTools(greedy); !slices.Equal(got, []string{"read_file", "bash"}) {
		t.Fatalf("with shell flag and a shell-capable parent: %v", got)
	}
	// verifier needs the parent's shell permission as well
	noShell := newTestAgent(t, f, withSubagents, withWrite)
	v, _ := (&agents.Catalog{Profiles: agents.Builtins()}).Get("verifier")
	if slices.Contains(noShell.childTools(v), "bash") {
		t.Fatal("a child got bash although the parent may not run shell commands")
	}
	if !slices.Contains(a.childTools(v), "bash") {
		t.Fatal("the verifier should keep bash under a shell-capable parent")
	}
	// the registry the child gets is the narrowed one, and bash is not destructive-capable
	if bad := hasAny(a.childTools(v), "write_file", "edit_file"); bad != "" {
		t.Fatalf("verifier got %s", bad)
	}
}

func TestChildOfAParentWithAnEmptyAllowlistHasNoTools(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f, withSubagents, func(o *Options) { o.Tools.Allowlist = []string{} })
	if a.ToolCount() != 0 {
		t.Fatalf("parent has %d tools", a.ToolCount())
	}
	for _, p := range agents.Builtins() {
		if got := a.childTools(p); len(got) != 0 || got == nil {
			t.Fatalf("%s: child tools = %#v, want an empty non-nil list (zero tools)", p.Name, got)
		}
	}
}

func TestChildRequestOmitsToolsWhenNothingIsLeft(t *testing.T) {
	// The parent may only call `agent`; the child inherits nothing it may use.
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), callAgent("c1", "explorer", "look around")),
		reply("I have no tools", "stop", use(1, 1)),
		reply("ok", "stop", use(1, 1)),
	)
	a := newTestAgent(t, f, withSubagents, func(o *Options) { o.Tools.Allowlist = []string{"agent"} })
	mustRun(t, a, "go")
	reqs := f.requests()
	if _, has := reqs[1]["tools"]; has {
		t.Fatalf("the child's request carried tools: %v", reqToolNames(reqs[1]))
	}
	if _, has := reqs[1]["tool_choice"]; has {
		t.Fatal("tool_choice must be omitted too")
	}
}

func TestChildOfAPlanModeParentKeepsNoShell(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f, withSubagents, withWrite, func(o *Options) { o.Runtime.Permissions.AllowShellCommands = true })
	v, _ := (&agents.Catalog{Profiles: agents.Builtins()}).Get("verifier")
	if !slices.Contains(a.childTools(v), "bash") {
		t.Fatal("precondition: the verifier has bash outside plan mode")
	}
	a.EnterPlan(nil)
	if got := a.childTools(v); slices.Contains(got, "bash") {
		t.Fatalf("plan mode must carry over to the child: %v", got)
	}
}

func TestChildCannotStartAnotherAgent(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), callAgent("c1", "explorer", "go deeper")),
		reply("", "tool_calls", use(1, 1), callAgent("k1", "explorer", "and deeper")), // the child tries
		reply("could not", "stop", use(1, 1)),
		reply("done", "stop", use(1, 1)),
	)
	a := newTestAgent(t, f, withSubagents)
	res := mustRun(t, a, "go")
	if res.StopReason != "stop" {
		t.Fatalf("stop = %q", res.StopReason)
	}
	reqs := f.requests()
	if len(reqs) != 4 {
		for i, r := range reqs {
			ms := reqMessages(r)
			t.Logf("req %d: tools=%v last=%v", i, reqToolNames(r), ms[len(ms)-1])
		}
		t.Fatalf("requests = %d", len(reqs))
	}
	// the child's follow-up request carries the refusal of its own agent call
	last := reqMessages(reqs[2])
	if got := last[len(last)-1]["content"].(string); !strings.Contains(got, "unknown tool") {
		t.Fatalf("the child's agent call should be an unknown tool, got %s", got)
	}
	// a child built directly with a parent link has no agent tool either
	c, err := New(Options{Model: a.opts.Model, Runtime: a.opts.Runtime, Client: a.opts.Client, Subagents: a.opts.Subagents,
		ParentLink: map[string]any{"depth": 1}})
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := c.Registry().Get(ToolAgent); ok {
		t.Fatal("a child agent was given the agent tool")
	}
}

func TestUnknownProfileIsAnErrorNotAFallback(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"c1", "agent", `{"profile":"general-purpose","prompt":"x"}`}),
		reply("ok", "stop", use(1, 1)),
	)
	a := newTestAgent(t, f, withSubagents)
	res := mustRun(t, a, "go")
	// the enum in the schema rejects it before the handler even runs
	tool := res.Transcript[len(res.Transcript)-2]
	if !strings.Contains(tool.Content, `"ok": false`) || !strings.Contains(tool.Content, "profile") {
		t.Fatalf("tool message = %s", tool.Content)
	}
	if len(f.requests()) != 2 {
		t.Fatalf("no child run was expected, requests = %d", len(f.requests()))
	}
	// asking the handler directly names what exists
	_, _, err := a.runSubagent(t.Context(), map[string]any{"profile": "nope", "prompt": "x"}, nil)
	if err == nil || !strings.Contains(err.Error(), "explorer") {
		t.Fatalf("err = %v", err)
	}
}

func TestPolicyDeniesTheAgentToolAndFiltersTheChild(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "p.json"), []byte(`{"denyTools":["grep_search"],"denyToolPrefixes":["AGE"]}`), 0o644); err != nil {
		t.Fatal(err)
	}
	pol := policy.Load(policy.Options{Getenv: func(k string) string {
		if k == policy.EnvFile {
			return filepath.Join(dir, "p.json")
		}
		return ""
	}})
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), callAgent("c1", "explorer", "x")),
		reply("ok", "stop", use(1, 1)),
	)
	a := newTestAgent(t, f, withSubagents, func(o *Options) { o.Tools.Deny = pol.Denied })
	res := mustRun(t, a, "go")
	if len(denials(res)) != 1 || denials(res)[0]["tool_name"] != "agent" {
		t.Fatalf("denials = %v", denials(res))
	}
	if len(f.requests()) != 2 {
		t.Fatalf("the child must not have run: %d requests", len(f.requests()))
	}
	// without the prefix rule the agent runs, and the denied tool stays denied for the child
	if err := os.WriteFile(filepath.Join(dir, "p.json"), []byte(`{"denyTools":["grep_search"]}`), 0o644); err != nil {
		t.Fatal(err)
	}
	pol = policy.Load(policy.Options{Getenv: func(k string) string {
		if k == policy.EnvFile {
			return filepath.Join(dir, "p.json")
		}
		return ""
	}})
	f2 := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), callAgent("c1", "explorer", "x")),
		reply("child done", "stop", use(1, 1)),
		reply("ok", "stop", use(1, 1)),
	)
	a2 := newTestAgent(t, f2, withSubagents, func(o *Options) { o.Tools.Deny = pol.Denied })
	mustRun(t, a2, "go")
	if slices.Contains(reqToolNames(f2.requests()[1]), "grep_search") {
		t.Fatal("a tool the policy denies reached the child")
	}
}

func TestChildGetsItsOwnBudgetFromTheParentsRemainingAllowance(t *testing.T) {
	f := newFakeServer(t)
	total, calls, cost := 1000, 10, 2.0
	a := newTestAgent(t, f, withSubagents, func(o *Options) {
		o.Runtime.BudgetConfig.MaxTotalTokens = &total
		o.Runtime.BudgetConfig.MaxToolCalls = &calls
		o.Runtime.BudgetConfig.MaxTotalCostUSD = &cost
		o.Model.Pricing.InputPerMillion = 1e6 // $1 per token, so the arithmetic is visible
	})
	a.cur = &run{usage: llm.Usage{InputTokens: 300, OutputTokens: 100}, counts: counters{ToolCalls: 4}}
	b := a.childBudget(5)
	if *b.MaxTotalTokens != 600 || *b.MaxToolCalls != 6 || *b.MaxSessionTurns != 5 || *b.MaxDelegatedTasks != 0 {
		t.Fatalf("budget = tokens %d calls %d turns %d delegated %d", *b.MaxTotalTokens, *b.MaxToolCalls, *b.MaxSessionTurns, *b.MaxDelegatedTasks)
	}
	if *b.MaxTotalCostUSD != 0 { // 2.0 - 300 tokens at $1 each is exhausted
		t.Fatalf("cost left = %v", *b.MaxTotalCostUSD)
	}
	// with no parent limits the child still gets a fixed tool-call cap
	a2 := newTestAgent(t, f, withSubagents)
	if b := a2.childBudget(3); *b.MaxToolCalls != defaultChildToolCalls || b.MaxTotalTokens != nil {
		t.Fatalf("default budget = %+v", b)
	}
}

func TestDelegatedTaskBudgetStopsTheSecondCall(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), callAgent("c1", "explorer", "one"), callAgent("c2", "explorer", "two")),
		reply("child one", "stop", use(1, 1)),
	)
	one := 1
	a := newTestAgent(t, f, withSubagents, func(o *Options) { o.Runtime.BudgetConfig.MaxDelegatedTasks = &one })
	res := mustRun(t, a, "go")
	if res.StopReason != StopBudgetExceeded || !strings.Contains(res.FinalOutput, "delegated-task") {
		t.Fatalf("stop = %q out = %q", res.StopReason, res.FinalOutput)
	}
	assertPaired(t, res.Transcript)
}

func TestSubagentProfilesFromOperatorFilesNarrowOnly(t *testing.T) {
	home := t.TempDir()
	dir := filepath.Join(home, ".sarina", "agents")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	body := "---\nname: scout\ndescription: looks at logs\ntools: write_file, read_file\nshell: true\nmax_turns: 3\n---\nRead logs and summarise them.\n"
	if err := os.WriteFile(filepath.Join(dir, "scout.md"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	cat := agents.Load(agents.Options{Home: home, Trusted: true})
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), callAgent("c1", "scout", "check")),
		reply("logs fine", "stop", use(1, 1)),
		reply("ok", "stop", use(1, 1)),
	)
	a := newTestAgent(t, f, withWrite, func(o *Options) { o.Subagents = cat }) // the parent has no shell
	mustRun(t, a, "go")
	names := reqToolNames(f.requests()[1])
	if !slices.Equal(names, []string{"read_file"}) {
		t.Fatalf("scout got %v; write_file and bash were requested but are not allowed", names)
	}
	_ = tools.ClassReadOnly
}
