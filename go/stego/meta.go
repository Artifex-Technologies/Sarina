package stego

import (
	"bytes"
	"compress/zlib"
	"encoding/binary"
	"fmt"
	"io"
	"regexp"
	"strings"
)

// Container is what the format's own structure says: its parts, any text metadata, where the format ends, and what follows.
type Container struct {
	Parts    []string // chunks or segments, in order
	Text     []string // metadata text (keyword: value)
	Unusual  []string // parts that do not belong to a normal file of this kind
	End      int      // where the format says the file ends (-1: unknown)
	Problems []string
}

// Trailing is what lies after End: a classic place to hide a file.
func (c Container) Trailing(b []byte) []byte {
	if c.End < 0 || c.End >= len(b) {
		return nil
	}
	return b[c.End:]
}

var pngStandard = map[string]bool{"IHDR": true, "PLTE": true, "IDAT": true, "IEND": true, "tEXt": true, "zTXt": true, "iTXt": true, "gAMA": true,
	"cHRM": true, "sRGB": true, "iCCP": true, "bKGD": true, "pHYs": true, "tIME": true, "tRNS": true, "sBIT": true, "hIST": true, "sPLT": true,
	"eXIf": true, "acTL": true, "fcTL": true, "fdAT": true, "oFFs": true, "pCAL": true, "sCAL": true, "sTER": true, "cICP": true, "mDCv": true, "cLLi": true}

// ParsePNG walks the chunks.
func ParsePNG(b []byte) Container {
	c := Container{End: -1}
	pos := 8
	idat := 0
	for pos+8 <= len(b) {
		n := int(binary.BigEndian.Uint32(b[pos:]))
		typ := string(b[pos+4 : pos+8])
		if pos+12+n > len(b) || n < 0 {
			c.Problems = append(c.Problems, fmt.Sprintf("chunk %s at %d claims %d bytes, past the end of the file", typ, pos, n))
			break
		}
		data := b[pos+8 : pos+8+n]
		if typ == "IDAT" {
			idat++
		} else {
			c.Parts = append(c.Parts, fmt.Sprintf("%s (%d bytes)", typ, n))
		}
		if !pngStandard[typ] {
			c.Unusual = append(c.Unusual, fmt.Sprintf("%s at offset %d, %d bytes%s", typ, pos, n, map[bool]string{true: " (private chunk: not in the standard)", false: ""}[typ[0] >= 'a' && typ[0] <= 'z']))
		}
		switch typ {
		case "tEXt":
			if k, v, ok := bytes.Cut(data, []byte{0}); ok {
				c.Text = append(c.Text, fmt.Sprintf("%s: %s", k, clipText(string(v), 400)))
			}
		case "zTXt":
			if k, rest, ok := bytes.Cut(data, []byte{0}); ok && len(rest) > 1 {
				if v, err := inflate(rest[1:]); err == nil {
					c.Text = append(c.Text, fmt.Sprintf("%s (compressed): %s", k, clipText(string(v), 400)))
				}
			}
		case "iTXt":
			if k, rest, ok := bytes.Cut(data, []byte{0}); ok && len(rest) > 2 {
				compressed := rest[0] == 1
				rest = rest[2:]
				_, rest, _ = bytes.Cut(rest, []byte{0}) // language
				_, rest, _ = bytes.Cut(rest, []byte{0}) // translated keyword
				v := rest
				if compressed {
					if d, err := inflate(rest); err == nil {
						v = d
					}
				}
				c.Text = append(c.Text, fmt.Sprintf("%s: %s", k, clipText(string(v), 400)))
			}
		case "IHDR":
			if n >= 13 {
				c.Parts[len(c.Parts)-1] = fmt.Sprintf("IHDR %dx%d, bit depth %d, colour type %d", binary.BigEndian.Uint32(data), binary.BigEndian.Uint32(data[4:]), data[8], data[9])
			}
		case "eXIf":
			c.Text = append(c.Text, exifText(data)...)
		}
		pos += 12 + n
		if typ == "IEND" {
			c.End = pos
			break
		}
	}
	if idat > 0 {
		c.Parts = append(c.Parts, fmt.Sprintf("IDAT x%d (the picture)", idat))
	}
	if c.End < 0 && len(c.Problems) == 0 {
		c.Problems = append(c.Problems, "no IEND chunk: the file is cut short")
	}
	return c
}

