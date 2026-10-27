package stego

import (
	"bytes"
	"crypto/rand"
	"fmt"
	"image"
	"image/draw"
	"image/jpeg"
	"image/png"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode"

	_ "image/gif" // decoded so a GIF can be cleaned (written back as PNG)
)

// Watermarks in text and code: the marks a person or a rogue agent can leave that a reader does not see. FindHidden covers the
// zero-width, tag and direction characters; this adds variation selectors (each can carry a byte: "emoji smuggling"), letters from
// another alphabet inside Latin words (homoglyphs: a Cyrillic а in "pаssword"), and trailing spaces and tabs read as bits.

// Selector is a variation selector's byte value, or -1: VS1-16 (U+FE00-FE0F) are 0-15, VS17-256 (U+E0100-E01EF) are 16-255.
func Selector(r rune) int {
	switch {
	case r >= 0xFE00 && r <= 0xFE0F:
		return int(r - 0xFE00)
	case r >= 0xE0100 && r <= 0xE01EF:
		return int(r-0xE0100) + 16
	}
	return -1
}

// emojiStyle is a variation selector used as intended: VS15/VS16 right after a symbol or an emoji (♥️), which is not a mark.
func emojiStyle(prev, r rune) bool {
	return (r == 0xFE0E || r == 0xFE0F) && (unicode.Is(unicode.So, prev) || unicode.Is(unicode.Sm, prev) || prev >= 0x1F000 || (prev >= 0x2190 && prev <= 0x2BFF))
}

// Homoglyph is a word that mixes Latin letters with look-alikes from another script.
type Homoglyph struct {
	Line    int
	Word    string
	Foreign string // the other script's letters, as U+ codes and names
}

// WatermarkReport is everything hidden in one text.
type WatermarkReport struct {
	Hidden       HiddenText
	Selectors    int    // variation selectors that are not emoji styling
	SelectorText string // what they read as, when they read as text
	Homoglyphs   []Homoglyph
	TrailingBits string // trailing spaces (0) and tabs (1) read as 8-bit text, when they do
}

// Found is whether anything at all was found.
func (w WatermarkReport) Found() bool {
	return w.Hidden.Count > 0 || w.Selectors > 0 || len(w.Homoglyphs) > 0 || w.TrailingBits != "" || w.Hidden.Trailing > 0
}

// Watermarks looks through a text or a source file for every kind of invisible or disguised mark.
func Watermarks(s string) WatermarkReport {
	w := WatermarkReport{Hidden: FindHidden(s)}
	var sel []byte
	var prev rune
	for _, r := range s {
		if v := Selector(r); v >= 0 && !emojiStyle(prev, r) {
			w.Selectors++
			sel = append(sel, byte(v))
		}
		prev = r
	}
	if len(sel) >= 2 && printable(string(sel)) {
		w.SelectorText = string(sel)
	}
	var bits strings.Builder
	for i, line := range strings.Split(s, "\n") {
		line = strings.TrimRight(line, "\r")
		trail := line[len(strings.TrimRight(line, " \t")):]
		for _, c := range trail {
			if c == ' ' {
				bits.WriteByte('0')
			} else {
				bits.WriteByte('1')
			}
		}
		for _, word := range strings.FieldsFunc(line, func(r rune) bool { return !unicode.IsLetter(r) && !unicode.IsDigit(r) && r != '_' }) {
			latin, foreign := false, []string{}
			for _, r := range word {
				switch {
				case unicode.Is(unicode.Latin, r):
					latin = true
				case unicode.Is(unicode.Cyrillic, r) || unicode.Is(unicode.Greek, r) || unicode.Is(unicode.Armenian, r) || (r >= 0xFF01 && r <= 0xFF5E):
					foreign = append(foreign, fmt.Sprintf("U+%04X %c", r, r))
				}
			}
			if latin && len(foreign) > 0 && len(w.Homoglyphs) < 50 {
				w.Homoglyphs = append(w.Homoglyphs, Homoglyph{Line: i + 1, Word: word, Foreign: strings.Join(foreign, ", ")})
			}
		}
	}
	if bits.Len() >= 16 {
		if t := bitsToText(bits.String(), 8); t != "" && printable(t) {
			w.TrailingBits = t
		}
	}
	return w
}

func printable(s string) bool {
	if s == "" {
		return false
	}
	for _, r := range s {
		if !(unicode.IsPrint(r) || r == '\n' || r == '\t') {
			return false
		}
	}
	return true
}

// Describe is the report as text.
func (w WatermarkReport) Describe() string {
	if !w.Found() {
		return "No watermark found: no invisible characters (zero-width, tag, direction, variation selectors), no look-alike letters from other alphabets inside words, no lines ending in spaces or tabs."
	}
	var b strings.Builder
	if w.Hidden.Count > 0 || w.Hidden.Trailing > 0 {
		b.WriteString(w.Hidden.Describe() + "\n")
	}
	if w.Selectors > 0 {
		fmt.Fprintf(&b, "%d variation selectors that are not emoji styling: each can carry a byte.", w.Selectors)
		if w.SelectorText != "" {
			fmt.Fprintf(&b, " Read as bytes they say: %s", clipText(w.SelectorText, 500))
		}
		b.WriteString("\n")
	}
	if w.TrailingBits != "" {
		fmt.Fprintf(&b, "Trailing whitespace read as bits (space 0, tab 1) says: %s\n", clipText(w.TrailingBits, 500))
	}
	for i, h := range w.Homoglyphs {
		if i == 10 {
			fmt.Fprintf(&b, "... and %d more words with look-alike letters\n", len(w.Homoglyphs)-10)
			break
		}
		fmt.Fprintf(&b, "Line %d: %q mixes Latin letters with look-alikes from another alphabet (%s): it looks like a normal word but is not the same text.\n", h.Line, h.Word, h.Foreign)
	}
	return strings.TrimRight(b.String(), "\n")
}

