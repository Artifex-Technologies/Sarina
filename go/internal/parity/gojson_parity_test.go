package parity

import (
	"bytes"
	"encoding/json"
	"math"
	"math/rand"
	"strconv"
	"strings"
	"testing"
)

// gojsonDocs are documents for Go's decoder: the scanner's every error (each place a document can stop or go wrong),
// numbers at and past float64's edges, strings with every escape, lone and paired surrogates, bytes that are not UTF-8,
// duplicate keys, what may follow a value, and the nesting limit.
var gojsonDocs = []string{
	`null`, `true`, `false`, `0`, `-0`, `1`, `1.0`, `1.5`, `1e3`, `1E-7`, `-2.50`, `123456789012345678901234567890`, `1e400`,
	`-1e400`, `1e-400`, `0.0`, `1e308`, `1.7976931348623157e308`, `1.7976931348623159e308`, `4.9e-324`, `2e-324`, `3e-324`,
	`9007199254740993`, `1E+2`, `1e-07`, `100000000000000000000`, `1e21`, `0.000001`, `0.0000001`, `-0.0`, `5e-1`,
	`""`, `"a"`, `"it's"`, `"say \"hi\""`, `"back\\slash"`, `"\/"`, `"\b\f\n\r\t"`, `"\u0001\u001f\u007f"`, `"\u0080 é"`,
	`" 😀"`, `"\ud800"`, `"\udc00"`, `"\ud83d\ude00"`, `"\ud83d"`, `"\ud83d\u0041"`, `"\ud83d\ud83d\ude00"`, `"x\u0000y"`,
	`"\u2028\u2029"`, "\"  \"", `"<a href='x'>&amp;</a>"`, `"\u003c\u003e\u0026"`, `"\uFFFD\uFFFE\uFFFF"`,
	"\"x\xffy\"", "\"\xe2\x84\"", "\"\xe2\x84y\"", "\"\xc3\"", "\"\xed\xa0\x80\"", "\"\xf4\x90\x80\x80\"", "\"\xc0\xaf\"",
	"\"tab\there\"", "\"nl\nhere\"", "\"\x7f\"", `[]`, `[1,"a",null,[true]]`, `{}`, `{"b":1,"a":"x","c":[]}`,
	`{"it's":{"z":false}}`, `[{"k":1.25e2}]`, `{"":null}`, `{"a":1,"a":2}`, `{"A":1,"a":2}`, `{"\ud800":1,"\udc00":2}`,
	"{\"\xff\":1}", ` {"a" : [ 1 , 2 ] } `, "\t\n\r [1]\r\n", `[[[[[[[[[[]]]]]]]]]]`, `{"a":{"b":{"c":{}}}}`,
	`{"x":1e400}`, `[1,1e400,2]`, `{"a":[{"b":-1e999}]}`,
	// syntax errors and documents cut short
	`{"a":1,}`, `[1 2]`, `{"a" 1}`, `{"a":1 "b"}`, "\"\x01\"", `"\x"`, `"\u12g4"`, `tru`, `trUe`, `nul`, `nulL`, `fals`, `falsy`,
	`1.x`, `-x`, `1e+`, `1e`, `1.`, `.5`, `01`, `-01`, `+1`, `-`, `1ee`, `{`, `[`, `{"a"`, `{"a":`, `{"a":1`, `[1,`, `"abc`, `"\`,
	`"\u12`, `'a'`, "\xc3\xa9", "\xef\xbb", "\xef\xbb\xbf{}", "[\xe2\x80\xa8]", `  `, ``, `{} {}`, `{} {`, `1 2`, `[1]x`, `truex`,
	`123abc`, `{"a":1}}`, `[1]]`, `{1:2}`, `{"a":1,"b"}`, `[,1]`, `[1,,2]`, `{,}`, `NaN`, `Infinity`, `-Infinity`, `nan`,
	"{\"a\":\"b\x00\"}", "[\x00]", `[1e400,x]`, `[1e400]x`, `{"a":1e400,}`, `[1e400`, `1e400 x`, `[1e400,1e401]`, `[1e400,"\x"]`, `{"\ud83d\ude00":1,"\uff01":2,"\ue000":3,"z":4,"~a/b":1e400}`, `{"5":1e400}`, `"\udc00\udc00"`, `"\udbff\udfff"`, `"\ud800\udbff\udc00"`, `"\uDBFF\uDFFF\uD800"`, `{"":1e400}`, `{"a.b":[0,{"~":[1e400]}]}`, `[-]`, `[0x10]`, `[1_000]`, `{"a":tru}`, `["a" "b"]`, `}`, `]`, `:`, `,`,
}

