package gui

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

const (
	maxEvents   = 4000
	maxResumeID = 64
)

var idRe = regexp.MustCompile(`^[A-Za-z0-9_-]{1,64}$`)

// Run states reported to the page.
const (
	StateIdle     = "idle"
	StateRunning  = "running"
	StateWaiting  = "waiting_approval"
	StateFinished = "finished"
)

// run is one agent turn in flight (or the last one that finished).
type run struct {
	id      string
	cancel  context.CancelFunc
	done    chan struct{}
	started time.Time
	scrub   func(string) string
	grants  map[string]bool // tools the operator allowed for the rest of the session

	// result of the run, valid once done is closed
	result map[string]any
	err    error
	agent  *agent.Agent

	mu       sync.Mutex
	finished bool
	waiting  int
	sessID   string
}

func (r *run) running() bool {
	r.mu.Lock()
	defer r.mu.Unlock()
	return !r.finished
}

func (r *run) state() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	switch {
	case r.finished:
		return StateFinished
	case r.waiting > 0:
		return StateWaiting
	}
	return StateRunning
}

// ---- events -------------------------------------------------------------------------

// emit appends an event to the stream and wakes every listener.
func (s *Server) emit(runID, typ string, fields map[string]any) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.seq++
	e := map[string]any{"seq": s.seq, "type": typ, "run_id": runID, "at": s.now()}
	for k, v := range fields {
		e[k] = v
	}
	s.events = append(s.events, e)
	if len(s.events) > maxEvents {
		s.events = append([]map[string]any(nil), s.events[len(s.events)-maxEvents:]...)
	}
	close(s.notify)
	s.notify = make(chan struct{})
}

// eventsAfter returns the events after seq and the channel to wait on for more.
func (s *Server) eventsAfter(seq int) ([]map[string]any, <-chan struct{}) {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []map[string]any
	for _, e := range s.events {
		if e["seq"].(int) > seq {
			out = append(out, e)
		}
	}
	return out, s.notify
}

// onAgentEvent translates the agent's events into the console's stream.
func (s *Server) onAgentEvent(r *run, e map[string]any) {
	typ, _ := e["type"].(string)
	str := func(k string) string { v, _ := e[k].(string); return v }
	switch typ {
	case "turn_start":
		s.emit(r.id, "turn_start", map[string]any{"turn": e["turn_index"]})
	case "content_delta":
		s.emit(r.id, "text_delta", map[string]any{"delta": r.scrub(str("delta"))})
	case "assistant_message":
		s.emit(r.id, "assistant_message", map[string]any{"turn": e["turn_index"], "content": r.scrub(str("content")), "tool_calls": e["tool_calls"]})
	case "tool_start":
		name := str("tool_name")
		effect := "read"
		if r.agent != nil {
			if t, ok := r.agent.Registry().Get(name); ok {
				switch t.Requires {
				case tools.PermShell:
					effect = "shell"
				case tools.PermWrite:
					effect = "write"
				}
			}
		}
		s.emit(r.id, "tool_call", map[string]any{"tool_call_id": str("tool_call_id"), "name": name, "effect": effect,
			"arguments": runkit.ScrubValue(e["arguments"], r.scrub)})
	case "tool_delta":
		s.emit(r.id, "tool_delta", map[string]any{"tool_call_id": str("tool_call_id"), "stream": str("stream"), "delta": r.scrub(str("delta"))})
	case "tool_result":
		md := map[string]any{}
		if m, ok := e["metadata"].(map[string]any); ok {
			if k, ok := m["error_kind"]; ok {
				md["error_kind"] = k
			}
		}
		s.emit(r.id, "tool_result", map[string]any{"tool_call_id": str("tool_call_id"), "name": str("tool_name"),
			"ok": e["ok"], "content": r.scrub(str("content")), "metadata": md})
	case "tier_escalation", "tier_refused", "tier_redaction", "tier_wake", "tier_notice":
		s.emit(r.id, "info", map[string]any{"name": typ, "text": r.scrub(str("line"))})
	default:
		for _, p := range []string{"auto_compact", "prompt_length", "reactive_compact", "tool_output_cleared", "session_notes",
			"subagent_", "continuation_request", "task_budget_exceeded"} {
			if strings.HasPrefix(typ, p) {
				s.emit(r.id, "info", map[string]any{"name": typ})
				return
			}
		}
	}
}

