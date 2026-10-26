package stego

import (
	"bytes"
	"fmt"
	"image"
	"image/color"
	"image/gif" // registered for image.Decode
	"image/jpeg"
	"image/png"
	"math"
	"strings"
)

var _ = gif.Decode
var _ = jpeg.Decode

// MaxPixels caps the images the LSB code looks at (a 40-megapixel image is 160 MB decoded).
const MaxPixels = 40_000_000

// Pixels is an image as RGBA samples plus, for an indexed image, its palette indices.
type Pixels struct {
	W, H    int
	RGBA    []uint8 // 4 bytes per pixel, row-major
	Indices []uint8 // palette indices, when the image is paletted
	Format  string
	Alpha   bool // the alpha channel varies (so it may carry data)
}

// DecodeImage decodes a PNG, JPEG, GIF or BMP-less image into samples.
func DecodeImage(b []byte) (*Pixels, error) {
	cfg, format, err := image.DecodeConfig(bytes.NewReader(b))
	if err != nil {
		return nil, fmt.Errorf("not an image I can decode: %v", err)
	}
	if cfg.Width*cfg.Height > MaxPixels {
		return nil, fmt.Errorf("the image is %dx%d, more than %d pixels: too big for me to look at bit by bit", cfg.Width, cfg.Height, MaxPixels)
	}
	img, _, err := image.Decode(bytes.NewReader(b))
	if err != nil {
		return nil, fmt.Errorf("cannot decode the image: %v", err)
	}
	r := img.Bounds()
	p := &Pixels{W: r.Dx(), H: r.Dy(), Format: format, RGBA: make([]uint8, 0, 4*r.Dx()*r.Dy())}
	if pal, ok := img.(*image.Paletted); ok {
		p.Indices = make([]uint8, 0, p.W*p.H)
		for y := 0; y < p.H; y++ {
			p.Indices = append(p.Indices, pal.Pix[y*pal.Stride:y*pal.Stride+p.W]...)
		}
	}
	first, varies := uint8(255), false
	for y := r.Min.Y; y < r.Max.Y; y++ {
		for x := r.Min.X; x < r.Max.X; x++ {
			var cr, cg, cb, ca uint8
			switch im := img.(type) {
			case *image.NRGBA:
				i := im.PixOffset(x, y)
				cr, cg, cb, ca = im.Pix[i], im.Pix[i+1], im.Pix[i+2], im.Pix[i+3]
			case *image.RGBA:
				i := im.PixOffset(x, y)
				cr, cg, cb, ca = im.Pix[i], im.Pix[i+1], im.Pix[i+2], im.Pix[i+3]
			default:
				c := color.NRGBAModel.Convert(img.At(x, y)).(color.NRGBA)
				cr, cg, cb, ca = c.R, c.G, c.B, c.A
			}
			if y == r.Min.Y && x == r.Min.X {
				first = ca
			} else if ca != first {
				varies = true
			}
			p.RGBA = append(p.RGBA, cr, cg, cb, ca)
		}
	}
	p.Alpha = varies
	return p, nil
}

// ChannelStats is the least-significant-bit picture of one channel.
type ChannelStats struct {
	Name     string
	OnesRate float64 // share of pixels whose LSB is 1 (about 0.5 for noise and for a payload; far from it for flat art)
	PoVp     float64 // pairs-of-values chi-square p-value: near 1 means the value pairs (2k, 2k+1) are equalised, as sequential LSB embedding does
	Flat     bool    // the channel is a single value: nothing to hide in
}

// LSBAnalysis measures the LSB planes of an image: per channel the share of ones and the pairs-of-values test (Westfeld & Pfitzmann),
// on the whole image and on its first tenth (a short payload sits at the start).
type LSBAnalysis struct {
	Channels    []ChannelStats
	FirstTenth  []ChannelStats
	Verdict     string
	Explanation string
}

