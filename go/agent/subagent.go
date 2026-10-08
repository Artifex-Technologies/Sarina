package agent

import (
	"context"
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// ToolAgent is the model-facing sub-agent tool.
const ToolAgent = "agent"

// SubagentStateKey is where a child session records its parent, inside the
// session file's existing plugin_state object:
// {"parent_session_id", "parent_tool_call_id", "profile", "depth"}.
const SubagentStateKey = "subagent"

const (
	defaultChildTurns     = 6
	defaultChildToolCalls = 40
)

// childDenied are tools a child never gets, whatever its profile says: it may
// not touch the parent's to-do list or plan, switch plan mode, run skills
// (which narrow the registry) or start another agent.
func childDenied(name string) bool {
	switch name {
	case ToolAgent, "skill", "TaskOutput", "TaskStop", "todo_write", "update_plan", "plan_get",
		tools.ToolEnterPlanMode, tools.ToolExitPlanMode:
		return true
	}
	return strings.HasPrefix(name, "task_")
}

// agentTool builds the `agent` tool for this (top-level) agent.
func (a *Agent) agentTool() tools.Tool {
	cat := a.opts.Subagents
	var b strings.Builder
	b.WriteString("Delegate a separate, self-contained investigation to a read-only helper that returns one report. " +
		"Do not use it for a single read or search; call read_file, grep_search or glob_search directly. " +
		"The helper does not see this conversation. Profiles:\n")
	for _, p := range cat.Profiles {
		fmt.Fprintf(&b, "- %s: %s\n", p.Name, p.Description)
	}
	profiles := make([]any, 0, len(cat.Profiles))
	for _, p := range cat.Profiles {
		profiles = append(profiles, p.Name)
	}
	return tools.Tool{
		Class: tools.ClassReadOnly, // children are read-only and can only use what the parent has
		Spec: llm.ToolSpec{
			Name:        ToolAgent,
			Description: strings.TrimRight(b.String(), "\n"),
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"profile":     map[string]any{"type": "string", "enum": profiles},
					"prompt":      map[string]any{"type": "string", "description": "Complete task, with all context.", "minLength": 1},
					"description": map[string]any{"type": "string", "description": "Short label."},
					"max_turns": map[string]any{"type": "integer", "description": fmt.Sprintf("1 to %d.", agents.MaxTurnsLimit),
						"minimum": 1, "maximum": agents.MaxTurnsLimit},
				},
				"required": []any{"profile", "prompt"},
			},
		},
		Handler: func(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
			return a.runSubagent(ctx, args, tc)
		},
	}
}

// childTools is the tool set a child may use: the parent's CURRENT registry
// (so the allowlist, a skill's narrowing, plan mode and policy denials are all
// already applied; an empty allowlist leaves an empty list), cut down to what
// the profile asks for, to read-only tools, and, for bash, to the case where
// both the profile and the parent allow shell commands. It never adds a name.
func (a *Agent) childTools(p *agents.Profile) []string {
	out := []string{}
	reg := a.registry
	for _, n := range reg.Names() {
		if childDenied(n) {
			continue
		}
		if p.Tools != nil && !inList(p.Tools, n) {
			continue
		}
		t, _ := reg.Get(n)
		switch {
		case t.Class == tools.ClassReadOnly:
		case n == "bash" && p.Shell && a.opts.Runtime.Permissions.AllowShellCommands:
		default:
			continue
		}
		out = append(out, n)
	}
	return out
}

