package agent

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

func reqToolNames(req map[string]any) []string {
	var out []string
	list, _ := req["tools"].([]any)
	for _, t := range list {
		fn, _ := t.(map[string]any)["function"].(map[string]any)
		out = append(out, fn["name"].(string))
	}
	return out
}

func withWrite(o *Options) { o.Runtime.Permissions.AllowFileWrite = true }

func mustLoad(t *testing.T, res *Result) *session.Session {
	t.Helper()
	s, err := session.Load(res.SessionPath)
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func TestEnterPlanModeRemovesMutatingToolsFromTheRequest(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"c1", "enter_plan_mode", "{}"}),
		reply("investigating", "stop", use(1, 1)))
	a := newTestAgent(t, f, withWrite, func(o *Options) { o.Runtime.Permissions.AllowShellCommands = true })
	res := mustRun(t, a, "refactor everything")
	reqs := f.requests()
	first, second := reqToolNames(reqs[0]), reqToolNames(reqs[1])
	for _, n := range []string{"write_file", "bash", "enter_plan_mode"} {
		if !slices.Contains(first, n) {
			t.Errorf("normal mode must offer %s: %v", n, first)
		}
	}
	if slices.Contains(first, "exit_plan_mode") {
		t.Errorf("exit_plan_mode must not be offered outside plan mode: %v", first)
	}
	for _, n := range []string{"write_file", "edit_file", "bash", "TaskStop", "todo_write", "update_plan", "task_create", "enter_plan_mode"} {
		if slices.Contains(second, n) {
			t.Errorf("plan mode must not offer %s at all: %v", n, second)
		}
	}
	for _, n := range []string{"read_file", "glob_search", "grep_search", "list_dir", "TaskOutput", "exit_plan_mode"} {
		if !slices.Contains(second, n) {
			t.Errorf("plan mode must offer %s: %v", n, second)
		}
	}
	if got := mustLoad(t, res).Plan(); !got.Active() {
		t.Fatalf("plan state not saved: %+v", got)
	}
}

func TestNormalSessionFileCarriesNoPlanEntry(t *testing.T) {
	f := newFakeServer(t, reply("hi", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	res := mustRun(t, a, "hello")
	raw, _ := os.ReadFile(res.SessionPath)
	if strings.Contains(string(raw), "plan_mode") {
		t.Fatalf("a normal session must not gain a new key:\n%s", raw)
	}
}

func submit(id, plan string) tcall {
	return tcall{id, "exit_plan_mode", `{"plan":"` + plan + `"}`}
}

func TestNonInteractivePlanPendingThenResumeWithApproval(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1),
			tcall{"c1", "enter_plan_mode", "{}"}, submit("c2", "1. edit a.txt"),
			tcall{"c3", "write_file", `{"path":"a.txt","content":"x"}`}),
		reply("done", "stop", use(1, 1)),
		reply("done again", "stop", use(1, 1)))
	opts := testOptions(t, f)
	withWrite(&opts)
	a, _ := New(opts)
	res := mustRun(t, a, "do it")
	if res.StopReason != StopPlanPending || res.Plan != "1. edit a.txt" || res.FinalOutput != "1. edit a.txt" {
		t.Fatalf("stop=%q plan=%q out=%q", res.StopReason, res.Plan, res.FinalOutput)
	}
	assertPaired(t, res.Transcript)
	if _, err := os.Stat(filepath.Join(opts.Runtime.CWD, "a.txt")); err == nil {
		t.Fatal("a call after the plan was submitted must not have run")
	}
	stored := mustLoad(t, res)
	if p := stored.Plan(); !p.Active() || p.Pending != "1. edit a.txt" {
		t.Fatalf("saved plan state %+v", p)
	}

	// A brand-new process resumes: still plan mode, tools still withheld.
	b, _ := New(opts)
	if _, err := b.Resume(t.Context(), "continue", stored); err != nil {
		t.Fatal(err)
	}
	if slices.Contains(reqToolNames(f.requests()[1]), "write_file") {
		t.Fatal("resume must preserve plan mode (write_file offered)")
	}
	stored = mustLoad(t, &Result{SessionPath: res.SessionPath})
	if !stored.Plan().Active() {
		t.Fatal("plan mode lost after resume")
	}

	// Operator approves from outside a turn (the --approve-plan path).
	c, _ := New(opts)
	c.AdoptPlan(stored)
	if _, err := c.DecidePlan(stored, PlanDecision{Action: PlanApprove}); err != nil {
		t.Fatal(err)
	}
	if _, err := c.Resume(t.Context(), "go", stored); err != nil {
		t.Fatal(err)
	}
	names := reqToolNames(f.requests()[2])
	if !slices.Contains(names, "write_file") {
		t.Fatalf("approval must restore tools: %v", names)
	}
	all := ""
	for _, m := range reqMessages(f.requests()[2]) {
		s, _ := m["content"].(string)
		all += s + "\n"
	}
	if !strings.Contains(all, "approved the plan") || !strings.Contains(all, "1. edit a.txt") {
		t.Fatalf("the approved plan must be in the next turn's context:\n%s", all)
	}
	p := mustLoad(t, &Result{SessionPath: res.SessionPath}).Plan()
	if p.Active() || p.Pending != "" || p.Note != "1. edit a.txt" {
		t.Fatalf("after approval: %+v", p)
	}
}