// Analyse runs the LSB measurements.
func Analyse(p *Pixels) LSBAnalysis {
	var a LSBAnalysis
	channels := []int{0, 1, 2}
	names := []string{"red", "green", "blue"}
	if p.Alpha {
		channels, names = append(channels, 3), append(names, "alpha")
	}
	stats := func(from, to int) []ChannelStats {
		var out []ChannelStats
		if p.Indices != nil {
			out = append(out, channelStats("palette index", p.Indices[from:to], 1, 0))
		}
		for i, ch := range channels {
			out = append(out, channelStats(names[i], p.RGBA[from*4:to*4], 4, ch))
		}
		return out
	}
	n := p.W * p.H
	a.Channels = stats(0, n)
	tenth := n / 10
	if tenth >= 256 {
		a.FirstTenth = stats(0, tenth)
	}
	// what the statistic says (the verdict that counts is written by Inspect, after the extraction: a statistic alone is a weak sign)
	// the channels are named: "in at least one channel" was read as the channels with p=0.00 (eval stego-clean-honest, 2026-10-26)
	var high, mid []string
	measured := 0
	where := func(part string, cs []ChannelStats) {
		for _, s := range cs {
			if s.Flat {
				continue
			}
			measured++
			if s.PoVp > 0.95 {
				high = append(high, fmt.Sprintf("%s%s (p=%.2f)", s.Name, part, s.PoVp))
			} else if s.PoVp > 0.5 {
				mid = append(mid, fmt.Sprintf("%s%s (p=%.2f)", s.Name, part, s.PoVp))
			}
		}
	}
	where("", a.Channels)
	where(" in the first tenth", a.FirstTenth)
	switch {
	case len(high) > 0:
		a.Verdict = "LSB values are equalised in " + strings.Join(high, ", ")
		if len(high) < measured {
			a.Verdict += "; not in the other channels"
		}
		a.Explanation = "The pairs-of-values test (p near 1: values 2k and 2k+1 occur equally often) is what sequential LSB replacement leaves, but sensor noise, dithering and synthetic noise leave the same; it is a reason to extract, not a finding."
	case len(mid) > 0:
		a.Verdict = "LSB values are partly equalised in " + strings.Join(mid, ", ")
		a.Explanation = "A short or partial payload could do this; so can ordinary noise."
	default:
		a.Verdict = "LSB values are not equalised: ordinary image structure or flat colour"
		a.Explanation = "No statistical sign of sequential LSB replacement. A payload spread by a key (steghide, randomised LSB) or in JPEG coefficients does not show here."
	}
	return a
}

func channelStats(name string, data []uint8, stride, offset int) ChannelStats {
	var hist [256]float64
	ones, n := 0.0, 0.0
	for i := offset; i < len(data); i += stride {
		v := data[i]
		hist[v]++
		n++
		if v&1 == 1 {
			ones++
		}
	}
	st := ChannelStats{Name: name}
	if n == 0 {
		return st
	}
	st.OnesRate = ones / n
	distinct := 0
	for _, h := range hist {
		if h > 0 {
			distinct++
		}
	}
	if distinct <= 1 {
		st.Flat = true
		return st
	}
	chi, df := 0.0, 0
	for k := 0; k < 128; k++ {
		e := (hist[2*k] + hist[2*k+1]) / 2
		if e < 5 {
			continue
		}
		d := hist[2*k] - e
		chi += d * d / e
		df++
	}
	if df == 0 {
		st.Flat = true
		return st
	}
	st.PoVp = 1 - chiSquareCDF(chi, float64(df))
	return st
}

// chiSquareCDF is P(X <= x) for a chi-square variable with k degrees of freedom (the regularised lower incomplete gamma).
func chiSquareCDF(x, k float64) float64 {
	if x <= 0 {
		return 0
	}
	return lowerGamma(k/2, x/2)
}