// ---- approvals ----------------------------------------------------------------------

type approval struct {
	ID        string         `json:"approval_id"`
	Kind      string         `json:"kind"` // "tool" or "plan"
	RunID     string         `json:"run_id"`
	SessionID string         `json:"session_id"`
	Tool      string         `json:"tool_name,omitempty"`
	Arguments map[string]any `json:"arguments,omitempty"`
	Plan      string         `json:"plan,omitempty"`
	Summary   string         `json:"summary"`
	Risk      string         `json:"risk"`
	Created   string         `json:"created_at"`
	decide    chan decision
}

type decision struct {
	Decision string
	Reason   string
}

func summarize(tool string, args map[string]any) string {
	if c, ok := args["command"].(string); ok && tool == "bash" {
		return "run: " + clip(c, 300)
	}
	if p, ok := args["path"].(string); ok {
		return fmt.Sprintf("%s on %s", tool, p)
	}
	keys := make([]string, 0, len(args))
	for k := range args {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var parts []string
	for _, k := range keys {
		parts = append(parts, fmt.Sprintf("%s=%s", k, clip(fmt.Sprint(args[k]), 60)))
	}
	return clip(fmt.Sprintf("%s(%s)", tool, strings.Join(parts, ", ")), 300)
}

func clip(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "..."
}

func riskFor(req tools.ApprovalRequest, root string) string {
	switch req.Needs {
	case tools.PermShell:
		if c, ok := req.Arguments["command"].(string); ok {
			if v := tools.CheckDestructive(c, root); v.Blocked {
				return "high: this command looks destructive (" + v.Reason + ")"
			}
		}
		return "high: runs a command on this machine"
	case tools.PermWrite:
		return "medium: changes files or saved state"
	}
	return "medium: this tool changes state"
}

// ask registers a pending approval and waits for the operator, the run being
// cancelled, or the timeout (which counts as a denial).
func (s *Server) ask(ctx context.Context, r *run, ap *approval) (decision, bool) {
	s.mu.Lock()
	s.nextApp++
	ap.ID = fmt.Sprintf("a%d-%s", s.nextApp, session.NewID()[:8])
	ap.RunID, ap.Created = r.id, s.now()
	ap.decide = make(chan decision, 1)
	s.approvals[ap.ID] = ap
	s.mu.Unlock()

	r.mu.Lock()
	r.waiting++
	r.mu.Unlock()
	s.emit(r.id, "approval_request", map[string]any{"approval": s.approvalView(ap, r.scrub)})
	defer func() {
		r.mu.Lock()
		r.waiting--
		r.mu.Unlock()
		s.mu.Lock()
		delete(s.approvals, ap.ID)
		s.mu.Unlock()
	}()

	timer := time.NewTimer(s.cfg.ApprovalTimeout)
	defer timer.Stop()
	var d decision
	answered := false
	select {
	case d = <-ap.decide:
		answered = true
	case <-ctx.Done():
		d = decision{Decision: "deny", Reason: "the run was cancelled"}
	case <-timer.C:
		d = decision{Decision: "deny", Reason: "no answer before the approval timeout"}
	}
	s.emit(r.id, "approval_resolved", map[string]any{"approval_id": ap.ID, "decision": d.Decision, "answered": answered})
	return d, answered
}

func (s *Server) approvalView(ap *approval, scrub func(string) string) map[string]any {
	v := map[string]any{"approval_id": ap.ID, "kind": ap.Kind, "run_id": ap.RunID, "session_id": ap.SessionID,
		"summary": scrub(ap.Summary), "risk": ap.Risk, "created_at": ap.Created}
	if ap.Kind == "tool" {
		v["tool_name"] = ap.Tool
		v["arguments"] = runkit.ScrubValue(ap.Arguments, scrub)
	} else {
		v["plan"] = scrub(ap.Plan)
	}
	return v
}

func (s *Server) listApprovals(w http.ResponseWriter, _ *http.Request) {
	scrub := s.scrubber()
	s.mu.Lock()
	list := make([]*approval, 0, len(s.approvals))
	for _, ap := range s.approvals {
		list = append(list, ap)
	}
	s.mu.Unlock()
	sort.Slice(list, func(i, j int) bool { return list[i].Created+list[i].ID < list[j].Created+list[j].ID })
	out := make([]map[string]any, 0, len(list))
	for _, ap := range list {
		out = append(out, s.approvalView(ap, scrub))
	}
	s.writeJSON(w, 200, out)
}

func (s *Server) answerApproval(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Decision string `json:"decision"`
		Reason   string `json:"reason"`
	}
	if !s.readJSON(w, r, &body) {
		return
	}
	s.mu.Lock()
	ap := s.approvals[r.PathValue("id")]
	s.mu.Unlock()
	if ap == nil {
		s.detail(w, http.StatusNotFound, "no pending approval with that id (it may have been answered or timed out)")
		return
	}
	valid := map[string]bool{"allow_once": true, "allow_session": true, "deny": true}
	if ap.Kind == "plan" {
		valid = map[string]bool{agent.PlanApprove: true, agent.PlanRevise: true, agent.PlanReject: true}
	}
	if !valid[body.Decision] {
		s.detail(w, http.StatusBadRequest, "decision is not valid for a "+ap.Kind+" approval")
		return
	}
	if ap.Kind == "plan" && body.Decision == agent.PlanRevise && strings.TrimSpace(body.Reason) == "" {
		s.detail(w, http.StatusBadRequest, "revising a plan needs feedback in reason")
		return
	}
	select {
	case ap.decide <- decision{body.Decision, body.Reason}:
		s.writeJSON(w, 200, map[string]any{"approval_id": ap.ID, "decision": body.Decision})
	default:
		s.detail(w, http.StatusConflict, "already answered")
	}
}