func TestModelCannotLeavePlanMode(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"c1", "enter_plan_mode", "{}"}),
		// still in plan mode: the model tries to enter again, to call a withheld tool, then submits
		reply("", "tool_calls", use(1, 1),
			tcall{"c2", "enter_plan_mode", "{}"},
			tcall{"c3", "write_file", `{"path":"x.txt","content":"x"}`},
			submit("c4", "plan")))
	opts := testOptions(t, f)
	withWrite(&opts)
	a, _ := New(opts)
	res := mustRun(t, a, "go")
	if res.StopReason != StopPlanPending {
		t.Fatalf("stop=%q", res.StopReason)
	}
	if _, err := os.Stat(filepath.Join(opts.Runtime.CWD, "x.txt")); err == nil {
		t.Fatal("write_file ran in plan mode")
	}
	if !a.PlanState().Active() || !mustLoad(t, res).Plan().Active() {
		t.Fatal("submitting a plan must not leave plan mode")
	}
	for _, m := range res.Transcript {
		if m.Role == "tool" && m.ToolCallID == "c3" && !strings.Contains(m.Content, "unknown tool") {
			t.Fatalf("withheld tool should be unknown, got %s", m.Content)
		}
	}
}

func TestExitPlanModeOutsidePlanModeIsRefused(t *testing.T) {
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), submit("c1", "sneaky")),
		reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f)
	res := mustRun(t, a, "go")
	if res.StopReason == StopPlanPending || a.PlanState().Active() {
		t.Fatal("exit_plan_mode must do nothing in normal mode")
	}
}

func TestInteractiveDecisions(t *testing.T) {
	cases := []struct {
		name     string
		d        PlanDecision
		wantMode bool // still in plan mode afterwards
		wantText string
		wantNote string
		write    bool // write_file offered on the next request
	}{
		{"approve", PlanDecision{Action: PlanApprove}, false, "approved the plan", "p1", true},
		{"revise", PlanDecision{Action: PlanRevise, Feedback: "add tests"}, true, "add tests", "", false},
		{"reject", PlanDecision{Action: PlanReject}, false, "rejected the plan", "", true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			f := newFakeServer(t,
				reply("", "tool_calls", use(1, 1), tcall{"c1", "enter_plan_mode", "{}"}),
				reply("", "tool_calls", use(1, 1), submit("c2", "p1")),
				reply("fin", "stop", use(1, 1)))
			a := newTestAgent(t, f, withWrite)
			var asked string
			a.SetPlanDecider(func(plan string) PlanDecision { asked = plan; return tc.d })
			res := mustRun(t, a, "go")
			if res.StopReason != "stop" || asked != "p1" {
				t.Fatalf("stop=%q asked=%q", res.StopReason, asked)
			}
			var toolText string
			for _, m := range res.Transcript {
				if m.ToolCallID == "c2" {
					toolText = m.Content
				}
			}
			if !strings.Contains(toolText, tc.wantText) {
				t.Fatalf("tool result %q lacks %q", toolText, tc.wantText)
			}
			names := reqToolNames(f.requests()[2])
			if slices.Contains(names, "write_file") != tc.write {
				t.Fatalf("write_file offered=%v, want %v", !tc.write, tc.write)
			}
			s := mustLoad(t, res).Plan()
			if s.Active() != tc.wantMode || s.Note != tc.wantNote || s.Pending != "" {
				t.Fatalf("state %+v", s)
			}
		})
	}
}

func TestEmptyAllowlistStaysEmptyThroughPlanMode(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f, func(o *Options) { o.Tools.Allowlist = []string{} })
	a.EnterPlan(nil)
	if a.ToolCount() != 0 {
		t.Fatalf("plan mode added tools to an empty allowlist: %v", a.Registry().Names())
	}
	a.AdoptPlan(&session.Session{PluginState: map[string]any{"plan_mode": map[string]any{"mode": "plan", "pending": "p"}}})
	if _, err := a.DecidePlan(nil, PlanDecision{Action: PlanApprove}); err != nil {
		t.Fatal(err)
	}
	if a.ToolCount() != 0 {
		t.Fatalf("approval added tools to an empty allowlist: %v", a.Registry().Names())
	}
	if err := a.rebuildTools(a.opts.Runtime.CWD); err != nil || a.ToolCount() != 0 {
		t.Fatalf("rebuild widened an empty allowlist: %v", a.Registry().Names())
	}
}

