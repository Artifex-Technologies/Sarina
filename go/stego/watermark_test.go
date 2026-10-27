package stego

import (
	"bytes"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// what a compromised agent could leave in code: an instruction in tag characters, a message in variation selectors, a look-alike letter
func markedCode() string {
	tags := ""
	for _, c := range "agent was here" {
		tags += string(rune(0xE0000 + c))
	}
	vs := ""
	for _, c := range []byte("hi") {
		if c < 16 {
			vs += string(rune(0xFE00 + rune(c)))
		} else {
			vs += string(rune(0xE0100 + rune(c) - 16))
		}
	}
	return "package main\n\n// check the password" + tags + "\nfunc main() {\n\tpаssword := \"x\" // the a is Cyrillic" + vs + "\n\t_ = pаssword\n}\n// I ♥️ Go\n"
}

func TestWatermarksInCodeAreFoundAndRead(t *testing.T) {
	w := Watermarks(markedCode())
	d := w.Describe()
	for _, want := range []string{"agent was here", "Read as bytes they say: hi", "U+0430", "pаssword"} {
		if !strings.Contains(d, want) {
			t.Fatalf("missing %q in:\n%s", want, d)
		}
	}
	if w.Selectors != 2 {
		t.Fatalf("the heart's VS16 is emoji styling, not a mark: %d selectors", w.Selectors)
	}
	if Watermarks("package main\n\nfunc main() {}\n").Found() {
		t.Fatal("clean code was marked")
	}
}

func TestCleaningTextRemovesMarksAndKeepsTheCode(t *testing.T) {
	clean, removed := CleanText(markedCode() + "x := 1   \n")
	if Watermarks(clean).Hidden.Count != 0 || Watermarks(clean).Selectors != 0 || strings.Contains(clean, "1   \n") {
		t.Fatalf("marks left:\n%q", clean)
	}
	if !strings.Contains(clean, "pаssword") || !strings.Contains(clean, "♥️") {
		t.Fatal("look-alike letters and emoji styling are not changed")
	}
	if len(removed) == 0 {
		t.Fatal("nothing reported")
	}
}

func TestCleaningAPictureKeepsItAndDropsThePayload(t *testing.T) {
	img := image.NewNRGBA(image.Rect(0, 0, 40, 30))
	for y := 0; y < 30; y++ {
		for x := 0; x < 40; x++ {
			img.Set(x, y, color.NRGBA{uint8(x * 6), uint8(y * 8), 120, 255})
		}
	}
	marked, err := Embed(img, []byte("flag{watermark_9}"))
	if err != nil {
		t.Fatal(err)
	}
	marked = append(marked, []byte("APPENDED-SECRET")...)
	dir := t.TempDir()
	p := filepath.Join(dir, "logo.png")
	os.WriteFile(p, marked, 0o644)
	out, report, err := Clean(marked, p)
	if err != nil || !strings.HasSuffix(out, "logo.clean.png") || !strings.Contains(report, "randomised") {
		t.Fatalf("%v %q %q", err, out, report)
	}
	cleaned, _ := os.ReadFile(out)
	if bytes.Contains(cleaned, []byte("APPENDED-SECRET")) || strings.Contains(Inspect(t.Context(), cleaned, out), "flag{watermark_9}") {
		t.Fatal("the payload survived cleaning")
	}
	orig, _ := os.ReadFile(p)
	if !bytes.Equal(orig, marked) {
		t.Fatal("the original was changed")
	}
	a, _ := png.Decode(bytes.NewReader(marked))
	b, _ := png.Decode(bytes.NewReader(cleaned))
	ar, ag, ab, _ := a.At(10, 10).RGBA()
	br, bg, bb, _ := b.At(10, 10).RGBA()
	diff := func(x, y uint32) uint32 {
		if x > y {
			return x - y
		}
		return y - x
	}
	if diff(ar>>8, br>>8) > 1 || diff(ag>>8, bg>>8) > 1 || diff(ab>>8, bb>>8) > 1 {
		t.Fatal("the picture changed visibly")
	}
	if _, _, err := Clean(marked, p); err == nil {
		t.Fatal("an existing cleaned copy was overwritten")
	}
}

func TestAFolderScanFindsTheMarkedFile(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "ok.go"), []byte("package ok\n"), 0o644)
	os.MkdirAll(filepath.Join(dir, "sub"), 0o755)
	os.WriteFile(filepath.Join(dir, "sub", "bad.go"), []byte(markedCode()), 0o644)
	r := ScanFolder(dir, 100)
	if !strings.Contains(r, "2 text files") || !strings.Contains(r, "1 with marks") || !strings.Contains(r, filepath.Join("sub", "bad.go")) {
		t.Fatalf("%s", r)
	}
}