func lowerGamma(a, x float64) float64 {
	if x < a+1 {
		// series
		sum, term := 1/a, 1/a
		for n := 1; n < 500; n++ {
			term *= x / (a + float64(n))
			sum += term
			if math.Abs(term) < math.Abs(sum)*1e-14 {
				break
			}
		}
		return sum * math.Exp(-x+a*math.Log(x)-lgamma(a))
	}
	// continued fraction for the upper part
	b := x + 1 - a
	c := 1 / 1e-300
	d := 1 / b
	h := d
	for i := 1; i < 500; i++ {
		an := -float64(i) * (float64(i) - a)
		b += 2
		d = an*d + b
		if math.Abs(d) < 1e-300 {
			d = 1e-300
		}
		c = b + an/c
		if math.Abs(c) < 1e-300 {
			c = 1e-300
		}
		d = 1 / d
		del := d * c
		h *= del
		if math.Abs(del-1) < 1e-14 {
			break
		}
	}
	return 1 - math.Exp(-x+a*math.Log(x)-lgamma(a))*h
}

func lgamma(a float64) float64 {
	v, _ := math.Lgamma(a)
	return v
}

// Describe renders the analysis.
func (a LSBAnalysis) Describe() string {
	var b strings.Builder
	line := func(s ChannelStats) string {
		if s.Flat {
			return fmt.Sprintf("%s: flat (one value)", s.Name)
		}
		return fmt.Sprintf("%s: LSB ones %.1f%%, pairs-of-values p=%.2f", s.Name, 100*s.OnesRate, s.PoVp)
	}
	b.WriteString("Whole image: ")
	for i, s := range a.Channels {
		if i > 0 {
			b.WriteString("; ")
		}
		b.WriteString(line(s))
	}
	if len(a.FirstTenth) > 0 {
		b.WriteString("\nFirst tenth: ")
		for i, s := range a.FirstTenth {
			if i > 0 {
				b.WriteString("; ")
			}
			b.WriteString(line(s))
		}
	}
	fmt.Fprintf(&b, "\nStatistic: %s. %s", a.Verdict, a.Explanation)
	return b.String()
}

// Order says which samples carry the bits, in what sequence.
type Order struct {
	Name     string
	Channels []int // 0 r, 1 g, 2 b, 3 a, -1 palette index
}

// Orders the extractor tries, the common ones first.
var Orders = []Order{{"rgb", []int{0, 1, 2}}, {"r", []int{0}}, {"g", []int{1}}, {"b", []int{2}}, {"rgba", []int{0, 1, 2, 3}}, {"bgr", []int{2, 1, 0}}, {"a", []int{3}}}

// ExtractBits reads `bits` low bits of each chosen sample (1-4), in order, and packs them into bytes, most significant bit first
// (msbFirst) or least first. At most max bytes.
func ExtractBits(p *Pixels, o Order, bits int, msbFirst bool, max int) []byte {
	if bits < 1 || bits > 4 {
		bits = 1
	}
	out := make([]byte, 0, max)
	var cur, have uint
	push := func(bit uint) bool {
		if msbFirst {
			cur = cur<<1 | bit
		} else {
			cur |= bit << have
		}
		have++
		if have == 8 {
			out = append(out, byte(cur))
			cur, have = 0, 0
			return len(out) >= max
		}
		return false
	}
	n := p.W * p.H
	for i := 0; i < n; i++ {
		for _, ch := range o.Channels {
			var v uint8
			if ch < 0 {
				if p.Indices == nil {
					return out
				}
				v = p.Indices[i]
			} else {
				v = p.RGBA[i*4+ch]
			}
			for k := bits - 1; k >= 0; k-- {
				if push(uint(v>>uint(k)) & 1) {
					return out
				}
			}
		}
	}
	return out
}

// Found is a stream that looks like a payload.
type Found struct {
	Order    string
	Bits     int
	MSBFirst bool
	Length   int    // from a 32-bit length header, if one fits
	Why      string // printable text, a file signature
	Data     []byte
}

