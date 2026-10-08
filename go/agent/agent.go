// Package agent implements the turn loop: model calls, tool dispatch, budgets,
// context-pressure management, persistence and resume.
package agent

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/skills"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Stop reasons produced by the loop itself. When the model answers without
// tool calls, StopReason is the backend's finish reason instead.
const (
	StopMaxTurns       = "max_turns"
	StopBudgetExceeded = "budget_exceeded"
	StopBackendError   = "backend_error"
	StopPromptTooLong  = "prompt_too_long"
	StopInterrupted    = "interrupted"
)

// Completer is the model transport (llm.Client satisfies it).
type Completer interface {
	Complete(ctx context.Context, req llm.Request, on func(llm.StreamEvent)) (*llm.Turn, error)
}

// Options configures an Agent.
type Options struct {
	Model         config.ModelConfig
	Runtime       config.RuntimeConfig
	Client        Completer // default: llm.NewClient(Model)
	Tools         tools.Builder
	Prompt        PromptOptions
	ContextWindow int // 0 = SARINA_CONTEXT_WINDOW, else inferred from the model name
	Now           func() time.Time
	Home          string               // for the global CLAUDE.md; default: user home
	OnEvent       func(map[string]any) // live event hook (optional)
	Getenv        func(string) string  // default os.Getenv

	// Discoverer asks the backend for its context window (nil = never ask).
	// The CLI installs llm.DefaultDiscoverer; tests inject fakes or leave it nil.
	Discoverer WindowDiscoverer
	// Scanner vets session notes and summaries before they are re-injected after
	// compaction (default skills.BasicScanner).
	Scanner skills.Scanner
	// Tiers, when set, is told when each turn starts and ends so it can
	// de-escalate, show escalations and record them in the session file.
	Tiers TierTracker

	// Subagents, when set, gives the model an `agent` tool that runs one of
	// these read-only profiles as a bounded child agent. Child agents never get
	// the tool themselves (depth limit 1).
	Subagents *agents.Catalog
	// SessionEnv are operator-chosen variables for tool subprocesses (the
	// policy's safeEnv). Applied after the secret filter, never echoed.
	SessionEnv map[string]string
	// ParentLink marks this agent as a child: it is written to the session
	// file's plugin_state.subagent and turns the agent tool off.
	ParentLink map[string]any
	// Approver, when set, is asked before each state-changing tool call (see
	// tools.Approver). Interactive front ends use it; nil asks nothing.
	Approver tools.Approver
}

// Agent runs prompts against a model. It is reusable across turns of a chat via Resume.
type Agent struct {
	opts     Options
	registry *tools.Registry
	toolCtx  *tools.Context

	plan    session.PlanState // plan mode; persisted in the session file
	decider PlanDecider       // interactive plan approval; nil = stop with plan_pending
	narrow  []string          // per-run tool narrowing (skills); nil = none

	cur        *run      // the run in progress (the agent tool reads it)
	curCall    string    // id of the tool call being executed
	childUsage llm.Usage // usage of sub-agents run by the current tool call
	childUsed  bool      // childUsage holds something to add to the run

	ctxSettings config.ContextSettings // notes, clearing, LLM compaction, discovery

	// CumulativeUsage and CumulativeCostUSD add up every run/resume on this agent.
	CumulativeUsage   llm.Usage
	CumulativeCostUSD float64
}

// Result is the outcome of one Run or Resume call.
type Result struct {
	FinalOutput         string
	Turns               int // turns used in THIS call
	ToolCalls           int // cumulative over the whole session
	Transcript          []session.Message
	Events              []map[string]any
	Usage               llm.Usage // cumulative over the whole session
	TotalCostUSD        float64
	StopReason          string // "" = the backend gave no finish reason
	FileHistory         []map[string]any
	SessionID           string
	SessionPath         string
	ScratchpadDirectory string
	Plan                string // with StopPlanPending: the plan awaiting the operator's decision
}

// New builds an agent.
func New(opts Options) (*Agent, error) {
	if opts.Runtime.CWD == "" {
		return nil, errors.New("agent: runtime cwd is required")
	}
	if opts.Now == nil {
		opts.Now = time.Now
	}
	if opts.Getenv == nil {
		opts.Getenv = os.Getenv
	}
	if opts.Home == "" {
		opts.Home, _ = os.UserHomeDir()
	}
	if opts.Client == nil {
		opts.Client = llm.NewClient(opts.Model)
	}
	a := &Agent{opts: opts, ctxSettings: opts.Runtime.ContextManagement.Settings(opts.Getenv)}
	if opts.Subagents != nil && opts.ParentLink == nil {
		a.opts.Tools.Extra = append(append([]tools.Tool(nil), opts.Tools.Extra...), a.agentTool())
	}
	if err := a.rebuildTools(opts.Runtime.CWD); err != nil {
		return nil, err
	}
	return a, nil
}

