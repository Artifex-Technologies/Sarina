package pyfmt

import (
	"bytes"
	"encoding/json"
	"math"
	"math/rand"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

var parityDocs = []string{
	`null`, `true`, `false`, `0`, `-0`, `1`, `1.0`, `1.5`, `1e3`, `1E-7`, `-2.50`, `123456789012345678901234567890`, `1e400`, `0.0`,
	`""`, `"a"`, `"it's"`, `"say \"hi\""`, `"both ' and \""`, `"back\\slash"`, `"\n\r\t"`, `"\u0001\u001f\u007f"`, `"\u0080 é"`,
	`" 😀"`, `"\ud800"`, `"x\u0000y"`, `"'"`, `"\""`, `[]`, `[1,"a",null,[true]]`, `{}`, `{"b":1,"a":"x","c":[]}`,
	`{"it's":{"z":false}}`, `[{"k":1.25e2}]`, `{"":null}`,
}

func genDoc(rng *rand.Rand, depth int) string {
	switch n := rng.Intn(8); {
	case n == 0 && depth < 3:
		var parts []string
		for i := rng.Intn(4); i > 0; i-- {
			parts = append(parts, genDoc(rng, depth+1))
		}
		return "[" + strings.Join(parts, ",") + "]"
	case n == 1 && depth < 3:
		var parts []string
		for i := rng.Intn(4); i > 0; i-- {
			parts = append(parts, `"`+string(rune('a'+rng.Intn(4)))+`":`+genDoc(rng, depth+1))
		}
		return "{" + strings.Join(parts, ",") + "}"
	case n < 5:
		return []string{`0`, `2`, `-3.5`, `1e21`, `1e20`, `0.000001`, `0.0000001`, `100000`, `1000000`, `12.0`, `7e-5`, `-0.0`}[rng.Intn(12)]
	}
	var b strings.Builder
	b.WriteByte('"')
	for i := rng.Intn(6); i > 0; i-- {
		b.WriteString([]string{"a", "'", `\"`, `\\`, `\n`, `\u0007`, "é", `­`, " ", `😀`, `\u007f`}[rng.Intn(11)])
	}
	b.WriteByte('"')
	return b.String()
}

// typed is a decoded value written so the Rust side rebuilds it exactly: a json.Number keeps its text, a float64 its bits,
// a string goes through parity.Text, an object's keys are kept as Go decoded them.
func typed(v any) any {
	switch x := v.(type) {
	case json.Number:
		return map[string]any{"n": string(x)}
	case float64:
		return map[string]any{"f": strconv.FormatUint(math.Float64bits(x), 16)}
	case string:
		return map[string]any{"s": parity.Text(x)}
	case []any:
		out := []any{}
		for _, e := range x {
			out = append(out, typed(e))
		}
		return out
	case map[string]any:
		m := map[string]any{}
		for k, e := range x {
			m[k] = typed(e)
		}
		return map[string]any{"o": m}
	}
	return v // nil, bool
}

// TestParityPyfmt records Truthy, Str, Repr and TypeName on JSON values decoded with UseNumber (as the Chymaera ports read
// them) and without it (float64), and ReprString (testdata/parity/pyfmt.json; docs/RUST_PORT.md).
func TestParityPyfmt(t *testing.T) {
	docs := append([]string(nil), parityDocs...)
	rng := rand.New(rand.NewSource(3))
	for i := 0; i < 300; i++ {
		docs = append(docs, genDoc(rng, 0))
	}
	var cases []parity.Case
	for _, doc := range docs {
		out := map[string]any{}
		for _, useNumber := range []bool{true, false} {
			dec := json.NewDecoder(bytes.NewReader([]byte(doc)))
			if useNumber {
				dec.UseNumber()
			}
			var v any
			if err := dec.Decode(&v); err != nil {
				out[map[bool]string{true: "number", false: "float"}[useNumber]] = map[string]any{"err": err.Error()}
				continue
			}
			out[map[bool]string{true: "number", false: "float"}[useNumber]] = map[string]any{"value": typed(v),
				"truthy": Truthy(v), "str": parity.Text(Str(v)), "repr": parity.Text(Repr(v)), "type": TypeName(v),
			}
		}
		cases = append(cases, parity.Case{Fn: "pyfmt", In: doc, Out: out}) // the text: it may hold a lone surrogate
	}
	for _, s := range []string{"", "a", "'", "\"", "'\"", "\\", "\x00\x1f\x7f", "\u0080 ", "\xff", "a\xc3", "é'"} {
		cases = append(cases, parity.Case{Fn: "ReprString", In: parity.Text(s), Out: parity.Text(ReprString(s))})
	}
	parity.Fixture(t, "pyfmt", cases)
}
