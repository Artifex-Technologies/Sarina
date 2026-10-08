// Package llm is the OpenAI-compatible chat-completions client, tuned for the
// quirks of local model servers.
package llm

import (
	"bytes"
	"encoding/json"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// Usage counts tokens. Total excludes reasoning (assumed to be part of output).
type Usage struct {
	InputTokens              int
	OutputTokens             int
	CacheCreationInputTokens int
	CacheReadInputTokens     int
	ReasoningTokens          int
}

// Total is input + output + cache creation + cache read.
func (u Usage) Total() int {
	return u.InputTokens + u.OutputTokens + u.CacheCreationInputTokens + u.CacheReadInputTokens
}

// Add sums two usages component-wise.
func (u Usage) Add(o Usage) Usage {
	return Usage{
		InputTokens:              u.InputTokens + o.InputTokens,
		OutputTokens:             u.OutputTokens + o.OutputTokens,
		CacheCreationInputTokens: u.CacheCreationInputTokens + o.CacheCreationInputTokens,
		CacheReadInputTokens:     u.CacheReadInputTokens + o.CacheReadInputTokens,
		ReasoningTokens:          u.ReasoningTokens + o.ReasoningTokens,
	}
}

// IsZero reports whether the total is zero.
func (u Usage) IsZero() bool { return u.Total() == 0 && u.ReasoningTokens == 0 }

type usageJSON struct {
	InputTokens              int `json:"input_tokens"`
	OutputTokens             int `json:"output_tokens"`
	CacheCreationInputTokens int `json:"cache_creation_input_tokens"`
	CacheReadInputTokens     int `json:"cache_read_input_tokens"`
	ReasoningTokens          int `json:"reasoning_tokens"`
	TotalTokens              int `json:"total_tokens"`
}

// MarshalJSON writes the five usage keys plus the derived total_tokens.
func (u Usage) MarshalJSON() ([]byte, error) {
	return json.Marshal(usageJSON{u.InputTokens, u.OutputTokens, u.CacheCreationInputTokens,
		u.CacheReadInputTokens, u.ReasoningTokens, u.Total()})
}

// UnmarshalJSON tolerates missing keys, numeric strings and non-objects.
func (u *Usage) UnmarshalJSON(b []byte) error {
	var v any
	if err := json.Unmarshal(b, &v); err != nil {
		*u = Usage{}
		return nil
	}
	m, _ := v.(map[string]any)
	g := func(k string) int { n, _ := config.AsInt(m[k]); return n }
	*u = Usage{g("input_tokens"), g("output_tokens"), g("cache_creation_input_tokens"),
		g("cache_read_input_tokens"), g("reasoning_tokens")}
	return nil
}

// UsageFromProvider maps a provider "usage" object (OpenAI, Ollama or Anthropic
// naming) onto Usage. Non-object input gives zeros.
func UsageFromProvider(v any) Usage {
	m, ok := v.(map[string]any)
	if !ok {
		return Usage{}
	}
	num := func(key string) int {
		if _, isBool := m[key].(bool); isBool {
			return 0
		}
		n, _ := config.AsInt(m[key])
		return n
	}
	first := func(keys ...string) int {
		for _, k := range keys {
			if n := num(k); n != 0 {
				return n
			}
		}
		return 0
	}
	u := Usage{
		InputTokens:              first("input_tokens", "prompt_tokens", "prompt_eval_count"),
		OutputTokens:             first("output_tokens", "completion_tokens", "eval_count"),
		CacheCreationInputTokens: num("cache_creation_input_tokens"),
		CacheReadInputTokens:     num("cache_read_input_tokens"),
		ReasoningTokens:          num("reasoning_tokens"),
	}
	if d, ok := m["completion_tokens_details"].(map[string]any); ok && u.ReasoningTokens == 0 {
		u.ReasoningTokens, _ = config.AsInt(d["reasoning_tokens"])
	}
	if d, ok := m["prompt_tokens_details"].(map[string]any); ok && u.CacheReadInputTokens == 0 {
		u.CacheReadInputTokens, _ = config.AsInt(d["cached_tokens"])
	}
	return u
}

// FunctionCall is the function half of a wire tool call.
type FunctionCall struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"`
}