func inList(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// childBudget gives the child its own limits: the parent's remaining
// allowance where the parent has a limit, a fixed tool-call cap, the turn
// limit, and no delegation.
func (a *Agent) childBudget(turns int) config.BudgetConfig {
	b := a.opts.Runtime.BudgetConfig
	out := config.BudgetConfig{MaxInputTokens: b.MaxInputTokens, MaxOutputTokens: b.MaxOutputTokens}
	var used llm.Usage
	var calls counters
	if a.cur != nil {
		used, calls = a.cur.usage, a.cur.counts
	}
	left := func(limit *int, have int) *int {
		if limit == nil {
			return nil
		}
		v := max(*limit-have, 0)
		return &v
	}
	out.MaxTotalTokens = left(b.MaxTotalTokens, used.Total())
	out.MaxReasoningTokens = left(b.MaxReasoningTokens, used.ReasoningTokens)
	out.MaxModelCalls = left(b.MaxModelCalls, calls.ModelCalls)
	if b.MaxTotalCostUSD != nil {
		v := max(*b.MaxTotalCostUSD-Cost(a.opts.Model.Pricing, used), 0)
		out.MaxTotalCostUSD = &v
	}
	tc := defaultChildToolCalls
	if b.MaxToolCalls != nil {
		tc = min(tc, max(*b.MaxToolCalls-calls.ToolCalls, 0))
	}
	out.MaxToolCalls = &tc
	out.MaxSessionTurns = &turns
	zero := 0
	out.MaxDelegatedTasks = &zero
	return out
}

func (a *Agent) runSubagent(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
	name, _ := args["profile"].(string)
	prompt, _ := args["prompt"].(string)
	label, _ := args["description"].(string)
	p, ok := a.opts.Subagents.Get(name)
	if !ok {
		return "", nil, fmt.Errorf("no agent profile named %q; available: %s", name, strings.Join(a.opts.Subagents.Names(), ", "))
	}
	if strings.TrimSpace(prompt) == "" {
		return "", nil, fmt.Errorf("prompt must not be empty")
	}
	rt := a.opts.Runtime
	turns := min(defaultChildTurns, rt.MaxTurns)
	if p.MaxTurns > 0 {
		turns = p.MaxTurns
	}
	if n, ok := config.AsInt(args["max_turns"]); ok && n > 0 {
		turns = n
	}
	turns = max(min(turns, agents.MaxTurnsLimit, rt.MaxTurns), 1)

	// The child can only be weaker: no writes, no destructive shell, shell only
	// when the parent has it and the profile asks for it.
	rt.MaxTurns = turns
	rt.Permissions = config.Permissions{AllowShellCommands: rt.Permissions.AllowShellCommands && p.Shell}
	rt.DisableClaudeMdDiscovery = rt.DisableClaudeMdDiscovery || !p.ProjectContext
	rt.StreamModelResponses = false
	rt.OutputSchema = nil
	rt.BudgetConfig = a.childBudget(turns)
	cm := config.ContextConfig{DisableNotes: true, DisableLLMCompaction: true, DisableDiscovery: true}
	if rt.ContextManagement != nil {
		cm.DisableClearing = rt.ContextManagement.DisableClearing
	}
	rt.ContextManagement = &cm

	builder := a.opts.Tools
	builder.Narrow = a.childTools(p)
	builder.PlanMode = a.plan.Active()
	parentID, callID := "", a.curCall
	if a.cur != nil {
		parentID = a.cur.sess.ID
	}
	link := map[string]any{"parent_session_id": parentID, "parent_tool_call_id": callID, "profile": p.Name, "depth": 1}
	childPrompt := p.Prompt + "\n\nWorking directory: " + rt.CWD + "\nYour final message is returned to the agent that started you."

	child, err := New(Options{
		Model: a.opts.Model, Runtime: rt, Client: a.opts.Client, Tools: builder,
		Prompt: PromptOptions{Override: childPrompt}, ContextWindow: a.opts.ContextWindow,
		Now: a.opts.Now, Home: a.opts.Home, Getenv: a.opts.Getenv, Discoverer: a.opts.Discoverer,
		Scanner: a.opts.Scanner, SessionEnv: a.opts.SessionEnv, ParentLink: link,
	})
	if err != nil {
		return "", nil, fmt.Errorf("cannot start the %s helper: %v", p.Name, err)
	}
	if a.cur != nil {
		a.cur.emit(event{"type": "subagent_start", "profile": p.Name, "label": label, "tools": child.registry.Names(), "max_turns": turns})
	}
	res, runErr := child.Run(ctx, prompt)
	if res == nil {
		return "", nil, fmt.Errorf("the %s helper failed: %v", p.Name, runErr)
	}
	a.childUsage, a.childUsed = res.Usage, true
	if a.cur != nil {
		a.cur.emit(event{"type": "subagent_end", "profile": p.Name, "child_session_id": res.SessionID,
			"stop_reason": res.StopReason, "turns": res.Turns, "tool_calls": res.ToolCalls})
	}
	md := map[string]any{
		"delegation": true, "child_session_id": res.SessionID, "subagent_profile": p.Name,
		"child_stop_reason": res.StopReason, "child_turns": res.Turns, "child_tool_calls": res.ToolCalls,
	}
	switch res.StopReason {
	case StopBackendError, StopPromptTooLong:
		return "", md, fmt.Errorf("the %s helper stopped early (%s): %s", p.Name, res.StopReason, res.FinalOutput)
	}
	limit := 12000
	if tc != nil && tc.MaxOutputChars > 0 {
		limit = tc.MaxOutputChars
	}
	out := res.FinalOutput
	if r := []rune(out); len(r) > limit {
		out = string(r[:limit]) + "\n...[helper report truncated]"
	}
	header := fmt.Sprintf("%s helper finished (stop: %s, turns: %d, tool calls: %d, session: %s)", p.Name, res.StopReason, res.Turns, res.ToolCalls, res.SessionID)
	if strings.TrimSpace(out) == "" {
		out = "(the helper returned no text)"
	}
	return header + "\n\n" + out, md, nil
}
