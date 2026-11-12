package tools

import (
	"encoding/json"
	"math/rand"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// numberTexts are strings a local model may send for a number: what strconv.ParseFloat accepts (underscores, hex floats,
// a bare point, the infinities) and what it refuses, with spaces TrimSpace removes or keeps.
var numberTexts = []string{
	"0", "1", "-1", "+1", "1.5", "-0", ".5", "5.", ".", "-.5", "+.5e1", "1e5", "1E5", "1e+5", "1e-5", "1e", "1e+", "e5", "1.2.3",
	"1_000", "1__0", "_1", "1_", "1_0.5", "1._5", "0x1p4", "0X1P-2", "0x1.8p1", "0x_1p0", "0x1", "0x10", "0xp1", "0x.p1", "0x1.p1",
	"0x1fffffffffffffp0", "0x1.fffffffffffff8p1023", "0x1p1024", "0x1p-1074", "0x1p-1075", "0x1.8p-1075", "0x1p-2000",
	"0x123456789abcdef0123p-80", "0x1.0000000000000801p0", "0x1.00000000000008p0", "0x1.00000000000018p0", "0b101", "0o7", "07",
	"1e400", "-1e400", "1e-400", "4.9e-324", "2.4703282292062327e-324", "2.4703282292062328e-324", "1.7976931348623157e308",
	"1.7976931348623159e308", "179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497792",
	"inf", "+Inf", "-inf", "Infinity", "infinity", "INFINITY", "nan", "NaN", "-nan", " 1", "1 ", "\t2\n", " 3 ", " 4",
	"　5", "1​", "", " ", "１", "1,5", "1 000", "0x1p1_0", "1e1_0", "1e_10", "0.000000000000000000000000000001", "1e0000000000005",
	"123456789012345678901234567890", "9007199254740993", "-9007199254740993.0000000001", "3.0", "2.5", "1e21", "1e20", "123456",
	"1234567", "0.0001", "0.00001", "-0.0", "10", "11", "500", "501",
}

// toolSchemas are the builtin tools' parameter schemas as JSON, the form the Rust port reads them in.
func toolSchemas(t *testing.T) map[string]any {
	out := map[string]any{}
	for _, tool := range Builtins() {
		b, err := json.Marshal(tool.Spec.Parameters)
		if err != nil {
			t.Fatal(err)
		}
		var v any
		if err := json.Unmarshal(b, &v); err != nil {
			t.Fatal(err)
		}
		out[tool.Spec.Name] = v
	}
	return out
}

func genSchema(rng *rand.Rand, depth int) map[string]any {
	types := []string{"object", "array", "string", "integer", "number", "boolean", "", "null", "Integer"}
	typ := types[rng.Intn(len(types))]
	if depth == 0 {
		typ = "object"
	}
	s := map[string]any{}
	if typ != "" || rng.Intn(2) == 0 {
		s["type"] = typ
	}
	switch typ {
	case "object":
		props := map[string]any{}
		var names []any
		for i := rng.Intn(4); i >= 0; i-- {
			name := string(rune('a' + rng.Intn(5)))
			if rng.Intn(12) == 0 {
				props[name] = []any{"string", nil, 1.0, []any{}}[rng.Intn(4)] // not a schema: the value passes untouched
			} else if depth < 2 {
				props[name] = genSchema(rng, depth+1)
			} else {
				props[name] = map[string]any{"type": []string{"string", "integer", "boolean"}[rng.Intn(3)]}
			}
			if rng.Intn(3) == 0 {
				names = append(names, name)
			}
		}
		if rng.Intn(10) != 0 {
			s["properties"] = props
		} else {
			s["properties"] = []any{"x"}
		}
		if len(names) > 0 || rng.Intn(5) == 0 {
			names = append(names, []any{"z", 1, nil}[rng.Intn(3)])
			s["required"] = names[:rng.Intn(len(names)+1)]
		}
	case "array":
		if rng.Intn(4) != 0 && depth < 3 {
			s["items"] = genSchema(rng, depth+1)
		}
	case "string":
		if rng.Intn(2) == 0 {
			s["minLength"] = []any{0.0, 0.5, 1.0, 2.0, 3.5, "2"}[rng.Intn(6)]
		}
		if rng.Intn(3) == 0 {
			s["enum"] = []any{"a", "b", "é", 3.0}[:1+rng.Intn(4)]
		}
	case "integer", "number":
		if rng.Intn(2) == 0 {
			s["minimum"] = []any{0.0, 1.0, -2.5, 1e6, "1"}[rng.Intn(5)]
		}
		if rng.Intn(2) == 0 {
			s["maximum"] = []any{10.0, 500.0, 0.5, 1e21, nil}[rng.Intn(5)]
		}
	}
	return s
}

func genValue(rng *rand.Rand, schema any, depth int) any {
	m, _ := schema.(map[string]any)
	typ, _ := m["type"].(string)
	if depth > 0 && rng.Intn(6) == 0 || depth > 4 {
		return []any{nil, true, false, 0.0, 1.0, "x", "", []any{}, map[string]any{}, 2.5, -1.0, 1e21}[rng.Intn(12)]
	}
	if depth == 0 {
		typ = "object"
	}
	switch typ {
	case "object":
		out := map[string]any{}
		props, _ := m["properties"].(map[string]any)
		names := make([]string, 0, len(props))
		for name := range props {
			names = append(names, name)
		}
		sort.Strings(names) // the random choices in a fixed order: the same corpus on every run
		for _, name := range names {
			if rng.Intn(4) != 0 {
				out[name] = genValue(rng, props[name], depth+1)
			}
		}
		if rng.Intn(4) == 0 {
			out["extra"] = genValue(rng, nil, depth+1)
		}
		return out
	case "array":
		var out []any
		for i := rng.Intn(4); i > 0; i-- {
			out = append(out, genValue(rng, m["items"], depth+1))
		}
		if out == nil && rng.Intn(2) == 0 {
			return []any{}
		}
		return out
	case "string":
		return []any{"a", "b", "c", "", "é", "ab", "abc", "éé", "\U0001F600", "a\x00", "\"q\"", "<&>", " "}[rng.Intn(13)]
	case "integer", "number":
		if rng.Intn(2) == 0 {
			return numberTexts[rng.Intn(len(numberTexts))]
		}
		return []any{0.0, 1.0, 1.5, -2.0, 10.0, 11.0, 500.0, 501.0, 1e6, 1e21, 0.0001, 0.00001, 123456789.0, -0.5, 3.0}[rng.Intn(15)]
	case "boolean":
		return []any{true, false, "true", "false", " TRUE ", "False\n", "yes", "1", 1.0, " true", "tru", "İ"}[rng.Intn(12)]
	}
	return genValue(rng, nil, depth+1)
}

func countProps(v any) int {
	n := 0
	switch x := v.(type) {
	case map[string]any:
		if p, ok := x["properties"].(map[string]any); ok {
			n += len(p)
		}
		for _, e := range x {
			n += countProps(e)
		}
	case []any:
		for _, e := range x {
			n += countProps(e)
		}
	}
	return n
}

// validated is ValidateArgs on JSON-decoded schema and args: the result, or the error; run many times when the schema has
// two properties or more, since Go checks them in map order and the first failing one names the error. Every error seen
// is kept, sorted.
func validated(t *testing.T, schemaJSON, argsJSON string) any {
	var schema, args map[string]any
	if err := json.Unmarshal([]byte(schemaJSON), &schema); err != nil {
		t.Fatal(err)
	}
	runs := 1
	if countProps(schema) >= 2 {
		runs = 400
	}
	errs := map[string]bool{}
	var out map[string]any
	for i := 0; i < runs; i++ {
		if err := json.Unmarshal([]byte(argsJSON), &args); err != nil {
			t.Fatal(err)
		}
		got, err := ValidateArgs(schema, args)
		if err != nil {
			errs[err.Error()] = true
		} else {
			out = got
		}
	}
	if out != nil && len(errs) > 0 {
		t.Fatalf("ValidateArgs(%s, %s) both passed and failed", schemaJSON, argsJSON)
	}
	if len(errs) == 0 {
		b, err := json.Marshal(out)
		if err != nil {
			t.Fatal(err)
		}
		return map[string]any{"ok": json.RawMessage(b)}
	}
	var list []string
	for e := range errs {
		list = append(list, e)
	}
	sort.Strings(list)
	if len(list) == 1 {
		return map[string]any{"err": list[0]}
	}
	return map[string]any{"errs": list}
}

func mustJSON(t *testing.T, v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

// TestParityValidateArgs records ValidateArgs on the builtin tools' schemas and generated ones, with the tests' arguments
// and generated ones, and strconv.ParseFloat on the number texts (testdata/parity/schema.json; docs/RUST_PORT.md).
func TestParityValidateArgs(t *testing.T) {
	var cases []parity.Case
	add := func(schema, args string) {
		cases = append(cases, parity.Case{Fn: "ValidateArgs", In: []any{json.RawMessage(schema), json.RawMessage(args)},
			Out: validated(t, schema, args)})
	}
	// the literal schema and the JSON one must agree, or the fixture would not stand for the real tools
	for name, s := range toolSchemas(t) {
		lit := map[string]any(nil)
		for _, tool := range Builtins() {
			if tool.Spec.Name == name {
				lit = tool.Spec.Parameters
			}
		}
		_, e1 := ValidateArgs(lit, map[string]any{})
		_, e2 := ValidateArgs(s.(map[string]any), map[string]any{})
		if parity.Err(e1) != parity.Err(e2) {
			t.Fatalf("%s: the literal schema says %v, its JSON %v", name, e1, e2)
		}
	}
	// the cases of TestValidateArgsCoercionAndNesting
	test := `{"type":"object","required":["n"],"properties":{"n":{"type":"integer","minimum":1,"maximum":10},"f":{"type":"number"},"b":{"type":"boolean"},"e":{"type":"string","enum":["a","b"]},"l":{"type":"array","items":{"type":"object","required":["k"],"properties":{"k":{"type":"string","minLength":1}}}}}}`
	for _, a := range []string{`{"n":"3","f":"1.5","b":"false","e":"a","extra":1}`, `{}`, `{"n":null}`, `{"n":11}`, `{"n":0}`, `{"n":true}`,
		`{"n":2,"e":"c"}`, `{"n":2,"b":1}`, `{"n":2,"f":false}`, `{"n":2,"l":[{}]}`, `{"n":2,"l":[{"k":""}]}`, `{"n":2,"l":["x"]}`,
		`{"n":2,"l":{}}`, `{"n":2,"l":null,"f":null}`} {
		add(test, a)
	}
	add(`null`, `{"x":1}`)
	add(`{}`, `{"x":1}`)
	add(`{"type":"string"}`, `{"x":1}`)
	add(`{"type":"object"}`, `null`)
	rng := rand.New(rand.NewSource(7))
	schemas := toolSchemas(t)
	names := make([]string, 0, len(schemas))
	for n := range schemas {
		names = append(names, n)
	}
	sort.Strings(names)
	for _, n := range names {
		sj := mustJSON(t, schemas[n])
		add(sj, `{}`)
		for i := 0; i < 12; i++ {
			add(sj, mustJSON(t, genValue(rng, schemas[n], 0)))
		}
	}
	for i := 0; i < 1500; i++ {
		s := genSchema(rng, 0)
		add(mustJSON(t, s), mustJSON(t, genValue(rng, s, 0)))
	}
	// every number text in a number and an integer field, and every boolean text
	for _, typ := range []string{"number", "integer"} {
		for _, x := range numberTexts {
			add(`{"type":"object","properties":{"x":{"type":"`+typ+`","minimum":-1e300}}}`, mustJSON(t, map[string]any{"x": x}))
		}
	}
	for _, x := range []string{"true", "false", " TRUE ", "False\n", " true　", "​true", "tRuE", "yes", "", "İ", "İ"} {
		add(`{"type":"object","properties":{"x":{"type":"boolean"}}}`, mustJSON(t, map[string]any{"x": x}))
	}
	for _, x := range numberTexts {
		f, err := strconv.ParseFloat(strings.TrimSpace(x), 64)
		cases = append(cases, parity.Case{Fn: "ParseFloat(TrimSpace)", In: x, Out: []any{strconv.FormatFloat(f, 'g', -1, 64), parity.Err(err)}})
	}
	if reflect.DeepEqual(cases, nil) {
		t.Fatal("no cases")
	}
	parity.Fixture(t, "schema", cases)
}
