package stego

import (
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"unicode"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityWatermark records (testdata/parity/watermark.json) Watermarks and its Describe, CleanText and ScanFolder
// on texts with every kind of mark (invisible characters, variation selectors in and out of emoji styling, look-alike
// letters from other alphabets, trailing whitespace carrying bits, invalid UTF-8) and on folders; and writes the
// Unicode tables the port needs (rust/sarina/src/gotext/scripts.rs).
func TestParityWatermark(t *testing.T) {
	rng := rand.New(rand.NewSource(17))
	vs := func(b byte) string { // a variation selector carrying byte b
		if b < 16 {
			return string(rune(0xFE00 + int(b)))
		}
		return string(rune(0xE0100 + int(b) - 16))
	}
	hide := func(s string) string {
		var b strings.Builder
		for _, c := range []byte(s) {
			b.WriteString(vs(c))
		}
		return b.String()
	}
	trail := func(s string) string { // bits of s as trailing spaces (0) and tabs (1), one byte per line
		var b strings.Builder
		for _, c := range []byte(s) {
			b.WriteString("line")
			for i := 7; i >= 0; i-- {
				if c>>uint(i)&1 == 1 {
					b.WriteByte('\t')
				} else {
					b.WriteByte(' ')
				}
			}
			b.WriteString("\n")
		}
		return b.String()
	}
	var many strings.Builder
	for i := 0; i < 60; i++ {
		fmt.Fprintf(&many, "w%d\u043erd ", i)
	}
	texts := []string{"", "plain text\nwith lines\n", "zero\u200bwidth\u200c and \u200d joiner \u2060 \ufeffBOM inside", "\ufeffstarts with a BOM",
		"tag \U000E0041\U000E0042 chars", "smile \u263a\ufe0f and text\ufe0e styled", "a\ufe0fb\ufe0e", "x" + hide("hidden msg") + "y", "x" + hide("\x01\x02") + "y",
		"arrow \u2190\ufe0f and \u2b1b\ufe0e and \U0001f004\ufe0f and \u2211\ufe0f", "one\ufe00", "p\u0430ypal and g\u03bfogle and micros\u0585ft", "\uff21\uff22\uff23 fullwidth and \uff21\uff22cd mixed",
		"\u0395\u03bb\u03bb\u03b7\u03bd\u03b9\u03ba\u03ac alone and \u0420\u0443\u0441\u0441\u043a\u0438\u0439 alone", "na\u00efve caf\u00e9 fa\u00e7ade", "under_score_w\u043erd and 123\u0430bc", many.String(), trail("Hi there"), trail("\x01\x02\x03"),
		"trailing  \nspaces\t\nonly  \t\n", "crlf line \r\nnext\t\r\n", "bad \xff utf8 \xfe\xfd here  ", "\xff\ufe0f", "word\u0301combining", "\u2190\ufe0e",
		trail("A") + trail("B"), "tab\tinside\tline", "a+\ufe0f and \u00d7\ufe0e and |\ufe0f", "ab\uff10 digits and \uff21\uff22 letters",
		strings.ReplaceAll(trail("Hi"), "\n", "\r\n"), "x" + hide("ok") + "\n" + trail("ab"), "\u043c\u0438\u0440 mixed w\u043erld", "\u1e01 Latin Extended Additional with \u03bf Greek",
	}
	pool := []string{"a", "b", "Z", "9", "_", " ", "\t", "\n", "\r", "\u0430", "\u0435", "\u03bf", "\u0585", "\uff21", "\ufe0e", "\ufe0f", "\ufe00", "\U000E0100", "\U000E01EF", "\u200b",
		"\u200d", "\U000E0020", "\u2603", "\u2211", "\u2190", "\U0001f004", "\ufeff", "\xff", "\u00e9", "\u00df", "\u4e2d", "\u0301", "x"}
	for i := 0; i < 400; i++ {
		var b strings.Builder
		for j := rng.Intn(40); j >= 0; j-- {
			b.WriteString(pool[rng.Intn(len(pool))])
		}
		texts = append(texts, b.String())
	}
	var marks, cleans []any
	for _, s := range texts {
		w := Watermarks(s)
		var hg []any
		for _, h := range w.Homoglyphs {
			hg = append(hg, []any{h.Line, h.Word, h.Foreign})
		}
		marks = append(marks, []any{parity.Text(s), w.Found(), w.Selectors, parity.Text(w.SelectorText), parity.Text(w.TrailingBits), hg, parity.Text(w.Describe())})
		out, removed := CleanText(s)
		cleans = append(cleans, []any{parity.Text(s), parity.Text(out), parity.Texts(removed)})
	}
	cases := []parity.Case{{Fn: "marks", Out: marks}, {Fn: "clean", Out: cleans}}

	// folders
	dir := t.TempDir()
	write := func(rel string, data []byte) {
		p := filepath.Join(dir, rel)
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("a.txt", []byte("clean text\n"))
	write("b.md", []byte("hidden\u200bmark\n"))
	write("c/d.txt", []byte("p\u0430ypal\n"))
	write("c/e.bin", []byte("bin\x00ary with \u200b"))
	write("c/pic.PNG", []byte("not really"))
	write("c/z.jpeg", nil)
	write(".git/x.txt", []byte("\u200b"))
	write("node_modules/y.txt", []byte("\u200b"))
	write("deep/er/f.txt", []byte(trail("hi")))
	write("big.txt", append([]byte("\u200b"), strings.Repeat("x", 4<<20)...))
	write("g.txt", []byte(strings.Repeat("x", 4<<20)))
	write("target/t.txt", []byte("\u200b"))
	write("h.log", []byte("bad \xff\u200b"))
	if err := os.Symlink(filepath.Join(dir, "b.md"), filepath.Join(dir, "link.txt")); err != nil {
		t.Fatal(err)
	}
	var scans []any
	for _, limit := range []int{0, 1, 2, 3, 100} {
		out := ScanFolder(dir, limit)
		scans = append(scans, []any{limit, strings.ReplaceAll(out, dir, "{DIR}")})
	}
	empty := t.TempDir()
	scans = append(scans, []any{-1, strings.ReplaceAll(ScanFolder(empty, 10), empty, "{DIR}")},
		[]any{-2, strings.ReplaceAll(ScanFolder(filepath.Join(empty, "missing"), 10), empty, "{DIR}")})
	cases = append(cases, parity.Case{Fn: "scan", In: "the tree is made by the Rust test the same way", Out: scans})
	parity.Fixture(t, "watermark", cases)

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
	src := "// Go's unicode.So, Sm, Latin, Cyrillic, Greek and Armenian (Unicode " + unicode.Version + "): stego's watermark checks.\n" +
		"// Written by go/stego's watermark parity test.\n\n" + table("SO", unicode.So) + "\n" + table("SM", unicode.Sm) + "\n" + table("LATIN", unicode.Latin) +
		"\n" + table("CYRILLIC", unicode.Cyrillic) + "\n" + table("GREEK", unicode.Greek) + "\n" + table("ARMENIAN", unicode.Armenian)
	parity.Generated(t, "rust/sarina/src/gotext/scripts.rs", []byte(src))
}
