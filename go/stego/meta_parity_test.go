package stego

import (
	"bytes"
	"compress/flate"
	"compress/zlib"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"hash/crc32"
	"image"
	"image/color"
	"image/gif"
	"image/jpeg"
	"image/png"
	"math/rand"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityStegoMeta records (testdata/parity/stegometa.json) the container readers: ParsePNG (text chunks, zlib
// through inflate, eXIf), ParseJPEG (comments, Exif in both byte orders, XMP, odd segments), ParseGIF, ParseRIFF,
// Trailing and Strings, on files Go's encoders make and on broken ones (truncations and byte changes at every
// position of a PNG and a JPEG), and inflate itself on good, damaged and over-long zlib streams.
func TestParityStegoMeta(t *testing.T) {
	rng := rand.New(rand.NewSource(7))
	img := image.NewRGBA(image.Rect(0, 0, 4, 3))
	for i := range img.Pix {
		img.Pix[i] = byte(rng.Intn(256))
	}
	var pngBuf, jpgBuf, gifBuf bytes.Buffer
	if err := png.Encode(&pngBuf, img); err != nil {
		t.Fatal(err)
	}
	if err := jpeg.Encode(&jpgBuf, img, &jpeg.Options{Quality: 50}); err != nil {
		t.Fatal(err)
	}
	pal := image.NewPaletted(image.Rect(0, 0, 2, 2), color.Palette{color.Black, color.White})
	if err := gif.Encode(&gifBuf, pal, nil); err != nil {
		t.Fatal(err)
	}
	z := func(level int, data []byte) []byte {
		var b bytes.Buffer
		w, _ := zlib.NewWriterLevel(&b, level)
		_, _ = w.Write(data)
		_ = w.Close()
		return b.Bytes()
	}
	raw := func(level int, data []byte) []byte { // deflate without the zlib frame
		var b bytes.Buffer
		w, _ := flate.NewWriter(&b, level)
		_, _ = w.Write(data)
		_ = w.Close()
		return b.Bytes()
	}
	frame := func(header []byte, deflate []byte, sum uint32) []byte {
		out := append(append([]byte(nil), header...), deflate...)
		return binary.BigEndian.AppendUint32(out, sum)
	}
	adler := func(b []byte) uint32 {
		a, c := uint32(1), uint32(0)
		for _, x := range b {
			a = (a + uint32(x)) % 65521
			c = (c + a) % 65521
		}
		return c<<16 | a
	}
	text := []byte(strings.Repeat("the quick brown fox jumps over the lazy dog; ", 40))
	big := bytes.Repeat([]byte("A"), 3<<20)
	random := make([]byte, 5000)
	rng.Read(random)

	// inflate on its own
	streams := [][]byte{z(-1, text), z(0, text), z(1, text), z(9, text), z(2, random), z(0, nil), z(9, big), z(9, append(text, random...)),
		frame([]byte{0x78, 0x9c}, raw(9, text), adler(text)+1), frame([]byte{0x78, 0x9c}, raw(9, text), adler(text))[:20],
		frame([]byte{0x78, 0x9c}, raw(9, big), 0), frame([]byte{0x78, 0xbb, 0, 0, 0, 1}, raw(9, text), adler(text)),
		frame([]byte{0x78, 0xbb, 0, 0, 0, 2}, raw(9, text), adler(text)), frame([]byte{0x79, 0x9c}, raw(9, text), adler(text)),
		frame([]byte{0x88, 0x98}, raw(9, text), adler(text)), frame([]byte{0x78, 0x9d}, raw(9, text), adler(text)), {0x78}, nil,
		append(z(9, text), 1, 2, 3), frame([]byte{0x78, 0x01}, []byte{0x07}, 0), frame([]byte{0x78, 0x01}, []byte{0x01, 5, 0, 0xfa, 0xff, 'h', 'e', 'l', 'l', 'o'}, adler([]byte("hello"))),
		frame([]byte{0x78, 0x01}, []byte{0x01, 5, 0, 0xfa, 0xfe, 'h', 'e', 'l', 'l', 'o'}, 0), frame([]byte{0x78, 0x01}, []byte{0x01, 5, 0, 0xfa, 0xff, 'h', 'e'}, 0),
		frame([]byte{0x78, 0x01}, []byte{0x00, 0, 0, 0xff, 0xff, 0x03, 0x00}, 1), frame([]byte{0x78, 0x01}, []byte{0x03, 0x00}, 1),
		frame([]byte{0x78, 0x01}, []byte{0x63, 0x00, 0x00}, 0)}
	// a stored block that ends exactly at, then just past, the 1 MiB inflate keeps
	stored := func(n int) []byte {
		var d []byte
		for n > 0 {
			k := min(n, 65535)
			n -= k
			final := byte(0)
			if n == 0 {
				final = 1
			}
			d = append(d, final, byte(k), byte(k>>8), ^byte(k), ^byte(k>>8))
			d = append(d, bytes.Repeat([]byte{'s'}, k)...)
		}
		return d
	}
	// (held in the fixture as {"stored": n, "good": checksum right}: 2 MiB of hex each otherwise)
	var inflates []any
	for _, st := range []struct {
		n    int
		good bool
	}{{1<<20 + 1, false}, {1<<20 - 1, false}, {1<<20 - 1, true}, {1 << 20, false}, {1 << 20, true}} {
		sum := uint32(0)
		if st.good {
			sum = adler(bytes.Repeat([]byte{'s'}, st.n))
		}
		out, err := inflate(frame([]byte{0x78, 0x01}, stored(st.n), sum))
		h := sha256.Sum256(out)
		inflates = append(inflates, []any{map[string]any{"stored": st.n, "good": st.good}, err == nil, len(out), hex.EncodeToString(h[:])})
	}
	// dynamic blocks put together by hand: more literal codes than deflate has, a repeat with nothing to repeat, a
	// repeat one past the end of the lengths
	streams = append(streams, frame([]byte{0x78, 0x01}, handBlock(287, false, 0), adler([]byte("aaa"))),
		frame([]byte{0x78, 0x01}, handBlock(257, true, 0), adler([]byte("aaa"))), frame([]byte{0x78, 0x01}, handBlock(286, false, 1), adler([]byte("aaa"))),
		frame([]byte{0x78, 0x01}, handBlock(257, false, 0), adler([]byte("aaa"))), frame([]byte{0x78, 0x01}, handBlock(286, false, 0), adler([]byte("aaa"))))
	// every byte of a dynamic-Huffman stream changed, and every truncation of it
	dyn := z(9, text[:300])
	for i := range dyn {
		for _, x := range []byte{0x01, 0x10, 0x80} {
			c := append([]byte(nil), dyn...)
			c[i] ^= x
			streams = append(streams, c)
		}
		streams = append(streams, dyn[:i])
	}
	fixed := z(1, []byte("abcabcabcabcabc hello hello hello"))
	for i := range fixed {
		c := append([]byte(nil), fixed...)
		c[i] ^= 0x04
		streams = append(streams, c)
	}
	for _, s := range streams {
		out, err := inflate(s)
		sum := sha256.Sum256(out)
		inflates = append(inflates, []any{hex.EncodeToString(s), err == nil, len(out), hex.EncodeToString(sum[:])})
	}
	cases := []parity.Case{{Fn: "inflate", Out: inflates}}

	// the containers
	chunk := func(typ string, data []byte) []byte {
		b := binary.BigEndian.AppendUint32(nil, uint32(len(data)))
		b = append(b, typ...)
		b = append(b, data...)
		return binary.BigEndian.AppendUint32(b, crc32.ChecksumIEEE(append([]byte(typ), data...)))
	}
	cat := func(parts ...[]byte) []byte {
		var out []byte
		for _, p := range parts {
			out = append(out, p...)
		}
		return out
	}
	p := pngBuf.Bytes()
	iend := bytes.LastIndex(p, []byte("IEND")) - 4
	withChunks := func(chunks ...[]byte) []byte { return cat(p[:iend], cat(chunks...), p[iend:]) }
	exif := func(order byteOrder, entries [][]byte, extra []byte) []byte {
		var b []byte
		if order == binary.LittleEndian {
			b = []byte("II*\x00")
		} else {
			b = []byte("MM\x00*")
		}
		b = order.AppendUint32(b, 8)
		b = order.AppendUint16(b, uint16(len(entries)))
		for _, e := range entries {
			b = append(b, e...)
		}
		b = order.AppendUint32(b, 0)
		return append(b, extra...)
	}
	entry := func(order byteOrder, tag, typ uint16, count uint32, value []byte) []byte {
		b := order.AppendUint16(nil, tag)
		b = order.AppendUint16(b, typ)
		b = order.AppendUint32(b, count)
		v := make([]byte, 4)
		copy(v, value)
		return append(b, v...)
	}
	off := func(order byteOrder, n uint32) []byte { return order.AppendUint32(nil, n) }
	var exifs [][]byte
	for _, o := range []byteOrder{binary.LittleEndian, binary.BigEndian} {
		// strings after the IFD: 8 + 2 + n*12 + 4
		const n = 10
		base := uint32(8 + 2 + n*12 + 4)
		strs := []byte("Canon\x00Model X1\x00\x00ASCII\x00\x00\x00a user comment here\x00a\x00b\x00c\x00\x00\x00my artist name\x00")
		exifs = append(exifs, exif(o, [][]byte{
			entry(o, 0x010f, 2, 6, off(o, base)), entry(o, 0x0110, 2, 10, off(o, base+6)), entry(o, 0x0131, 2, 4, []byte("GIM\x00")),
			entry(o, 0x9286, 7, 28, off(o, base+16)), entry(o, 0x9c9b, 1, 10, off(o, base+44)), entry(o, 0x8825, 4, 1, off(o, 0)),
			entry(o, 0x013b, 2, 15, off(o, base+54)), entry(o, 0x8298, 2, 100, off(o, 9999)), entry(o, 0xa420, 2, 4, []byte("i\x00d\x00")),
			entry(o, 0x010e, 5, 0x20000000, off(o, 8)),
		}, strs))
		exifs = append(exifs, exif(o, [][]byte{entry(o, 0x9286, 7, 9, off(o, 8+2+12+4)), entry(o, 0x9286, 7, 8, off(o, 8+2+12+4))}, []byte("ASCII\x00\x00\x00Z")))
		// an Exif sub-IFD, a loop back to the first IFD, deeper than three levels
		exifs = append(exifs, exif(o, [][]byte{entry(o, 0x8769, 4, 1, off(o, 26)), entry(o, 0x010f, 2, 3, []byte("ab\x00"))},
			cat(o.AppendUint16(nil, 2), entry(o, 0xa005, 4, 1, off(o, 8)), entry(o, 0x0110, 2, 4, []byte("cam\x00")))))
		exifs = append(exifs, exif(o, [][]byte{entry(o, 0x8769, 4, 1, off(o, 8))}, nil), exif(o, [][]byte{entry(o, 0x8769, 4, 1, off(o, 0xffffff))}, nil))
		var chain []byte
		for i := 0; i < 6; i++ {
			chain = append(chain, cat(o.AppendUint16(nil, 2), entry(o, 0x8769, 4, 1, off(o, uint32(26+(i+1)*30))), entry(o, 0x0131, 2, 4, []byte{'L', byte('0' + i), 0, 0}), o.AppendUint32(nil, 0))...)
		}
		exifs = append(exifs, exif(o, [][]byte{entry(o, 0x8769, 4, 1, off(o, 26))}, chain))
	}
	many := make([][]byte, 250)
	for i := range many {
		many[i] = entry(binary.LittleEndian, 0x0131, 2, 2, []byte{byte('a' + i%26), 0})
	}
	exifs = append(exifs, exif(binary.LittleEndian, many, nil), []byte("XX*\x00\x08\x00\x00\x00"), []byte("II*\x00"), exif(binary.LittleEndian, nil, nil)[:9])
	xmp := []byte(`http://ns.adobe.com/xap/1.0/` + "\x00" + `<x:xmpmeta><rdf:Description><dc:description><rdf:Alt><rdf:li xml:lang="x-default">a  hidden
	message</rdf:li></rdf:Alt></dc:description><dc:creator>me</dc:creator><xmp:CreatorTool>` + "\xff\xfe" + `tool</xmp:CreatorTool><dc:title></dc:title></rdf:Description></x:xmpmeta>`)
	app := func(m byte, data []byte) []byte {
		return cat([]byte{0xff, m}, binary.BigEndian.AppendUint16(nil, uint16(len(data)+2)), data)
	}
	j := jpgBuf.Bytes()
	withSegs := func(segs ...[]byte) []byte { return cat(j[:2], cat(segs...), j[2:]) }
	riff := func(form string, chunks ...[]byte) []byte {
		body := cat(append([]byte(form), cat(chunks...)...))
		return cat([]byte("RIFF"), binary.LittleEndian.AppendUint32(nil, uint32(len(body))), body)
	}
	rc := func(id string, data []byte) []byte {
		b := append([]byte(id), binary.LittleEndian.AppendUint32(nil, uint32(len(data)))...)
		b = append(b, data...)
		if len(data)%2 == 1 {
			b = append(b, 0)
		}
		return b
	}
	fmtc := cat(binary.LittleEndian.AppendUint16(nil, 1), binary.LittleEndian.AppendUint16(nil, 2), binary.LittleEndian.AppendUint32(nil, 44100), make([]byte, 6), binary.LittleEndian.AppendUint16(nil, 16))
	info := cat([]byte("INFO"), rc("INAM", []byte("Title\x00")), rc("IART", []byte("odd")), rc("ICMT", []byte(strings.Repeat("c", 400))))
	type in struct {
		kind string
		b    []byte
	}
	g := gifBuf.Bytes()
	gifComment := cat(g[:len(g)-1], []byte{0x21, 0xfe, 5, 'h', 'e', 'l', 'l', 'o', 3, ' ', 'y', 'o', 0}, []byte{0x21, 0xfe, 9, 'c', 'u', 't'}, g[len(g)-1:])
	inputs := []in{{"png", p}, {"png", withChunks(chunk("tEXt", []byte("Comment\x00 hello there ")), chunk("tEXt", []byte("no separator")),
		chunk("zTXt", cat([]byte("Secret\x00\x00"), z(9, text))), chunk("zTXt", []byte("Bad\x00\x00\x78\x9c\x01")), chunk("zTXt", []byte("Short\x00\x00")),
		chunk("iTXt", cat([]byte("XML\x00\x00\x00en\x00Titel\x00"), []byte("plain itxt \xff\xfe"))), chunk("iTXt", cat([]byte("Z\x00\x01\x00\x00\x00"), z(9, []byte("packed itxt")))),
		chunk("iTXt", cat([]byte("Zbad\x00\x01\x00\x00\x00"), []byte{1, 2, 3})), chunk("iTXt", []byte("k\x00\x01")), chunk("eXIf", exifs[0]), chunk("prVt", []byte("x")),
		chunk("ABCD", nil), chunk("zTXt", cat([]byte("Big\x00\x00"), z(9, big))), chunk("cLLi", nil), chunk("mDCv", nil), chunk("sTER", nil))},
		{"png", cat(p[:8], chunk("IHDR", make([]byte, 12)), chunk("IEND", nil))},
		{"png", withChunks(chunk("tEXt", cat([]byte("Long\x00"), bytes.Repeat([]byte("é"), 300))))},
		{"png", append(append([]byte(nil), p...), "trailing secret data"...)}, {"png", p[:len(p)-5]}, {"png", p[:30]}, {"png", p[:8]},
		{"png", cat(p[:8], chunk("IHDR", []byte{0, 0, 0, 1}), chunk("IEND", nil))}, {"png", cat(p[:8], []byte{0xff, 0xff, 0xff, 0xff}, []byte("IDAT"))},
		{"jpeg", j}, {"jpeg", withSegs(app(0xfe, []byte("a comment  ")), app(0xe1, cat([]byte("Exif\x00\x00"), exifs[1])), app(0xe1, xmp), app(0xe5, []byte("odd")),
			app(0xe1, []byte("other")), app(0xed, []byte("iptc")), app(0xee, []byte("Adobe")), app(0xef, nil), app(0xe3, nil), app(0xc1, []byte{8, 0, 3, 0, 4, 1}), app(0xc2, []byte{8, 0}))},
		{"jpeg", withSegs(app(0xe1, []byte("http://ns.adobe.com/xap/1.0/\x00<x/>")))}, {"jpeg", append(append([]byte(nil), j...), "after the end"...)},
		{"jpeg", j[:len(j)-2]}, {"jpeg", j[:40]}, {"jpeg", []byte{0xff, 0xd8, 0x00, 0x00, 0x00}}, {"jpeg", []byte{0xff, 0xd8, 0xff, 0xd8, 0xff, 0x01, 0xff, 0xd0, 0xff, 0xe0, 0x00, 0x00}},
		{"jpeg", []byte{0xff, 0xd8, 0xff, 0xe0, 0x00, 0x01, 0, 0}}, {"jpeg", []byte{0xff, 0xd8, 0xff, 0xe0, 0xff, 0xff, 0, 0}},
		{"gif", g}, {"gif", gifComment}, {"gif", append(append([]byte(nil), g...), "tail"...)}, {"gif", g[:len(g)-1]},
		{"riff", riff("WAVE", rc("fmt ", fmtc), rc("LIST", info), rc("data", []byte{1, 2, 3}), rc("zzzz", []byte("hidden!")))},
		{"riff", riff("WEBP", rc("VP8 ", []byte{1, 2}), rc("EXIF", []byte("e")))}, {"riff", riff("WAVE", rc("fmt ", fmtc[:10]))[:30]},
		{"riff", cat([]byte("RIFF"), binary.LittleEndian.AppendUint32(nil, 4), []byte("WAVE"), rc("data", []byte("tail beyond")))}, {"riff", []byte("RIFF\x00")},
		{"riff", cat(riff("WAVE", rc("fmt ", fmtc)), rc("big ", nil)[:4], binary.LittleEndian.AppendUint32(nil, 1000))},
		{"riff", riff("WAVE", rc("LIST", cat([]byte("INFO"), []byte("INAM"), binary.LittleEndian.AppendUint32(nil, 50), []byte("x"))))},
	}
	for _, e := range exifs {
		inputs = append(inputs, in{"png", withChunks(chunk("eXIf", e))}, in{"jpeg", withSegs(app(0xe1, cat([]byte("Exif\x00\x00"), e)))})
	}
	// every truncation and a byte change at every position of a PNG with text and Exif, and of a JPEG with Exif
	rich := withChunks(chunk("zTXt", cat([]byte("S\x00\x00"), z(9, []byte("zipped words")))), chunk("eXIf", exifs[0]))
	richJ := withSegs(app(0xe1, cat([]byte("Exif\x00\x00"), exifs[1])), app(0xfe, []byte("hi")))
	for _, r := range []in{{"png", rich}, {"jpeg", richJ[:400]}} {
		for i := range r.b {
			c := append([]byte(nil), r.b...)
			c[i] ^= 0x41
			inputs = append(inputs, in{r.kind, c}, in{r.kind, r.b[:i]})
		}
	}
	var rows []any
	for _, x := range inputs {
		var c Container
		switch x.kind {
		case "png":
			c = ParsePNG(x.b)
		case "jpeg":
			c = ParseJPEG(x.b)
		case "gif":
			c = ParseGIF(x.b)
		default:
			c = ParseRIFF(x.b)
		}
		rows = append(rows, []any{x.kind, hex.EncodeToString(x.b), parity.Texts(c.Parts), parity.Texts(c.Text), parity.Texts(c.Unusual), c.End,
			parity.Texts(c.Problems), len(c.Trailing(x.b))})
	}
	cases = append(cases, parity.Case{Fn: "parse", Out: rows})

	var strs []any
	for _, s := range []string{"", "short", "a plain line of words\x00then flag{hidden_one} here\nwww.example.com/path and passw0rd=x",
		"QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=\x01aGVsbG8gd29ybGQgaGVsbG8gd29ybGQ=\x02AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
		strings.Repeat("x", 300) + " secret", "KEY\tvalue tab ok\x7fdel\xffhigh bytes -----BEGIN RSA something", "file.zip in here!\x00steg it",
		"Mail me at a@b-c.io ok", "the KEYS are not \\bkey\\b but key is", strings.Repeat("ctf{a} ", 30)} {
		worth, total := Strings([]byte(s), 8, 15)
		w2, t2 := Strings([]byte(s), 4, 2)
		strs = append(strs, []any{parity.Text(s), parity.Texts(worth), total, parity.Texts(w2), t2})
	}
	cases = append(cases, parity.Case{Fn: "strings", Out: strs})
	parity.Fixture(t, "stegometa", cases)
}

type byteOrder interface {
	binary.ByteOrder
	binary.AppendByteOrder
}

// handBlock is a final dynamic-Huffman deflate block holding "aaa": literal 'a' (1 bit), end of block and literal
// code 286 (2 bits each), no distance codes, with nlit literal/length lengths sent. startWith16 sends a repeat code
// first (nothing to repeat yet); overrun makes the last run of zero lengths that many longer than the lengths left.
func handBlock(nlit int, startWith16 bool, overrun int) []byte {
	var out []byte
	var acc uint32
	var n uint
	put := func(v uint32, bits uint) {
		acc |= v << n
		n += bits
		for n >= 8 {
			out = append(out, byte(acc))
			acc >>= 8
			n -= 8
		}
	}
	code := func(c uint32, bits uint) { // Huffman codes go most significant bit first
		var r uint32
		for i := uint(0); i < bits; i++ {
			r |= (c >> i & 1) << (bits - 1 - i)
		}
		put(r, bits)
	}
	put(1, 1)                // final
	put(2, 2)                // dynamic
	put(uint32(nlit-257), 5) // HLIT
	put(0, 5)                // HDIST: one distance code
	put(uint32(19-4), 4)     // HCLEN: all 19
	// code-length code lengths in the deflate order 16 17 18 0 8 7 9 6 10 5 11 4 12 3 13 2 14 1 15:
	// 18 -> 1 bit (0), 1 -> 2 bits (10), 2 -> 3 bits (110), 0 -> 4 bits (1110), 16 -> 4 bits (1111)
	lens := map[int]uint32{18: 1, 1: 2, 2: 3, 0: 4, 16: 4}
	for _, sym := range []int{16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15} {
		put(lens[sym], 3)
	}
	zeros := func(k int) { // runs of 11..138 with 18, then single zeros
		for k >= 11 {
			r := min(k, 138)
			code(0, 1)
			put(uint32(r-11), 7)
			k -= r
		}
		for ; k > 0; k-- {
			code(0b1110, 4)
		}
	}
	if startWith16 {
		code(0b1111, 4)
		put(0, 2)
	}
	zeros(97)
	code(0b10, 2) // 'a': length 1
	zeros(256 - 98)
	code(0b110, 3) // end of block: length 2
	if nlit > 286 {
		zeros(286 - 257)
		code(0b110, 3) // literal code 286: length 2
		zeros(nlit - 287 + 1 + overrun)
	} else {
		code(0b110, 3) // code 257: length 2, keeps the code complete
		zeros(nlit - 258 + 1 + overrun)
	}
	for i := 0; i < 3; i++ {
		code(0, 1) // 'a'
	}
	code(0b10, 2) // end of block
	if n > 0 {
		out = append(out, byte(acc))
	}
	return out
}