func TestAllowlistWinsOverPlanMode(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f, func(o *Options) {
		o.Tools.Allowlist = []string{"read_file", "write_file", "exit_plan_mode"}
		withWrite(o)
	})
	a.EnterPlan(nil)
	if got := a.Registry().Names(); !slices.Equal(got, []string{"read_file", "exit_plan_mode"}) {
		t.Fatalf("plan mode: %v", got)
	}
	a.AdoptPlan(&session.Session{PluginState: map[string]any{"plan_mode": map[string]any{"mode": "plan", "pending": "p"}}})
	a.DecidePlan(nil, PlanDecision{Action: PlanApprove})
	if got := a.Registry().Names(); !slices.Equal(got, []string{"read_file", "write_file"}) {
		t.Fatalf("normal mode: %v", got)
	}
}

func TestRebuildReappliesPlanMode(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f)
	a.EnterPlan(nil)
	if err := a.rebuildTools(a.opts.Runtime.CWD); err != nil {
		t.Fatal(err)
	}
	if _, ok := a.Registry().Get("write_file"); ok {
		t.Fatal("a registry rebuild undid plan mode")
	}
	if a.toolCtx.Registry != a.registry {
		t.Fatal("tool context points at a stale registry")
	}
}

func TestOperatorEntersPlanModeBeforeTheSessionExists(t *testing.T) {
	f := newFakeServer(t, reply("planning", "stop", use(1, 1)))
	a := newTestAgent(t, f, withWrite)
	a.EnterPlan(nil)
	res := mustRun(t, a, "go")
	if slices.Contains(reqToolNames(f.requests()[0]), "write_file") {
		t.Fatal("first request of a plan-mode session offered write_file")
	}
	if !mustLoad(t, res).Plan().Active() {
		t.Fatal("not saved")
	}
}

func TestDecisionsNeedPendingPlanAndPlanMode(t *testing.T) {
	f := newFakeServer(t)
	a := newTestAgent(t, f)
	if _, err := a.DecidePlan(nil, PlanDecision{Action: PlanApprove}); err == nil {
		t.Fatal("approve in normal mode must fail")
	}
	a.EnterPlan(nil)
	if _, err := a.DecidePlan(nil, PlanDecision{Action: PlanApprove}); err == nil {
		t.Fatal("approve without a submitted plan must fail")
	}
	if _, err := a.DecidePlan(nil, PlanDecision{Action: "maybe"}); err == nil {
		t.Fatal("unknown decision must fail")
	}
	if !a.PlanState().Active() {
		t.Fatal("failed decisions must not leave plan mode")
	}
	if _, err := a.DecidePlan(nil, PlanDecision{Action: PlanReject}); err != nil || a.PlanState().Active() {
		t.Fatal("operator can always reject out of plan mode")
	}
}

func TestNarrowToolsOnlyNarrowsAndEndsWithTheRun(t *testing.T) {
	f := newFakeServer(t, reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) { o.Tools.Allowlist = []string{"read_file", "list_dir"} })
	a.NarrowTools([]string{"read_file", "bash", "write_file"})
	if got := a.Registry().Names(); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("narrowing widened the allowlist: %v", got)
	}
	a.NarrowTools([]string{"list_dir", "read_file"})
	if got := a.Registry().Names(); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("second narrowing must intersect: %v", got)
	}
	mustRun(t, a, "x")
	if got := reqToolNames(f.requests()[0]); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("request tools %v", got)
	}
	if got := a.Registry().Names(); !slices.Equal(got, []string{"read_file", "list_dir"}) {
		t.Fatalf("narrowing must end with the run: %v", got)
	}
	a.NarrowTools([]string{})
	if a.ToolCount() != 0 {
		t.Fatal("an empty narrowing means no tools")
	}
}

func TestToolReportingAllowedToolsNarrowsTheRest(t *testing.T) {
	probe := tools.Tool{Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{Name: "probe", Description: "p", Parameters: map[string]any{"type": "object", "properties": map[string]any{}}},
		Handler: func(_ context.Context, _ map[string]any, _ *tools.Context) (string, map[string]any, error) {
			return "go", map[string]any{"skill_allowed_tools": []string{"read_file", "bash"}}, nil
		}}
	f := newFakeServer(t,
		reply("", "tool_calls", use(1, 1), tcall{"c1", "probe", "{}"}),
		reply("ok", "stop", use(1, 1)))
	a := newTestAgent(t, f, func(o *Options) {
		o.Tools.Extra = []tools.Tool{probe}
		o.Tools.Allowlist = []string{"probe", "read_file", "write_file"}
	})
	mustRun(t, a, "x")
	if got := reqToolNames(f.requests()[1]); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("after the skill-style call: %v", got)
	}
}