// ---- starting a run -----------------------------------------------------------------

var pasteRe = regexp.MustCompile(`\[Pasted text #(\d+)[^\]]*\]`)

type pasted struct {
	Type    string `json:"type"`
	Content string `json:"content"`
}

// expandPastes replaces "[Pasted text #N ...]" placeholders with the text the
// page sent separately.
func expandPastes(prompt string, contents map[string]pasted) string {
	if len(contents) == 0 {
		return prompt
	}
	return pasteRe.ReplaceAllStringFunc(prompt, func(m string) string {
		id := pasteRe.FindStringSubmatch(m)[1]
		if p, ok := contents[id]; ok && p.Content != "" {
			return p.Content
		}
		return m
	})
}

type chatBody struct {
	Prompt          *string           `json:"prompt"`
	ResumeSessionID *string           `json:"resume_session_id"`
	PastedContents  map[string]pasted `json:"pasted_contents"`
}

// startRun validates, builds the agent from the current settings and runs the
// turn in the background.
func (s *Server) startRun(b chatBody) (*run, error) {
	if b.Prompt == nil || *b.Prompt == "" {
		return nil, &apiErr{422, "prompt: field required (min length 1)"}
	}
	prompt := strings.TrimSpace(expandPastes(*b.Prompt, b.PastedContents))
	if prompt == "" {
		return nil, bad("prompt must not be blank")
	}

	// Reserve the single run slot first, so two requests cannot both start.
	s.mu.Lock()
	if s.run != nil && s.run.running() {
		s.mu.Unlock()
		return nil, &apiErr{http.StatusConflict, "a run is already in progress; stop it first"}
	}
	st := s.st
	st.ExtraDirs = append([]string(nil), s.st.ExtraDirs...)
	prev := s.run
	scrub := runkit.Scrubber(st.APIKey, s.token)
	ctx, cancel := context.WithCancel(context.Background())
	r := &run{id: session.NewID()[:12], done: make(chan struct{}), started: time.Now(), scrub: scrub,
		grants: map[string]bool{}, cancel: cancel}
	s.run = r
	s.mu.Unlock()
	release := func() { // the run never started: give the slot back
		cancel()
		s.mu.Lock()
		if s.run == r {
			s.run = prev
		}
		s.mu.Unlock()
	}

	var stored *session.Session
	if b.ResumeSessionID != nil && *b.ResumeSessionID != "" {
		id := *b.ResumeSessionID
		if !idRe.MatchString(id) {
			release()
			return nil, &apiErr{404, "session to resume not found"}
		}
		var err error
		if stored, err = session.LoadByID(st.SessionDir, id); err != nil {
			release()
			return nil, &apiErr{404, "session to resume not found"}
		}
	}

	if text, ok := cli.LocalSlash(prompt); ok { // answered here, no model call
		s.finishLocal(r, prompt, text, stored)
		return r, nil
	}

	m, rt := st.ModelRuntime()
	var approver tools.Approver
	if st.ApprovalMode == ApprovalAsk {
		approver = func(ctx context.Context, req tools.ApprovalRequest) (bool, string) {
			r.mu.Lock()
			granted := r.grants[req.Tool]
			r.mu.Unlock()
			if granted {
				return true, ""
			}
			ap := &approval{Kind: "tool", SessionID: r.currentSession(), Tool: req.Tool, Arguments: req.Arguments,
				Summary: summarize(req.Tool, req.Arguments), Risk: riskFor(req, rt.CWD)}
			d, _ := s.ask(ctx, r, ap)
			switch d.Decision {
			case "allow_session":
				r.mu.Lock()
				r.grants[req.Tool] = true
				r.mu.Unlock()
				return true, ""
			case "allow_once":
				return true, ""
			}
			return false, d.Reason
		}
	}
	built, err := runkit.Build(runkit.Spec{Model: m, Runtime: rt, Prompt: st.PromptOptions(), Home: s.cfg.Home,
		Getenv: s.cfg.Getenv, NewClient: s.cfg.NewClient, Approver: approver, Discover: s.cfg.Discover,
		UseTiers: true, Subagents: true, Escalation: s.cfg.Escalation})
	if err != nil {
		release()
		return nil, bad("%s", scrub(err.Error()))
	}
	a := built.Agent
	r.agent = a
	if stored != nil {
		r.sessID = stored.ID
	}
	a.SetEventHook(func(e map[string]any) { s.onAgentEvent(r, e) })
	a.SetPlanDecider(func(plan string) agent.PlanDecision {
		ap := &approval{Kind: "plan", SessionID: r.currentSession(), Plan: plan,
			Summary: "The agent submitted a plan and is waiting for your decision.", Risk: "none: nothing changes until you approve"}
		d, answered := s.ask(ctx, r, ap)
		if !answered {
			return agent.PlanDecision{}
		}
		return agent.PlanDecision{Action: d.Decision, Feedback: d.Reason}
	})

	s.emit(r.id, "run_start", map[string]any{"prompt": scrub(prompt), "resumed": stored != nil})
	for _, n := range built.Notices {
		s.emit(r.id, "info", map[string]any{"name": "notice", "text": scrub(n)})
	}

	go func() {
		defer cancel()
		var res *agent.Result
		var err error
		if stored != nil {
			res, err = a.Resume(ctx, prompt, stored)
		} else {
			res, err = a.Run(ctx, prompt)
		}
		if res != nil && err == nil {
			r.result = runkit.Result(res, scrub)
			r.mu.Lock()
			r.sessID = res.SessionID
			r.mu.Unlock()
			s.mu.Lock()
			s.st.ActiveSessionID = res.SessionID
			s.mu.Unlock()
			s.emit(r.id, "usage", map[string]any{"usage": res.Usage, "total_cost_usd": res.TotalCostUSD, "turns": res.Turns, "tool_calls": res.ToolCalls})
		} else {
			if err == nil {
				err = fmt.Errorf("the run produced no result")
			}
			r.err = err
		}
		r.mu.Lock()
		r.finished = true
		r.mu.Unlock()
		close(r.done)
		if r.err != nil {
			s.emit(r.id, "error", map[string]any{"error": scrub(r.err.Error()), "error_type": runkit.ErrorType(r.err)})
		}
		if r.result != nil {
			s.emit(r.id, "done", map[string]any{"result": r.result})
		} else {
			s.emit(r.id, "done", map[string]any{"result": nil})
		}
	}()
	return r, nil
}

