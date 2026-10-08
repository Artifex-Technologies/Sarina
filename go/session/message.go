// Package session holds the message model, the secret redactor, and the
// on-disk session file that Chymaera_Sentry reads.
package session

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Message states.
const (
	StateFinal      = "final"
	StateStreaming  = "streaming"
	StateTombstoned = "tombstoned"
)

// Message kinds (metadata "kind") the loop relies on.
const (
	KindCompactBoundary   = "compact_boundary"
	KindSnipped           = "snipped_message"
	KindContinuation      = "continuation_request"
	KindFileHistoryReplay = "file_history_replay"
	KindCompactionReplay  = "compaction_replay"
	ReminderOpen          = "<system-reminder>"
	ReminderClose         = "</system-reminder>"
	mutationTotalsKey     = "mutation_totals"
	lineageKey            = "lineage_id"
	revisionKey           = "revision"
)

// Message is a transcript entry: what is sent to the model plus bookkeeping.
type Message struct {
	Role       string
	Content    string
	Name       string
	ToolCallID string
	ToolCalls  []llm.WireToolCall
	MessageID  string
	State      string
	StopReason string
	Usage      llm.Usage
	Metadata   map[string]any
}

// Wire returns the exact shape sent to the backend.
func (m Message) Wire() llm.Message {
	return llm.Message{Role: m.Role, Content: m.Content, Name: m.Name, ToolCallID: m.ToolCallID, ToolCalls: m.ToolCalls}
}

// Kind returns metadata "kind" or "".
func (m Message) Kind() string {
	k, _ := m.Metadata["kind"].(string)
	return k
}

// IsReminder reports whether the message is a <system-reminder> envelope.
func (m Message) IsReminder() bool {
	return m.Role == "user" && strings.HasPrefix(m.Content, ReminderOpen)
}

// SetMeta sets one metadata key, allocating the map if needed.
func (m *Message) SetMeta(key string, v any) {
	if m.Metadata == nil {
		m.Metadata = map[string]any{}
	}
	m.Metadata[key] = v
}

// Mutate records an in-place change: lineage id, revision, per-kind totals.
func (m *Message) Mutate(kind string) {
	if m.Metadata == nil {
		m.Metadata = map[string]any{}
	}
	if _, ok := m.Metadata[lineageKey]; !ok {
		id := m.MessageID
		if id == "" {
			id = m.Role + "_lineage"
		}
		m.Metadata[lineageKey] = id
	}
	rev, _ := config.AsInt(m.Metadata[revisionKey])
	m.Metadata[revisionKey] = rev + 1
	totals := map[string]any{}
	switch t := m.Metadata[mutationTotalsKey].(type) {
	case map[string]any:
		totals = t
	case map[string]int:
		for k, v := range t {
			totals[k] = v
		}
	}
	n, _ := config.AsInt(totals[kind])
	totals[kind] = n + 1
	m.Metadata[mutationTotalsKey] = totals
}

// Blocks derives the structured mirror of the message (not authoritative).
func (m Message) Blocks() []map[string]any {
	blocks := []map[string]any{}
	switch m.Role {
	case "tool":
		if m.Content != "" {
			blocks = append(blocks, map[string]any{
				"type": "tool_result", "name": m.Name, "tool_call_id": m.ToolCallID, "text": m.Content})
		}
	default:
		if m.Content != "" {
			blocks = append(blocks, map[string]any{"type": "text", "text": m.Content})
		}
		for _, c := range m.ToolCalls {
			blocks = append(blocks, map[string]any{
				"type": "tool_call", "id": c.ID, "name": c.Function.Name, "arguments": c.Function.Arguments})
		}
	}
	return blocks
}

type messageJSON struct {
	Role       string             `json:"role"`
	Content    string             `json:"content"`
	Name       string             `json:"name,omitempty"`
	ToolCallID string             `json:"tool_call_id,omitempty"`
	ToolCalls  []llm.WireToolCall `json:"tool_calls,omitempty"`
	Blocks     []map[string]any   `json:"blocks"`
	MessageID  string             `json:"message_id,omitempty"`
	State      string             `json:"state,omitempty"`
	StopReason string             `json:"stop_reason,omitempty"`
	Usage      *llm.Usage         `json:"usage,omitempty"`
	Metadata   map[string]any     `json:"metadata,omitempty"`
}

// MarshalJSON writes the stored form (sent shape plus bookkeeping).
func (m Message) MarshalJSON() ([]byte, error) {
	j := messageJSON{
		Role: m.Role, Content: m.Content, Name: m.Name, ToolCallID: m.ToolCallID,
		ToolCalls: m.ToolCalls, Blocks: m.Blocks(), MessageID: m.MessageID,
		StopReason: m.StopReason, Metadata: m.Metadata,
	}
	if m.State != "" && m.State != StateFinal {
		j.State = m.State
	}
	if m.Usage.Total() != 0 {
		u := m.Usage
		j.Usage = &u
	}
	return marshalRaw(j)
}