// CleanText removes invisible marks and trailing whitespace. Look-alike letters are only listed, never changed: a Cyrillic word can be
// meant, and replacing letters could change what code does. A BOM at the very start is kept.
func CleanText(s string) (string, []string) {
	counts := map[string]int{}
	var b strings.Builder
	var prev rune
	for i, r := range s {
		switch {
		case r == 0xFEFF && i == 0:
			b.WriteRune(r)
		case isHidden(r):
			name := zeroWidthNames[r]
			if name == "" {
				name = "tag character"
			}
			counts[name]++
		case Selector(r) >= 0 && !emojiStyle(prev, r):
			counts["variation selector"]++
		default:
			b.WriteRune(r)
		}
		prev = r
	}
	lines := strings.Split(b.String(), "\n")
	for i, l := range lines {
		cr := strings.HasSuffix(l, "\r")
		t := strings.TrimRight(strings.TrimSuffix(l, "\r"), " \t")
		if len(t) != len(strings.TrimSuffix(l, "\r")) {
			counts["line with trailing spaces or tabs"]++
		}
		if cr {
			t += "\r"
		}
		lines[i] = t
	}
	var removed []string
	for k, n := range counts {
		removed = append(removed, fmt.Sprintf("%d %s", n, k))
	}
	sort.Strings(removed)
	return strings.Join(lines, "\n"), removed
}

// CleanImage writes the picture again without anything but its pixels (no metadata, nothing after the end of the format) and with
// every colour's lowest bit replaced by a random one, which removes an LSB payload whatever its order or key and changes no colour
// by more than 1 in 255: the eye cannot see it. PNG and GIF come out as PNG, JPEG as JPEG (re-encoding also scrambles a
// payload hidden in its coefficients).
func CleanImage(b []byte) ([]byte, string, error) {
	img, format, err := image.Decode(bytes.NewReader(b))
	if err != nil {
		return nil, "", fmt.Errorf("not a picture I can clean (PNG, JPEG or GIF): %v", err)
	}
	px := image.NewNRGBA(img.Bounds())
	draw.Draw(px, px.Bounds(), img, img.Bounds().Min, draw.Src)
	noise := make([]byte, len(px.Pix))
	if _, err := rand.Read(noise); err != nil {
		return nil, "", err
	}
	for i := range px.Pix {
		if i%4 != 3 { // colour channels, not alpha
			px.Pix[i] = px.Pix[i]&0xFE | noise[i]&1
		}
	}
	var out bytes.Buffer
	ext := ".png"
	if format == "jpeg" {
		err, ext = jpeg.Encode(&out, px, &jpeg.Options{Quality: 95}), ".jpg"
	} else {
		err = png.Encode(&out, px)
	}
	return out.Bytes(), ext, err
}

// cleanPath is where the cleaned copy goes: next to the original, never over it.
func cleanPath(orig, ext string) string {
	base := strings.TrimSuffix(orig, filepath.Ext(orig))
	if ext == "" {
		ext = filepath.Ext(orig)
	}
	return base + ".clean" + ext
}

// ScanFolder reports watermarks in every text file under dir (up to limit files, skipping .git, node_modules and build output) and
// lists the pictures there for a closer look.
func ScanFolder(dir string, limit int) string {
	var b strings.Builder
	var pictures []string
	files, marked := 0, 0
	_ = filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			switch d.Name() {
			case ".git", "node_modules", "target", ".port_sessions", "__pycache__":
				return filepath.SkipDir
			}
			return nil
		}
		if files >= limit {
			return filepath.SkipAll
		}
		info, err := d.Info()
		if err != nil || !info.Mode().IsRegular() || info.Size() > 4<<20 {
			return nil
		}
		data, err := os.ReadFile(p)
		if err != nil {
			return nil
		}
		rel, _ := filepath.Rel(dir, p)
		switch strings.ToLower(filepath.Ext(p)) {
		case ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp":
			pictures = append(pictures, rel)
			return nil
		}
		if bytes.IndexByte(data, 0) >= 0 {
			return nil // binary
		}
		files++
		if w := Watermarks(string(data)); w.Found() {
			marked++
			fmt.Fprintf(&b, "== %s\n%s\n", rel, w.Describe())
		}
		return nil
	})
	head := fmt.Sprintf("Looked at %d text files under %s: %d with marks.", files, dir, marked)
	if marked == 0 {
		head += " None has invisible characters, look-alike letters or trailing whitespace."
	}
	if len(pictures) > 0 {
		head += fmt.Sprintf("\nPictures there (look at each with action inspect: metadata, data after the end, LSB): %s", strings.Join(pictures, ", "))
	}
	return strings.TrimRight(head+"\n"+b.String(), "\n")
}
