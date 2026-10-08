package tools

import (
	"fmt"
	"math"
	"strconv"
	"strings"
	"unicode/utf8"
)

// ValidateArgs checks args against a tool's JSON-Schema parameters and returns
// a coerced copy. It enforces: required, type, enum, minimum, maximum,
// minLength, and (recursively) array items and nested object properties.
//
// Local models often send numbers and booleans as strings, so numeric strings
// are accepted for integer/number and "true"/"false" for boolean, and the copy
// holds the coerced value (numbers as float64). A JSON boolean is never
// accepted as a number. Unknown properties are passed through untouched.
func ValidateArgs(schema map[string]any, args map[string]any) (map[string]any, error) {
	if schema == nil {
		return args, nil
	}
	v, err := validateValue(schema, args, "arguments")
	if err != nil {
		return nil, err
	}
	m, _ := v.(map[string]any)
	if m == nil {
		m = map[string]any{}
	}
	return m, nil
}

func schemaMap(v any) map[string]any {
	m, _ := v.(map[string]any)
	return m
}

func stringList(v any) []string {
	switch t := v.(type) {
	case []string:
		return t
	case []any:
		var out []string
		for _, x := range t {
			if s, ok := x.(string); ok {
				out = append(out, s)
			}
		}
		return out
	}
	return nil
}

func validateValue(schema map[string]any, v any, where string) (any, error) {
	typ, _ := schema["type"].(string)
	switch typ {
	case "object":
		m, ok := v.(map[string]any)
		if !ok {
			return nil, fmt.Errorf("%s must be an object", where)
		}
		out := make(map[string]any, len(m))
		for k, x := range m {
			out[k] = x
		}
		for _, name := range stringList(schema["required"]) {
			if _, present := out[name]; !present || out[name] == nil {
				return nil, fmt.Errorf("%s is required", qualify(where, name))
			}
		}
		props := schemaMap(schema["properties"])
		for name, ps := range props {
			x, present := out[name]
			if !present || x == nil {
				delete(out, name)
				continue
			}
			sub := schemaMap(ps)
			if sub == nil {
				continue
			}
			cv, err := validateValue(sub, x, qualify(where, name))
			if err != nil {
				return nil, err
			}
			out[name] = cv
		}
		return out, nil
	case "array":
		list, ok := v.([]any)
		if !ok {
			if ss, isStr := v.([]string); isStr {
				for _, s := range ss {
					list = append(list, s)
				}
			} else {
				return nil, fmt.Errorf("%s must be an array", where)
			}
		}
		items := schemaMap(schema["items"])
		out := make([]any, len(list))
		for i, x := range list {
			if items == nil {
				out[i] = x
				continue
			}
			cv, err := validateValue(items, x, fmt.Sprintf("%s[%d]", where, i))
			if err != nil {
				return nil, err
			}
			out[i] = cv
		}
		return out, nil
	case "string":
		s, ok := v.(string)
		if !ok {
			return nil, fmt.Errorf("%s must be a string", where)
		}
		if n, ok := numberOf(schema["minLength"]); ok && float64(utf8.RuneCountInString(s)) < n {
			if n == 1 {
				return nil, fmt.Errorf("%s must not be empty", where)
			}
			return nil, fmt.Errorf("%s must be at least %v characters", where, n)
		}
		if err := checkEnum(schema, s, where); err != nil {
			return nil, err
		}
		return s, nil
	case "integer", "number":
		f, ok := toNumber(v)
		if !ok {
			return nil, fmt.Errorf("%s must be a %s", where, typ)
		}
		if typ == "integer" && f != math.Trunc(f) {
			return nil, fmt.Errorf("%s must be a whole number, got %v", where, f)
		}
		if lo, ok := numberOf(schema["minimum"]); ok && f < lo {
			return nil, fmt.Errorf("%s must be at least %v, got %v", where, lo, f)
		}
		if hi, ok := numberOf(schema["maximum"]); ok && f > hi {
			return nil, fmt.Errorf("%s must be at most %v, got %v", where, hi, f)
		}
		return f, nil
	case "boolean":
		switch t := v.(type) {
		case bool:
			return t, nil
		case string:
			switch strings.ToLower(strings.TrimSpace(t)) {
			case "true":
				return true, nil
			case "false":
				return false, nil
			}
		}
		return nil, fmt.Errorf("%s must be true or false", where)
	}
	return v, nil
}

func checkEnum(schema map[string]any, s, where string) error {
	enum := stringList(schema["enum"])
	if len(enum) == 0 {
		return nil
	}
	for _, e := range enum {
		if e == s {
			return nil
		}
	}
	return fmt.Errorf("%s must be one of %s, got %q", where, strings.Join(enum, ", "), s)
}

func qualify(where, name string) string {
	if where == "arguments" {
		return name
	}
	return where + "." + name
}

// toNumber accepts numbers and numeric strings, never booleans.
func toNumber(v any) (float64, bool) {
	switch t := v.(type) {
	case float64:
		return t, !math.IsNaN(t) && !math.IsInf(t, 0)
	case float32:
		return float64(t), true
	case int:
		return float64(t), true
	case int64:
		return float64(t), true
	case string:
		f, err := strconv.ParseFloat(strings.TrimSpace(t), 64)
		return f, err == nil && !math.IsNaN(f) && !math.IsInf(f, 0)
	}
	return 0, false
}

func numberOf(v any) (float64, bool) {
	switch t := v.(type) {
	case float64:
		return t, true
	case int:
		return float64(t), true
	case int64:
		return float64(t), true
	}
	return 0, false
}
