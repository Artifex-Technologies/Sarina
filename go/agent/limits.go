package agent

import (
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// CountTokens estimates tokens as ceil(chars/4), at least 1 for non-empty text.
func CountTokens(s string) int {
	n := utf8.RuneCountInString(s)
	if n == 0 {
		return 0
	}
	return (n + 3) / 4
}

// InferContextWindow guesses a context window from the model name. An explicit
// value (SARINA_CONTEXT_WINDOW or --context-window) always takes precedence.
func InferContextWindow(model string) int {
	m := strings.ToLower(model)
	switch {
	case strings.Contains(m, "qwen3-coder"), strings.Contains(m, "devstral"):
		return 256000
	case strings.Contains(m, "qwen"):
		return 131072
	case strings.Contains(m, "claude"):
		return 200000
	default:
		return 128000
	}
}

// Snapshot is the pre-flight arithmetic for one request.
type Snapshot struct {
	Window         int
	Projected      int
	MessageTokens  int
	Framing        int
	ReservedOutput int
	Buffer         int
	ReservedSchema int
	Hard           int
	Soft           int
	Overflow       int
	SoftOverflow   int
	ExceedsHard    bool
	ExceedsSoft    bool
}

func wireCallTokens(c llm.WireToolCall) int {
	return CountTokens(c.Function.Name) + CountTokens(c.Function.Arguments) + CountTokens(c.ID)
}

// MessageTokens counts the content of every message plus its tool calls.
func MessageTokens(msgs []session.Message) int {
	n := 0
	for _, m := range msgs {
		n += CountTokens(m.Content)
		for _, c := range m.ToolCalls {
			n += wireCallTokens(c)
		}
	}
	return n
}

func toolSpecTokens(specs []llm.ToolSpec) int {
	n := 0
	for _, s := range specs {
		if b, err := s.MarshalJSON(); err == nil {
			n += CountTokens(string(b))
		}
	}
	return n
}

// Budget constants.
const (
	defaultOutputReserve = 4096
	minOutputReserve     = 1024
	schemaOverhead       = 256
	compactBufferMax     = 13000
)

// TakeSnapshot computes the token budget for the messages plus tool specs.
func TakeSnapshot(window int, msgs []session.Message, specs []llm.ToolSpec, rt config.RuntimeConfig) Snapshot {
	s := Snapshot{Window: window}
	s.MessageTokens = MessageTokens(msgs) + toolSpecTokens(specs)
	for _, m := range msgs {
		s.Framing += 5
		if m.Name != "" {
			s.Framing += 2
		}
		s.Framing += 12 * len(m.ToolCalls)
	}
	s.Framing += 3
	if rt.BudgetConfig.MaxOutputTokens != nil {
		s.ReservedOutput = max(*rt.BudgetConfig.MaxOutputTokens, 0)
	} else {
		s.ReservedOutput = min(defaultOutputReserve, max(window/16, minOutputReserve))
	}
	if rt.OutputSchema != nil {
		s.ReservedSchema = schemaOverhead
	}
	s.Hard = max(window-s.ReservedOutput-s.ReservedSchema, 0)
	if rt.BudgetConfig.MaxInputTokens != nil {
		s.Hard = min(s.Hard, max(*rt.BudgetConfig.MaxInputTokens, 0))
	}
	s.Buffer = min(compactBufferMax, max(window/10, 1024))
	s.Soft = max(s.Hard-s.Buffer, 0)
	s.Projected = s.MessageTokens + s.Framing
	s.Overflow = max(s.Projected-s.Hard, 0)
	s.SoftOverflow = max(s.Projected-s.Soft, 0)
	s.ExceedsHard = s.Overflow > 0
	s.ExceedsSoft = s.SoftOverflow > 0
	return s
}

// counters are the quantities the budgets are checked against.
type counters struct {
	Usage        llm.Usage
	Cost         float64
	ToolCalls    int
	Delegated    int
	ModelCalls   int
	SessionTurns int
}

// Cost prices cumulative usage.
func Cost(p config.Pricing, u llm.Usage) float64 {
	return float64(u.InputTokens)/1e6*p.InputPerMillion +
		float64(u.OutputTokens)/1e6*p.OutputPerMillion +
		float64(u.CacheCreationInputTokens)/1e6*p.CacheCreationPerMillion +
		float64(u.CacheReadInputTokens)/1e6*p.CacheReadPerMillion
}

// checkBudget returns an explanation for the first breached budget (strictly
// greater-than), in the specified order, or "" when none is exceeded.
func checkBudget(b config.BudgetConfig, c counters) string {
	over := func(limit *int, have int, what string) string {
		if limit != nil && have > *limit {
			return fmt.Sprintf("%s budget exceeded: %d used, limit %d", what, have, *limit)
		}
		return ""
	}
	for _, s := range []string{
		over(b.MaxTotalTokens, c.Usage.Total(), "token (total)"),
		over(b.MaxInputTokens, c.Usage.InputTokens, "token (input)"),
		over(b.MaxOutputTokens, c.Usage.OutputTokens, "token (output)"),
		over(b.MaxReasoningTokens, c.Usage.ReasoningTokens, "token (reasoning)"),
	} {
		if s != "" {
			return s
		}
	}
	if b.MaxTotalCostUSD != nil && c.Cost > *b.MaxTotalCostUSD {
		return fmt.Sprintf("cost budget exceeded: $%.6f spent, limit $%.6f", c.Cost, *b.MaxTotalCostUSD)
	}
	for _, s := range []string{
		over(b.MaxToolCalls, c.ToolCalls, "tool-call"),
		over(b.MaxDelegatedTasks, c.Delegated, "delegated-task"),
		over(b.MaxModelCalls, c.ModelCalls, "model-call"),
		over(b.MaxSessionTurns, c.SessionTurns, "session-turn"),
	} {
		if s != "" {
			return s
		}
	}
	return ""
}
