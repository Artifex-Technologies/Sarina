package agent

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// Session notes: a running, structured summary of the session kept in
// <session dir>/<id>.notes.md. The session file records, under
// plugin_state.session_notes, how many leading messages the notes already
// cover. A model call refreshes them every N tool-using turns (or when the
// context passes a fraction of the window); compaction reuses fresh notes as
// the summary with no model call at all.

const (
	notesStateKey     = "session_notes"
	notesSectionCap   = 2500 // runes kept per section
	minWindowTurns    = 2    // tool turns between window-triggered updates
	notesToolResultMx = 1500 // chars of one tool result shown to the notes model
)

var notesSections = []string{"Task / goal", "Current state", "Files touched", "Decisions", "Open questions", "Next steps"}

const notesPrompt = `You maintain the working notes for a coding-agent session so the agent can keep going after its older conversation is discarded. You will receive the previous notes (possibly empty) and the newest part of the transcript. Rewrite the notes so they are accurate and complete as of now.

Reply with the notes only: markdown with exactly these six second-level headings, in this order, and nothing before or after them:

## Task / goal
## Current state
## Files touched
## Decisions
## Open questions
## Next steps

Rules: be concrete (paths, commands, error text, identifiers). Keep what the previous notes got right, correct what is now wrong, drop what no longer matters. Record the user's requests and constraints faithfully. Write "(none)" under a heading with nothing to say. Never copy secrets, tokens or passwords into the notes. Do not call tools. Keep it under about 600 words.`

// notesCoverage reports how many leading messages the notes cover. ok is false
// when the session has no notes state. The stored message id guards against a
// transcript that changed underneath the index.
func notesCoverage(s *session.Session) (covered int, ok bool) {
	m, isMap := s.PluginState[notesStateKey].(map[string]any)
	if !isMap {
		return 0, false
	}
	c, _ := config.AsInt(m["last_summarised_index"])
	id, _ := m["last_summarised_message_id"].(string)
	if c <= 0 {
		return 0, true
	}
	if c <= len(s.Messages) && (id == "" || s.Messages[c-1].MessageID == id) {
		return c, true
	}
	for i := len(s.Messages) - 1; i >= 0 && id != ""; i-- {
		if s.Messages[i].MessageID == id {
			return i + 1, true
		}
	}
	return 0, true
}

func setNotesCoverage(s *session.Session, c int) {
	c = max(min(c, len(s.Messages)), 0)
	if s.PluginState == nil {
		s.PluginState = map[string]any{}
	}
	id := ""
	if c > 0 {
		id = s.Messages[c-1].MessageID
	}
	s.PluginState[notesStateKey] = map[string]any{"last_summarised_index": c, "last_summarised_message_id": id}
}

// remapCoverage converts a coverage count from before msgs[prefix:cut] was
// folded into one boundary message to the count afterwards.
func remapCoverage(c, prefix, cut int) int {
	if c >= cut {
		return prefix + 1 + c - cut
	}
	return min(c, prefix)
}

func (a *Agent) notesPath(id string) string {
	return filepath.Join(a.opts.Runtime.SessionDirectory, id+".notes.md")
}

// NotesPath is where the notes for a session live.
func (a *Agent) NotesPath(id string) string { return a.notesPath(id) }

func (a *Agent) readNotes(id string) string {
	b, err := os.ReadFile(a.notesPath(id))
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(b))
}

func writeFileAtomic(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), filepath.Base(path)+".*.tmp")
	if err != nil {
		return err
	}
	name := tmp.Name()
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(name)
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(name)
		return err
	}
	_ = os.Chmod(name, 0o600)
	if err := os.Rename(name, path); err != nil {
		os.Remove(name)
		return err
	}
	return nil
}

