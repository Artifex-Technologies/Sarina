package stego

import (
	"os"
	"path/filepath"
	"sort"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityStegoJPEG records (testdata/parity/stegojpeg.json) DecodeImage on JPEG files (testdata/parity/jpeg, made
// by libjpeg-turbo's cjpeg and PIL from a synthetic picture: baseline and progressive, every chroma sampling Go knows
// and some it decodes the flexible way, restart markers, grey, RGB, CMYK, arithmetic coding and 12-bit precision it
// refuses), and on every truncation and a byte change at every position of a baseline and a progressive file. The
// changed files are named by (file, position, xor) instead of written out.
func TestParityStegoJPEG(t *testing.T) {
	dir := filepath.Join(parity.Root(), "testdata", "parity", "jpeg")
	names, err := filepath.Glob(filepath.Join(dir, "*.jpg"))
	if err != nil || len(names) == 0 {
		t.Fatal("no test JPEGs", err)
	}
	sort.Strings(names)
	var files []any
	data := map[string][]byte{}
	for _, n := range names {
		b, err := os.ReadFile(n)
		if err != nil {
			t.Fatal(err)
		}
		data[filepath.Base(n)] = b
		files = append(files, []any{filepath.Base(n), lsbDescribe(b)})
	}
	cases := []parity.Case{{Fn: "file", Out: files}}
	brief := func(b []byte) any { // the error, or the size and the pixels' hash
		p, err := DecodeImage(b)
		if err != nil {
			return err.Error()
		}
		return []any{p.W, p.H, shortSum(p.RGBA)}
	}
	for _, name := range []string{"base.jpg", "prog.jpg", "restartb.jpg", "adobe1.jpg"} {
		b := data[name]
		var rows []any
		for i := range b {
			c := append([]byte(nil), b...)
			c[i] ^= 0x41
			rows = append(rows, []any{i, brief(c), brief(b[:i])})
		}
		cases = append(cases, parity.Case{Fn: "changes", In: name, Out: rows})
	}
	parity.Fixture(t, "stegojpeg", cases)
}