// SetEventHook installs a live event listener (used for streaming output).
func (a *Agent) SetEventHook(f func(map[string]any)) { a.opts.OnEvent = f }

// ToolCount is the number of tools the model is offered.
func (a *Agent) ToolCount() int { return a.registry.Len() }

// Registry exposes the active registry.
func (a *Agent) Registry() *tools.Registry { return a.registry }

// rebuildTools (re)creates the registry and tool context for a workspace. The
// builder re-applies the allowlist every time.
func (a *Agent) rebuildTools(cwd string) error {
	abs, err := filepath.Abs(cwd)
	if err != nil {
		return err
	}
	if real, err := filepath.EvalSymlinks(abs); err == nil {
		abs = real
	}
	a.opts.Runtime.CWD = abs
	a.toolCtx = &tools.Context{
		Root:             abs,
		CommandTimeout:   time.Duration(a.opts.Runtime.CommandTimeoutSeconds * float64(time.Second)),
		MaxOutputChars:   a.opts.Runtime.MaxOutputChars,
		Permissions:      a.opts.Runtime.Permissions,
		Registry:         a.registry,
		SessionDirectory: a.opts.Runtime.SessionDirectory,
		Runtimes:         map[string]any{tools.RuntimePlanActive: func() bool { return a.plan.Active() }},
		SessionEnv:       a.opts.SessionEnv,
		Approver:         a.opts.Approver,
	}
	a.refreshRegistry()
	return nil
}

func (a *Agent) window() int {
	n, _ := a.resolveWindow()
	return n
}

// Run starts a new session with prompt.
func (a *Agent) Run(ctx context.Context, prompt string) (*Result, error) {
	a.refreshRegistry()
	sess, err := a.newSessionFor(prompt)
	if err != nil {
		a.endRun()
		return nil, err
	}
	return a.drive(ctx, sess, prompt, false)
}

// Resume continues a stored session with a new prompt. The stored prompt and
// context are reused as-is; counters carry over from the stored file.
func (a *Agent) Resume(ctx context.Context, prompt string, stored *session.Session) (*Result, error) {
	if stored == nil {
		return nil, errors.New("agent: resume needs a stored session")
	}
	stored.MigrateRetiredTools() // idempotent; a model must never see a retired tool name
	stored.Messages = session.RepairPairing(stored.Messages)
	a.setPlan(stored.Plan(), nil) // plan state comes from the file, not from this process
	a.appendReplays(stored)
	if stored.ScratchpadDirectory == "" {
		dir := filepath.Join(a.opts.Runtime.ScratchpadRoot, stored.ID)
		if os.MkdirAll(dir, 0o755) == nil {
			stored.ScratchpadDirectory = dir
		}
	}
	return a.drive(ctx, stored, prompt, true)
}

func (a *Agent) newSession() (*session.Session, error) { return a.newSessionFor("") }

// newSessionFor starts a session; prompt is the real first user prompt, used to
// pick which curated-memory entries are worth injecting.
func (a *Agent) newSessionFor(prompt string) (*session.Session, error) {
	rt := a.opts.Runtime
	s := &session.Session{ID: session.NewID(), Model: a.opts.Model, Runtime: rt}
	scratch := filepath.Join(rt.ScratchpadRoot, s.ID)
	if err := os.MkdirAll(scratch, 0o755); err != nil {
		return nil, fmt.Errorf("cannot create scratchpad: %w", err)
	}
	s.ScratchpadDirectory = scratch

	var userCtx, sysCtx session.OrderedMap
	userCtx.Set("currentDate", "Today's date is "+a.opts.Now().Format("2006-01-02")+".")
	userCtx.Set("scratchpad", "Scratch space for temporary working files you create yourself (use it instead of /tmp): "+scratch+". Files the user asks for belong in the workspace, not here.")
	if !rt.DisableClaudeMdDiscovery {
		if g := discoverGuidance(rt.CWD, rt.AdditionalWorkingDirectories, a.opts.Home); g != "" {
			userCtx.Set("claudeMd", g)
		}
		if block := curatedMemory(rt.CWD, a.opts.Home, prompt); block != "" {
			userCtx.Set("curatedMemory", block)
		}
	}
	if gs := gitSnapshot(rt.CWD); gs != "" {
		sysCtx.Set("gitStatus", gs)
	}
	sysCtx.Set("scratchpadDirectory", scratch)
	s.UserContext, s.SystemContext = userCtx, sysCtx

	env := envInfo{CWD: rt.CWD, IsGit: isGitRepo(rt.CWD), ExtraDirs: rt.AdditionalWorkingDirectories,
		Scratchpad: scratch, Model: a.opts.Model.Model}
	s.SystemPromptParts = BuildSystemPromptParts(a.opts.Prompt, env, a.baseToolNames(), rt.Permissions)

	s.SetPlan(a.plan)
	if a.opts.ParentLink != nil {
		if s.PluginState == nil {
			s.PluginState = map[string]any{}
		}
		s.PluginState[SubagentStateKey] = a.opts.ParentLink
	}
	s.Append(session.Message{Role: "system", Content: renderSystemMessage(s.SystemPromptParts, sysCtx), MessageID: "system_0"})
	if len(userCtx) > 0 {
		s.Append(session.Message{Role: "user", Content: renderUserContext(userCtx), MessageID: "user_context_0"})
	}
	if a.plan.Active() {
		s.Append(session.Message{Role: "user", Content: reminder(planModeNotice), MessageID: s.NextMessageID("plan_mode"),
			Metadata: map[string]any{"kind": "plan_mode_entered"}})
	}
	return s, nil
}