// SearchLSB tries the usual orders and bit directions and reports the streams that start like text or a known file.
func SearchLSB(p *Pixels, maxBytes int) []Found {
	var found []Found
	orders := Orders
	if p.Indices != nil {
		orders = append([]Order{{"palette index", []int{-1}}}, orders...)
	}
	for _, o := range orders {
		if contains(o.Channels, 3) && !p.Alpha {
			continue
		}
		for _, msb := range []bool{true, false} {
			data := ExtractBits(p, o, 1, msb, maxBytes)
			if f, ok := judge(data); ok {
				f.Order, f.MSBFirst, f.Bits = o.Name, msb, 1
				found = append(found, f)
			}
		}
	}
	return found
}

func contains(xs []int, x int) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}

// judge says whether a bit stream looks like a payload: a file signature, or text (with or without a 32-bit length in front).
func judge(data []byte) (Found, bool) {
	if len(data) < 8 {
		return Found{}, false
	}
	if k := Detect(data); k.Name != "unknown" && k.Name != "text" && k.Name != "script" {
		return Found{Why: "starts like a " + k.Name + " file", Data: data}, true
	}
	body := data
	length := 0
	for _, le := range []bool{false, true} {
		var n uint32
		if le {
			n = uint32(data[0]) | uint32(data[1])<<8 | uint32(data[2])<<16 | uint32(data[3])<<24
		} else {
			n = uint32(data[0])<<24 | uint32(data[1])<<16 | uint32(data[2])<<8 | uint32(data[3])
		}
		if n >= 4 && int(n) <= len(data)-4 && textLike(data[4:4+n]) {
			return Found{Length: int(n), Why: fmt.Sprintf("a 32-bit length (%d) followed by that much text", n), Data: data[4 : 4+n]}, true
		}
	}
	// text until the first run of zero bytes
	end := len(body)
	if i := bytes.Index(body, []byte{0, 0, 0, 0}); i >= 0 {
		end = i
	}
	if end >= 8 && textLike(body[:end]) {
		return Found{Length: length, Why: "reads as text", Data: body[:end]}, true
	}
	return Found{}, false
}

func textLike(b []byte) bool {
	if len(b) < 4 {
		return false
	}
	ok := 0
	for _, c := range b {
		if c >= 0x20 && c < 0x7f || c == '\n' || c == '\r' || c == '\t' {
			ok++
		}
	}
	return float64(ok) >= 0.95*float64(len(b))
}

// Embed writes payload into the LSBs of the image's RGB samples (order rgb, msb first, a 32-bit length first), the plain scheme many
// tools and tutorials use. It is here so tests and fixtures can make a known-positive image; it returns a PNG.
func Embed(img image.Image, payload []byte) ([]byte, error) {
	r := img.Bounds()
	n := r.Dx() * r.Dy()
	need := (4 + len(payload)) * 8
	if need > n*3 {
		return nil, fmt.Errorf("the image has room for %d bytes, the payload is %d", n*3/8-4, len(payload))
	}
	out := image.NewNRGBA(r)
	for y := r.Min.Y; y < r.Max.Y; y++ {
		for x := r.Min.X; x < r.Max.X; x++ {
			c := color.NRGBAModel.Convert(img.At(x, y)).(color.NRGBA)
			i := out.PixOffset(x, y)
			out.Pix[i], out.Pix[i+1], out.Pix[i+2], out.Pix[i+3] = c.R, c.G, c.B, c.A
		}
	}
	data := append([]byte{byte(len(payload) >> 24), byte(len(payload) >> 16), byte(len(payload) >> 8), byte(len(payload))}, payload...)
	bit := 0
	for _, c := range data {
		for k := 7; k >= 0; k-- {
			pix, ch := bit/3, bit%3
			i := pix*4 + ch
			out.Pix[i] = out.Pix[i]&0xfe | (c>>uint(k))&1
			bit++
		}
	}
	var buf bytes.Buffer
	if err := png.Encode(&buf, out); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// toImage turns samples back into an image (for re-encoding, embedding).
func toImage(p *Pixels) image.Image {
	out := image.NewNRGBA(image.Rect(0, 0, p.W, p.H))
	copy(out.Pix, p.RGBA)
	return out
}
