package agent

import (
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

type event = map[string]any

func collapse(s string, limit int) string {
	s = strings.Join(strings.Fields(s), " ")
	if utf8.RuneCountInString(s) > limit {
		r := []rune(s)
		s = string(r[:limit]) + "..."
	}
	return s
}

// protectedPrefix is the number of leading messages compaction never touches:
// the system message and, if present, the user-context reminder (but never an
// earlier compaction boundary).
func protectedPrefix(msgs []session.Message) int {
	if len(msgs) == 0 || msgs[0].Role != "system" {
		return 0
	}
	if len(msgs) > 1 && msgs[1].IsReminder() && msgs[1].Kind() == "" {
		return 2
	}
	return 1
}

type reduceOpts struct {
	target          int
	allowCompaction bool
	reactive        bool
	preserve        int
	turn            int
	notes           string // vetted session notes to keep alongside a heuristic boundary
}

// reduce shrinks the transcript toward the token target without a model call:
// snip old bulky messages first, then fold history into a boundary note. It
// returns whether anything changed and the events to emit.
func reduce(s *session.Session, o reduceOpts) (bool, []event) {
	var events []event
	changed := false
	for i := 0; i < 6; i++ {
		if MessageTokens(s.Messages) <= o.target {
			break
		}
		if ev, ok := snipPass(s, o); ok {
			changed = true
			events = append(events, ev)
			continue
		}
		if o.allowCompaction {
			if ev, ok := compactPass(s, o); ok {
				changed = true
				events = append(events, ev)
				if o.reactive {
					continue
				}
			}
		}
		break
	}
	return changed, events
}

func snippable(m session.Message) bool {
	switch m.Kind() {
	case session.KindCompactBoundary, session.KindSnipped, session.KindFileHistoryReplay:
		return false
	}
	switch m.Role {
	case "tool":
		return true
	case "assistant":
		return len(m.ToolCalls) > 0 || utf8.RuneCountInString(m.Content) > 600
	case "user":
		return m.Kind() == session.KindContinuation
	}
	return false
}

// snipPass replaces bulky old messages with short tombstones. Roles, ids and
// tool_call linkage are preserved, so the conversation stays valid.
func snipPass(s *session.Session, o reduceOpts) (event, bool) {
	prefix := protectedPrefix(s.Messages)
	end := len(s.Messages) - o.preserve
	total := MessageTokens(s.Messages)
	var ids []string
	removed := 0
	for i := prefix; i < end; i++ {
		m := s.Messages[i]
		if !snippable(m) {
			continue
		}
		if !o.reactive && total <= o.target {
			break
		}
		repl := fmt.Sprintf("%s\nAn earlier %s message (%s) was removed to save context. Preview: %s\n%s",
			session.ReminderOpen, m.Role, m.MessageID, collapse(m.Content, 120), session.ReminderClose)
		before, after := CountTokens(m.Content), CountTokens(repl)
		if after >= before {
			continue
		}
		orig := m
		m.Content = repl
		m.State = session.StateTombstoned
		m.StopReason = "snipped_for_context"
		m.Metadata = map[string]any{
			"kind": session.KindSnipped, "turn_index": o.turn,
			"original_token_estimate": before, "replacement_token_estimate": after,
			"snipped_role": orig.Role, "snipped_message_id": orig.MessageID,
			"snipped_kind": orig.Kind(),
		}
		if orig.Role == "tool" {
			m.Metadata["tool_name"] = orig.Name
			m.Metadata["tool_call_id"] = orig.ToolCallID
		}
		m.Mutate("snip_tombstone")
		s.Messages[i] = m
		total -= before - after
		removed += before - after
		ids = append(ids, orig.MessageID)
		if o.reactive && len(ids) >= 3 {
			break
		}
	}
	if len(ids) == 0 {
		return nil, false
	}
	typ := "snip_boundary"
	if o.reactive {
		typ = "reactive_snip_boundary"
	}
	return event{"type": typ, "turn_index": o.turn, "snipped_count": len(ids),
		"estimated_tokens_removed": removed, "snipped_message_ids": ids}, true
}

func label(m session.Message) string {
	if m.Role == "tool" {
		return "tool:" + m.Name
	}
	return m.Role
}

// pairSafeCut returns the index where the preserved tail starts: the last
// `preserve` messages, moved back so that a kept tool result is never separated
// from the assistant message that made the call. Everything before the result
// is "foldable"; the cut never goes below prefix.
func pairSafeCut(msgs []session.Message, prefix, preserve int) int {
	cut := len(msgs) - preserve
	if cut < prefix {
		cut = prefix
	}
	for cut > prefix && cut < len(msgs) && msgs[cut].Role == "tool" {
		cut--
	}
	return cut
}

// compactPass folds everything between the protected prefix and the preserved
// tail into one boundary note built from a digest of the folded messages (no
// model call). o.notes, when set, is appended (already vetted by the caller).
func compactPass(s *session.Session, o reduceOpts) (event, bool) {
	msgs := s.Messages
	prefix := protectedPrefix(msgs)
	cut := pairSafeCut(msgs, prefix, o.preserve)
	cand := msgs[prefix:cut]
	if len(cand) == 0 {
		return nil, false
	}
	var b strings.Builder
	b.WriteString(session.ReminderOpen + "\n")
	fmt.Fprintf(&b, "Earlier conversation history (%d messages) was compacted to save context. Oldest first:\n", len(cand))
	shown := cand
	if len(shown) > 24 {
		shown = shown[:24]
	}
	for _, m := range shown {
		text := collapse(m.Content, 160)
		if text == "" {
			text = "(empty)"
		}
		fmt.Fprintf(&b, "- %s: %s\n", label(m), text)
	}
	if len(cand) > 24 {
		fmt.Fprintf(&b, "... plus %d older messages\n", len(cand)-24)
	}
	if o.notes != "" {
		b.WriteString("Session notes kept alongside this digest (they may lag the newest messages):\n" + o.notes + "\n")
	}
	b.WriteString("Treat the messages after this note as the working set; rely on them over this digest.\n")
	b.WriteString(session.ReminderClose)
	typ := "compact_boundary"
	if o.reactive {
		typ = "reactive_compact_boundary"
	}
	return foldHistory(s, prefix, cut, b.String(), o, typ, nil), true
}

// foldHistory replaces msgs[prefix:cut] with one boundary message carrying
// body. The cut must already be pair-safe. Earlier boundaries inside the folded
// range deepen the new one. Notes coverage is shifted to the new indexes.
func foldHistory(s *session.Session, prefix, cut int, body string, o reduceOpts, evType string, extra map[string]any) event {
	msgs := s.Messages
	cand, tail := msgs[prefix:cut], msgs[cut:]
	before := MessageTokens(msgs)
	depth, nested := 0, 0
	kinds := map[string]any{}
	ids := make([]string, 0, len(cand))
	for _, m := range cand {
		k := m.Kind()
		if k == "" {
			k = m.Role
		}
		n, _ := config.AsInt(kinds[k])
		kinds[k] = n + 1
		ids = append(ids, m.MessageID)
		if m.Kind() == session.KindCompactBoundary {
			nested++
			d, _ := config.AsInt(m.Metadata["compaction_depth"])
			depth = max(depth, d)
		}
	}
	depth++
	tailIDs := make([]string, 0, len(tail))
	for _, m := range tail {
		tailIDs = append(tailIDs, m.MessageID)
	}
	removed := max(MessageTokens(cand)-CountTokens(body), 0)
	boundary := session.Message{
		Role: "user", Content: body, State: session.StateFinal,
		MessageID: fmt.Sprintf("compact_boundary_%d_%d", o.turn, len(cand)),
		Metadata: map[string]any{
			"kind": session.KindCompactBoundary, "turn_index": o.turn,
			"compacted_message_count": len(cand), "estimated_tokens_before": before,
			"estimated_tokens_removed": removed,
			"preserved_tail_count":     len(tail), "preserved_tail_ids": tailIDs,
			"compaction_depth": depth, "nested_compaction_count": nested,
			"compacted_kinds": kinds, "compacted_message_ids": ids,
		},
	}
	for k, v := range extra {
		boundary.Metadata[k] = v
	}
	coverage, hadNotes := notesCoverage(s)
	out := make([]session.Message, 0, prefix+1+len(tail))
	out = append(out, msgs[:prefix]...)
	out = append(out, boundary)
	out = append(out, tail...)
	s.Messages = out
	if hadNotes {
		setNotesCoverage(s, remapCoverage(coverage, prefix, cut))
	}

	ev := event{"type": evType, "turn_index": o.turn, "compacted_message_count": len(cand),
		"estimated_tokens_before": before, "estimated_tokens_removed": removed,
		"preserved_tail_count": len(tail), "preserved_tail_ids": tailIDs,
		"compaction_depth": depth, "nested_compaction_count": nested, "compacted_message_ids": ids}
	for k, v := range extra {
		ev[k] = v
	}
	return ev
}
