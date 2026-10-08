package agent

import (
	"context"
	"fmt"
	"regexp"
	"strings"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// Compaction with a summary. Order of preference when history must shrink:
//  1. fresh session notes become the summary (no model call),
//  2. a summarising model call (retrying with less history if the request is
//     itself too long), guarded by a circuit breaker,
//  3. the heuristic digest of compact.go, which cannot fail.
//
// The cut is always pair-safe (pairSafeCut), and the model is sent a flattened
// text transcript, so no path can leave a tool call without its result.

const (
	breakerLimit     = 3
	maxSummaryRetry  = 3
	summaryToolChars = 2000
)

const summaryPrompt = `You are condensing the older part of a coding-agent conversation so the agent can continue with a much shorter history. You will receive the transcript to condense, and sometimes working notes written earlier. Everything you write replaces that older history, so anything you leave out is lost to the agent.

Answer in plain text only. Do not call tools. Put the summary between <summary> and </summary> tags, using these headings in this order:

1. User's goal and standing instructions
2. Everything the user asked for, in order (every user message, close to verbatim)
3. Work done so far (what was tried, what worked)
4. Files and commands involved, and why they matter (paths, names, key snippets)
5. Errors met and how they were fixed or worked around
6. Decisions taken and facts learned that must not be re-discovered
7. Work still pending
8. Exact current state and the very next step, consistent with the user's latest request

Be specific and compact. Never include secrets, tokens or passwords; refer to them by name only.`

var (
	analysisRe = regexp.MustCompile(`(?s)<analysis>.*?</analysis>`)
	summaryRe  = regexp.MustCompile(`(?s)<summary>(.*?)</summary>`)
	blankRunRe = regexp.MustCompile(`\n{3,}`)
)

// cleanSummary strips a scratch <analysis> block and unwraps <summary>.
func cleanSummary(s string) string {
	s = analysisRe.ReplaceAllString(s, "")
	if m := summaryRe.FindStringSubmatch(s); m != nil {
		s = m[1]
	} else {
		s = strings.NewReplacer("<summary>", "", "</summary>", "").Replace(s)
	}
	s = strings.NewReplacer(session.ReminderOpen, "", session.ReminderClose, "").Replace(s)
	return strings.TrimSpace(blankRunRe.ReplaceAllString(s, "\n\n"))
}

var tooLongTextPhrases = append([]string{"prompt_too_long", "context_length_exceeded"}, promptTooLongPhrases...)

func replyTooLong(text string) bool {
	l := strings.ToLower(text)
	for _, p := range tooLongTextPhrases {
		if strings.Contains(l, p) {
			return true
		}
	}
	return false
}

func errTooLong(err error) bool {
	if isPromptTooLong(err) {
		return true
	}
	l := strings.ToLower(err.Error())
	return strings.Contains(l, "prompt") && strings.Contains(l, "long")
}

// groupMessages splits a transcript into rounds: each starts at a user or
// assistant message and carries the tool results that follow it.
func groupMessages(msgs []session.Message) [][]session.Message {
	var groups [][]session.Message
	for _, m := range msgs {
		if m.Role == "tool" && len(groups) > 0 {
			groups[len(groups)-1] = append(groups[len(groups)-1], m)
			continue
		}
		groups = append(groups, []session.Message{m})
	}
	return groups
}

func flatten(groups [][]session.Message) []session.Message {
	var out []session.Message
	for _, g := range groups {
		out = append(out, g...)
	}
	return out
}

func foldedBody(n int, text, closing string) string {
	return fmt.Sprintf("%s\nEarlier conversation history (%d messages) was compacted to save context; the session continues from this.\n%s\n%s\n%s",
		session.ReminderOpen, n, text, closing, session.ReminderClose)
}

// notesCompact folds the history into the session notes when they are fresh
// (cover everything that would be folded), pass the scanner, and exist. No model
// call is made.
func (a *Agent) notesCompact(r *run, turn, preserve int, reactive bool) bool {
	s := r.sess
	prefix := protectedPrefix(s.Messages)
	cut := pairSafeCut(s.Messages, prefix, preserve)
	if cut <= prefix {
		return false
	}
	covered, ok := notesCoverage(s)
	if !ok || covered < cut {
		return false
	}
	notes := a.vettedNotes(r, turn)
	if notes == "" {
		return false
	}
	body := foldedBody(cut-prefix, notes, "Treat the messages after this note as the working set; rely on them over these notes.")
	ev := foldHistory(s, prefix, cut, body, reduceOpts{preserve: preserve, turn: turn, reactive: reactive},
		"auto_compact_summary", map[string]any{"summary_source": "notes", "summary_model_calls": 0})
	r.emit(ev)
	return true
}

// compactHistory shrinks the transcript with a summary-style compaction and
// reports how (empty string: nothing was compacted).
func (a *Agent) compactHistory(ctx context.Context, r *run, turn, preserve int, allowLLM bool) string {
	s := r.sess
	prefix := protectedPrefix(s.Messages)
	cut := pairSafeCut(s.Messages, prefix, preserve)
	if cut <= prefix {
		return ""
	}
	if a.notesCompact(r, turn, preserve, false) {
		return "notes"
	}
	notes := a.vettedNotes(r, turn)
	if allowLLM && a.ctxSettings.LLMCompaction && ctx.Err() == nil {
		if r.compactFailures >= breakerLimit {
			r.emit(event{"type": "auto_compact_circuit_breaker", "turn_index": turn, "failures": r.compactFailures})
		} else if a.llmCompact(ctx, r, turn, prefix, cut, preserve, notes) {
			return "llm"
		}
	}
	if ctx.Err() != nil {
		return ""
	}
	ev, ok := compactPass(s, reduceOpts{preserve: preserve, turn: turn, notes: notes})
	if !ok {
		return ""
	}
	r.emit(ev)
	return "heuristic"
}

// llmCompact asks the model to summarise msgs[prefix:cut] and folds the history
// into the answer. Any failure is counted against the breaker, reported, and
// leaves the transcript untouched so the caller can fall back.
func (a *Agent) llmCompact(ctx context.Context, r *run, turn, prefix, cut, preserve int, notes string) bool {
	s := r.sess
	window, _ := a.resolveWindow()
	groups := groupMessages(s.Messages[prefix:cut])
	count := cut - prefix
	budgetChars := max(window, 2048) * 3 // leave room for instructions and the reply
	var usage llm.Usage
	retries, dropped := 0, 0

	fail := func(reason string) bool {
		r.compactFailures++
		r.emit(event{"type": "auto_compact_failed", "turn_index": turn, "reason": reason, "count": r.compactFailures})
		return false
	}
	drop := func() bool {
		if len(groups) < 2 {
			return false
		}
		n := max(len(groups)/5, 1)
		n = min(n, len(groups)-1)
		for _, g := range groups[:n] {
			dropped += len(g)
		}
		groups = groups[n:]
		return true
	}

	for {
		// Shed the oldest rounds up front if the text would not fit the window anyway.
		text := renderTranscript(flatten(groups), summaryToolChars)
		for utf8.RuneCountInString(text) > budgetChars && drop() {
			text = renderTranscript(flatten(groups), summaryToolChars)
		}
		var b strings.Builder
		if notes != "" {
			b.WriteString("Working notes written earlier (they may lag the newest messages):\n" + notes + "\n\n")
		}
		if dropped > 0 {
			fmt.Fprintf(&b, "(The oldest %d messages did not fit and are not shown.)\n\n", dropped)
		}
		b.WriteString("Transcript to condense:\n" + text + "\nWrite the summary now, as plain text, with no tool calls.")
		req := llm.Request{
			Messages:  []llm.Message{{Role: "system", Content: summaryPrompt}, {Role: "user", Content: b.String()}},
			MaxTokens: sideMaxTokens(window, 3000),
		}
		t, err := a.opts.Client.Complete(ctx, req, nil)
		tooLong := false
		switch {
		case err != nil:
			if ctx.Err() != nil {
				return false // interrupted: not the summariser's fault
			}
			if !errTooLong(err) {
				return fail("model call failed: " + err.Error())
			}
			tooLong = true
		default:
			usage = usage.Add(r.charge(t, req))
			if replyTooLong(t.Content) && len(t.Content) < 400 {
				tooLong = true
			}
		}
		if tooLong {
			if retries >= maxSummaryRetry || !drop() {
				return fail("compact request itself too long")
			}
			retries++
			continue
		}
		summary := cleanSummary(t.Content)
		if summary == "" {
			return fail("empty_response")
		}
		if res := a.scanner().Scan(summary); res.Blocked {
			return fail("summary rejected by the security scan: " + res.Reason)
		}
		body := foldedBody(count, "Summary of that history:\n"+summary,
			"Treat the messages after this note as the working set; rely on them over this summary.")
		ev := foldHistory(s, prefix, cut, body, reduceOpts{preserve: preserve, turn: turn}, "auto_compact_summary",
			map[string]any{"summary_source": "llm", "summary_model_calls": retries + 1,
				"summary_usage_tokens": usage.Total(), "ptl_retries": retries})
		r.compactFailures = 0
		r.emit(ev)
		return true
	}
}