// appendReplays adds the file-history and compaction replay reminders on resume.
func (a *Agent) appendReplays(s *session.Session) {
	if n := len(s.FileHistory); n > 0 && !replayCountMatches(s.Messages, n) {
		paths := map[string]bool{}
		var order []string
		snaps := 0
		for _, e := range s.FileHistory {
			if p, ok := e["path"].(string); ok && !paths[p] {
				paths[p] = true
				order = append(order, p)
			}
			if _, ok := e["after_snapshot_id"]; ok {
				snaps++
			}
		}
		var b strings.Builder
		b.WriteString(session.ReminderOpen + "\n")
		fmt.Fprintf(&b, "This session already has %d recorded tool actions touching %d distinct paths.\n", n, len(order))
		if len(order) > 0 {
			shown := order
			if len(shown) > 4 {
				shown = shown[:4]
			}
			b.WriteString("Paths: " + strings.Join(shown, ", "))
			if len(order) > 4 {
				fmt.Fprintf(&b, " +%d more", len(order)-4)
			}
			b.WriteString("\n")
		}
		start := max(n-10, 0)
		for _, e := range s.FileHistory[start:] {
			fmt.Fprintf(&b, "- %v #%v turn %v path=%v command=%v\n", firstNonNil(e["tool_name"], e["action"]),
				e["history_entry_id"], e["turn_index"], e["path"], e["command"])
		}
		if start > 0 {
			fmt.Fprintf(&b, "(%d older entries omitted)\n", start)
		}
		b.WriteString("Use this record to avoid repeating edits or commands that already happened.\n")
		b.WriteString(session.ReminderClose)
		s.Append(session.Message{Role: "user", Content: b.String(), MessageID: fmt.Sprintf("file_history_replay_%d", n),
			Metadata: map[string]any{"kind": session.KindFileHistoryReplay, "file_history_count": n,
				"file_history_unique_paths": len(order), "file_history_snapshot_count": snaps}})
	}
	boundaries, snipped := 0, 0
	hasReplay := false
	var latest *session.Message
	for i := range s.Messages {
		switch s.Messages[i].Kind() {
		case session.KindCompactBoundary:
			boundaries++
			latest = &s.Messages[i]
		case session.KindSnipped:
			snipped++
		case session.KindCompactionReplay:
			hasReplay = true
		}
	}
	if (boundaries > 0 || snipped > 0) && !hasReplay {
		var b strings.Builder
		b.WriteString(session.ReminderOpen + "\n")
		fmt.Fprintf(&b, "Earlier history in this session was reduced: %d compaction boundaries, %d snipped messages.\n", boundaries, snipped)
		if latest != nil {
			fmt.Fprintf(&b, "Latest boundary: %s (depth %v).\n", latest.MessageID, latest.Metadata["compaction_depth"])
		}
		b.WriteString("Treat the surviving transcript and the summaries in it as authoritative.\n")
		b.WriteString(session.ReminderClose)
		s.Append(session.Message{Role: "user", Content: b.String(),
			MessageID: fmt.Sprintf("compaction_replay_%d_%d", boundaries, snipped),
			Metadata:  map[string]any{"kind": session.KindCompactionReplay}})
	}
}

func firstNonNil(vs ...any) any {
	for _, v := range vs {
		if v != nil {
			return v
		}
	}
	return ""
}

func replayCountMatches(msgs []session.Message, n int) bool {
	for i := len(msgs) - 1; i >= 0; i-- {
		if msgs[i].Kind() == session.KindFileHistoryReplay {
			c, _ := config.AsInt(msgs[i].Metadata["file_history_count"])
			return c == n
		}
	}
	return false
}

// ---- the run ---------------------------------------------------------------

type run struct {
	a        *Agent
	sess     *session.Session
	events   []map[string]any
	usage    llm.Usage
	cost     float64
	counts   counters // ToolCalls and ModelCalls are cumulative; SessionTurns = baseTurns + turnsRun
	baseTurn int
	turnsRun int
	esc      int
	done     map[string]bool
	segments []string
	lastText string

	// toolTurnsSince counts tool-using turns since the notes were last refreshed.
	toolTurnsSince int
	// compactFailures is the circuit breaker's count of consecutive failed
	// summarising compactions. It lives on the run, so every Run/Resume starts
	// with the breaker closed (the old design never reset it).
	compactFailures int
}

