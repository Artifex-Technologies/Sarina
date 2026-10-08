package llm

import (
	"fmt"
	"regexp"
	"strings"
)

// Normalize cleans a model turn for local-server quirks, in place:
//   - inline <think> blocks move from Content to Reasoning;
//   - when the server returned no structured tool calls, calls written as text
//     (<tool_call> blocks, <|python_tag|>, bare JSON) for a known tool become
//     real calls and are removed from the text;
//   - arguments cut off by a length stop are refused rather than executed;
//   - tool-call ids are made non-empty and unique within the message.
func Normalize(t *Turn, toolNames []string) {
	vis, reasoning := splitThink(t.Content)
	if reasoning != "" || vis != t.Content {
		t.Content = vis
		if reasoning != "" {
			if t.Reasoning != "" {
				t.Reasoning += "\n"
			}
			t.Reasoning += reasoning
		}
	}
	if len(t.ToolCalls) == 0 && len(toolNames) > 0 {
		known := map[string]bool{}
		for _, n := range toolNames {
			known[n] = true
		}
		if calls, rest := extractEmbedded(t.Content, known); len(calls) > 0 {
			t.ToolCalls = calls
			t.Content = rest
		}
	}
	truncatedStop := t.FinishReason == "length" || t.FinishReason == "max_tokens"
	for i := range t.ToolCalls {
		c := &t.ToolCalls[i]
		if c.truncated && truncatedStop && c.ParseError == "" {
			c.ParseError = "arguments were cut off by the output length limit"
			c.Arguments = map[string]any{}
		}
	}
	seen := map[string]bool{}
	for i := range t.ToolCalls {
		c := &t.ToolCalls[i]
		id := c.ID
		if id == "" || seen[id] {
			id = fmt.Sprintf("call_%d", i)
			for n := 1; seen[id]; n++ {
				id = fmt.Sprintf("call_%d_%d", i, n)
			}
		}
		c.ID = id
		seen[id] = true
	}
}

func splitThink(s string) (visible, reasoning string) {
	const open, closeTag = "<think>", "</think>"
	if !strings.Contains(s, "think>") {
		return s, ""
	}
	var vis, rea strings.Builder
	for {
		o := strings.Index(s, open)
		c := strings.Index(s, closeTag)
		if c >= 0 && (o < 0 || c < o) {
			rea.WriteString(s[:c])
			s = s[c+len(closeTag):]
			continue
		}
		if o < 0 {
			break
		}
		vis.WriteString(s[:o])
		rest := s[o+len(open):]
		c2 := strings.Index(rest, closeTag)
		if c2 < 0 {
			rea.WriteString(rest)
			s = ""
			break
		}
		rea.WriteString(rest[:c2])
		s = rest[c2+len(closeTag):]
	}
	vis.WriteString(s)
	return strings.TrimSpace(vis.String()), strings.TrimSpace(rea.String())
}

var toolCallBlock = regexp.MustCompile(`(?s)<tool_call>(.*?)(?:</tool_call>|$)`)

func extractEmbedded(content string, known map[string]bool) ([]ToolCall, string) {
	var calls []ToolCall
	rest := content

	// 1. <tool_call>{...}</tool_call> blocks (Qwen / Hermes style).
	if strings.Contains(rest, "<tool_call>") {
		rest = toolCallBlock.ReplaceAllStringFunc(rest, func(block string) string {
			inner := toolCallBlock.FindStringSubmatch(block)[1]
			if c, ok := callFromText(inner, known); ok {
				calls = append(calls, c)
				return ""
			}
			return block
		})
	}

	// 2. Llama python tag: everything after it is the call.
	if idx := strings.Index(rest, "<|python_tag|>"); idx >= 0 && len(calls) == 0 {
		body := rest[idx+len("<|python_tag|>"):]
		for _, stop := range []string{"<|eom_id|>", "<|eot_id|>"} {
			body = strings.TrimSuffix(strings.TrimSpace(body), stop)
		}
		if c, ok := callFromText(body, known); ok {
			calls = append(calls, c)
			rest = rest[:idx]
		}
	}

	// 3. The whole message is a JSON call (or an array of calls).
	if len(calls) == 0 {
		trimmed := stripFences(rest)
		if strings.HasPrefix(trimmed, "{") || strings.HasPrefix(trimmed, "[") {
			if v, _, ok := RepairJSON(trimmed); ok {
				var objs []any
				if arr, isArr := v.([]any); isArr {
					objs = arr
				} else {
					objs = []any{v}
				}
				var found []ToolCall
				for _, o := range objs {
					m, isMap := o.(map[string]any)
					if !isMap {
						found = nil
						break
					}
					c, ok := callFromObject(m, known)
					if !ok {
						found = nil
						break
					}
					found = append(found, c)
				}
				if len(found) > 0 {
					calls = found
					rest = ""
				}
			}
		}
	}
	return calls, strings.TrimSpace(rest)
}

func callFromText(text string, known map[string]bool) (ToolCall, bool) {
	v, _, ok := RepairJSON(text)
	if !ok {
		return ToolCall{}, false
	}
	m, isMap := v.(map[string]any)
	if !isMap {
		return ToolCall{}, false
	}
	return callFromObject(m, known)
}

func callFromObject(m map[string]any, known map[string]bool) (ToolCall, bool) {
	src := m
	if fn, ok := m["function"].(map[string]any); ok {
		src = fn
	}
	name, _ := src["name"].(string)
	if name == "" {
		name, _ = m["name"].(string)
	}
	if !known[name] {
		return ToolCall{}, false
	}
	c := ToolCall{Name: name, Arguments: map[string]any{}}
	args := src["arguments"]
	if args == nil {
		args = src["parameters"]
	}
	if args == nil {
		args = m["arguments"]
	}
	if args == nil {
		args = m["parameters"]
	}
	switch a := args.(type) {
	case nil:
	case map[string]any:
		c.Arguments = a
	case string:
		parsed, trunc, err := ParseArguments(a)
		c.RawArguments = a
		if err != "" {
			c.ParseError = err
		} else {
			c.Arguments = parsed
			c.truncated = trunc
		}
	default:
		c.ParseError = "arguments must be a JSON object"
	}
	return c, true
}