// finishLocal completes a run that needed no model: a slash command answered
// by the console itself.
func (s *Server) finishLocal(r *run, prompt, text string, stored *session.Session) {
	sid := ""
	if stored != nil {
		sid = stored.ID
	}
	r.mu.Lock()
	r.sessID = sid
	r.mu.Unlock()
	r.result = map[string]any{
		"final_output": text, "turns": 0, "tool_calls": 0, "session_id": sid,
		"usage": map[string]int{"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0,
			"cache_read_input_tokens": 0, "reasoning_tokens": 0, "total_tokens": 0},
		"total_cost_usd": 0, "stop_reason": "slash_command",
		"transcript": []map[string]any{{"role": "user", "content": prompt}, {"role": "assistant", "content": text}},
	}
	s.emit(r.id, "run_start", map[string]any{"prompt": prompt, "resumed": stored != nil})
	s.emit(r.id, "assistant_message", map[string]any{"turn": 0, "content": text, "tool_calls": 0})
	r.mu.Lock()
	r.finished = true
	r.mu.Unlock()
	close(r.done)
	r.cancel()
	s.emit(r.id, "done", map[string]any{"result": r.result})
}

func (r *run) currentSession() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.sessID
}

// ---- handlers -----------------------------------------------------------------------

// chat starts a run. With Accept: text/event-stream the response is the run's
// event stream (ending after "done"); otherwise it blocks and returns the
// run result, exactly like the service does.
func (s *Server) chat(w http.ResponseWriter, r *http.Request) {
	var body chatBody
	if !s.readJSON(w, r, &body) {
		return
	}
	run, err := s.startRun(body)
	if err != nil {
		s.fail(w, err)
		return
	}
	if strings.Contains(r.Header.Get("Accept"), "text/event-stream") {
		s.stream(w, r, 0, run.id, true)
		return
	}
	select {
	case <-run.done:
	case <-r.Context().Done():
		run.cancel() // a script that hung up does not leave an agent running
		<-run.done
	}
	if run.err != nil {
		s.writeJSON(w, 500, map[string]any{"error": run.scrub(run.err.Error()), "error_type": runkit.ErrorType(run.err)})
		return
	}
	s.writeJSON(w, 200, run.result)
}