func (r *run) emit(e map[string]any) {
	r.events = append(r.events, e)
	if r.a.opts.OnEvent != nil {
		r.a.opts.OnEvent(e)
	}
}

func (r *run) budgetText() string {
	r.counts.Usage, r.counts.Cost = r.usage, r.cost
	r.counts.SessionTurns = r.baseTurn + r.turnsRun
	return checkBudget(r.a.opts.Runtime.BudgetConfig, r.counts)
}

func (a *Agent) drive(ctx context.Context, sess *session.Session, prompt string, resumed bool) (*Result, error) {
	rt := a.opts.Runtime
	sess.Model, sess.Runtime = a.opts.Model, rt
	r := &run{a: a, sess: sess, done: map[string]bool{}}
	a.cur = r
	defer func() { a.cur = nil }()
	defer a.endRun()
	a.beginTierTurn(r)
	a.refreshRegistry()
	if resumed {
		r.usage = sess.Usage
		r.counts.ToolCalls = sess.ToolCalls
		r.counts.ModelCalls = sess.BudgetState.ModelCalls
		r.esc = sess.BudgetState.Escalations
		r.baseTurn = sess.Turns
		for _, e := range sess.FileHistory {
			if e["history_kind"] == "delegation" {
				r.counts.Delegated++
			}
		}
	}
	r.cost = Cost(a.opts.Model.Pricing, r.usage)
	startUsage, startCost := r.usage, r.cost
	if n, src := a.resolveWindow(); strings.HasPrefix(src, "backend:") {
		r.emit(event{"type": "context_window", "tokens": n, "source": src})
	}

	sess.Append(session.Message{Role: "user", Content: prompt, MessageID: sess.NextMessageID("user")})

	finish := func(stop, out string, turns int) (*Result, error) {
		return a.finish(r, stop, out, turns, startUsage, startCost)
	}

	if why := r.budgetText(); why != "" {
		return finish(StopBudgetExceeded, why, 0)
	}

	maxTurns := rt.MaxTurns
	for turn := 1; turn <= maxTurns; turn++ {
		if ctx.Err() != nil {
			return finish(StopInterrupted, "Interrupted before the model was called.", turn-1)
		}
		preserve := max(rt.CompactPreserveMessages, 0)
		if ev, ok := clearToolOutputs(sess, a.window(), toolSpecTokens(a.registry.Specs()), a.ctxSettings, turn); ok {
			r.emit(ev)
		}
		if t := rt.AutoSnipThresholdTokens; t != nil && *t > 0 {
			_, evs := reduce(sess, reduceOpts{target: *t, preserve: preserve, turn: turn})
			r.emitAll(evs)
		}
		if t := rt.AutoCompactThresholdTokens; t != nil && *t > 0 {
			_, evs := reduce(sess, reduceOpts{target: *t, preserve: preserve, turn: turn})
			r.emitAll(evs)
			if MessageTokens(sess.Messages) > *t {
				// The operator's explicit threshold stays model-free: notes if fresh, else the digest.
				if !a.notesCompact(r, turn, preserve, false) {
					_, evs := reduce(sess, reduceOpts{target: *t, allowCompaction: true, preserve: preserve, turn: turn,
						notes: a.vettedNotes(r, turn)})
					r.emitAll(evs)
				}
			}
		}
		if tooLong, text := a.preflight(ctx, r, turn, preserve); tooLong {
			return finish(StopPromptTooLong, text, turn-1)
		}

		r.emit(event{"type": "turn_start", "turn_index": turn})
		t, err := a.callModel(ctx, r, turn, preserve)
		if err != nil {
			if ctx.Err() != nil {
				return finish(StopInterrupted, "Interrupted while waiting for the model.", turn-1)
			}
			return finish(StopBackendError, err.Error(), turn-1)
		}

		r.emit(event{"type": "assistant_message", "turn_index": turn, "content": t.Content, "tool_calls": len(t.ToolCalls)})
		r.counts.ModelCalls++
		r.turnsRun++
		r.usage = r.usage.Add(t.Usage)
		r.cost = Cost(a.opts.Model.Pricing, r.usage)
		r.lastText = t.Content

		if why := r.budgetText(); why != "" {
			r.answerPending(t.ToolCalls, "not run: "+why)
			return finish(StopBudgetExceeded, why, turn)
		}

		if len(t.ToolCalls) == 0 {
			r.segments = append(r.segments, t.Content)
			if t.FinishReason == "length" || t.FinishReason == "max_tokens" {
				sess.Append(session.Message{Role: "user", MessageID: fmt.Sprintf("continuation_%d", turn),
					Content:  "Your last reply hit the output length limit and was cut off. Carry on from exactly where it stopped and do not repeat what you already wrote.",
					Metadata: map[string]any{"kind": session.KindContinuation, "continuation_index": len(r.segments)}})
				r.emit(event{"type": "continuation_request", "turn_index": turn, "reason": t.FinishReason,
					"continuation_index": len(r.segments)})
				continue
			}
			return finish(t.FinishReason, strings.Join(r.segments, ""), turn)
		}

		r.segments = nil
		if stop, out := a.runToolCalls(ctx, r, t.ToolCalls, turn); stop != "" {
			return finish(stop, out, turn)
		}
		a.maybeUpdateNotes(ctx, r, turn)
	}
	out := strings.Join(r.segments, "")
	if out == "" {
		out = r.lastText
	}
	if out == "" {
		out = "Stopped: the turn limit was reached before the task finished."
	}
	return finish(StopMaxTurns, out, maxTurns)
}