// genJSON builds a random document (fixed seed) from small pieces, sometimes damaged by a cut or a changed byte.
func genJSON(rng *rand.Rand, depth int) string {
	switch n := rng.Intn(9); {
	case n == 0 && depth < 4:
		var parts []string
		for i := rng.Intn(4); i > 0; i-- {
			parts = append(parts, genJSON(rng, depth+1))
		}
		return "[" + strings.Join(parts, ",") + "]"
	case n == 1 && depth < 4:
		var parts []string
		for i := rng.Intn(4); i > 0; i-- {
			parts = append(parts, `"`+[]string{"a", "b", "A", "é", "<", `\u0041`, "", "a"}[rng.Intn(8)]+`":`+genJSON(rng, depth+1))
		}
		return "{" + strings.Join(parts, ",") + "}"
	case n < 5:
		return []string{`0`, `2`, `-3.5`, `1e21`, `1e20`, `0.000001`, `0.0000001`, `100000`, `12.0`, `7e-5`, `-0.0`, `1e400`,
			`3.141592653589793`, `0.1`, `1e-7`, `123456789.123456789`, `true`, `false`, `null`}[rng.Intn(19)]
	}
	var b strings.Builder
	b.WriteByte('"')
	for i := rng.Intn(6); i > 0; i-- {
		b.WriteString([]string{"a", "'", `\"`, `\\`, `\n`, `\u0007`, "é", " ", `😀`, `\u007f`, `\ud800`, "\xff", "<&>",
			`\u2028`, `\/`, `\t`, `\b`, "\u2029"}[rng.Intn(18)])
	}
	b.WriteByte('"')
	return b.String()
}

func damage(rng *rand.Rand, doc string) string {
	if doc == "" {
		return doc
	}
	b := []byte(doc)
	switch rng.Intn(3) {
	case 0:
		return string(b[:rng.Intn(len(b))])
	case 1:
		const pool = "{}[]:,\"\\ x0-e.\x00\xff"
		b[rng.Intn(len(b))] = pool[rng.Intn(len(pool))]
	default:
		i := rng.Intn(len(b) + 1)
		b = append(b[:i], append([]byte{"{}[]:,\" 1"[rng.Intn(9)]}, b[i:]...)...)
	}
	return string(b)
}

// encodeNoHTML is an Encoder with SetEscapeHTML(false) and the given indent, its trailing newline cut (the session file's
// and the tool calls' writers).
func encodeNoHTML(v any, indent string) (string, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if indent != "" {
		enc.SetIndent("", indent)
	}
	if err := enc.Encode(v); err != nil {
		return "", err
	}
	return strings.TrimRight(buf.String(), "\n"), nil
}

func marshalOut(v any) map[string]any {
	out := map[string]any{}
	if b, err := json.Marshal(v); err != nil {
		out["err"] = err.Error()
	} else {
		out["html"] = Text(string(b))
	}
	if s, err := encodeNoHTML(v, ""); err == nil {
		out["plain"] = Text(s)
	}
	if s, err := encodeNoHTML(v, "  "); err == nil {
		out["indent"] = Text(s)
	}
	return out
}

