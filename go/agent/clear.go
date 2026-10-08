package agent

import (
	"fmt"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// metaCleared marks a tool message whose output was replaced by a marker.
const metaCleared = "tool_output_cleared"

// clearToolOutputs is the size-based clearing pass. When the estimated context
// (messages plus tool specs) passes the configured fraction of the window, every
// tool result except the most recent ClearKeepRecent, and larger than
// ClearMinTokens, is replaced by a one-line marker. The tool message keeps its
// role, name and tool_call_id, so the call it answers stays paired. The marker
// is recorded in the message metadata, which makes the pass idempotent: a
// cleared message is never touched again. It costs no model call.
func clearToolOutputs(s *session.Session, window, specTokens int, st config.ContextSettings, turn int) (event, bool) {
	if !st.Clear || window <= 0 {
		return nil, false
	}
	if float64(MessageTokens(s.Messages)+specTokens) <= st.ClearWindowFraction*float64(window) {
		return nil, false
	}
	prefix := protectedPrefix(s.Messages)
	var toolIdx []int
	for i := prefix; i < len(s.Messages); i++ {
		if s.Messages[i].Role == "tool" {
			toolIdx = append(toolIdx, i)
		}
	}
	keep := max(st.ClearKeepRecent, 0)
	if len(toolIdx) <= keep {
		return nil, false
	}
	var ids []string
	removed := 0
	for _, i := range toolIdx[:len(toolIdx)-keep] {
		m := s.Messages[i]
		if m.Metadata[metaCleared] == true || m.Kind() == session.KindSnipped {
			continue
		}
		before := CountTokens(m.Content)
		if before <= st.ClearMinTokens {
			continue
		}
		name := m.Name
		if name == "" {
			name = "a tool"
		}
		marker := fmt.Sprintf("[Output cleared to save context: %s returned about %d tokens. Run the tool again if you need it.]", name, before)
		after := CountTokens(marker)
		if after >= before {
			continue
		}
		m.Content = marker
		m.SetMeta(metaCleared, true)
		m.SetMeta("original_token_estimate", before)
		m.SetMeta("cleared_turn_index", turn)
		m.Mutate("tool_output_cleared")
		s.Messages[i] = m
		removed += before - after
		ids = append(ids, m.MessageID)
	}
	if len(ids) == 0 {
		return nil, false
	}
	return event{"type": "tool_output_cleared", "turn_index": turn, "cleared_count": len(ids),
		"estimated_tokens_removed": removed, "cleared_message_ids": ids}, true
}