func (r *run) emitAll(evs []event) {
	for _, e := range evs {
		r.emit(e)
	}
}

// finish records counters in the session, writes it, and builds the Result.
func (a *Agent) finish(r *run, stop, out string, turns int, startUsage llm.Usage, startCost float64) (*Result, error) {
	s := r.sess
	a.recordTiers(s)
	s.SetPlan(a.plan)
	s.Turns = r.baseTurn + turns
	s.ToolCalls = r.counts.ToolCalls
	s.Usage = r.usage
	s.TotalCostUSD = r.cost
	s.BudgetState = session.BudgetState{ModelCalls: r.counts.ModelCalls, SessionTurns: s.Turns,
		ToolCalls: r.counts.ToolCalls, DelegatedTasks: r.counts.Delegated, Escalations: r.esc}
	a.CumulativeUsage = a.CumulativeUsage.Add(llm.Usage{
		InputTokens:              r.usage.InputTokens - startUsage.InputTokens,
		OutputTokens:             r.usage.OutputTokens - startUsage.OutputTokens,
		CacheCreationInputTokens: r.usage.CacheCreationInputTokens - startUsage.CacheCreationInputTokens,
		CacheReadInputTokens:     r.usage.CacheReadInputTokens - startUsage.CacheReadInputTokens,
		ReasoningTokens:          r.usage.ReasoningTokens - startUsage.ReasoningTokens,
	})
	a.CumulativeCostUSD += r.cost - startCost

	res := &Result{
		FinalOutput: out, Turns: turns, ToolCalls: r.counts.ToolCalls,
		Transcript: append([]session.Message(nil), s.Messages...), Events: r.events,
		Usage: r.usage, TotalCostUSD: r.cost, StopReason: stop, FileHistory: s.FileHistory,
		SessionID: s.ID, ScratchpadDirectory: s.ScratchpadDirectory,
	}
	if stop == StopPlanPending {
		res.Plan = s.Plan().Pending
	}
	path, err := s.Save(a.opts.Runtime.SessionDirectory)
	if err != nil {
		return res, fmt.Errorf("saving session: %w", err)
	}
	res.SessionPath = path
	return res, nil
}

// preflight enforces the context window. It returns true (and an explanation)
// when the next request cannot fit even after reduction.
func (a *Agent) preflight(ctx context.Context, r *run, turn, preserve int) (bool, string) {
	rt := a.opts.Runtime
	specs := a.registry.Specs()
	snap := TakeSnapshot(a.window(), r.sess.Messages, specs, rt)
	if !snap.ExceedsSoft && !snap.ExceedsHard {
		return false, ""
	}
	r.emit(event{"type": "prompt_length_check", "turn_index": turn, "projected_tokens": snap.Projected,
		"soft_limit": snap.Soft, "hard_limit": snap.Hard, "overflow_tokens": snap.Overflow,
		"exceeds_hard": snap.ExceedsHard})
	target := snap.Soft
	if snap.ExceedsHard {
		target = snap.Hard
	}
	// reduce() measures message tokens only; leave room for framing and tool specs.
	overhead := snap.Projected - MessageTokens(r.sess.Messages)
	goal := max(target-overhead, 0)
	// Cheapest first: snip bulky old messages; then compact (notes, summary, digest).
	changed, evs := reduce(r.sess, reduceOpts{target: goal, preserve: preserve, turn: turn})
	r.emitAll(evs)
	strategy := "snip"
	if MessageTokens(r.sess.Messages) > goal {
		if how := a.compactHistory(ctx, r, turn, preserve, true); how != "" {
			changed, strategy = true, how
		}
	}
	if changed {
		snap = TakeSnapshot(a.window(), r.sess.Messages, specs, rt)
		r.emit(event{"type": "prompt_length_recovery", "turn_index": turn, "strategy": strategy,
			"projected_tokens": snap.Projected, "exceeds_hard": snap.ExceedsHard})
		if !snap.ExceedsSoft && !snap.ExceedsHard {
			return false, ""
		}
	}
	if snap.ExceedsHard {
		win, src := a.resolveWindow()
		msg := fmt.Sprintf("Stopped before calling the model: the conversation is about %d tokens, over the hard limit of %d (soft limit %d) for this context window of %d tokens (%s).",
			snap.Projected, snap.Hard, snap.Soft, win, describeWindowSource(src))
		if win <= 8192 {
			msg += " That window is small for an agent: the tool definitions and system prompt alone take several thousand tokens." +
				" Raise the backend's context (Ollama: a Modelfile with `PARAMETER num_ctx 16384`, or set OLLAMA_CONTEXT_LENGTH=16384)" +
				" or pass --context-window if the backend really has more."
		}
		return true, msg
	}
	r.emit(event{"type": "prompt_length_warning", "turn_index": turn, "projected_tokens": snap.Projected,
		"soft_limit": snap.Soft, "hard_limit": snap.Hard})
	return false, ""
}