// TestParityGojson records Go's encoding/json (testdata/parity/gojson.json): Unmarshal and one Decoder.Decode into an
// `any` (the value, or the error text: the scanner's, the stream reader's, a number out of range), the value written back
// by Marshal, by an Encoder without HTML escaping, and indented; float64s written; strings written.
func TestParityGojson(t *testing.T) {
	docs := append([]string(nil), gojsonDocs...)
	rng := rand.New(rand.NewSource(20261111))
	for i := 0; i < 700; i++ {
		d := genJSON(rng, 0)
		docs = append(docs, d)
		if i%2 == 0 {
			docs = append(docs, damage(rng, d))
		}
	}
	for _, long := range []string{`{"a":[1,{"b":"c\u00e9\ud83d\ude00"}],"d":-1.5e-3,"e":true}`, `["x\\y",null,{"":[]}]`} {
		for i := 0; i <= len(long); i++ {
			docs = append(docs, long[:i])
		}
	}
	// the nesting limit: 10000 levels read, 10001 do not
	for _, n := range []int{9999, 10000, 10001} {
		docs = append(docs, strings.Repeat("[", n)+strings.Repeat("]", n))
		docs = append(docs, strings.Repeat(`{"a":`, n)+"1"+strings.Repeat("}", n))
	}
	var cases []Case
	seen := map[string]bool{}
	for _, doc := range docs {
		if seen[doc] {
			continue
		}
		seen[doc] = true
		out := map[string]any{}
		var v any
		err := json.Unmarshal([]byte(doc), &v)
		out["unmarshal"] = map[string]any{"v": Typed(v), "err": Err(err)}
		if err == nil {
			out["marshal"] = marshalOut(v)
		}
		var d any
		dec := json.NewDecoder(strings.NewReader(doc))
		err = dec.Decode(&d)
		more := dec.More()
		out["decode"] = map[string]any{"v": Typed(d), "err": Err(err), "more": more}
		in := Text(doc)
		if len(doc) > 2000 {
			// the nesting cases: rebuilt on the Rust side, and only the errors kept (a value that deep would not fit a
			// fixture reader's own nesting limit)
			if strings.HasPrefix(doc, "{") {
				in = map[string]any{"objects": strings.Count(doc, "{")}
			} else {
				in = map[string]any{"arrays": strings.Count(doc, "[")}
			}
			out = map[string]any{"unmarshal": Err(json.Unmarshal([]byte(doc), &v)), "decode": Err(err)}
		}
		cases = append(cases, Case{Fn: "doc", In: in, Out: out})
	}
	floats := []float64{0, math.Copysign(0, -1), 1, -1, 0.1, 1.5, 1e20, 1e21, 1e-6, 1e-7, 9.999999999999999e20,
		123456789012345680000, 1.7976931348623157e308, 5e-324, -5e-324, 2.2250738585072014e-308, 1e100, 1e-100, 12.5e-7,
		0.000001234, 100, 1e6, 1e15, 1e16, 3.141592653589793, 2.718281828459045e-10, -1e21, math.NaN(), math.Inf(1),
		math.Inf(-1), 0.3, 1.0000000000000002, 4503599627370496.5, 9007199254740993}
	for i := 0; i < 600; i++ {
		var f float64
		switch rng.Intn(4) {
		case 0:
			f = math.Float64frombits(rng.Uint64())
		case 1:
			f = rng.NormFloat64() * math.Pow(10, float64(rng.Intn(50)-25))
		case 2:
			f = float64(rng.Int63n(1<<53)) * math.Pow(10, float64(rng.Intn(30)-7))
		default:
			f = float64(rng.Intn(100000)) / float64(1+rng.Intn(1000))
		}
		floats = append(floats, f)
	}
	for _, f := range floats {
		b, err := json.Marshal(f)
		cases = append(cases, Case{Fn: "float", In: strconv.FormatUint(math.Float64bits(f), 16),
			Out: map[string]any{"json": string(b), "err": Err(err)}})
	}
	strs := []string{"", "a", "<>&", "\u2028\u2029", "\x00\x01\x08\x09\x0a\x0b\x0c\x0d\x1f\x7f", "\"\\/", "é😀",
		"\xff", "a\xe2\x84b", "\xed\xa0\x80", "\uFFFD", "\ufeff", "</script>", "\U0010FFFF", "\x80\x80"}
	for i := 0; i < 200; i++ {
		var b []byte
		for j := rng.Intn(12); j > 0; j-- {
			b = append(b, []byte{0, 7, 8, 9, 10, 12, 13, 27, '"', '\\', '/', '<', '>', '&', 'a', 0x7f, 0xc3, 0xa9, 0xe2, 0x80,
				0xa8, 0xff, 0xf0, 0x9f, 0x98, 0x80}[rng.Intn(26)])
		}
		strs = append(strs, string(b))
	}
	seenStr := map[string]bool{}
	for _, s := range strs {
		if seenStr[s] {
			continue
		}
		seenStr[s] = true
		cases = append(cases, Case{Fn: "string", In: Text(s), Out: marshalOut(s)})
	}
	Fixture(t, "gojson", cases)
}
