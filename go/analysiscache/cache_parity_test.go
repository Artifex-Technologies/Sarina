package analysiscache

import (
	"math"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityAnalysisCache records New over files of every shape, Get, Set's file byte for byte and its errors
// (testdata/parity/analysiscache.json; {DIR} is the cache's folder).
func TestParityAnalysisCache(t *testing.T) {
	type op struct {
		set     bool
		content string
		value   any
	}
	values := []any{nil, true, 1.5, "Graphics", "é   <tag> & \U0001F600", []any{"a", 1.5, nil}, map[string]any{"label": "Graphics", "score": 0.62},
		map[string]any{"z": map[string]any{"b": []any{}, "a": map[string]any{}}}, 1e21, 1e-7, math.Copysign(0, -1), 100.0, math.NaN(), math.Inf(1), 12345678901234567890.0}
	var cases []parity.Case
	pre := []*string{nil}
	for _, s := range []string{`{"namespace":"ns","entries":{"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824":"cached"}}`,
		`{"namespace":"other","entries":{"x":1}}`, `{"namespace":"ns","entries":null}`, `{"namespace":"ns"}`, `{"namespace":"ns","entries":[]}`,
		`{"namespace":"ns","entries":{}}`, `{"Namespace":"ns","ENTRIES":{"a":1}}`, `{"namespace":"ns","entries":{"a":1},"entries":{"b":2}}`,
		`{"namespace":"ns","entries":{"a":1e400}}`, `{nope`, ``, `null`, `[]`, `{"namespace":5,"entries":{}}`, `{"namespace":"NS","entries":{}}`,
		`{"namespace":"ns","entries":{"a":1},"x":[}`, "{\"namespace\":\"ns\",\"entries\":{\"\xff\":1}}"} {
		s := s
		pre = append(pre, &s)
	}
	for i, p := range pre {
		for _, ns := range []string{"ns", ""} {
			dir := filepath.Join(t.TempDir(), "sub")
			path := filepath.Join(dir, "c.json")
			in := map[string]any{"ns": ns}
			if p != nil {
				os.MkdirAll(dir, 0o755)
				os.WriteFile(path, []byte(*p), 0o644)
				in["file"] = parity.Text(*p)
			}
			c := New(path, ns)
			var out []any
			for _, content := range []string{"hello", "x", "", "a"} {
				v, ok := c.Get(content)
				out = append(out, []any{ok, parity.Typed(v)})
			}
			// a few writes, each followed by the file
			vals := values
			if i > 0 {
				vals = values[5:8]
			}
			for j, v := range vals {
				err := c.Set([]string{"hello", "x", "é", ""}[j%4], v)
				raw, _ := os.ReadFile(path)
				out = append(out, []any{strings.ReplaceAll(parity.Err(err), dir, "{DIR}"), parity.Text(string(raw))})
			}
			st, err := os.Stat(path)
			if err == nil {
				out = append(out, int(st.Mode().Perm()))
			}
			back := New(path, ns)
			for _, content := range []string{"hello", "x", "é", ""} {
				v, ok := back.Get(content)
				out = append(out, []any{ok, parity.Typed(v)})
			}
			cases = append(cases, parity.Case{Fn: "cache", In: in, Out: out})
		}
	}
	// Set where the folder cannot be made or written
	base := t.TempDir()
	os.WriteFile(filepath.Join(base, "file"), nil, 0o644)
	os.Mkdir(filepath.Join(base, "ro"), 0o555)
	for _, rel := range []string{"file/c.json", "ro/c.json", "ro/sub/c.json"} {
		err := New(filepath.Join(base, rel), "ns").Set("x", 1.0)
		// the temporary file's random part is left out
		why := regexp.MustCompile(`\.cache-[0-9]+\.tmp`).ReplaceAllString(parity.Err(err), ".cache-N.tmp")
		cases = append(cases, parity.Case{Fn: "setFails", In: rel, Out: strings.ReplaceAll(why, base, "{DIR}")})
	}
	os.Chmod(filepath.Join(base, "ro"), 0o755)
	parity.Fixture(t, "analysiscache", cases)
}
