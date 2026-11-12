package sessionsearch

import (
	"fmt"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
	"unicode"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// searchFiles are made-up saved sessions (only "messages" matters to the search), each with the modification time it gets
// (seconds since 2026-01-01: newer files come first).
var searchFiles = []struct {
	name string
	mod  int64
	body string
}{
	{"aaa.json", 100, `{"messages":[{"role":"system","content":"pytest everywhere"},{"role":"user","content":"how do I configure pytest fixtures?"},{"role":"assistant","content":"Use conftest.py; pytest finds it."}]}`},
	{"bbb.json", 200, `{"messages":[{"role":"user","content":"unrelated chatter about lunch"},{"role":"user","content":"<system-reminder>pytest fixtures</system-reminder>"},{"role":"","content":"pytest"},{"role":"tool","content":"   "},{"role":"tool","content":["pytest"]},{"role":"assistant","content":"Lunch was PYTEST-shaped: pytest, pytest; pytest!"}]}`},
	{"ccc.json", 200, `{"session_id":"x","messages":[{"role":"user","content":"Café crème: Über STRASSE straße İstanbul ı"},{"role":"assistant","content":"digits ٣٤ and Ⅻ and x́y and 2026"}]}`},
	{"ddd.json", 50, `{"messages":[{"role":"user","content":"` + strings.Repeat("filler words here ", 30) + `needle in the middle ` + strings.Repeat("more filler text ", 30) + `"}]}`},
	{"eee.json", 300, `{"messages":"not a list"}`},
	{"fff.json", 300, `{"messages":[{"role":"user","content":1e400}]}`},
	{"ggg.json", 10, `{"messages":[{"role":"user","content":"short"},{"role":"user","content":"` + strings.Repeat("é", 400) + `"},{"role":"user","content":"` + strings.Repeat("a", 330) + `"}]}`},
	{"hhh.json", 400, `{"messages":[{"Role":"user","CONTENT":"keys in any case: pytest"},{"role":"user","content":"x","role":"assistant"}]}`},
	{"iii.txt", 500, `{"messages":[{"role":"user","content":"pytest in a txt file"}]}`},
	{"jjj.json", 60, `not json`},
	{"kkk.json", 60, `{"messages":[{"role":"user","content":"the the the the"},{"role":"user","content":"the cat the cat"},{"role":"assistant","content":"cat"}]}`},
}

const sparseFile = `{"messages":[{"role":"user","content":"!!!"},{"role":"user","content":"??"},{"role":"user","content":"..."},{"role":"user","content":"x"},{"role":"assistant","content":"x y"}]}`

var searchQueries = []string{"pytest", "pytest fixtures", "PyTest", "  pytest  ", "", "   ", "needle", "needle in the middle", "lunch pytest",
	"café", "CAFÉ", "straße", "strasse", "İstanbul", "istanbul", "ı", "٣٤", "Ⅻ", "x", "y", "2026", "!!!", "pytest!",
	"conftest.py", "the", "the cat", "cat the", "cat", "nothing matches this", "short", "é", "a", "aa", "e", "fixtures?", "use", ";", "lunch was",
	"filler", "more filler", "middle more", "words here filler"}

// TestParitySessionSearch records Search over made-up session folders (testdata/parity/sessionsearch.json), Go's amd64
// math.Log the ranking uses, and writes Go's letter and digit tables for the word splitting
// (rust/sarina/src/gotext/letters.rs).
func TestParitySessionSearch(t *testing.T) {
	var cases []parity.Case
	dir := t.TempDir()
	epoch := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	for _, f := range searchFiles {
		p := filepath.Join(dir, f.name)
		os.WriteFile(p, []byte(f.body), 0o600)
		os.Chtimes(p, epoch, epoch.Add(time.Duration(f.mod)*time.Second))
	}
	os.Mkdir(filepath.Join(dir, "folder.json"), 0o755)
	var files []any
	for _, f := range searchFiles {
		files = append(files, []any{f.name, f.mod, parity.Text(f.body)})
	}
	cases = append(cases, parity.Case{Fn: "files", In: files})
	hitsOut := func(hs []Hit) any {
		out := []any{}
		for _, h := range hs {
			out = append(out, []any{h.SessionID, h.Role, h.TurnIndex, parity.Text(h.Snippet), strconv.FormatUint(math.Float64bits(h.Score), 16)})
		}
		return out
	}
	for _, q := range searchQueries {
		for _, k := range []int{0, 1, 3, 100} {
			cases = append(cases, parity.Case{Fn: "search", In: []any{parity.Text(q), k}, Out: hitsOut(Search(q, dir, k))})
		}
	}
	cases = append(cases, parity.Case{Fn: "missing", Out: hitsOut(Search("pytest", filepath.Join(dir, "missing"), 3))})
	// a folder whose messages are mostly without words: fewer words than messages on average
	sparse := t.TempDir()
	os.WriteFile(filepath.Join(sparse, "s.json"), []byte(sparseFile), 0o600)
	for _, q := range []string{"x", "x y", "!!!", "y"} {
		cases = append(cases, parity.Case{Fn: "sparse", In: q, Out: hitsOut(Search(q, sparse, 5))})
	}
	// math.Log on the values the ranking meets and on its edges
	var logs []string
	xs := []float64{1, 2, 0.5, 1.5, 0, math.Copysign(0, -1), -1, math.Inf(1), math.Inf(-1), math.NaN(), 5e-324, 2.2250738585072014e-308, 1e-310,
		math.MaxFloat64, 0.7071067811865476, 0.7071067811865475, 1.4142135623730951, 1.0000000000000002, 0.9999999999999999}
	rng := rand.New(rand.NewSource(7))
	for i := 0; i < 400; i++ {
		xs = append(xs, rng.Float64()*10, rng.ExpFloat64()*1e6, 1+rng.Float64()*1e-6)
	}
	for n := 1; n <= 30; n++ {
		for m := 0; m <= n; m++ {
			xs = append(xs, (float64(n)-float64(m)+0.5)/(float64(m)+0.5)+1)
		}
	}
	for _, x := range xs {
		logs = append(logs, strconv.FormatUint(math.Float64bits(x), 16)+":"+strconv.FormatUint(math.Float64bits(math.Log(x)), 16))
	}
	cases = append(cases, parity.Case{Fn: "log", Out: logs})
	parity.Fixture(t, "sessionsearch", cases)
	table := func(name string, rt *unicode.RangeTable) string {
		var b strings.Builder
		b.WriteString("#[rustfmt::skip]\npub static " + name + ": &[(u32, u32)] = &[\n")
		add := func(lo, hi, stride uint32) {
			if stride == 1 {
				fmt.Fprintf(&b, "    (0x%x, 0x%x),\n", lo, hi)
				return
			}
			for r := lo; r <= hi; r += stride {
				fmt.Fprintf(&b, "    (0x%x, 0x%x),\n", r, r)
			}
		}
		for _, r := range rt.R16 {
			add(uint32(r.Lo), uint32(r.Hi), uint32(r.Stride))
		}
		for _, r := range rt.R32 {
			add(r.Lo, r.Hi, r.Stride)
		}
		b.WriteString("];\n")
		return b.String()
	}
	src := "// Go's unicode.Letter and unicode.Digit (Unicode " + unicode.Version + "): the word characters of the session search.\n" +
		"// Written by go/sessionsearch's parity test.\n\n" + table("LETTER", unicode.Letter) + "\n" + table("DIGIT", unicode.Digit)
	parity.Generated(t, "rust/sarina/src/gotext/letters.rs", []byte(src))
}