// WireToolCall is the OpenAI shape for a tool call inside an assistant message.
type WireToolCall struct {
	ID       string       `json:"id"`
	Type     string       `json:"type"`
	Function FunctionCall `json:"function"`
}

// Message is exactly what is sent to the backend per message.
type Message struct {
	Role       string         `json:"role"`
	Content    string         `json:"content"`
	Name       string         `json:"name,omitempty"`
	ToolCallID string         `json:"tool_call_id,omitempty"`
	ToolCalls  []WireToolCall `json:"tool_calls,omitempty"`
}

// ToolCall is a parsed model tool call. If the arguments could not be parsed
// even after repair, ParseError is set and Arguments is empty; the loop turns
// that into a failed tool result instead of aborting.
type ToolCall struct {
	ID           string
	Name         string
	Arguments    map[string]any
	RawArguments string
	ParseError   string
	truncated    bool // arguments only parsed after closing a cut-off structure
}

// ArgumentsJSON returns the arguments as a compact JSON string for replay.
func (c ToolCall) ArgumentsJSON() string {
	if c.Arguments == nil {
		return "{}"
	}
	return string(compactASCII(c.Arguments))
}

// Wire converts to the OpenAI message shape.
func (c ToolCall) Wire() WireToolCall {
	return WireToolCall{ID: c.ID, Type: "function", Function: FunctionCall{Name: c.Name, Arguments: c.ArgumentsJSON()}}
}

func compactASCII(v any) []byte {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return []byte("{}")
	}
	return []byte(escapeNonASCII(strings.TrimRight(buf.String(), "\n")))
}

func escapeNonASCII(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r < 0x80:
			b.WriteRune(r)
		case r > 0xFFFF:
			r -= 0x10000
			b.WriteString(hex4(0xD800 + (r >> 10)))
			b.WriteString(hex4(0xDC00 + (r & 0x3FF)))
		default:
			b.WriteString(hex4(r))
		}
	}
	return b.String()
}

func hex4(r rune) string {
	const digits = "0123456789abcdef"
	return string([]byte{'\\', 'u', digits[(r>>12)&0xF], digits[(r>>8)&0xF], digits[(r>>4)&0xF], digits[r&0xF]})
}

// ToolSpec is a model-visible function definition.
type ToolSpec struct {
	Name        string
	Description string
	Parameters  map[string]any
}

// MarshalJSON produces the OpenAI function-tool shape.
func (t ToolSpec) MarshalJSON() ([]byte, error) {
	params := t.Parameters
	if params == nil {
		params = map[string]any{"type": "object", "properties": map[string]any{}}
	}
	return json.Marshal(map[string]any{
		"type": "function",
		"function": map[string]any{
			"name": t.Name, "description": t.Description, "parameters": params,
		},
	})
}

// Request is one model call.
type Request struct {
	Messages  []Message
	Tools     []ToolSpec
	Schema    *config.OutputSchema
	MaxTokens int // 0 = not sent
	Stream    bool
}

// Turn is the model's reply after normalisation.
type Turn struct {
	Content         string
	Reasoning       string
	ToolCalls       []ToolCall
	FinishReason    string
	HasFinishReason bool
	Usage           Usage
	UsageReported   bool
	Raw             map[string]any
}

// StreamEvent mirrors the events a streaming response produces.
type StreamEvent struct {
	Type           string // message_start, content_delta, tool_call_delta, usage, message_stop
	Delta          string
	ToolCallIndex  int
	ToolCallID     string
	ToolName       string
	ArgumentsDelta string
	FinishReason   string
	Usage          *Usage
}

// BackendError is the single error type for backend problems. Body text of an
// HTTP failure is included in the message because callers match on it.
type BackendError struct {
	Message string
	Status  int
}

func (e *BackendError) Error() string { return e.Message }