var promptTooLongPhrases = []string{"prompt is too long", "maximum context length", "context length exceeded",
	"too many tokens", "input too long", "context window"}

func isPromptTooLong(err error) bool {
	msg := strings.ToLower(err.Error())
	for _, p := range promptTooLongPhrases {
		if strings.Contains(msg, p) {
			return true
		}
	}
	return false
}

func (r *run) request() llm.Request {
	rt := r.a.opts.Runtime
	msgs := make([]llm.Message, 0, len(r.sess.Messages))
	for _, m := range r.sess.Messages {
		msgs = append(msgs, m.Wire())
	}
	req := llm.Request{Messages: msgs, Tools: r.a.registry.Specs(), Schema: rt.OutputSchema, Stream: rt.StreamModelResponses}
	if rt.BudgetConfig.MaxOutputTokens != nil && *rt.BudgetConfig.MaxOutputTokens > 0 {
		req.MaxTokens = *rt.BudgetConfig.MaxOutputTokens
	}
	return req
}

func streamEventDict(e llm.StreamEvent) event {
	d := event{"type": e.Type, "delta": e.Delta, "tool_call_index": nil, "tool_call_id": nil, "tool_name": nil,
		"arguments_delta": nil, "finish_reason": nil, "usage": nil, "raw_event": nil}
	switch e.Type {
	case "content_delta":
	case "tool_call_delta":
		d["tool_call_index"] = e.ToolCallIndex
		d["tool_call_id"] = e.ToolCallID
		d["tool_name"] = e.ToolName
		d["arguments_delta"] = e.ArgumentsDelta
	case "usage":
		if e.Usage != nil {
			d["usage"] = *e.Usage
		}
	case "message_stop":
		d["finish_reason"] = e.FinishReason
	}
	return d
}

// callModel performs one model call (with the single reactive-compaction retry)
// and commits the assistant message. On success the returned turn carries
// effective usage (estimated when the server reported none).
func (a *Agent) callModel(ctx context.Context, r *run, turn, preserve int) (*llm.Turn, error) {
	streamIdx := -1
	attempt := func() (*llm.Turn, llm.Request, error) {
		req := r.request()
		streamIdx = -1
		var on func(llm.StreamEvent)
		if req.Stream {
			on = func(e llm.StreamEvent) {
				r.emit(streamEventDict(e))
				switch e.Type {
				case "message_start":
					streamIdx = r.sess.Append(session.Message{Role: "assistant", MessageID: r.sess.NextMessageID("assistant"),
						State: session.StateStreaming})
				case "content_delta":
					if streamIdx >= 0 {
						m := &r.sess.Messages[streamIdx]
						m.Content += e.Delta
						m.Mutate("assistant_delta_append")
					}
				case "tool_call_delta":
					if streamIdx >= 0 {
						r.sess.Messages[streamIdx].Mutate("assistant_tool_call_delta")
					}
				}
			}
		}
		t, err := a.opts.Client.Complete(ctx, req, on)
		if err != nil && streamIdx >= 0 && streamIdx == len(r.sess.Messages)-1 {
			r.sess.Messages = r.sess.Messages[:streamIdx] // drop the half-built message
			streamIdx = -1
		}
		return t, req, err
	}

	t, req, err := attempt()
	if err != nil && ctx.Err() == nil && isPromptTooLong(err) {
		half := max(preserve/2, 1)
		changed := a.notesCompact(r, turn, half, true) // fresh notes: free
		if !changed {
			var evs []event
			changed, evs = reduce(r.sess, reduceOpts{target: 0, allowCompaction: true, reactive: true,
				preserve: half, turn: turn, notes: a.vettedNotes(r, turn)})
			r.emitAll(evs)
		}
		if changed {
			t, req, err = attempt()
			if err == nil {
				r.emit(event{"type": "reactive_compact_retry", "turn_index": turn})
			}
		}
	}
	if err != nil {
		return nil, err
	}

	if !t.UsageReported {
		in := MessageTokens(r.sess.Messages) + toolSpecTokens(req.Tools)
		out := CountTokens(t.Content)
		for _, c := range t.ToolCalls {
			out += CountTokens(c.Name) + CountTokens(c.ArgumentsJSON())
		}
		t.Usage = llm.Usage{InputTokens: in, OutputTokens: out}
	}
	msg := session.Message{Role: "assistant", Content: t.Content, State: session.StateFinal,
		StopReason: t.FinishReason, Usage: t.Usage}
	for _, c := range t.ToolCalls {
		msg.ToolCalls = append(msg.ToolCalls, c.Wire())
	}
	if streamIdx >= 0 {
		cur := r.sess.Messages[streamIdx]
		msg.MessageID, msg.Metadata = cur.MessageID, cur.Metadata
		r.sess.Messages[streamIdx] = msg
		r.sess.Messages[streamIdx].Mutate("assistant_finalize")
	} else {
		msg.MessageID = r.sess.NextMessageID("assistant")
		r.sess.Append(msg)
	}
	idx := len(r.sess.Messages) - 1
	if streamIdx >= 0 {
		idx = streamIdx
	}
	if !t.UsageReported {
		r.sess.Messages[idx].SetMeta("usage_estimated", true)
	}
	if t.Reasoning != "" {
		// kept for the operator's inspection only; never sent back to the model
		r.sess.Messages[idx].SetMeta("has_reasoning", true)
	}
	return t, nil
}