func inflate(b []byte) ([]byte, error) {
	r, err := zlib.NewReader(bytes.NewReader(b))
	if err != nil {
		return nil, err
	}
	defer r.Close()
	return io.ReadAll(io.LimitReader(r, 1<<20))
}

func clipText(s string, n int) string {
	s = strings.ToValidUTF8(strings.TrimSpace(s), "�")
	if len(s) > n {
		return s[:n] + "…"
	}
	return s
}

var jpegApp = map[byte]string{0xe0: "APP0 (JFIF)", 0xe1: "APP1 (Exif or XMP)", 0xe2: "APP2 (ICC profile)", 0xed: "APP13 (Photoshop IPTC)", 0xee: "APP14 (Adobe)",
	0xdb: "DQT (quantisation)", 0xc4: "DHT (Huffman)", 0xc0: "SOF0 (baseline)", 0xc2: "SOF2 (progressive)", 0xdd: "DRI", 0xfe: "COM (comment)", 0xda: "SOS (image data)"}

// ParseJPEG walks the segments to the end-of-image marker.
func ParseJPEG(b []byte) Container {
	c := Container{End: -1}
	pos := 2
	for pos+4 <= len(b) {
		if b[pos] != 0xff {
			c.Problems = append(c.Problems, fmt.Sprintf("expected a marker at %d", pos))
			break
		}
		m := b[pos+1]
		if m == 0xd8 || (m >= 0xd0 && m <= 0xd7) || m == 0x01 || m == 0xff {
			pos += 2
			continue
		}
		n := int(binary.BigEndian.Uint16(b[pos+2:]))
		if pos+2+n > len(b) {
			c.Problems = append(c.Problems, fmt.Sprintf("segment %02x at %d runs past the end", m, pos))
			break
		}
		data := b[pos+4 : pos+2+n]
		name := jpegApp[m]
		if name == "" {
			name = fmt.Sprintf("marker FF%02X", m)
		}
		switch m {
		case 0xfe:
			c.Text = append(c.Text, "comment: "+clipText(string(data), 400))
		case 0xe1:
			switch {
			case bytes.HasPrefix(data, []byte("Exif\x00\x00")):
				name = "APP1 (Exif)"
				c.Text = append(c.Text, exifText(data[6:])...)
			case bytes.HasPrefix(data, []byte("http://ns.adobe.com/xap/1.0/")):
				name = "APP1 (XMP)"
				c.Text = append(c.Text, xmpText(data)...)
			}
		case 0xc0, 0xc1, 0xc2:
			if len(data) >= 6 {
				name += fmt.Sprintf(" %dx%d", binary.BigEndian.Uint16(data[3:]), binary.BigEndian.Uint16(data[1:]))
			}
		}
		if m >= 0xe3 && m <= 0xef && m != 0xed && m != 0xee {
			c.Unusual = append(c.Unusual, fmt.Sprintf("APP%d segment at %d, %d bytes: not one of the usual ones", m-0xe0, pos, n))
		}
		c.Parts = append(c.Parts, fmt.Sprintf("%s (%d bytes)", name, n))
		pos += 2 + n
		if m == 0xda {
			// entropy-coded data until the end-of-image marker (FFD9 cannot occur inside it)
			if i := bytes.Index(b[pos:], []byte{0xff, 0xd9}); i >= 0 {
				c.End = pos + i + 2
			} else {
				c.Problems = append(c.Problems, "no end-of-image marker: the file is cut short")
			}
			break
		}
	}
	return c
}

