package llm

import (
	"bytes"
	"encoding/json"
	"strings"
)

// decodeFirst decodes the first JSON value in s, ignoring trailing text.
func decodeFirst(s string) (any, bool) {
	dec := json.NewDecoder(strings.NewReader(s))
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil, false
	}
	return v, true
}

func stripFences(s string) string {
	s = strings.TrimSpace(s)
	if strings.HasPrefix(s, "```") {
		if nl := strings.IndexByte(s, '\n'); nl >= 0 {
			s = s[nl+1:]
		} else {
			s = strings.TrimPrefix(s, "```")
		}
		s = strings.TrimSpace(strings.TrimSuffix(strings.TrimSpace(s), "```"))
	}
	return s
}

// RepairJSON parses s as JSON, tolerating the damage small local models do:
// code fences, trailing commas, single-quoted strings, raw newlines inside
// strings, and output truncated mid-structure. truncated reports that the
// result needed closing characters added, i.e. the input was cut off.
func RepairJSON(s string) (v any, truncated bool, ok bool) {
	s = stripFences(s)
	if v, ok := decodeFirst(s); ok {
		return v, false, true
	}
	fixed, trunc := repairText(s)
	if v, ok := decodeFirst(fixed); ok {
		return v, trunc, true
	}
	return nil, false, false
}

func repairText(s string) (string, bool) {
	var out []byte
	var stack []byte
	inDouble, inSingle := false, false
	trimComma := func() {
		i := len(out)
		for i > 0 && (out[i-1] == ' ' || out[i-1] == '\n' || out[i-1] == '\t' || out[i-1] == '\r') {
			i--
		}
		if i > 0 && out[i-1] == ',' {
			out = out[:i-1]
		}
	}
	escCtl := func(c byte) bool {
		switch c {
		case '\n':
			out = append(out, '\\', 'n')
		case '\r':
			out = append(out, '\\', 'r')
		case '\t':
			out = append(out, '\\', 't')
		default:
			return false
		}
		return true
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case inDouble:
			switch {
			case c == '\\' && i+1 < len(s):
				out = append(out, c, s[i+1])
				i++
			case c == '"':
				inDouble = false
				out = append(out, c)
			case escCtl(c):
			default:
				out = append(out, c)
			}
		case inSingle:
			switch {
			case c == '\\' && i+1 < len(s):
				if s[i+1] == '\'' {
					out = append(out, '\'')
				} else {
					out = append(out, c, s[i+1])
				}
				i++
			case c == '\'':
				inSingle = false
				out = append(out, '"')
			case c == '"':
				out = append(out, '\\', '"')
			case escCtl(c):
			default:
				out = append(out, c)
			}
		default:
			switch c {
			case '"':
				inDouble = true
				out = append(out, c)
			case '\'':
				inSingle = true
				out = append(out, '"')
			case '{':
				stack = append(stack, '}')
				out = append(out, c)
			case '[':
				stack = append(stack, ']')
				out = append(out, c)
			case '}', ']':
				trimComma()
				if len(stack) > 0 && stack[len(stack)-1] == c {
					stack = stack[:len(stack)-1]
				}
				out = append(out, c)
			default:
				out = append(out, c)
			}
		}
	}
	truncated := false
	if inDouble || inSingle {
		out = append(out, '"')
		truncated = true
	}
	if len(stack) > 0 {
		truncated = true
		out = bytes.TrimRight(out, " \t\r\n")
		trimComma()
		if n := len(out); n > 0 && out[n-1] == ':' {
			out = append(out, "null"...)
		}
		for i := len(stack) - 1; i >= 0; i-- {
			out = append(out, stack[i])
		}
	}
	return string(out), truncated
}

// ParseArguments turns an arguments string into an object. Empty means {}.
// err is non-empty when the text is not usable as an object.
func ParseArguments(raw string) (args map[string]any, truncated bool, err string) {
	if strings.TrimSpace(raw) == "" {
		return map[string]any{}, false, ""
	}
	v, trunc, ok := RepairJSON(raw)
	if !ok {
		return nil, false, "arguments are not valid JSON"
	}
	m, isMap := v.(map[string]any)
	if !isMap {
		return nil, false, "arguments must be a JSON object"
	}
	return m, trunc, ""
}