// answerPending gives every call a tool result saying it was not run, so the
// transcript stays valid for strict backends.
func (r *run) answerPending(calls []llm.ToolCall, reason string) {
	for _, c := range calls {
		if r.done[c.ID] {
			continue
		}
		r.sess.Append(session.NotRunMessage(c.ID, c.Name, reason, r.sess.NextMessageID("tool")))
		r.done[c.ID] = true
	}
}

// runToolCalls executes a turn's calls in order. A non-empty stop means the run
// must end with that stop reason; every call has been answered either way.
func (a *Agent) runToolCalls(ctx context.Context, r *run, calls []llm.ToolCall, turn int) (stop, out string) {
	for i, call := range calls {
		if ctx.Err() != nil {
			r.answerPending(calls[i:], "not run: the session was interrupted")
			return StopInterrupted, "Interrupted during tool execution."
		}
		if r.done[call.ID] {
			r.sess.Append(session.Message{Role: "tool", Name: call.Name, ToolCallID: call.ID, State: session.StateFinal,
				MessageID: r.sess.NextMessageID("tool"), StopReason: "tool_duplicate",
				Content: session.ToolResultBody(call.Name, false,
					"this tool call id was already executed earlier in this run; it was not run again",
					map[string]any{"error_kind": "duplicate_call"}),
				Metadata: map[string]any{"tool_name": call.Name, "tool_call_id": call.ID, "phase": "completed"}})
			continue
		}
		r.counts.ToolCalls++
		if _, offered := a.registry.Get(ToolAgent); offered && call.Name == ToolAgent {
			r.counts.Delegated++
		}
		if why := r.budgetText(); why != "" {
			r.emit(event{"type": "task_budget_exceeded", "turn_index": turn, "tool_name": call.Name,
				"tool_call_id": call.ID, "reason": why})
			r.answerPending(calls[i:], "not run: "+why)
			return StopBudgetExceeded, why
		}

		idx := r.sess.Append(session.Message{Role: "tool", Name: call.Name, ToolCallID: call.ID,
			State: session.StateStreaming, MessageID: r.sess.NextMessageID("tool"),
			Metadata: map[string]any{"tool_name": call.Name, "tool_call_id": call.ID, "phase": "starting"}})
		r.emit(event{"type": "tool_start", "turn_index": turn, "tool_name": call.Name, "tool_call_id": call.ID,
			"message_id": r.sess.Messages[idx].MessageID, "arguments": call.Arguments})

		a.curCall = call.ID
		var res tools.Result
		planPending := ""
		if call.ParseError != "" {
			res = tools.Result{Name: call.Name, Content: fmt.Sprintf(
				"The arguments for %s could not be used: %s. Received: %s. Send the call again with a valid JSON object.",
				call.Name, call.ParseError, collapse(call.RawArguments, 200)),
				Metadata: map[string]any{"error_kind": "invalid_arguments"}}
		} else if a.opts.Runtime.StreamModelResponses {
			// Tools that produce output over time (bash) report it live; the permission
			// gate is the same on this path as on the blocking one.
			res = a.registry.ExecuteStream(ctx, call.Name, call.Arguments, a.toolCtx, func(stream, text string) {
				r.emit(event{"type": "tool_delta", "turn_index": turn, "tool_name": call.Name,
					"tool_call_id": call.ID, "stream": stream, "delta": text})
			})
		} else {
			res = a.registry.Execute(ctx, call.Name, call.Arguments, a.toolCtx)
		}

		if a.childUsed { // sub-agent tokens count against this session's budgets too
			r.usage = r.usage.Add(a.childUsage)
			r.cost = Cost(a.opts.Model.Pricing, r.usage)
			a.childUsage, a.childUsed = llm.Usage{}, false
		}
		if a.opts.Runtime.StreamModelResponses && res.Metadata["streamed"] != true {
			for _, chunk := range chunks(res.Content, 400) {
				r.emit(event{"type": "tool_delta", "turn_index": turn, "tool_name": call.Name,
					"tool_call_id": call.ID, "stream": "tool", "delta": chunk})
			}
		}
		if res.OK {
			if act, _ := res.Metadata["plan_action"].(string); act != "" {
				planPending = a.onPlanAction(r, act, call.Arguments["plan"], &res)
			}
			if names, ok := res.Metadata["skill_allowed_tools"].([]string); ok {
				a.NarrowTools(names)
			}
		}
		if res.Metadata["error_kind"] == "permission_denied" {
			r.emit(event{"type": "tool_permission_denial", "turn_index": turn, "tool_name": call.Name,
				"tool_call_id": call.ID, "reason": res.Content, "source": "tool_runtime"})
		}

		m := &r.sess.Messages[idx]
		m.Content = session.Redact(session.ToolResultBody(call.Name, res.OK, res.Content, res.Metadata))
		m.State = session.StateFinal
		m.StopReason = "tool_completed"
		for k, v := range res.Metadata {
			m.Metadata[k] = v
		}
		m.Metadata["tool_name"], m.Metadata["tool_call_id"], m.Metadata["phase"] = call.Name, call.ID, "completed"
		r.done[call.ID] = true
		r.emit(event{"type": "tool_result", "turn_index": turn, "tool_name": call.Name, "tool_call_id": call.ID,
			"ok": res.OK, "metadata": res.Metadata, "content": clipRunes(session.Redact(res.Content), 20000)})

		a.recordHistory(r, call, res, turn)

		if planPending != "" {
			r.answerPending(calls[i+1:], "not run: the plan is waiting for the operator's decision")
			return StopPlanPending, planPending
		}

		if cwd, ok := res.Metadata["cwd_update"].(string); ok && res.OK && cwd != "" {
			if st, err := os.Stat(cwd); err == nil && st.IsDir() {
				if err := a.rebuildTools(cwd); err == nil {
					r.sess.Runtime = a.opts.Runtime
				}
			}
		}
	}
	return "", ""
}