func (s *Server) cancel(w http.ResponseWriter, _ *http.Request) {
	s.mu.Lock()
	r := s.run
	s.mu.Unlock()
	if r == nil || !r.running() {
		s.writeJSON(w, 200, map[string]any{"cancelled": false})
		return
	}
	r.cancel()
	s.writeJSON(w, 200, map[string]any{"cancelled": true, "run_id": r.id})
}

func (s *Server) runStatus(w http.ResponseWriter, _ *http.Request) {
	scrub := s.scrubber()
	s.mu.Lock()
	r := s.run
	last := s.seq
	var pending []*approval
	for _, ap := range s.approvals {
		pending = append(pending, ap)
	}
	s.mu.Unlock()
	out := map[string]any{"state": StateIdle, "run_id": nil, "session_id": nil, "last_seq": last, "pending_approvals": []any{}}
	if r != nil {
		out["state"] = r.state()
		out["run_id"] = r.id
		out["started_at"] = r.started.UTC().Format(time.RFC3339)
		if sid := r.currentSession(); sid != "" {
			out["session_id"] = sid
		}
	}
	views := make([]map[string]any, 0, len(pending))
	for _, ap := range pending {
		views = append(views, s.approvalView(ap, scrub))
	}
	out["pending_approvals"] = views
	s.writeJSON(w, 200, out)
}

// eventStream is the reattachable stream: GET /api/events?after=N replays what
// the page missed, then follows live.
func (s *Server) eventStream(w http.ResponseWriter, r *http.Request) {
	after := 0
	fmt.Sscanf(r.URL.Query().Get("after"), "%d", &after)
	s.stream(w, r, after, "", false)
}

// stream writes server-sent events. With untilDone it ends after the "done" of
// runID.
func (s *Server) stream(w http.ResponseWriter, r *http.Request, after int, runID string, untilDone bool) {
	fl, ok := w.(http.Flusher)
	if !ok {
		s.detail(w, 500, "streaming is not supported here")
		return
	}
	h := w.Header()
	h.Set("Content-Type", "text/event-stream")
	h.Set("Cache-Control", "no-store")
	h.Set("X-Accel-Buffering", "no")
	w.WriteHeader(200)
	fl.Flush()
	beat := time.NewTicker(15 * time.Second)
	defer beat.Stop()
	for {
		evs, wake := s.eventsAfter(after)
		for _, e := range evs {
			after = e["seq"].(int)
			if untilDone && e["run_id"] != runID {
				continue
			}
			b, _ := json.Marshal(e)
			fmt.Fprintf(w, "id: %d\nevent: %s\ndata: %s\n\n", after, e["type"], b)
			if untilDone && e["type"] == "done" {
				fl.Flush()
				return
			}
		}
		fl.Flush()
		select {
		case <-wake:
		case <-beat.C:
			fmt.Fprint(w, ": keep-alive\n\n")
			fl.Flush()
		case <-r.Context().Done():
			return
		}
	}
}
