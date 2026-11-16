package stego

import (
	"bytes"
	"compress/zlib"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"image"
	"image/color"
	"image/gif"
	"image/jpeg"
	"image/png"
	mrand "math/rand"
	"os"
	"path/filepath"
	"testing"
	"testing/cryptotest"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityStegoEncode records (testdata/parity/stegoencode.json) what Sarina writes as pictures: Embed's PNG (Go's
// image/png encoder over compress/flate's default level) for pictures that compress well, badly and not at all, small
// and large, opaque and not, with payloads that fit and one that does not; and CleanImage on PNG, GIF and JPEG files
// (PNG or JPEG out), with crypto/rand fixed by testing/cryptotest so the noise it adds is the same on every run. The
// noise is recorded beside each case, so the port can be given the same bytes.
// pieces records each Write a writer is given.
type pieces struct {
	sizes []int
	all   []byte
}

func (p *pieces) Write(b []byte) (int, error) {
	p.sizes = append(p.sizes, len(b))
	p.all = append(p.all, b...)
	return len(b), nil
}

func TestParityStegoEncode(t *testing.T) {
	rng := mrand.New(mrand.NewSource(23))
	out := func(b []byte) any { // small outputs whole, large ones by hash
		if len(b) <= 4096 {
			return hex.EncodeToString(b)
		}
		return fmt.Sprintf("%d:%s", len(b), shortSum(b))
	}
	encPNG := func(m image.Image) []byte {
		var b bytes.Buffer
		if err := png.Encode(&b, m); err != nil {
			t.Fatal(err)
		}
		return b.Bytes()
	}
	nrgba := func(w, h int, f func(x, y int) color.NRGBA) *image.NRGBA {
		m := image.NewNRGBA(image.Rect(0, 0, w, h))
		for y := 0; y < h; y++ {
			for x := 0; x < w; x++ {
				m.SetNRGBA(x, y, f(x, y))
			}
		}
		return m
	}
	noise := func(alpha bool) func(x, y int) color.NRGBA {
		return func(x, y int) color.NRGBA {
			a := uint8(255)
			if alpha {
				a = uint8(rng.Intn(256))
			}
			return color.NRGBA{uint8(rng.Intn(256)), uint8(rng.Intn(256)), uint8(rng.Intn(256)), a}
		}
	}
	pictures := map[string]image.Image{
		"1x1":        nrgba(1, 1, func(x, y int) color.NRGBA { return color.NRGBA{1, 2, 3, 255} }),
		"noise 13x7": nrgba(13, 7, noise(false)),
		"alpha 13x7": nrgba(13, 7, noise(true)),
		"flat 512":   nrgba(512, 512, func(x, y int) color.NRGBA { return color.NRGBA{200, 100, 50, 255} }),
		"stripes 300": nrgba(300, 211, func(x, y int) color.NRGBA {
			return color.NRGBA{uint8(x % 7 * 30), uint8(y % 5 * 40), uint8((x + y) % 3 * 80), 255}
		}),
		"gradient 257": nrgba(257, 129, func(x, y int) color.NRGBA { return color.NRGBA{uint8(x), uint8(y), uint8(x ^ y), uint8(255 - x%2)} }),
		"noise 300":    nrgba(300, 200, noise(false)),
		"text-like":    nrgba(97, 61, func(x, y int) color.NRGBA { v := uint8(" .:-=+*#%@"[(x*y+x/3)%10]); return color.NRGBA{v, v, v, 255} }),
		"repeats":      nrgba(400, 100, func(x, y int) color.NRGBA { v := uint8(x % 260); return color.NRGBA{v, v, uint8(y % 3), 255} }),
	}
	names := []string{"1x1", "noise 13x7", "alpha 13x7", "flat 512", "stripes 300", "gradient 257", "noise 300", "text-like", "repeats"}
	payloads := [][]byte{[]byte("hi"), []byte("a secret meeting at the old mill, bring the key"), bytes.Repeat([]byte("Z"), 3000), make([]byte, 100000)}
	var embeds []any
	sources := map[string]any{}
	for _, name := range names {
		src := encPNG(pictures[name])
		sources[name] = hex.EncodeToString(src)
		p, err := DecodeImage(src)
		if err != nil {
			t.Fatal(err)
		}
		for _, pl := range payloads {
			b, err := Embed(toImage(p), pl)
			embeds = append(embeds, []any{name, len(pl), out(b), parity.Err(err)})
		}
	}
	cases := []parity.Case{{Fn: "embed", In: sources, Out: embeds}}

	// CleanImage: the inputs are files; crypto/rand fixed per case
	inputs := []struct {
		name string
		b    []byte
	}{{"png opaque", encPNG(pictures["noise 13x7"])}, {"png alpha", encPNG(pictures["alpha 13x7"])}, {"png big", encPNG(pictures["stripes 300"])},
		{"garbage", []byte("not a picture")}}
	pal := image.NewPaletted(image.Rect(0, 0, 9, 5), color.Palette{color.Black, color.NRGBA{255, 0, 0, 128}, color.RGBA{0, 0, 255, 255}})
	for i := range pal.Pix {
		pal.Pix[i] = uint8(rng.Intn(3))
	}
	inputs = append(inputs, struct {
		name string
		b    []byte
	}{"png paletted with alpha", encPNG(pal)})
	var gb bytes.Buffer
	gpal := image.NewPaletted(image.Rect(0, 0, 21, 11), color.Palette{color.Black, color.White, color.RGBA{9, 99, 199, 255}})
	for i := range gpal.Pix {
		gpal.Pix[i] = uint8(rng.Intn(3))
	}
	_ = gif.Encode(&gb, gpal, nil)
	var jb bytes.Buffer
	_ = jpeg.Encode(&jb, pictures["stripes 300"], &jpeg.Options{Quality: 60})
	inputs = append(inputs, struct {
		name string
		b    []byte
	}{"gif", gb.Bytes()}, struct {
		name string
		b    []byte
	}{"jpeg", jb.Bytes()})
	for _, f := range []string{"base.jpg", "cmyk.jpg", "gray.jpg", "rgb.jpg"} {
		b, err := os.ReadFile(filepath.Join(parity.Root(), "testdata", "parity", "jpeg", f))
		if err != nil {
			t.Fatal(err)
		}
		inputs = append(inputs, struct {
			name string
			b    []byte
		}{f, b})
	}
	var cleans []any
	for i, in := range inputs {
		var nz []byte
		t.Run(fmt.Sprintf("noise-%d", i), func(t *testing.T) {
			cryptotest.SetGlobalRandom(t, uint64(100+i))
			if p, err := DecodeImage(in.b); err == nil {
				nz = make([]byte, 4*p.W*p.H)
				_, _ = rand.Read(nz)
			}
		})
		var b []byte
		var ext string
		var err error
		t.Run(fmt.Sprintf("clean-%d", i), func(t *testing.T) {
			cryptotest.SetGlobalRandom(t, uint64(100+i))
			b, ext, err = CleanImage(in.b)
		})
		cleans = append(cleans, []any{in.name, hex.EncodeToString(in.b), hex.EncodeToString(nz), out(b), ext, parity.Err(err)})
	}
	cases = append(cases, parity.Case{Fn: "clean", Out: cleans})

	// compress/zlib at the default level on its own: every block kind the bit writer picks, each Write it makes
	text := bytes.Repeat([]byte("the quick brown fox jumps over the lazy dog, and the dog jumps back over the fox; "), 2000)
	x := uint64(88172645463325252) // xorshift64, so the Rust test makes the same bytes
	next := func() uint64 { x ^= x << 13; x ^= x >> 7; x ^= x << 17; return x }
	random := make([]byte, 200000)
	for i := range random {
		random[i] = byte(next())
	}
	skewed := make([]byte, 70000)
	for i := range skewed {
		v := next()
		skewed[i] = byte(v%16) * byte(v>>8%3)
	}
	streams := [][]byte{nil, []byte("a"), []byte("hello world, hello world"), bytes.Repeat([]byte("ab"), 16), bytes.Repeat([]byte("xyz"), 11),
		random[:40], random[:100], random[:127], random[:128], random[:1000], random[:1025], random[:5000], random[:65535], random[:65536], random,
		text[:200], text[:3000], text[:65535], text[:65536], text, skewed, skewed[:1500], append(append([]byte(nil), text[:40000]...), random[:40000]...),
		append(append([]byte(nil), random[:30000]...), text[:90000]...), bytes.Repeat([]byte{0}, 300000), append(bytes.Repeat([]byte{7}, 70000), text[:500]...)}
	var zs []any
	for _, data := range streams {
		for _, size := range []int{0, 1, 1000, 70000} {
			var w pieces
			z := zlib.NewWriter(&w)
			if size == 0 {
				_, _ = z.Write(data)
			} else {
				for b := data; len(b) > 0; {
					k := min(size, len(b))
					_, _ = z.Write(b[:k])
					b = b[k:]
				}
			}
			_ = z.Close()
			if size == 1 && len(data) > 5000 {
				continue
			}
			zs = append(zs, []any{len(data), shortSum(data), size, w.sizes, out(w.all)})
		}
	}
	cases = append(cases, parity.Case{Fn: "zlib", In: "the inputs are made again by the Rust test (xorshift64 and the text)", Out: zs})
	parity.Fixture(t, "stegoencode", cases)
}