// clipRunes shortens s to at most n runes for an event payload.
func clipRunes(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "..."
}

func chunks(s string, size int) []string {
	r := []rune(s)
	var out []string
	for len(r) > size {
		out = append(out, string(r[:size]))
		r = r[size:]
	}
	if len(r) > 0 {
		out = append(out, string(r))
	}
	return out
}

func (a *Agent) recordHistory(r *run, call llm.ToolCall, res tools.Result, turn int) {
	if res.Metadata["delegation"] == true {
		r.sess.FileHistory = append(r.sess.FileHistory, map[string]any{
			"history_kind": "delegation", "timestamp": a.opts.Now().UTC().Format(time.RFC3339), "turn_index": turn,
			"tool_call_id": call.ID, "tool_name": call.Name, "ok": res.OK,
			"history_entry_id":  fmt.Sprintf("%d:%s:%s", turn, call.ID, call.Name),
			"child_session_id":  res.Metadata["child_session_id"],
			"subagent_profile":  res.Metadata["subagent_profile"],
			"child_stop_reason": res.Metadata["child_stop_reason"],
			"result_preview":    collapse(res.Content, 220),
		})
		return
	}
	path, hasPath := res.Metadata["path"].(string)
	command, hasCmd := res.Metadata["command"].(string)
	if !hasPath && !hasCmd {
		return
	}
	e := map[string]any{}
	for k, v := range res.Metadata {
		e[k] = v
	}
	e["timestamp"] = a.opts.Now().UTC().Format(time.RFC3339)
	e["turn_index"] = turn
	e["tool_call_id"] = call.ID
	e["tool_name"] = call.Name
	e["ok"] = res.OK
	e["history_entry_id"] = fmt.Sprintf("%d:%s:%s", turn, call.ID, call.Name)
	e["result_preview"] = collapse(res.Content, 220)
	if hasPath {
		e["history_kind"] = "file_change"
		e["changed_paths"] = []string{path}
		for _, side := range []string{"before", "after"} {
			if h, ok := res.Metadata[side+"_sha256"].(string); ok && h != "" {
				e[side+"_snapshot_id"] = path + ":" + h[:min(12, len(h))]
			}
		}
	} else {
		e["history_kind"] = "shell"
		_ = command
	}
	r.sess.FileHistory = append(r.sess.FileHistory, e)
}
