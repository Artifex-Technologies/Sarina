package parity

import (
	"encoding/json"
	"math"
	"strconv"
)

// Typed is a decoded JSON value (what encoding/json puts in an `any`) written so the Rust side rebuilds it exactly: a
// float64 as its bits {"f": hex}, a json.Number as {"n": text}, a string as {"s": Text}, an object as {"o": {...}} (its
// keys as Go decoded them), arrays, bools and nil as themselves. An int (set in memory, never decoded) is {"i": decimal}.
func Typed(v any) any {
	switch x := v.(type) {
	case json.Number:
		return map[string]any{"n": string(x)}
	case float64:
		return map[string]any{"f": strconv.FormatUint(math.Float64bits(x), 16)}
	case int:
		return map[string]any{"i": strconv.Itoa(x)}
	case string:
		return map[string]any{"s": Text(x)}
	case []any:
		out := []any{}
		for _, e := range x {
			out = append(out, Typed(e))
		}
		return out
	case map[string]any:
		m := map[string]any{}
		for k, e := range x {
			m[k] = Typed(e)
		}
		return map[string]any{"o": m}
	}
	return v // nil, bool
}
