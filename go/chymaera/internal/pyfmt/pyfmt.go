// Package pyfmt reproduces the few Python rendering and truthiness rules the
// Chymaera ports must keep so report text stays byte-compatible with the
// original (str(True) is "True", repr of a string uses single quotes, ...).
package pyfmt

import (
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
	"strings"
)

// Truthy is Python's bool(v) for JSON-decoded values (numbers as json.Number).
func Truthy(v any) bool {
	switch x := v.(type) {
	case nil:
		return false
	case bool:
		return x
	case string:
		return x != ""
	case json.Number:
		f, err := x.Float64()
		return err != nil || f != 0
	case float64:
		return x != 0
	case int:
		return x != 0
	case int64:
		return x != 0
	case []any:
		return len(x) > 0
	case map[string]any:
		return len(x) > 0
	}
	return true
}

// Str is Python's str(v).
func Str(v any) string {
	if s, ok := v.(string); ok {
		return s
	}
	return Repr(v)
}

// Repr is Python's repr(v) for JSON-decoded values.
func Repr(v any) string {
	switch x := v.(type) {
	case nil:
		return "None"
	case bool:
		if x {
			return "True"
		}
		return "False"
	case string:
		return reprString(x)
	case json.Number:
		return x.String()
	case float64:
		return strconv.FormatFloat(x, 'g', -1, 64)
	case []any:
		parts := make([]string, len(x))
		for i, e := range x {
			parts[i] = Repr(e)
		}
		return "[" + strings.Join(parts, ", ") + "]"
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		parts := make([]string, len(keys))
		for i, k := range keys {
			parts[i] = reprString(k) + ": " + Repr(x[k])
		}
		return "{" + strings.Join(parts, ", ") + "}"
	}
	return fmt.Sprint(v)
}

// TypeName is Python's type(v).__name__ for JSON-decoded values.
func TypeName(v any) string {
	switch x := v.(type) {
	case nil:
		return "NoneType"
	case bool:
		return "bool"
	case string:
		return "str"
	case json.Number:
		if strings.ContainsAny(x.String(), ".eE") {
			return "float"
		}
		return "int"
	case []any:
		return "list"
	case map[string]any:
		return "dict"
	}
	return "object"
}

// ReprString is Python's repr(s) for a string.
func ReprString(s string) string { return reprString(s) }

func reprString(s string) string {
	quote := byte('\'')
	if strings.Contains(s, "'") && !strings.Contains(s, `"`) {
		quote = '"'
	}
	var b strings.Builder
	b.WriteByte(quote)
	for _, r := range s {
		switch {
		case r == '\\':
			b.WriteString(`\\`)
		case r == rune(quote):
			b.WriteByte('\\')
			b.WriteRune(r)
		case r == '\n':
			b.WriteString(`\n`)
		case r == '\r':
			b.WriteString(`\r`)
		case r == '\t':
			b.WriteString(`\t`)
		case r < 0x20 || r == 0x7f:
			fmt.Fprintf(&b, `\x%02x`, r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte(quote)
	return b.String()
}
