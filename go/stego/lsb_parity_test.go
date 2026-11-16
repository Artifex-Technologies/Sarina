package stego

import (
	"bytes"
	"compress/zlib"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"hash/crc32"
	"image"
	"image/color"
	"image/gif"
	"image/png"
	"math"
	"math/rand"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityStegoLSB records (testdata/parity/stegolsb.json) DecodeImage on PNG and GIF files (every colour type and
// bit depth, interlaced, transparency, palettes too short, IDAT split anywhere, broken checksums, chunk order, zlib and
// LZW damage, every truncation and a byte change at every position of a small PNG and GIF), the pixels it gives,
// Analyse's description, SearchLSB's finds, ExtractBits in every order, Diff, and math.Lgamma behind the statistic.
// JPEG is the next part.
func TestParityStegoLSB(t *testing.T) {
	rng := rand.New(rand.NewSource(11))
	sum := func(b []byte) string {
		if b == nil {
			return ""
		}
		h := sha256.Sum256(b)
		return hex.EncodeToString(h[:8])
	}
	describe := func(b []byte) any {
		p, err := DecodeImage(b)
		if err != nil {
			return map[string]any{"err": err.Error()}
		}
		var found []any
		for _, f := range SearchLSB(p, 4096) {
			found = append(found, []any{f.Order, f.Bits, f.MSBFirst, f.Length, f.Why, hex.EncodeToString(f.Data)})
		}
		var ex []any
		for _, o := range append([]Order{{"palette index", []int{-1}}}, Orders...) {
			for _, bits := range []int{1, 2, 4, 9} {
				ex = append(ex, hex.EncodeToString(ExtractBits(p, o, bits, bits%2 == 0, 40)))
			}
		}
		return map[string]any{"w": p.W, "h": p.H, "format": p.Format, "alpha": p.Alpha, "rgba": sum(p.RGBA), "indices": sum(p.Indices),
			"describe": Analyse(p).Describe(), "found": found, "extract": ex}
	}

	// PNGs from Go's encoder, one per image type
	rect := image.Rect(0, 0, 13, 7)
	fill := func(set func(x, y int, r *rand.Rand)) {
		for y := 0; y < 7; y++ {
			for x := 0; x < 13; x++ {
				set(x, y, rng)
			}
		}
	}
	gray, gray16 := image.NewGray(rect), image.NewGray16(rect)
	rgba, nrgba, rgba64, nrgba64 := image.NewRGBA(rect), image.NewNRGBA(rect), image.NewRGBA64(rect), image.NewNRGBA64(rect)
	fill(func(x, y int, r *rand.Rand) {
		gray.SetGray(x, y, color.Gray{uint8(r.Intn(256))})
		gray16.SetGray16(x, y, color.Gray16{uint16(r.Intn(65536))})
		rgba.SetRGBA(x, y, color.RGBA{uint8(r.Intn(256)), uint8(r.Intn(256)), uint8(r.Intn(256)), 255})
		nrgba.SetNRGBA(x, y, color.NRGBA{uint8(r.Intn(256)), uint8(r.Intn(256)), uint8(r.Intn(256)), uint8(r.Intn(256))})
		a := uint16(r.Intn(65536))
		rgba64.SetRGBA64(x, y, color.RGBA64{uint16(r.Intn(int(a) + 1)), uint16(r.Intn(int(a) + 1)), 0, a})
		nrgba64.SetNRGBA64(x, y, color.NRGBA64{uint16(r.Intn(65536)), uint16(r.Intn(65536)), uint16(r.Intn(65536)), uint16(r.Intn(65536))})
	})
	var images []image.Image
	images = append(images, gray, gray16, rgba, nrgba, rgba64, nrgba64)
	for _, n := range []int{2, 4, 16, 256} {
		pal := make(color.Palette, n)
		for i := range pal {
			pal[i] = color.NRGBA{uint8(rng.Intn(256)), uint8(rng.Intn(256)), uint8(rng.Intn(256)), uint8(255 - (i%3)*100)}
		}
		pm := image.NewPaletted(rect, pal)
		for i := range pm.Pix {
			pm.Pix[i] = uint8(rng.Intn(n))
		}
		images = append(images, pm)
	}
	enc := func(m image.Image) []byte {
		var b bytes.Buffer
		if err := png.Encode(&b, m); err != nil {
			t.Fatal(err)
		}
		return b.Bytes()
	}
	type in struct {
		name string
		b    []byte
	}
	var inputs []in
	for _, m := range images {
		inputs = append(inputs, in{"go encoder", enc(m)})
	}
	// a noisy picture big enough for the first-tenth statistic, and the same with a message in its LSBs
	noisy := image.NewNRGBA(image.Rect(0, 0, 64, 48))
	for i := range noisy.Pix {
		noisy.Pix[i] = uint8(rng.Intn(256))
		if i%4 == 3 {
			noisy.Pix[i] = 255
		}
	}
	withMsg, err := Embed(noisy, []byte("meet me at the old mill at midnight, bring the key"))
	if err != nil {
		t.Fatal(err)
	}
	flat := image.NewNRGBA(image.Rect(0, 0, 64, 48))
	for i := range flat.Pix {
		flat.Pix[i] = uint8(100 + i%4)
	}
	withFile, _ := Embed(flat, append([]byte("PK\x03\x04"), bytes.Repeat([]byte{0x41}, 40)...))
	inputs = append(inputs, in{"noisy", enc(noisy)}, in{"message", withMsg}, in{"flat", enc(flat)}, in{"file", withFile},
		in{"first tenth exactly 256", enc(noisy.SubImage(image.Rect(0, 0, 64, 40)))})

	// hand-made PNGs: every colour type and depth, filters of every kind, interlacing, transparency, split IDATs
	chunk := func(typ string, data []byte) []byte {
		b := binary.BigEndian.AppendUint32(nil, uint32(len(data)))
		b = append(b, typ...)
		b = append(b, data...)
		return binary.BigEndian.AppendUint32(b, crc32.ChecksumIEEE(append([]byte(typ), data...)))
	}
	ihdr := func(w, h, depth, ct, interlace int) []byte {
		b := binary.BigEndian.AppendUint32(nil, uint32(w))
		b = binary.BigEndian.AppendUint32(b, uint32(h))
		return chunk("IHDR", append(b, byte(depth), byte(ct), 0, 0, byte(interlace)))
	}
	channels := map[int]int{0: 1, 2: 3, 3: 1, 4: 2, 6: 4}
	scan := func(w, h, depth, ct, interlace int) []byte { // filtered scanlines, random filters
		bpp := channels[ct] * depth
		var out []byte
		passes := [][4]int{{1, 1, 0, 0}}
		if interlace == 1 {
			passes = [][4]int{{8, 8, 0, 0}, {8, 8, 4, 0}, {4, 8, 0, 4}, {4, 4, 2, 0}, {2, 4, 0, 2}, {2, 2, 1, 0}, {1, 2, 0, 1}}
		}
		for _, p := range passes {
			pw, ph := (w-p[2]+p[0]-1)/p[0], (h-p[3]+p[1]-1)/p[1]
			if pw <= 0 || ph <= 0 {
				continue
			}
			row := (bpp*pw + 7) / 8
			prev := make([]byte, row)
			step := (bpp + 7) / 8
			for y := 0; y < ph; y++ {
				raw := make([]byte, row)
				rng.Read(raw)
				ft := byte(rng.Intn(5))
				f := make([]byte, row)
				for i := range raw {
					var a, c byte
					if i >= step {
						a, c = raw[i-step], prev[i-step]
					}
					b := prev[i]
					switch ft {
					case 0:
						f[i] = raw[i]
					case 1:
						f[i] = raw[i] - a
					case 2:
						f[i] = raw[i] - b
					case 3:
						f[i] = raw[i] - byte((int(a)+int(b))/2)
					case 4:
						pa, pb, pc := absInt(int(b)-int(c)), absInt(int(a)-int(c)), absInt(int(a)+int(b)-2*int(c))
						pr := c
						if pa <= pb && pa <= pc {
							pr = a
						} else if pb <= pc {
							pr = b
						}
						f[i] = raw[i] - pr
					}
				}
				out = append(append(out, ft), f...)
				prev = raw
			}
		}
		return out
	}
	zl := func(b []byte) []byte {
		var o bytes.Buffer
		w := zlib.NewWriter(&o)
		_, _ = w.Write(b)
		_ = w.Close()
		return o.Bytes()
	}
	sig := []byte("\x89PNG\r\n\x1a\n")
	build := func(w, h, depth, ct, interlace int, extra []byte, splits ...int) []byte {
		data := zl(scan(w, h, depth, ct, interlace))
		out := append(append([]byte(nil), sig...), ihdr(w, h, depth, ct, interlace)...)
		out = append(out, extra...)
		prev := 0
		for _, s := range append(splits, len(data)) {
			s = min(s, len(data))
			out = append(out, chunk("IDAT", data[prev:s])...)
			prev = s
		}
		return append(out, chunk("IEND", nil)...)
	}
	plte := func(n int) []byte {
		b := make([]byte, 3*n)
		rng.Read(b)
		return chunk("PLTE", b)
	}
	for _, c := range [][2]int{{1, 0}, {2, 0}, {4, 0}, {8, 0}, {16, 0}, {8, 2}, {16, 2}, {8, 4}, {16, 4}, {8, 6}, {16, 6}} {
		for _, il := range []int{0, 1} {
			for _, size := range [][2]int{{1, 1}, {13, 7}, {9, 17}} {
				inputs = append(inputs, in{"hand", build(size[0], size[1], c[0], c[1], il, nil)})
			}
		}
	}
	for _, depth := range []int{1, 2, 4, 8} {
		for _, il := range []int{0, 1} {
			inputs = append(inputs, in{"palette", build(11, 6, depth, 3, il, plte(1<<depth))}, in{"short palette", build(11, 6, depth, 3, il, plte(1))},
				in{"palette+trns", build(11, 6, depth, 3, il, append(plte(1<<depth), chunk("tRNS", []byte{0, 128, 255})...))},
				in{"palette+long trns", build(11, 6, depth, 3, il, append(plte(1), chunk("tRNS", []byte{7, 8, 9, 10})...))})
		}
	}
	trns := map[int][]byte{0: {0, 0}, 2: {0, 0, 0, 0, 0, 0}}
	for _, c := range [][2]int{{1, 0}, {2, 0}, {4, 0}, {8, 0}, {16, 0}, {8, 2}, {16, 2}} {
		t := trns[c[1]]
		if c[0] < 8 {
			t = []byte{0, 1}
		}
		inputs = append(inputs, in{"trns", build(9, 9, c[0], c[1], 0, chunk("tRNS", t))}, in{"trns interlaced", build(9, 9, c[0], c[1], 1, chunk("tRNS", t))})
	}
	// a 1-bit grey picture whose transparent value is set, so some pixels are transparent
	inputs = append(inputs, in{"trns g1 white", build(16, 4, 1, 0, 0, chunk("tRNS", []byte{0, 1}))})
	big := build(40, 40, 16, 6, 0, nil) // 12,800 bytes of noise: IDAT past bufio's 4,096
	inputs = append(inputs, in{"big", big}, in{"split", build(40, 40, 16, 6, 0, nil, 0, 0, 1, 2, 4095, 4096, 4097, 9000)},
		in{"split small", build(13, 7, 8, 2, 1, nil, 3, 3, 10, 11)})

	// broken ones
	good := build(13, 7, 8, 2, 0, nil)
	iend := chunk("IEND", nil)
	idat := func(b []byte) []byte { return chunk("IDAT", b) }
	stream := zl(scan(13, 7, 8, 2, 0))
	head := append(append([]byte(nil), sig...), ihdr(13, 7, 8, 2, 0)...)
	cat := func(parts ...[]byte) []byte {
		var o []byte
		for _, p := range parts {
			o = append(o, p...)
		}
		return o
	}
	badCRC := func(c []byte) []byte { c = append([]byte(nil), c...); c[len(c)-1] ^= 1; return c }
	rows := scan(13, 7, 8, 2, 0)
	rowsBadFilter := append([]byte(nil), rows...)
	rowsBadFilter[0] = 9
	bigStream := zl(scan(40, 40, 16, 6, 0))
	for _, junk := range []int{10, 100, 1000, 3000, 4000, 4020, 4040, 4060, 4070, 4080, 4085, 4090, 4093, 4096, 4097, 4500, 6000, 9000, 20000} {
		inputs = append(inputs, in{"big stream then junk", cat(sig, ihdr(40, 40, 16, 6, 0), idat(append(append([]byte(nil), bigStream...), make([]byte, junk)...)), iend)},
			in{"big stream split then junk", cat(sig, ihdr(40, 40, 16, 6, 0), idat(bigStream[:5000]), idat(append(append([]byte(nil), bigStream[5000:]...), make([]byte, junk)...)), iend)})
	}
	for _, x := range []in{
		{"empty", nil}, {"sig only", sig}, {"bad sig", cat([]byte("\x89PNG\r\n\x1a\x0b"), ihdr(1, 1, 8, 0, 0))}, {"no IEND", good[:len(good)-12]},
		{"IDAT crc", cat(head, badCRC(idat(stream)), iend)}, {"IHDR crc", cat(sig, badCRC(ihdr(13, 7, 8, 2, 0)), idat(stream), iend)},
		{"IEND crc", cat(head, idat(stream), badCRC(iend))}, {"unknown crc", cat(head, badCRC(chunk("zzZz", []byte("x"))), idat(stream), iend)},
		{"IDAT first", cat(sig, idat(stream), iend)}, {"two IHDR", cat(head, ihdr(1, 1, 8, 0, 0), idat(stream), iend)},
		{"IEND early", cat(head, iend)}, {"PLTE for gray", cat(sig, ihdr(2, 2, 8, 0, 0), plte(2), idat(zl([]byte{0, 1, 2, 0, 3, 4})), iend)},
		{"PLTE for rgb", cat(head, plte(3), idat(stream), iend)}, {"PLTE 257", cat(sig, ihdr(2, 2, 8, 3, 0), chunk("PLTE", make([]byte, 771)), idat(stream), iend)},
		{"PLTE 5 for 2-bit", cat(sig, ihdr(2, 2, 2, 3, 0), plte(5), idat(zl([]byte{0, 0, 0, 0})), iend)},
		{"no PLTE", cat(sig, ihdr(2, 2, 8, 3, 0), idat(zl([]byte{0, 1, 2, 0, 3, 4})), iend)},
		{"tRNS bad length", cat(head, chunk("tRNS", []byte{1, 2}), idat(stream), iend)}, {"tRNS 257", cat(sig, ihdr(2, 2, 8, 3, 0), plte(4), chunk("tRNS", make([]byte, 257)), idat(stream), iend)},
		{"tRNS gray alpha", cat(sig, ihdr(2, 2, 8, 4, 0), chunk("tRNS", []byte{0, 0}), idat(stream), iend)},
		{"tRNS after", cat(head, idat(stream), chunk("tRNS", trns[2]), iend)},
		{"bad depth", cat(sig, ihdr(2, 2, 3, 0, 0), idat(stream), iend)}, {"bad ct", cat(sig, ihdr(2, 2, 8, 5, 0), idat(stream), iend)},
		{"16-bit palette", cat(sig, ihdr(2, 2, 16, 3, 0), idat(stream), iend)}, {"interlace 2", cat(sig, ihdr(2, 2, 8, 0, 2), idat(stream), iend)},
		{"compression 1", cat(sig, chunk("IHDR", []byte{0, 0, 0, 2, 0, 0, 0, 2, 8, 0, 1, 0, 0}), idat(stream), iend)},
		{"filter method 1", cat(sig, chunk("IHDR", []byte{0, 0, 0, 2, 0, 0, 0, 2, 8, 0, 0, 1, 0}), idat(stream), iend)},
		{"zero width", cat(sig, ihdr(0, 2, 8, 0, 0), idat(stream), iend)}, {"negative height", cat(sig, ihdr(2, -1, 8, 0, 0), idat(stream), iend)},
		{"IHDR 12", cat(sig, chunk("IHDR", make([]byte, 12)), idat(stream), iend)}, {"IEND 1", cat(head, idat(stream), chunk("IEND", []byte{0}))},
		{"huge", cat(sig, ihdr(10000, 10000, 8, 0, 0), idat(stream), iend)}, {"chunk length", cat(head, []byte{0x80, 0, 0, 0}, []byte("zzZz"))},
		{"bad filter", cat(head, idat(zl(rowsBadFilter)), iend)}, {"short rows", cat(head, idat(zl(rows[:len(rows)-5])), iend)},
		{"long rows", cat(head, idat(zl(append(rows, 0, 0, 0))), iend)}, {"stream then junk", cat(head, idat(append(append([]byte(nil), stream...), 1, 2, 3)), iend)},
		{"big stream then junk", cat(sig, ihdr(40, 40, 16, 6, 0), idat(append(append([]byte(nil), bigStream...), make([]byte, 5000)...)), iend)},
		{"stream then IDAT", cat(head, idat(stream), idat([]byte{1, 2, 3}), iend)}, {"bad zlib header", cat(head, idat(append([]byte{0x78, 0x9d}, stream[2:]...)), iend)},
		{"zlib checksum", cat(head, idat(badCRC(stream)), iend)}, {"no checksum", cat(head, idat(stream[:len(stream)-4]), iend)},
		{"deflate damage", cat(head, idat(append(append(append([]byte(nil), stream[:9]...), 0xff), stream[10:]...)), iend)},
		{"empty IDAT then text", cat(head, idat(nil), chunk("tEXt", []byte("a\x00b")), iend)},
		{"IDAT then other", cat(head, idat(stream[:20]), chunk("tEXt", []byte("a\x00b")), idat(stream[20:]), iend)},
		{"PLTE+tRNS rgb", cat(head, plte(2), chunk("tRNS", []byte{0, 1, 0, 2, 0, 3}), idat(stream), iend)},
		{"cut in IDAT", cat(head, idat(stream))[:len(head)+30]}, {"cut in CRC", cat(head, idat(stream))[:len(head)+8+len(stream)+2]},
	} {
		inputs = append(inputs, x)
	}

	// GIFs
	gifOf := func(m *image.Paletted, o *gif.Options) []byte {
		var b bytes.Buffer
		if err := gif.Encode(&b, m, o); err != nil {
			t.Fatal(err)
		}
		return b.Bytes()
	}
	pal := color.Palette{color.Black, color.White, color.RGBA{255, 0, 0, 255}, color.RGBA{0, 0, 255, 255}}
	gm := image.NewPaletted(image.Rect(0, 0, 13, 7), pal)
	for i := range gm.Pix {
		gm.Pix[i] = uint8(rng.Intn(4))
	}
	g := gifOf(gm, nil)
	var multi bytes.Buffer
	_ = gif.EncodeAll(&multi, &gif.GIF{Image: []*image.Paletted{gm, gm}, Delay: []int{1, 2}})
	offset := image.NewPaletted(image.Rect(2, 1, 9, 5), pal)
	for i := range offset.Pix {
		offset.Pix[i] = uint8(rng.Intn(4))
	}
	var off bytes.Buffer
	_ = gif.EncodeAll(&off, &gif.GIF{Image: []*image.Paletted{offset}, Config: image.Config{ColorModel: pal, Width: 13, Height: 7}})
	// hand-made GIF pieces: header, screen, global table, extensions, image descriptor, LZW data in sub-blocks
	lzwData := func(lit int, codes []int) []byte { // LSB-first codes of growing width
		var out []byte
		var acc uint32
		var n uint
		width := uint(lit + 1)
		hi := (1 << lit) + 1
		for _, c := range codes {
			acc |= uint32(c) << n
			n += width
			for n >= 8 {
				out = append(out, byte(acc))
				acc >>= 8
				n -= 8
			}
			if c == 1<<lit {
				width, hi = uint(lit+1), (1<<lit)+1
				continue
			}
			hi++
			if hi >= 1<<width && width < 12 {
				width++
			}
		}
		if n > 0 {
			out = append(out, byte(acc))
		}
		return out
	}
	blocks := func(b []byte) []byte {
		var out []byte
		for len(b) > 0 {
			k := min(len(b), 255)
			out = append(append(out, byte(k)), b[:k]...)
			b = b[k:]
		}
		return append(out, 0)
	}
	screen := func(w, h int, table bool) []byte {
		b := []byte("GIF89a")
		b = binary.LittleEndian.AppendUint16(b, uint16(w))
		b = binary.LittleEndian.AppendUint16(b, uint16(h))
		if table {
			return append(b, 0x81, 0, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255)
		}
		return append(b, 0, 0, 0)
	}
	desc := func(l, t, w, h int, flags byte) []byte {
		b := []byte{0x2c}
		for _, v := range []int{l, t, w, h} {
			b = binary.LittleEndian.AppendUint16(b, uint16(v))
		}
		return append(b, flags)
	}
	gce := func(flags, ti byte) []byte { return []byte{0x21, 0xf9, 4, flags, 0, 0, ti, 0} }
	px := []int{4, 0, 1, 2, 3, 0, 1, 2, 3, 3, 2, 1, 0, 5}
	gifs := []in{{"gif", g}, {"gif multi", multi.Bytes()}, {"gif offset", off.Bytes()},
		{"gif hand", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif interlaced", cat(screen(2, 6, true), desc(0, 0, 2, 6, 0x40), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif transparent", cat(screen(4, 3, true), gce(1, 2), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif transparent beyond", cat(screen(4, 3, true), gce(1, 9), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif local table", cat(screen(4, 3, false), desc(0, 0, 4, 3, 0x80), []byte{1, 2, 3, 4, 5, 6}, []byte{2}, blocks(lzwData(2, []int{4, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 5})), []byte{0x3b})},
		{"gif bad pixel", cat(screen(4, 3, false), desc(0, 0, 4, 3, 0x80), []byte{1, 2, 3, 4, 5, 6}, []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif no table", cat(screen(4, 3, false), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif too few", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px[:6])), []byte{0x3b})},
		{"gif no eof code", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px[:13])), []byte{0x3b})},
		{"gif no eof code lit 8", cat(screen(2, 2, true), desc(0, 0, 2, 2, 0), []byte{8}, blocks(lzwData(8, []int{256, 0, 1, 1, 0})), []byte{0x3b})},
		{"gif transparent 254", cat(screen(2, 1, true), gce(1, 254), desc(0, 0, 2, 1, 0), []byte{8}, blocks(lzwData(8, []int{256, 255, 0, 257})), []byte{0x3b})},
		{"gif transparent 254 in range", cat(screen(2, 1, true), gce(1, 254), desc(0, 0, 2, 1, 0), []byte{8}, blocks(lzwData(8, []int{256, 254, 0, 257})), []byte{0x3b})},
		{"gif wide", cat(screen(100, 50, true), desc(0, 0, 100, 50, 0), []byte{2}, blocks(lzwData(2, append(append([]int{4}, make([]int, 5000)...), 5))), []byte{0x3b})},
		{"gif too many", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, append(px[:13:13], 1, 1, 5))), []byte{0x3b})},
		{"gif invalid code", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, []int{4, 0, 7, 5})), []byte{0x3b})},
		{"gif kwkwk", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, []int{4, 1, 6, 6, 3, 5})), []byte{0x3b})},
		{"gif trailing blocks", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px))[:len(blocks(lzwData(2, px)))-1], []byte{2, 9, 9, 0}, []byte{0x3b})},
		{"gif lit 1", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{1}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif lit 9", cat(screen(4, 3, true), desc(0, 0, 4, 3, 0), []byte{9}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif frame too big", cat(screen(4, 3, true), desc(1, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif trailer only", cat(screen(4, 3, true), []byte{0x3b})}, {"gif unknown block", cat(screen(4, 3, true), []byte{0x99})},
		{"gif extensions", cat(screen(4, 3, true), []byte{0x21, 0xfe, 3, 'h', 'i', '!', 0}, []byte{0x21, 0xff, 11}, []byte("NETSCAPE2.0"), []byte{3, 1, 5, 0, 0},
			[]byte{0x21, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 'a', 'b', 0}, desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif unknown extension", cat(screen(4, 3, true), []byte{0x21, 0x02, 0}, []byte{0x3b})},
		{"gif bad gce", cat(screen(4, 3, true), []byte{0x21, 0xf9, 5, 0, 0, 0, 0, 0}, []byte{0x3b})},
		{"gif gce terminator", cat(screen(4, 3, true), []byte{0x21, 0xf9, 4, 0, 0, 0, 0, 7}, []byte{0x3b})},
		{"gif huge", cat(screen(65535, 65535, true), []byte{0x3b})}, {"gif87", cat([]byte("GIF87a"), screen(4, 3, true)[6:], desc(0, 0, 4, 3, 0), []byte{2}, blocks(lzwData(2, px)), []byte{0x3b})},
		{"gif88", cat([]byte("GIF88a"), screen(4, 3, true)[6:])}, {"gif short", []byte("GIF89a\x04\x00")},
	}
	inputs = append(inputs, gifs...)
	// every truncation and a byte change at every position of a small PNG and the hand-made GIF
	smallPNG := build(5, 3, 8, 6, 0, nil)
	for _, b := range [][]byte{smallPNG, gifs[3].b} {
		for i := range b {
			c := append([]byte(nil), b...)
			c[i] ^= 0x21
			inputs = append(inputs, in{"changed", c}, in{"cut", b[:i]})
		}
	}
	var decs []any
	for _, x := range inputs {
		decs = append(decs, []any{x.name, hex.EncodeToString(x.b), describe(x.b)})
	}
	cases := []parity.Case{{Fn: "decode", Out: decs}}

	// Diff
	base := enc(noisy)
	lsb := image.NewNRGBA(noisy.Rect)
	copy(lsb.Pix, noisy.Pix)
	lsb.Pix[0] ^= 1
	lsb.Pix[4*70+2] ^= 1
	lsb.Pix[4*3000+3] ^= 1
	most := image.NewNRGBA(noisy.Rect)
	copy(most.Pix, lsb.Pix)
	most.Pix[100] ^= 0x80
	larger := image.NewNRGBA(noisy.Rect)
	copy(larger.Pix, noisy.Pix)
	for i := 0; i < 40; i++ {
		larger.Pix[i*13] ^= 0x10
	}
	var diffs []any
	half := image.NewNRGBA(noisy.Rect)
	copy(half.Pix, noisy.Pix)
	half.Pix[8] ^= 1
	half.Pix[400] ^= 0x40
	for _, pair := range [][2][]byte{{base, base}, {base, enc(lsb)}, {base, enc(most)}, {base, enc(larger)}, {base, enc(flat)}, {enc(gray), enc(rgba)}, {base, enc(gray)}, {base, enc(half)}} {
		pa, _ := DecodeImage(pair[0])
		pb, _ := DecodeImage(pair[1])
		out, err := Diff(pa, pb)
		diffs = append(diffs, []any{sum(pair[0]), sum(pair[1]), out, parity.Err(err)})
	}
	cases = append(cases, parity.Case{Fn: "diff", In: []any{hex.EncodeToString(base), hex.EncodeToString(enc(lsb)), hex.EncodeToString(enc(most)),
		hex.EncodeToString(enc(larger)), hex.EncodeToString(enc(flat)), hex.EncodeToString(enc(gray)), hex.EncodeToString(enc(rgba)), hex.EncodeToString(enc(half))}, Out: diffs})

	// math.Lgamma at the half-integers the statistic uses and elsewhere
	var lg []any
	for _, x := range []float64{0.5, 1, 1.5, 2, 2.5, 3, 7.5, 8, 8.5, 63.5, 64, 1e-25, 0.1, 0.25, 0.7, 0.8, 0.95, 1.2, 1.3, 1.6, 1.8, 1.99, 2.01, 5.3, 1e10, 1e18, 3e17} {
		v, _ := math.Lgamma(x)
		lg = append(lg, []any{math.Float64bits(x), math.Float64bits(v)})
	}
	for i := 0; i < 200; i++ {
		x := rng.ExpFloat64() * 10
		v, _ := math.Lgamma(x)
		lg = append(lg, []any{math.Float64bits(x), math.Float64bits(v)})
	}
	cases = append(cases, parity.Case{Fn: "lgamma", Out: lg})
	parity.Fixture(t, "stegolsb", cases)
}

func absInt(x int) int {
	if x < 0 {
		return -x
	}
	return x
}
