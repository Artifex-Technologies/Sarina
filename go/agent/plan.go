package agent

import (
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// StopPlanPending ends a non-interactive run whose model submitted a plan: the
// plan is in Result.Plan (and FinalOutput) and the session file, and resuming
// with a decision continues.
const StopPlanPending = "plan_pending"

// Plan decisions the operator can make.
const (
	PlanApprove = "approve"
	PlanRevise  = "revise"
	PlanReject  = "reject"
)

// PlanDecision is the operator's answer to a submitted plan. An empty Action
// means "no answer now": the run stops with plan_pending instead.
type PlanDecision struct {
	Action   string
	Feedback string // for revise
}

// PlanDecider asks the operator about a submitted plan (interactive runs).
type PlanDecider func(plan string) PlanDecision

// SetPlanDecider installs the interactive approval prompt. With none, a
// submitted plan ends the run with StopPlanPending.
func (a *Agent) SetPlanDecider(d PlanDecider) { a.decider = d }

// PlanState is the current plan-mode state.
func (a *Agent) PlanState() session.PlanState { return a.plan }

// NarrowTools restricts the registry to the listed names until the current or
// next run ends. It can only narrow: names outside the allowlist grant
// nothing, and repeated calls intersect. nil is ignored; an empty non-nil
// list means no tools.
func (a *Agent) NarrowTools(names []string) {
	if names == nil {
		return
	}
	if a.narrow == nil {
		a.narrow = append([]string{}, names...)
	} else {
		keep := map[string]bool{}
		for _, n := range names {
			keep[n] = true
		}
		next := []string{}
		for _, n := range a.narrow {
			if keep[n] {
				next = append(next, n)
			}
		}
		a.narrow = next
	}
	a.refreshRegistry()
}

func (a *Agent) effectiveBuilder() tools.Builder {
	b := a.opts.Tools
	b.PlanMode = b.PlanMode || a.plan.Active()
	perms := a.opts.Runtime.Permissions
	b.Perms = &perms
	// A narrowing fixed in the options (a child agent's tool set) stays; the
	// per-run one (skills) can only narrow it further.
	switch {
	case b.Narrow == nil:
		b.Narrow = a.narrow
	case a.narrow != nil:
		var both []string
		for _, n := range b.Narrow {
			if inList(a.narrow, n) {
				both = append(both, n)
			}
		}
		b.Narrow = append([]string{}, both...)
	}
	return b
}

// refreshRegistry re-derives the registry from the builder (allowlist first,
// then narrowing, then plan mode), keeping the rest of the tool context.
func (a *Agent) refreshRegistry() {
	a.registry = a.effectiveBuilder().Build()
	if a.toolCtx != nil {
		a.toolCtx.Registry = a.registry
	}
}

// baseToolNames lists the tools of the allowlist alone, for the system prompt,
// which must not change with a passing plan mode or skill restriction.
func (a *Agent) baseToolNames() []string {
	b := a.opts.Tools
	perms := a.opts.Runtime.Permissions
	b.Perms = &perms
	return b.Build().Names()
}

func (a *Agent) endRun() {
	if a.narrow != nil {
		a.narrow = nil
		a.refreshRegistry()
	}
}

// setPlan installs plan state on the agent and, when given, the session.
func (a *Agent) setPlan(p session.PlanState, s *session.Session) {
	if p.Mode == "" {
		p.Mode = session.PlanNormal
	}
	a.plan = p
	if s != nil {
		s.SetPlan(p)
	}
	a.refreshRegistry()
}

const planModeNotice = "Plan mode is on. You can only read and search. Investigate, then submit your plan with exit_plan_mode; " +
	"nothing is changed until the operator approves it."

func reminder(text string) string {
	return session.ReminderOpen + "\n" + text + "\n" + session.ReminderClose
}

// EnterPlan turns plan mode on at the operator's request. With a session, the
// model is told in the transcript; without one, the next session starts in it.
func (a *Agent) EnterPlan(s *session.Session) {
	if a.plan.Active() {
		return
	}
	p := a.plan
	p.Mode, p.Pending = session.PlanActive, ""
	a.setPlan(p, s)
	if s != nil {
		s.Append(session.Message{Role: "user", Content: reminder(planModeNotice), MessageID: s.NextMessageID("plan_mode"),
			Metadata: map[string]any{"kind": "plan_mode_entered"}})
	}
}

// applyDecision performs the state change for an operator decision and returns
// the text the model is told. It is the only way back to normal mode.
func (a *Agent) applyDecision(s *session.Session, d PlanDecision) (string, error) {
	p := a.plan
	if !p.Active() {
		return "", fmt.Errorf("plan mode is not on")
	}
	switch d.Action {
	case PlanApprove:
		if p.Pending == "" {
			return "", fmt.Errorf("there is no submitted plan to approve")
		}
		plan := p.Pending
		p.Mode, p.Pending, p.Note = session.PlanNormal, "", plan
		a.setPlan(p, s)
		return "The operator approved the plan. Plan mode is off and your normal tools are available again. Approved plan:\n" + plan, nil
	case PlanRevise:
		if p.Pending == "" {
			return "", fmt.Errorf("there is no submitted plan to revise")
		}
		p.Pending = ""
		a.setPlan(p, s)
		fb := strings.TrimSpace(d.Feedback)
		if fb == "" {
			fb = "(no details given)"
		}
		return "The operator did not approve the plan and asks for changes:\n" + fb +
			"\nPlan mode is still on. Revise the plan and submit it again with exit_plan_mode.", nil
	case PlanReject:
		p.Mode, p.Pending = session.PlanNormal, ""
		a.setPlan(p, s)
		return "The operator rejected the plan. Plan mode is off and the plan is discarded; do not carry it out.", nil
	}
	return "", fmt.Errorf("unknown plan decision %q (want approve, revise or reject)", d.Action)
}

// DecidePlan applies the operator's decision outside a running turn (the
// /plan command, a resume flag). When s is non-nil the model is told in the
// transcript, so the next turn sees it.
func (a *Agent) DecidePlan(s *session.Session, d PlanDecision) (string, error) {
	text, err := a.applyDecision(s, d)
	if err != nil {
		return "", err
	}
	if s != nil {
		s.Append(session.Message{Role: "user", Content: reminder(text), MessageID: s.NextMessageID("plan_decision"),
			Metadata: map[string]any{"kind": "plan_decision", "decision": d.Action}})
	}
	return text, nil
}

// onPlanAction handles a plan_action reported by enter/exit_plan_mode. It may
// rewrite the tool result (so the model reads the operator's answer) and
// returns a plan that is now awaiting approval and must end the run.
func (a *Agent) onPlanAction(r *run, action string, planArg any, res *tools.Result) (pending string) {
	switch action {
	case "enter":
		p := a.plan
		p.Mode, p.Pending = session.PlanActive, ""
		a.setPlan(p, r.sess)
	case "exit":
		plan, _ := planArg.(string)
		p := a.plan
		p.Pending = plan
		a.setPlan(p, r.sess)
		if a.decider != nil {
			d := a.decider(plan)
			if d.Action != "" {
				if text, err := a.applyDecision(r.sess, d); err == nil {
					res.Content = text
					res.Metadata["plan_decision"] = d.Action
					return ""
				}
			}
		}
		res.Content = "Plan submitted. It is waiting for the operator's decision; plan mode stays on until then."
		return plan
	}
	return ""
}

// AdoptPlan loads a stored session's plan state into the agent, so a decision
// made before Resume applies to the right state. Resume does this itself.
func (a *Agent) AdoptPlan(s *session.Session) { a.setPlan(s.Plan(), nil) }