// MessageFromMap loads a stored message tolerantly.
func MessageFromMap(m map[string]any) Message {
	str := func(k string) string { s, _ := m[k].(string); return s }
	msg := Message{
		Role: str("role"), Content: str("content"), Name: str("name"),
		ToolCallID: str("tool_call_id"), MessageID: str("message_id"),
		State: str("state"), StopReason: str("stop_reason"),
	}
	if msg.State == "" {
		msg.State = StateFinal
	}
	if list, ok := m["tool_calls"].([]any); ok {
		for _, item := range list {
			tm, ok := item.(map[string]any)
			if !ok {
				continue
			}
			fn, _ := tm["function"].(map[string]any)
			id, _ := tm["id"].(string)
			name, _ := fn["name"].(string)
			args, _ := fn["arguments"].(string)
			msg.ToolCalls = append(msg.ToolCalls, llm.WireToolCall{
				ID: id, Type: "function", Function: llm.FunctionCall{Name: name, Arguments: args}})
		}
	}
	if u, ok := m["usage"].(map[string]any); ok {
		msg.Usage = llm.UsageFromProvider(u)
	}
	if md, ok := m["metadata"].(map[string]any); ok && len(md) > 0 {
		msg.Metadata = md
	}
	return msg
}

// EncodeJSON renders v as 2-space indented, ASCII-only JSON (non-ASCII becomes
// \uXXXX; <, > and & stay literal), with no trailing newline.
func EncodeJSON(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return []byte(asciiOnly(strings.TrimRight(buf.String(), "\n"))), nil
}

func asciiOnly(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r < 0x80:
			b.WriteRune(r)
		case r > 0xFFFF:
			r -= 0x10000
			fmt.Fprintf(&b, "\\u%04x\\u%04x", 0xD800+(r>>10), 0xDC00+(r&0x3FF))
		default:
			fmt.Fprintf(&b, "\\u%04x", r)
		}
	}
	return b.String()
}

type toolBody struct {
	Tool     string         `json:"tool"`
	OK       bool           `json:"ok"`
	Content  string         `json:"content"`
	Metadata map[string]any `json:"metadata,omitempty"`
}

// ToolResultBody serialises a tool result the way the model sees it.
func ToolResultBody(tool string, ok bool, content string, metadata map[string]any) string {
	b, err := EncodeJSON(toolBody{tool, ok, content, metadata})
	if err != nil {
		b, _ = EncodeJSON(toolBody{tool, ok, content, nil})
	}
	return string(b)
}

// NotRunMessage answers a tool call that was deliberately not executed, so the
// conversation stays valid (every tool_call needs a tool result).
func NotRunMessage(id, name, reason, messageID string) Message {
	md := map[string]any{"error_kind": "not_run", "tool_name": name, "tool_call_id": id, "phase": "completed"}
	return Message{
		Role: "tool", Name: name, ToolCallID: id, MessageID: messageID, State: StateFinal,
		StopReason: "tool_not_run",
		Content:    ToolResultBody(name, false, reason, map[string]any{"error_kind": "not_run"}),
		Metadata:   md,
	}
}

// RepairPairing makes a transcript valid for strict backends: every assistant
// tool call gets a tool result (a synthetic "not run" one if missing) placed
// right after its siblings, and tool messages with no matching call are dropped.
func RepairPairing(msgs []Message) []Message {
	out := make([]Message, 0, len(msgs))
	for i := 0; i < len(msgs); i++ {
		m := msgs[i]
		if m.Role == "tool" {
			continue // orphan: real results are consumed with their assistant below
		}
		out = append(out, m)
		if m.Role != "assistant" || len(m.ToolCalls) == 0 {
			continue
		}
		want := map[string]bool{}
		for _, c := range m.ToolCalls {
			want[c.ID] = true
		}
		answered := map[string]bool{}
		j := i + 1
		for ; j < len(msgs) && msgs[j].Role == "tool"; j++ {
			if want[msgs[j].ToolCallID] && !answered[msgs[j].ToolCallID] {
				answered[msgs[j].ToolCallID] = true
				out = append(out, msgs[j])
			}
		}
		for _, c := range m.ToolCalls {
			if !answered[c.ID] {
				out = append(out, NotRunMessage(c.ID, c.Function.Name,
					"this call has no recorded result (the session stopped before it ran)", fmt.Sprintf("tool_repair_%s", c.ID)))
			}
		}
		i = j - 1
	}
	return out
}

// marshalRaw is json.Marshal without HTML escaping, so <, > and & stay literal
// like Python's json.dumps.
func marshalRaw(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimRight(buf.Bytes(), "\n"), nil
}
