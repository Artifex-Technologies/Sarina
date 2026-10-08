package tools

import (
	"context"
	"fmt"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Class is a tool's plan-mode classification.
type Class int

const (
	// ClassUnset: no decision was made. Treated as mutating.
	ClassUnset Class = iota
	// ClassReadOnly: changes nothing; offered in plan mode.
	ClassReadOnly
	// ClassMutating: changes files, state or processes; withheld in plan mode.
	ClassMutating
	// ClassPlanControl: the plan-mode switches. enter_plan_mode is offered only
	// outside plan mode, exit_plan_mode only inside it.
	ClassPlanControl
)

// Names and limits of the plan-mode tools.
const (
	ToolEnterPlanMode = "enter_plan_mode"
	ToolExitPlanMode  = "exit_plan_mode"
	// MaxPlanChars bounds the plan text the model may submit.
	MaxPlanChars = 20000
	// RuntimePlanActive is the Context.Runtimes key under which the agent
	// installs a func() bool reporting whether plan mode is on.
	RuntimePlanActive = "plan_mode_active"
)

// ForMode returns the registry as the model should see it. In plan mode only
// read-only tools and exit_plan_mode remain; outside it exit_plan_mode is
// dropped (there is nothing to submit). It never adds a tool.
func (r *Registry) ForMode(plan bool) *Registry {
	out := NewRegistry()
	for _, n := range r.order {
		t := r.tools[n]
		keep := false
		switch {
		case n == ToolExitPlanMode:
			keep = plan
		case plan:
			keep = t.Class == ClassReadOnly
		default:
			keep = true
		}
		if keep {
			out.Register(t)
		}
	}
	return out
}

func planActive(tc *Context) (active, known bool) {
	if tc == nil {
		return false, false
	}
	f, ok := tc.Runtimes[RuntimePlanActive].(func() bool)
	if !ok || f == nil {
		return false, false
	}
	return f(), true
}

// PlanTools returns enter_plan_mode and exit_plan_mode. The handlers only
// request the transition (metadata plan_action); the agent owns the state, and
// exit_plan_mode never leaves plan mode itself: it submits the plan for the
// operator's decision.
func PlanTools() []Tool {
	return []Tool{
		{
			Class: ClassPlanControl,
			Spec: llm.ToolSpec{
				Name:        ToolEnterPlanMode,
				Description: "Switch to read-only plan mode to propose a plan for approval.",
				Parameters:  obj(map[string]any{}),
			},
			Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
				if active, known := planActive(tc); known && active {
					return "", nil, fmt.Errorf("already in plan mode")
				}
				return "Plan mode is on. Only read-only tools are available. Investigate, then call " + ToolExitPlanMode +
						" with your plan; the operator decides whether to approve it.",
					map[string]any{"plan_action": "enter"}, nil
			},
		},
		{
			Class: ClassPlanControl,
			Spec: llm.ToolSpec{
				Name:        ToolExitPlanMode,
				Description: "Submit your plan for the operator to approve, revise or reject.",
				Parameters: obj(map[string]any{
					"plan": prop("string", fmt.Sprintf("Plain text, at most %d characters.", MaxPlanChars), "minLength", 1),
				}, "plan"),
			},
			Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
				if active, known := planActive(tc); known && !active {
					return "", nil, fmt.Errorf("not in plan mode, so there is no plan to submit")
				}
				plan := argStr(args, "plan")
				if n := len([]rune(plan)); n > MaxPlanChars {
					return "", nil, fmt.Errorf("the plan is %d characters; the limit is %d. Shorten it and submit again", n, MaxPlanChars)
				}
				return "Plan submitted to the operator.", map[string]any{"plan_action": "exit", "plan_chars": len([]rune(plan))}, nil
			},
		},
	}
}