// sectionIndex maps a heading to one of notesSections, or -1.
func sectionIndex(heading string) int {
	h := strings.ToLower(strings.Trim(heading, "#*: \t"))
	switch {
	case strings.HasPrefix(h, "task"), strings.HasPrefix(h, "goal"):
		return 0
	case strings.HasPrefix(h, "current"), strings.HasPrefix(h, "state"):
		return 1
	case strings.HasPrefix(h, "files"):
		return 2
	case strings.HasPrefix(h, "decision"):
		return 3
	case strings.HasPrefix(h, "open"), strings.HasPrefix(h, "question"):
		return 4
	case strings.HasPrefix(h, "next"):
		return 5
	}
	return -1
}

// normaliseNotes turns the model's reply into the canonical notes file: fixed
// headings in a fixed order, each capped. A reply with fewer than three
// recognisable sections is rejected.
func normaliseNotes(raw string) (string, bool) {
	var bodies [6][]string
	cur := -1
	for _, line := range strings.Split(raw, "\n") {
		t := strings.TrimSpace(line)
		if strings.HasPrefix(t, "```") {
			continue
		}
		if strings.HasPrefix(t, "#") {
			if i := sectionIndex(t); i >= 0 {
				cur = i
				continue
			}
			if cur < 0 {
				continue // a title line before the first section
			}
		}
		if cur >= 0 {
			bodies[cur] = append(bodies[cur], strings.TrimRight(line, " \t\r"))
		}
	}
	known := 0
	var b strings.Builder
	b.WriteString("# Session notes\n")
	for i, name := range notesSections {
		text := strings.TrimSpace(strings.Join(bodies[i], "\n"))
		if text != "" {
			known++
		} else {
			text = "(none)"
		}
		if utf8.RuneCountInString(text) > notesSectionCap {
			text = string([]rune(text)[:notesSectionCap]) + "\n... (truncated)"
		}
		fmt.Fprintf(&b, "\n## %s\n%s\n", name, text)
	}
	if known < 3 {
		return "", false
	}
	return b.String(), true
}

// renderTranscript flattens messages into plain text for a summarising model.
// Tool calls and results become text, so the request is valid for any backend
// whatever the cut points were.
func renderTranscript(msgs []session.Message, toolChars int) string {
	var b strings.Builder
	for _, m := range msgs {
		switch m.Role {
		case "tool":
			fmt.Fprintf(&b, "[tool result: %s]\n%s\n\n", m.Name, truncateChars(m.Content, toolChars))
		case "assistant":
			if strings.TrimSpace(m.Content) != "" {
				fmt.Fprintf(&b, "[assistant]\n%s\n", truncateChars(m.Content, 4000))
			}
			for _, c := range m.ToolCalls {
				fmt.Fprintf(&b, "[assistant called %s with %s]\n", c.Function.Name, truncateChars(c.Function.Arguments, 400))
			}
			b.WriteString("\n")
		default:
			fmt.Fprintf(&b, "[%s]\n%s\n\n", m.Role, truncateChars(m.Content, 6000))
		}
	}
	return b.String()
}

func truncateChars(s string, n int) string {
	if utf8.RuneCountInString(s) <= n {
		return s
	}
	return string([]rune(s)[:n]) + " ...[truncated]"
}

// tailWithin returns the longest suffix of msgs whose rendering stays under
// maxChars (always at least the last message).
func tailWithin(msgs []session.Message, toolChars, maxChars int) []session.Message {
	total := 0
	for i := len(msgs) - 1; i >= 0; i-- {
		total += utf8.RuneCountInString(renderTranscript(msgs[i:i+1], toolChars))
		if total > maxChars && i < len(msgs)-1 {
			return msgs[i+1:]
		}
	}
	return msgs
}

// charge books a side call (notes or summarising) against the run: its usage,
// estimated if the server reported none, and one model call.
func (r *run) charge(t *llm.Turn, req llm.Request) llm.Usage {
	u := t.Usage
	if !t.UsageReported {
		in := 0
		for _, m := range req.Messages {
			in += CountTokens(m.Content)
		}
		u = llm.Usage{InputTokens: in, OutputTokens: CountTokens(t.Content)}
	}
	r.usage = r.usage.Add(u)
	r.cost = Cost(r.a.opts.Model.Pricing, r.usage)
	r.counts.ModelCalls++
	return u
}