// exifText reads the ASCII tags people care about from a TIFF/Exif block, and says whether GPS tags are there.
func exifText(t []byte) []string {
	if len(t) < 8 {
		return nil
	}
	var order binary.ByteOrder
	switch string(t[:2]) {
	case "II":
		order = binary.LittleEndian
	case "MM":
		order = binary.BigEndian
	default:
		return nil
	}
	names := map[uint16]string{0x010e: "description", 0x010f: "make", 0x0110: "model", 0x0131: "software", 0x0132: "modified", 0x013b: "artist",
		0x8298: "copyright", 0x9003: "taken", 0x9286: "user comment", 0xa420: "image id", 0x9c9b: "XP title", 0x9c9c: "XP comment", 0x9c9e: "XP keywords"}
	var out []string
	seen := map[uint32]bool{}
	var walk func(off uint32, depth int)
	walk = func(off uint32, depth int) {
		if depth > 3 || seen[off] || int(off)+2 > len(t) {
			return
		}
		seen[off] = true
		n := int(order.Uint16(t[off:]))
		for i := 0; i < n && i < 200; i++ {
			e := int(off) + 2 + i*12
			if e+12 > len(t) {
				return
			}
			tag, typ, count := order.Uint16(t[e:]), order.Uint16(t[e+2:]), order.Uint32(t[e+4:])
			size := map[uint16]uint32{1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 7: 1, 9: 4, 10: 8}[typ] * count
			var data []byte
			if size <= 4 {
				data = t[e+8 : e+8+int(size)]
			} else if voff := order.Uint32(t[e+8:]); int(voff)+int(size) <= len(t) {
				data = t[voff : voff+size]
			}
			switch {
			case tag == 0x8769 || tag == 0xa005:
				walk(order.Uint32(t[e+8:]), depth+1)
			case tag == 0x8825:
				out = append(out, "GPS: the picture carries GPS tags (a location)")
			case names[tag] != "" && (typ == 2 || typ == 7 || typ == 1):
				v := data
				if tag == 0x9286 && len(v) > 8 {
					v = v[8:]
				}
				if tag >= 0x9c9b { // UTF-16LE
					var r []byte
					for j := 0; j+1 < len(v); j += 2 {
						if v[j+1] == 0 && v[j] != 0 {
							r = append(r, v[j])
						}
					}
					v = r
				}
				if s := clipText(strings.TrimRight(string(v), "\x00"), 300); s != "" {
					out = append(out, names[tag]+": "+s)
				}
			}
		}
	}
	walk(order.Uint32(t[4:]), 0)
	return out
}

var xmpTags = regexp.MustCompile(`(?s)<(dc:description|dc:creator|dc:subject|dc:title|xmp:CreatorTool|photoshop:Instructions|dc:rights)[^>]*>(.*?)</`)

func xmpText(b []byte) []string {
	var out []string
	for _, m := range xmpTags.FindAllSubmatch(b, 8) {
		v := regexp.MustCompile(`<[^>]+>`).ReplaceAll(m[2], []byte(" "))
		if s := clipText(strings.Join(strings.Fields(string(v)), " "), 300); s != "" {
			out = append(out, "XMP "+string(m[1])+": "+s)
		}
	}
	if len(out) == 0 {
		out = append(out, fmt.Sprintf("XMP block of %d bytes (no description tags)", len(b)))
	}
	return out
}

// ParseGIF finds comment extensions and the trailer.
func ParseGIF(b []byte) Container {
	c := Container{End: -1}
	for i := 0; i+2 < len(b); i++ {
		if b[i] == 0x21 && b[i+1] == 0xfe { // comment extension: sub-blocks
			var text []byte
			j := i + 2
			for j < len(b) && b[j] != 0 {
				n := int(b[j])
				if j+1+n > len(b) {
					break
				}
				text = append(text, b[j+1:j+1+n]...)
				j += 1 + n
			}
			c.Text = append(c.Text, "comment: "+clipText(string(text), 400))
		}
	}
	if i := bytes.LastIndexByte(b, 0x3b); i >= 0 {
		c.End = i + 1
	}
	c.Parts = append(c.Parts, fmt.Sprintf("GIF %s", string(b[:6])))
	return c
}

