package stego

import (
	"fmt"
	"sort"
	"strings"
	"unicode"
)

// zeroWidthNames are the invisible characters used to hide text in text.
var zeroWidthNames = map[rune]string{
	0x200B: "zero-width space", 0x200C: "zero-width non-joiner", 0x200D: "zero-width joiner", 0x2060: "word joiner", 0x2061: "function application",
	0x2062: "invisible times", 0x2063: "invisible separator", 0x2064: "invisible plus", 0xFEFF: "zero-width no-break space (BOM)", 0x00AD: "soft hyphen",
	0x180E: "Mongolian vowel separator", 0x200E: "left-to-right mark", 0x200F: "right-to-left mark", 0x202A: "LRE", 0x202B: "RLE", 0x202C: "PDF", 0x202D: "LRO",
	0x202E: "right-to-left override", 0x2066: "LRI", 0x2067: "RLI", 0x2068: "FSI", 0x2069: "PDI", 0x034F: "combining grapheme joiner", 0x115F: "Hangul filler",
	0x1160: "Hangul jungseong filler", 0x3164: "Hangul filler (wide)", 0xFFA0: "halfwidth Hangul filler",
}

func isHidden(r rune) bool {
	if _, ok := zeroWidthNames[r]; ok {
		return true
	}
	return r >= 0xE0000 && r <= 0xE007F
}

// HiddenText is what invisible characters in a text decode to.
type HiddenText struct {
	Count    int
	Kinds    map[rune]int
	Decoded  []string // each line: how it was read, and the text
	Trailing int      // lines ending in spaces or tabs (whitespace steganography)
}

// FindHidden looks for zero-width, tag and format characters and tries the common ways of reading them: tag characters as ASCII,
// two invisible characters as the bits of bytes.
func FindHidden(s string) HiddenText {
	h := HiddenText{Kinds: map[rune]int{}}
	var hidden []rune
	for _, r := range s {
		if isHidden(r) {
			h.Count++
			h.Kinds[r]++
			hidden = append(hidden, r)
		}
	}
	for _, line := range strings.Split(s, "\n") {
		if t := strings.TrimRight(line, "\r"); t != strings.TrimRight(t, " \t") {
			h.Trailing++
		}
	}
	if h.Count == 0 {
		return h
	}
	// tag characters: each one is an ASCII character + 0xE0000
	var tag strings.Builder
	for _, r := range hidden {
		if r >= 0xE0020 && r <= 0xE007E {
			tag.WriteRune(r - 0xE0000)
		}
	}
	if tag.Len() >= 2 {
		h.Decoded = append(h.Decoded, "tag characters (U+E00xx) read as ASCII: "+clipText(tag.String(), 500))
	}
	// binary: the two most frequent invisible characters (not tags) as 0 and 1, a third as a separator
	type kv struct {
		r rune
		n int
	}
	var kinds []kv
	for r, n := range h.Kinds {
		if r < 0xE0000 {
			kinds = append(kinds, kv{r, n})
		}
	}
	sort.Slice(kinds, func(i, j int) bool {
		if kinds[i].n != kinds[j].n {
			return kinds[i].n > kinds[j].n
		}
		return kinds[i].r < kinds[j].r
	})
	if len(kinds) >= 2 {
		a, b := kinds[0].r, kinds[1].r
		var sep rune = -1
		if len(kinds) >= 3 {
			sep = kinds[2].r
		}
		for _, assign := range [][2]rune{{a, b}, {b, a}} {
			var bits strings.Builder
			for _, r := range hidden {
				switch r {
				case assign[0]:
					bits.WriteByte('0')
				case assign[1]:
					bits.WriteByte('1')
				case sep:
					bits.WriteByte(' ')
				}
			}
			for _, width := range []int{8, 7, 16} {
				if t := bitsToText(bits.String(), width); t != "" {
					h.Decoded = append(h.Decoded, fmt.Sprintf("U+%04X as 0 and U+%04X as 1, %d bits per character: %s", assign[0], assign[1], width, clipText(t, 500)))
					break
				}
			}
			if len(h.Decoded) > 0 && strings.Contains(h.Decoded[len(h.Decoded)-1], "as 0 and") {
				break
			}
		}
	}
	return h
}

// bitsToText groups a bit string (spaces separate characters when present) into width-bit characters, if the result is printable.
func bitsToText(bits string, width int) string {
	var groups []string
	if strings.Contains(bits, " ") {
		groups = strings.Fields(bits)
	} else {
		for i := 0; i+width <= len(bits); i += width {
			groups = append(groups, bits[i:i+width])
		}
	}
	if len(groups) < 2 {
		return ""
	}
	var b strings.Builder
	for _, g := range groups {
		v := 0
		for _, c := range g {
			v = v<<1 | int(c-'0')
		}
		r := rune(v)
		if !(unicode.IsPrint(r) || r == '\n' || r == '\t') || r > 0xFFFF {
			return ""
		}
		b.WriteRune(r)
	}
	return b.String()
}

// Describe renders the findings.
func (h HiddenText) Describe() string {
	if h.Count == 0 && h.Trailing == 0 {
		return "No invisible characters (zero-width, tag or direction marks) and no lines ending in spaces or tabs."
	}
	var b strings.Builder
	if h.Count > 0 {
		fmt.Fprintf(&b, "%d invisible characters:", h.Count)
		var rs []rune
		for r := range h.Kinds {
			rs = append(rs, r)
		}
		sort.Slice(rs, func(i, j int) bool { return rs[i] < rs[j] })
		for _, r := range rs {
			name := zeroWidthNames[r]
			if name == "" {
				name = "tag character"
			}
			fmt.Fprintf(&b, " U+%04X %s x%d;", r, name, h.Kinds[r])
		}
		b.WriteString("\n")
		if len(h.Decoded) == 0 {
			b.WriteString("They do not decode as ASCII by the usual schemes (tag characters, or two kinds as bits). A single kind repeated is sometimes just copy-paste residue; many kinds in a pattern is a hidden message in a scheme I do not know.\n")
		}
		for _, d := range h.Decoded {
			b.WriteString("Hidden text, " + d + "\n")
		}
	}
	if h.Trailing > 0 {
		fmt.Fprintf(&b, "%d line(s) end in spaces or tabs: trailing whitespace can carry data (the snow method); editors also leave it by accident.\n", h.Trailing)
	}
	return strings.TrimRight(b.String(), "\n")
}