func sideMaxTokens(window, want int) int {
	return max(min(want, window/4), 256)
}

// notesDue reports whether a notes refresh should run after a tool-using turn.
func (a *Agent) notesDue(r *run) (bool, string) {
	st := a.ctxSettings
	if !st.Notes || r.toolTurnsSince == 0 {
		return false, ""
	}
	if r.toolTurnsSince >= st.NotesEveryTurns {
		return true, "cadence"
	}
	if r.toolTurnsSince >= minWindowTurns {
		w, _ := a.resolveWindow()
		used := MessageTokens(r.sess.Messages) + toolSpecTokens(a.registry.Specs())
		if float64(used) >= st.NotesWindowFraction*float64(w) {
			return true, "window"
		}
	}
	return false, ""
}

// maybeUpdateNotes runs after each tool-using turn. It never fails the turn.
func (a *Agent) maybeUpdateNotes(ctx context.Context, r *run, turn int) {
	if !a.ctxSettings.Notes {
		return
	}
	r.toolTurnsSince++
	due, why := a.notesDue(r)
	if !due || ctx.Err() != nil {
		return
	}
	r.toolTurnsSince = 0 // after a failure too: wait out the cadence instead of retrying every turn
	if err := a.updateNotes(ctx, r, turn, why); err != nil {
		r.emit(event{"type": "session_notes_failed", "turn_index": turn, "reason": err.Error()})
	}
}

func (a *Agent) updateNotes(ctx context.Context, r *run, turn int, why string) error {
	s := r.sess
	prefix := protectedPrefix(s.Messages)
	covered, _ := notesCoverage(s)
	covered = max(covered, prefix)
	if covered >= len(s.Messages) {
		return nil
	}
	window, _ := a.resolveWindow()
	fresh := tailWithin(s.Messages[covered:], notesToolResultMx, max(window, 4096)*2)
	prev := a.readNotes(s.ID)
	if prev == "" {
		prev = "(no notes yet)"
	}
	req := llm.Request{
		Messages: []llm.Message{
			{Role: "system", Content: notesPrompt},
			{Role: "user", Content: "Previous notes:\n" + prev + "\n\nNewest part of the transcript:\n" + renderTranscript(fresh, notesToolResultMx) +
				"\nWrite the updated notes now."},
		},
		MaxTokens: sideMaxTokens(window, 2048),
	}
	t, err := a.opts.Client.Complete(ctx, req, nil)
	if err != nil {
		return fmt.Errorf("notes model call failed: %w", err)
	}
	u := r.charge(t, req)
	text, ok := normaliseNotes(t.Content)
	if !ok {
		return fmt.Errorf("notes reply had no usable sections")
	}
	if err := writeFileAtomic(a.notesPath(s.ID), []byte(text)); err != nil {
		return fmt.Errorf("cannot write notes: %w", err)
	}
	setNotesCoverage(s, len(s.Messages))
	r.emit(event{"type": "session_notes_updated", "turn_index": turn, "trigger": why,
		"covered_messages": len(s.Messages), "usage_tokens": u.Total(), "path": a.notesPath(s.ID)})
	return nil
}

// vettedNotes returns the session's notes if they exist and pass the scanner.
func (a *Agent) vettedNotes(r *run, turn int) string {
	if !a.ctxSettings.Notes {
		return ""
	}
	text := a.readNotes(r.sess.ID)
	if text == "" {
		return ""
	}
	if res := a.scanner().Scan(text); res.Blocked {
		r.emit(event{"type": "session_notes_blocked", "turn_index": turn, "reason": res.Reason})
		return ""
	}
	return text
}