// ParseRIFF lists the chunks of a WAV or WebP and reads LIST/INFO text (title, artist, comment...).
func ParseRIFF(b []byte) Container {
	c := Container{End: -1}
	if len(b) < 12 {
		return c
	}
	size := int(binary.LittleEndian.Uint32(b[4:]))
	c.End = 8 + size
	if c.End > len(b) {
		c.Problems = append(c.Problems, fmt.Sprintf("the RIFF header claims %d bytes but the file has %d", c.End, len(b)))
		c.End = -1
	}
	pos := 12
	for pos+8 <= len(b) {
		id := string(b[pos : pos+4])
		n := int(binary.LittleEndian.Uint32(b[pos+4:]))
		if pos+8+n > len(b) {
			break
		}
		data := b[pos+8 : pos+8+n]
		if id == "LIST" && len(data) >= 4 && string(data[:4]) == "INFO" {
			p := 4
			for p+8 <= len(data) {
				k := string(data[p : p+4])
				m := int(binary.LittleEndian.Uint32(data[p+4:]))
				if p+8+m > len(data) {
					break
				}
				c.Text = append(c.Text, fmt.Sprintf("%s: %s", k, clipText(strings.TrimRight(string(data[p+8:p+8+m]), "\x00"), 300)))
				p += 8 + m + (m & 1)
			}
		}
		if id == "fmt " && len(data) >= 16 {
			c.Parts = append(c.Parts, fmt.Sprintf("fmt: format %d, %d channel(s), %d Hz, %d bits", binary.LittleEndian.Uint16(data), binary.LittleEndian.Uint16(data[2:]), binary.LittleEndian.Uint32(data[4:]), binary.LittleEndian.Uint16(data[14:])))
		} else {
			c.Parts = append(c.Parts, fmt.Sprintf("%s (%d bytes)", strings.TrimSpace(id), n))
		}
		if id != "fmt " && id != "data" && id != "LIST" && id != "fact" && id != "VP8 " && id != "VP8L" && id != "VP8X" && id != "ALPH" && id != "ANIM" && id != "ANMF" && id != "EXIF" && id != "XMP " && id != "ICCP" && id != "cue " && id != "smpl" && id != "bext" && id != "id3 " && id != "PEAK" && id != "JUNK" {
			c.Unusual = append(c.Unusual, fmt.Sprintf("chunk %q at %d, %d bytes", id, pos, n))
		}
		pos += 8 + n + (n & 1)
	}
	return c
}

var interesting = regexp.MustCompile(`(?i)(flag\{|ctf\{|https?://|www\.|@[a-z0-9-]+\.[a-z]{2,}|passw|secret|\bkey\b|token|-----BEGIN|\.(zip|pdf|exe|sh|py)\b|base64|hidden|steg)`)
var base64ish = regexp.MustCompile(`^[A-Za-z0-9+/]{24,}={0,2}$`)

// Strings returns the printable ASCII runs of at least min bytes: the ones worth reading first (URLs, flags, keys, long base64), then
// the count of the rest.
func Strings(b []byte, min, max int) (worth []string, total int) {
	var cur []byte
	flush := func() {
		if len(cur) >= min {
			total++
			s := string(cur)
			if len(worth) < max && (interesting.MatchString(s) || base64ish.MatchString(s)) {
				worth = append(worth, clipText(s, 200))
			}
		}
		cur = cur[:0]
	}
	for _, c := range b {
		if c >= 0x20 && c < 0x7f || c == '\t' {
			cur = append(cur, c)
		} else {
			flush()
		}
	}
	flush()
	return worth, total
}
