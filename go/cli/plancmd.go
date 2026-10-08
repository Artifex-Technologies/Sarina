package cli

import (
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agent"
)

func init() {
	RegisterSlash("plan", "plan mode: /plan (enter), /plan show, /plan approve, /plan revise <feedback>, /plan reject", planSlash)
}

// planSlash is the operator's side of plan mode. The decisions here are the
// only way (besides the interactive prompt and the resume flags) out of it.
func planSlash(sc *SlashContext, args string) SlashResult {
	sub, rest, _ := strings.Cut(args, " ")
	rest = strings.TrimSpace(rest)
	st := sc.Agent.PlanState()
	switch strings.ToLower(sub) {
	case "", "on", "enter":
		if st.Active() {
			return SlashResult{Text: planStatus(st.Pending)}
		}
		sc.Agent.EnterPlan(sc.Current())
		return SlashResult{Text: "Plan mode is on: the model can only read and search until you approve a plan."}
	case "show", "status":
		if !st.Active() {
			return SlashResult{Text: "Plan mode is off."}
		}
		return SlashResult{Text: planStatus(st.Pending)}
	case "approve", "revise", "reject":
		d := agent.PlanDecision{Action: strings.ToLower(sub), Feedback: rest}
		if _, err := sc.Agent.DecidePlan(sc.Current(), d); err != nil {
			return SlashResult{Text: "Cannot " + d.Action + ": " + err.Error()}
		}
		switch d.Action {
		case agent.PlanApprove:
			return SlashResult{Text: "Plan approved. Plan mode is off; send a message to let the model proceed."}
		case agent.PlanRevise:
			return SlashResult{Text: "Feedback recorded. Plan mode stays on; send a message to let the model revise."}
		}
		return SlashResult{Text: "Plan rejected. Plan mode is off."}
	}
	return SlashResult{Text: fmt.Sprintf("Unknown /plan option %q. Use /plan, /plan show, /plan approve, /plan revise <feedback> or /plan reject.", sub)}
}

func planStatus(pending string) string {
	if pending == "" {
		return "Plan mode is on. No plan has been submitted yet."
	}
	return "Plan mode is on. Submitted plan awaiting your decision:\n" + pending
}

// askPlanDecision shows a submitted plan and reads the operator's decision from
// the interactive input. No answer (end of input, Ctrl-C) returns an empty
// decision, which leaves the plan pending.
func askPlanDecision(env *Env, lines <-chan string, plan string) agent.PlanDecision {
	fmt.Fprintf(env.Stdout, "\n--- plan awaiting your decision ---\n%s\n-----------------------------------\n", plan)
	read := func(prompt string) (string, bool) {
		fmt.Fprint(env.Stdout, prompt)
		select {
		case l, ok := <-lines:
			return strings.TrimSpace(l), ok
		case <-env.Interrupts:
			fmt.Fprintln(env.Stdout)
			return "", false
		}
	}
	for {
		ans, ok := read("approve, revise or reject? [a/r/j] ")
		if !ok {
			return agent.PlanDecision{}
		}
		switch strings.ToLower(ans) {
		case "a", "approve", "y", "yes":
			return agent.PlanDecision{Action: agent.PlanApprove}
		case "j", "reject", "n", "no":
			return agent.PlanDecision{Action: agent.PlanReject}
		case "r", "revise":
			fb, ok := read("what should change? ")
			if !ok {
				return agent.PlanDecision{}
			}
			return agent.PlanDecision{Action: agent.PlanRevise, Feedback: fb}
		}
	}
}
