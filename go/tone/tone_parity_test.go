package tone

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// sig is a stand-in for speech made with integer arithmetic only, so the Rust test makes the same samples
// (rust/sarina/tests/parity_tone.rs): a sawtooth `per` samples long (gliding by `glide` samples each quarter second),
// shaped into syllables of `syl` samples with `gap` between them and a pause of `pause` after every `every`, at `amp`
// (1000 is full scale), over noise of up to `noise`, `lead` samples of quiet at each end; channel c holds sample/(c+1).
type sig struct {
	Seed                                              uint64
	Rate, Chans, N, Lead, Per, Glide, Syl, Gap, Every int
	Pause, Amp, Noise                                 int
}

func (s sig) wav() []byte {
	st := s.Seed*2654435761 + 1
	rnd := func() uint64 {
		st ^= st << 13
		st ^= st >> 7
		st ^= st << 17
		return st
	}
	env := make([]int, s.N)
	pos, count := s.Lead, 0
	for s.Syl > 0 && pos+s.Syl <= s.N-s.Lead {
		for p := 0; p < s.Syl; p++ {
			env[pos+p] = min(p, s.Syl-p) * 2048 / s.Syl
		}
		pos += s.Syl + s.Gap
		count++
		if s.Every > 0 && count%s.Every == 0 {
			pos += s.Pause
		}
	}
	data := make([]byte, 0, s.N*s.Chans*2)
	ph := 0
	for j := 0; j < s.N; j++ {
		per := s.Per + s.Glide*((j/(s.Rate/4))%5-2)
		ph++
		if ph >= per {
			ph = 0
		}
		v := s.Amp * (ph*2000/per - 1000) * env[j] / (1000 * 1024)
		if s.Noise > 0 {
			v += int(rnd()%uint64(2*s.Noise+1)) - s.Noise
		}
		v = max(-32768, min(32767, v))
		for c := 0; c < s.Chans; c++ {
			data = binary.LittleEndian.AppendUint16(data, uint16(int16(v/(c+1))))
		}
	}
	return wavFile(1, uint16(s.Chans), uint32(s.Rate), 16, data)
}

func wavFile(format, chans uint16, rate uint32, bits uint16, data []byte, extra ...[]byte) []byte {
	b := []byte("RIFF\x00\x00\x00\x00WAVE")
	for _, e := range extra {
		b = append(b, e...)
	}
	b = append(b, "fmt "...)
	b = binary.LittleEndian.AppendUint32(b, 16)
	b = binary.LittleEndian.AppendUint16(b, format)
	b = binary.LittleEndian.AppendUint16(b, chans)
	b = binary.LittleEndian.AppendUint32(b, rate)
	b = binary.LittleEndian.AppendUint32(b, rate*uint32(chans)*uint32(bits)/8)
	b = binary.LittleEndian.AppendUint16(b, chans*bits/8)
	b = binary.LittleEndian.AppendUint16(b, bits)
	b = append(b, "data"...)
	b = binary.LittleEndian.AppendUint32(b, uint32(len(data)))
	return append(b, data...)
}

func bits(f float64) string { return fmt.Sprintf("%016x", math.Float64bits(f)) }

func featureBits(f Features) []string {
	return []string{bits(f.LoudDB), bits(f.F0Median), bits(f.F0RangeSt), bits(f.Rate), bits(f.PauseRatio), bits(f.VoicedSec), bits(f.Bright)}
}

// zone is a TZif file (version 2) with no transitions and one rule, the POSIX TZ string `rule`: every time is read by
// the rule, as Go reads it past a file's last transition. Made up here so the fixture does not depend on this
// computer's tzdata.
func zone(off int32, rule string) []byte {
	var b bytes.Buffer
	header := func(n [6]int) {
		b.WriteString("TZif2")
		b.Write(make([]byte, 15))
		for _, x := range n {
			_ = binary.Write(&b, binary.BigEndian, uint32(x))
		}
	}
	header([6]int{})
	header([6]int{0, 0, 0, 0, 1, 4})
	_ = binary.Write(&b, binary.BigEndian, uint32(off))
	b.Write([]byte{0, 0})
	b.WriteString("ZZZ\x00")
	b.WriteString("\n" + rule + "\n")
	return b.Bytes()
}

// TestParityTone records (testdata/parity/tone.json) what sarina-tone is made of: ReadWAV on good and broken files,
// Analyze on made-up speech of many kinds (read through ReadWAV), the baseline (Load, Mood with Z, Update, Save's file)
// over many saved files and measurements, the mood history (ReadHistory, AppendHistory's file) and Pattern in made-up
// time zones. The whole program (flags, files, errors, exit status) is compared by rust/parity/tone-differential.py.
func TestParityTone(t *testing.T) {
	var cases []parity.Case
	dir := t.TempDir()

	// ReadWAV
	sixteen := func(vals ...int16) []byte {
		var b []byte
		for _, v := range vals {
			b = binary.LittleEndian.AppendUint16(b, uint16(v))
		}
		return b
	}
	pcm := sixteen(0, 1, -1, 32767, -32768, 1000, -1000, 12345, -12345, 7)
	chunk := func(id string, body []byte) []byte {
		b := append([]byte(id), binary.LittleEndian.AppendUint32(nil, uint32(len(body)))...)
		b = append(b, body...)
		if len(body)%2 == 1 {
			b = append(b, 0)
		}
		return b
	}
	fmtBody := func(chans uint16, rate uint32, bits uint16) []byte {
		b := binary.LittleEndian.AppendUint16(nil, 1)
		b = binary.LittleEndian.AppendUint16(b, chans)
		b = binary.LittleEndian.AppendUint32(b, rate)
		b = binary.LittleEndian.AppendUint32(b, rate*2)
		b = binary.LittleEndian.AppendUint16(b, 2)
		return binary.LittleEndian.AppendUint16(b, bits)
	}
	riff := func(chunks ...[]byte) []byte {
		b := []byte("RIFF\x10\x00\x00\x00WAVE")
		for _, c := range chunks {
			b = append(b, c...)
		}
		return b
	}
	sized := func(id string, size uint32, body []byte) []byte {
		return append(append([]byte(id), binary.LittleEndian.AppendUint32(nil, size)...), body...)
	}
	wavs := map[string][]byte{
		"mono":                      wavFile(1, 1, 16000, 16, pcm),
		"stereo":                    wavFile(1, 2, 44100, 16, pcm),
		"three channels":            wavFile(1, 3, 8000, 16, pcm),
		"odd data length":           wavFile(1, 1, 16000, 16, pcm[:7]),
		"stereo, half a frame":      wavFile(1, 2, 16000, 16, pcm[:6]),
		"float format, 16 bits":     wavFile(3, 1, 16000, 16, pcm),
		"8 bits":                    wavFile(1, 1, 16000, 8, pcm),
		"24 bits":                   wavFile(1, 1, 16000, 24, pcm),
		"no channels":               wavFile(1, 0, 16000, 16, pcm),
		"rate 0":                    wavFile(1, 1, 0, 16, pcm),
		"empty data":                wavFile(1, 1, 16000, 16, nil),
		"an odd chunk first":        wavFile(1, 1, 16000, 16, pcm, chunk("LIST", []byte("abc"))),
		"a long chunk first":        wavFile(1, 1, 16000, 16, pcm, chunk("JUNK", make([]byte, 30))),
		"data before fmt":           riff(chunk("data", pcm), chunk("fmt ", fmtBody(1, 16000, 16))),
		"two fmt chunks":            riff(chunk("fmt ", fmtBody(2, 8000, 16)), chunk("fmt ", fmtBody(1, 16000, 16)), chunk("data", pcm)),
		"two data chunks":           riff(chunk("fmt ", fmtBody(1, 16000, 16)), chunk("data", pcm[:4]), chunk("data", pcm)),
		"data size 0, then data":    riff(chunk("fmt ", fmtBody(1, 16000, 16)), sized("data", 0, pcm)),
		"data size past the end":    riff(chunk("fmt ", fmtBody(1, 16000, 16)), sized("data", 0xFFFFFFFF, pcm)),
		"data size short":           riff(chunk("fmt ", fmtBody(1, 16000, 16)), sized("data", 6, pcm)),
		"odd data size, more":       riff(chunk("fmt ", fmtBody(1, 16000, 16)), sized("data", 5, pcm)),
		"a chunk past the end":      riff(chunk("fmt ", fmtBody(1, 16000, 16)), sized("JUNK", 0x7FFFFFFF, pcm)),
		"fmt cut short":             append(riff(chunk("JUNK", make([]byte, 20))), sized("fmt ", 16, fmtBody(1, 16000, 16)[:10])...),
		"fmt size 14":               riff(sized("fmt ", 14, fmtBody(1, 16000, 16)[:14]), sized("data", 20, pcm)),
		"fmt, no data":              append(riff(chunk("fmt ", fmtBody(1, 16000, 16))), make([]byte, 30)...),
		"no fmt":                    riff(chunk("JUNK", make([]byte, 30)), chunk("data", pcm)),
		"not RIFF":                  append([]byte("RIFX"), wavFile(1, 1, 16000, 16, pcm)[4:]...),
		"not WAVE":                  append(append([]byte("RIFF\x00\x00\x00\x00WAVX"), chunk("fmt ", fmtBody(1, 16000, 16))...), chunk("data", pcm)...),
		"43 bytes":                  wavFile(1, 1, 16000, 16, nil)[:43],
		"44 bytes, header only":     wavFile(1, 1, 16000, 16, nil)[:44],
		"empty":                     {},
		"only RIFF WAVE":            append([]byte("RIFF\x00\x00\x00\x00WAVE"), make([]byte, 32)...),
		"a chunk header at the end": append(riff(chunk("fmt ", fmtBody(1, 16000, 16))), make([]byte, 7)...),
		"huge channel count":        riff(chunk("fmt ", fmtBody(65535, 16000, 16)), chunk("data", pcm)),
	}
	for _, name := range sortedKeys(wavs) {
		p := filepath.Join(dir, "w.wav")
		if err := os.WriteFile(p, wavs[name], 0o600); err != nil {
			t.Fatal(err)
		}
		x, rate, err := ReadWAV(p)
		var xs []string
		for _, v := range x {
			xs = append(xs, bits(v))
		}
		cases = append(cases, parity.Case{Fn: "wav", In: map[string]any{"name": name, "data": fmt.Sprintf("%x", wavs[name])},
			Out: map[string]any{"err": parity.Err(err), "rate": rate, "samples": xs}})
	}

	// Analyze, through ReadWAV, on made-up speech
	var sigs []sig
	for i, r := range []int{16000, 8000, 11025, 22050} {
		for j, hz := range []int{80, 120, 180, 260, 340} {
			syl := r * 3 / 16
			sigs = append(sigs, sig{Seed: uint64(i*10 + j), Rate: r, Chans: 1, N: r * 4, Lead: r / 5, Per: r / hz, Glide: (j % 3) * r / hz / 20,
				Syl: syl, Gap: r / 16, Every: 4 + j, Pause: r * 3 / 10, Amp: 300 + 100*j, Noise: 3 + j})
		}
	}
	for k := 0; k < 40; k++ {
		// a spread of paces, pauses, loudness and pitch movement at 16 kHz, as the voice records
		r := 16000
		sigs = append(sigs, sig{Seed: uint64(100 + k), Rate: r, Chans: 1 + k%3/2, N: r * (3 + k%5), Lead: r / 5 * (1 + k%2), Per: r / (90 + 7*k),
			Glide: k % 4 * 3, Syl: r / (3 + k%6) * 3 / 4, Gap: r / (3 + k%6) / 4, Every: 2 + k%7, Pause: r / 10 * (k % 6), Amp: 120 + 37*k%900,
			Noise: k % 9 * 4})
	}
	sigs = append(sigs,
		sig{Seed: 1, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 120, Syl: 3000, Gap: 1000, Every: 0, Amp: 0, Noise: 20},           // noise only
		sig{Seed: 2, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 120, Syl: 3000, Gap: 1000, Every: 3, Pause: 4000, Amp: 0},         // silence
		sig{Seed: 3, Rate: 16000, Chans: 1, N: 16000, Lead: 1600, Per: 120, Syl: 3000, Gap: 1000, Every: 3, Pause: 4000, Amp: 600, Noise: 4}, // a second
		sig{Seed: 4, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 0, Per: 120, Syl: 3000, Gap: 0, Every: 0, Amp: 600, Noise: 4},                // no quiet at all
		sig{Seed: 5, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 120, Syl: 3000, Gap: 1000, Every: 2, Pause: 3000, Amp: 40000, Noise: 4},
		sig{Seed: 6, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 30, Syl: 3000, Gap: 1000, Every: 2, Pause: 3000, Amp: 700, Noise: 4},  // 533 Hz
		sig{Seed: 7, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 300, Syl: 3000, Gap: 1000, Every: 2, Pause: 3000, Amp: 700, Noise: 4}, // 53 Hz
		sig{Seed: 8, Rate: 640, Chans: 1, N: 640 * 6, Lead: 128, Per: 5, Syl: 120, Gap: 40, Every: 3, Pause: 100, Amp: 700, Noise: 4},
		sig{Seed: 9, Rate: 639, Chans: 1, N: 639 * 6, Lead: 128, Per: 5, Syl: 120, Gap: 40, Every: 3, Pause: 100, Amp: 700, Noise: 4},
		sig{Seed: 10, Rate: 1000, Chans: 1, N: 1000 * 6, Lead: 200, Per: 8, Syl: 190, Gap: 60, Every: 3, Pause: 200, Amp: 700, Noise: 4},
		sig{Seed: 11, Rate: 300, Chans: 1, N: 300 * 8, Lead: 60, Per: 3, Syl: 60, Gap: 20, Every: 3, Pause: 60, Amp: 700, Noise: 4},
		sig{Seed: 12, Rate: 44100, Chans: 2, N: 44100 * 2, Lead: 8000, Per: 300, Syl: 8000, Gap: 3000, Every: 3, Pause: 9000, Amp: 500, Noise: 6},
		sig{Seed: 13, Rate: 48000, Chans: 1, N: 48000 * 2, Lead: 9000, Per: 200, Syl: 9000, Gap: 3000, Every: 2, Pause: 9000, Amp: 500, Noise: 6},
		sig{Seed: 14, Rate: 16000, Chans: 1, N: 16000*25/1000*10 - 1, Lead: 100, Per: 120, Syl: 900, Gap: 200, Every: 0, Amp: 600, Noise: 4},
		sig{Seed: 15, Rate: 16000, Chans: 1, N: 16000 * 2, Lead: 4000, Per: 120, Syl: 19000, Gap: 0, Every: 0, Amp: 600, Noise: 4},
		sig{Seed: 16, Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 133, Glide: 40, Syl: 2600, Gap: 600, Every: 5, Pause: 5000, Amp: 450, Noise: 2},
	)
	for k, per := range []int{1120, 1200, 1280, 1300, 1440} {
		// syllables seven to nine frames apart: the peak picker's spacing from both sides
		sigs = append(sigs, sig{Seed: uint64(200 + k), Rate: 16000, Chans: 1, N: 16000 * 3, Lead: 3200, Per: 120, Syl: per * 3 / 4, Gap: per / 4,
			Every: 6, Pause: 4000, Amp: 500 + 50*k, Noise: 2})
	}
	for _, s := range sigs {
		p := filepath.Join(dir, "s.wav")
		if err := os.WriteFile(p, s.wav(), 0o600); err != nil {
			t.Fatal(err)
		}
		x, rate, err := ReadWAV(p)
		if err != nil {
			t.Fatal(err)
		}
		f, ok := Analyze(x, rate)
		out := map[string]any{"ok": ok}
		if ok {
			out["features"] = featureBits(f)
		}
		cases = append(cases, parity.Case{Fn: "analyze", In: s, Out: out})
	}

	// the baseline: a saved file, then measurements one after another (Mood with Z, then Update unless the case says not),
	// then what Save writes
	usual := func(i int) Features {
		w := float64(i%7) - 3
		return Features{LoudDB: -28 + w*0.4, F0Median: 130 + w*2, F0RangeSt: 5 + w*0.2, Rate: 4.5 + w*0.1, PauseRatio: 0.25 + w*0.01,
			VoicedSec: 3 + w*0.1, Bright: 1800 + w*20}
	}
	specials := []Features{
		{LoudDB: -27, F0Median: 132, F0RangeSt: 4, Rate: 7.5, PauseRatio: 0.02, VoicedSec: 3, Bright: 1900},   // rushed
		{LoudDB: -20, F0Median: 160, F0RangeSt: 9, Rate: 5.2, PauseRatio: 0.2, VoicedSec: 3, Bright: 2300},    // animated
		{LoudDB: -38, F0Median: 118, F0RangeSt: 2.5, Rate: 3, PauseRatio: 0.6, VoicedSec: 3, Bright: 1300},    // tired
		{LoudDB: -28, F0Median: 130, F0RangeSt: 5, Rate: 4.5, PauseRatio: 0.25, VoicedSec: 3, Bright: 1800},   // usual
		{LoudDB: -24, F0Median: 140, F0RangeSt: 6.5, Rate: 5.6, PauseRatio: 0.22, VoicedSec: 3, Bright: 2000}, // near the edges
		{LoudDB: -31, F0Median: 125, F0RangeSt: 3.9, Rate: 3.9, PauseRatio: 0.33, VoicedSec: 3, Bright: 1650},
		{LoudDB: 0, F0Median: 0, F0RangeSt: 0, Rate: 0, PauseRatio: 0, VoicedSec: 0, Bright: 0},
		{LoudDB: math.Copysign(0, -1), F0Median: 1e300, F0RangeSt: -1e300, Rate: 5e-324, PauseRatio: 1e-310, VoicedSec: 1, Bright: 1e18},
	}
	seq := func(n int, extra ...Features) []Features {
		var out []Features
		for i := 0; i < n; i++ {
			out = append(out, usual(i))
		}
		return append(out, extra...)
	}
	full := `{"n":30,"mean":{"loud":-28,"f0":130,"range":5,"rate":4.5,"pause":0.25,"bright":1800},` +
		`"var":{"loud":4,"f0":100,"range":2,"rate":0.3,"pause":0.01,"bright":9000},"count":{"loud":30,"f0":30,"range":30,"rate":30,"pause":30,"bright":30}}`
	old := `{"n":30,"mean":{"loud":-28,"f0":130,"range":5,"rate":4.5,"pause":0.25},"var":{"loud":4,"f0":100,"range":2,"rate":0.3,"pause":0.01}}`
	// measurements whose z-scores sit exactly on the moods' thresholds (and a step beside them), against a baseline whose
	// means are 0 and whose spreads are the floors
	edge := `{"n":30,"mean":{"loud":0,"f0":0,"range":0,"rate":0,"pause":0,"bright":0},"var":{"loud":0,"f0":0,"range":0,"rate":0,"pause":0,"bright":0},` +
		`"count":{"loud":30,"f0":30,"range":30,"rate":30,"pause":30,"bright":30}}`
	at := func(z, sd float64, step int) float64 {
		v := z * sd
		for k := 0; k < 8 && v/sd != z; k++ {
			v = math.Nextafter(v, z*sd+math.Copysign(1, z)*float64(k%2*2-1))
		}
		for ; step > 0; step-- {
			v = math.Nextafter(v, math.Inf(1))
		}
		for ; step < 0; step++ {
			v = math.Nextafter(v, math.Inf(-1))
		}
		return v
	}
	var edges []Features
	for _, st := range []int{-1, 0, 1} {
		edges = append(edges,
			Features{Rate: at(1, 0.6, st), PauseRatio: at(-0.3, 0.08, 0), F0RangeSt: at(0.6, 1.5, 0)},
			Features{Rate: at(1, 0.6, 0), PauseRatio: at(-0.3, 0.08, st), F0RangeSt: at(0.6, 1.5, 0)},
			Features{Rate: at(1, 0.6, 0), PauseRatio: at(-0.3, 0.08, 0), F0RangeSt: at(0.6, 1.5, st)},
			Features{LoudDB: at(4, 2, st), PauseRatio: at(0.4, 0.08, 0)},
			Features{LoudDB: at(4, 2, 0), PauseRatio: at(0.4, 0.08, st)},
			Features{LoudDB: at(-4, 2, st), PauseRatio: at(0.3, 0.08, 0)},
			Features{LoudDB: at(-4, 2, 0), PauseRatio: at(0.3, 0.08, st)},
		)
	}
	type bcase struct {
		name, file string
		fs         []Features
		update     bool
	}
	bcases := []bcase{
		{"no file", "", seq(16, specials...), true},
		{"no file, no update", "", seq(3), false},
		{"empty object", "{}", seq(14, specials[:3]...), true},
		{"null", "null", seq(2), true},
		{"a list", "[]", seq(2), true},
		{"not JSON", "{\"n\":", seq(2), true},
		{"trailing garbage", full + " x", seq(2), true},
		{"full", full, append(specials, seq(3)...), true},
		{"full, no update", full, specials, false},
		{"old, without counts", old, append(specials, seq(14, specials...)...), true},
		{"counts null", strings.Replace(full, `"count":{`, `"count":null,"x":{`, 1), specials, true},
		{"counts empty", strings.Replace(full, `"count":{"loud":30,"f0":30,"range":30,"rate":30,"pause":30,"bright":30}`, `"count":{}`, 1), seq(13, specials...), true},
		{"counts partly", strings.Replace(full, `"bright":30}`, `"bright":3}`, 1), append(specials, seq(10, specials...)...), true},
		{"means but no variances", `{"n":20,"mean":{"loud":-30,"f0":130}}`, seq(3, specials...), true},
		{"variances but no means", `{"n":20,"var":{"loud":4,"zz":1}}`, seq(3, specials...), true},
		{"means null", `{"n":20,"mean":null,"var":{"loud":4}}`, seq(2), true},
		{"variances null", `{"n":20,"mean":{"loud":-30},"var":null}`, seq(2), true},
		{"capital keys", `{"N":40,"MEAN":{"loud":-28,"f0":130,"range":5,"rate":4.5,"pause":0.25},"Var":{"loud":4,"f0":100},"COUNT":{"loud":40}}`, specials, true},
		{"duplicate keys", `{"n":5,"n":25,"mean":{"loud":1},"mean":{"f0":130,"f0":140},"var":{},"count":{"f0":25}}`, specials, true},
		{"n as text", `{"n":"30","mean":{"loud":-28},"var":{"loud":4}}`, specials[:3], true},
		{"n a fraction", `{"n":30.5,"mean":{"loud":-28},"var":{"loud":4}}`, specials[:3], true},
		{"n with an exponent", `{"n":3e1,"mean":{"loud":-28},"var":{"loud":4}}`, specials[:3], true},
		{"n negative", `{"n":-5,"mean":{"loud":-28},"var":{"loud":4}}`, seq(3), true},
		{"n at the top", `{"n":9223372036854775807,"mean":{"loud":-28},"var":{"loud":4},"count":{"loud":9223372036854775807,"f0":-1,"rate":0,"pause":-2}}`, seq(3), true},
		{"n past the top", `{"n":9223372036854775808,"mean":{"loud":-28}}`, seq(2), true},
		{"a mean as text", `{"n":30,"mean":{"loud":"x","f0":130},"var":{"loud":4,"f0":"y"},"count":{"loud":30,"f0":1.5}}`, specials[:4], true},
		{"a mean null", `{"n":30,"mean":{"loud":null,"f0":130},"var":{"loud":4},"count":{"loud":null}}`, specials[:4], true},
		{"negative variance", `{"n":30,"mean":{"loud":-28,"f0":130,"range":5,"rate":4.5,"pause":0.25},"var":{"loud":-4,"f0":-100,"range":2,"rate":-0.3,"pause":0.01}}`, specials, true},
		{"huge values", `{"n":30,"mean":{"loud":1e308,"f0":-1e308},"var":{"loud":1e308,"f0":1e308},"count":{"loud":30,"f0":30}}`, specials[:3], true},
		{"huge values, no update", `{"n":30,"mean":{"loud":1e308,"f0":-1e308},"var":{"loud":1e308,"f0":1e308},"count":{"loud":30,"f0":30}}`, specials[:3], false},
		{"odd keys", "{\"n\":12,\"mean\":{\"<a&b>\":1,\"\u2028\":2,\"\xff\":3,\"loud\":-28},\"var\":{\"\\u00e9\":1},\"count\":{\"z\":1}}", specials[:2], true},
		{"unknown fields", `{"n":12,"x":[1,2],"Mean2":{},"mean":{"loud":-28},"var":{"loud":4}}`, specials[:2], true},
		{"a long run", "", seq(240, specials...), true},
		{"on the thresholds", edge, edges, false},
	}
	for _, c := range bcases {
		p := filepath.Join(dir, "base", c.name+".json")
		_ = os.RemoveAll(filepath.Dir(p))
		if c.file != "" {
			_ = os.MkdirAll(filepath.Dir(p), 0o700)
			if err := os.WriteFile(p, []byte(c.file), 0o600); err != nil {
				t.Fatal(err)
			}
		}
		b := Load(p)
		var steps []any
		var ins [][]string
		for _, f := range c.fs {
			ins = append(ins, featureBits(f))
			mood := b.Mood(f)
			z := b.Z(f)
			var zs []string
			for _, k := range []string{"loud", "f0", "range", "rate", "pause", "bright"} {
				zs = append(zs, bits(z[k]))
			}
			if c.update {
				b.Update(f)
			}
			steps = append(steps, map[string]any{"mood": mood, "hint": HintFor(mood), "z": zs, "n": b.N})
		}
		err := b.Save(p)
		saved, _ := os.ReadFile(p)
		cases = append(cases, parity.Case{Fn: "baseline", In: map[string]any{"name": c.name, "file": parity.Text(c.file), "features": ins, "update": c.update},
			Out: map[string]any{"steps": steps, "err": parity.Err(err), "saved": parity.Text(string(saved))}})
	}

	// the mood history: what ReadHistory reads, and the file AppendHistory leaves
	long := func(n int) string { return `{"t":5,"mood":"` + strings.Repeat("a", n-len(`{"t":5,"mood":""}`)) + `"}` }
	var many strings.Builder
	for i := 0; i < 605; i++ {
		fmt.Fprintf(&many, "{\"t\":%d,\"mood\":\"usual\"}\n", 1000+i)
	}
	histories := map[string]string{
		"none":                 "",
		"one":                  "{\"t\":1700000000,\"mood\":\"low\"}\n",
		"no last newline":      "{\"t\":1,\"mood\":\"low\"}\n{\"t\":2,\"mood\":\"tense\"}",
		"CRLF":                 "{\"t\":1,\"mood\":\"low\"}\r\n{\"t\":2,\"mood\":\"tense\"}\r\r\n{\"t\":3,\"mood\":\"upbeat\"}\r",
		"bad lines":            "\n \nnull\n{}\n[1]\n{\"t\":0,\"mood\":\"low\"}\n{\"t\":-1,\"mood\":\"low\"}\n{\"t\":3,\"mood\":\"\"}\n{\"t\":\"4\",\"mood\":\"low\"}\n{\"t\":5.5,\"mood\":\"low\"}\n{\"t\":6,\"mood\":7}\n{\"t\":7,\"mood\":\"low\"} x\n{\"t\":8,\"mood\":\"low\"}\n",
		"keys":                 "{\"T\":9,\"MOOD\":\"low\"}\n{\"t\":10,\"mood\":\"a\",\"mood\":\"b\",\"x\":1}\n{\"t\":11,\"mood\":null}\n{\"t\":12,\"t\":null,\"mood\":\"c\"}\n{\"\u017f\":1}\n",
		"odd moods":            "{\"t\":1,\"mood\":\"<b>&\"}\n{\"t\":2,\"mood\":\"\u2028\u2029\"}\n{\"t\":3,\"mood\":\"\xff\xfe\"}\n{\"t\":4,\"mood\":\"\\ud800\"}\n{\"t\":5,\"mood\":\"tab\\there\"}\n",
		"big times":            "{\"t\":9223372036854775807,\"mood\":\"low\"}\n{\"t\":9223372036854775808,\"mood\":\"low\"}\n{\"t\":1e3,\"mood\":\"low\"}\n",
		"a line of 65535":      long(65535) + "\n{\"t\":6,\"mood\":\"low\"}\n",
		"a line of 65536":      "{\"t\":4,\"mood\":\"x\"}\n" + long(65536) + "\n{\"t\":6,\"mood\":\"low\"}\n",
		"a last line of 65535": "{\"t\":4,\"mood\":\"x\"}\n" + long(65535),
		"a last line of 65536": "{\"t\":4,\"mood\":\"x\"}\n" + long(65536),
		"a CR line of 65536":   long(65535) + "\r\n{\"t\":6,\"mood\":\"low\"}\n",
		"605 entries":          many.String(),
		"a byte order mark":    "\ufeff{\"t\":1,\"mood\":\"low\"}\n{\"t\":2,\"mood\":\"low\"}\n",
	}
	for _, name := range sortedKeys(histories) {
		p := filepath.Join(dir, "hist", "m.jsonl")
		_ = os.RemoveAll(filepath.Dir(p))
		if name != "none" {
			_ = os.MkdirAll(filepath.Dir(p), 0o700)
			if err := os.WriteFile(p, []byte(histories[name]), 0o600); err != nil {
				t.Fatal(err)
			}
		}
		var read []any
		for _, e := range ReadHistory(p) {
			read = append(read, []any{e.T, parity.Text(e.Mood)})
		}
		err := AppendHistory(p, Entry{T: 1763400000, Mood: Low})
		after, _ := os.ReadFile(p)
		data := histories[name]
		var in any = parity.Text(data)
		if len(data) > 4000 {
			in = nil // made again on the Rust side from the name
		}
		cases = append(cases, parity.Case{Fn: "history", In: map[string]any{"name": name, "data": in},
			Out: map[string]any{"read": read, "err": parity.Err(err), "after": short(after)}})
	}

	// Pattern in made-up zones, at made-up times
	zones := []struct {
		off  int32
		rule string
	}{
		{0, "UTC0"},
		{3600, "CET-1CEST,M3.5.0,M10.5.0/3"},
		{-18000, "EST5EDT,M3.2.0,M11.1.0"},
		{-10800, "<-03>3<-02>,M11.1.0/0,M2.3.0/0"}, // summer time starting at midnight: that midnight does not exist
		{-3600, "<-01>1<+00>,M3.5.0/0,M10.5.0/1"},
		{37800, "<+1030>-10:30<+11>-11,M10.1.0,M4.1.0"},
		{19800, "IST-5:30"},
		{45900, "<+1245>-12:45<+1345>,M9.5.0/2:45,M4.1.0/3:45"},
	}
	nows := []int64{1763400000, 1763420400, 1774749599, 1774749600, 1774753200, 1762052400, 1762056000, 1762052399, 1772000000, 1761868800,
		1761872400, 1730260800, 1730264400, 1767225600, 1767229199, 1775001600}
	moods := []string{Usual, Low, Tense, Upbeat, Usual, Low, Low, Tense, "other", Usual}
	st := uint64(88172645463325252)
	rnd := func(n int) int {
		st ^= st << 13
		st ^= st >> 7
		st ^= st << 17
		return int(st % uint64(n))
	}
	for zi, z := range zones {
		data := zone(z.off, z.rule)
		loc, err := time.LoadLocationFromTZData("x", data)
		if err != nil {
			t.Fatal(z.rule, err)
		}
		for ni, sec := range nows {
			now := time.Unix(sec, int64(ni%3)*400000000).In(loc)
			for k := 0; k < 6; k++ {
				var h []Entry
				switch k {
				case 0: // the boundaries
					y, m, d := now.Date()
					mid := time.Date(y, m, d, 0, 0, 0, 0, loc).Unix()
					for _, t := range []int64{sec, sec + 1, sec - 7*86400, sec - 7*86400 + 1, sec - 14*86400, sec - 14*86400 + 1, mid, mid - 1, mid + 1,
						mid + 4*3600, mid + 4*3600 - 1, mid + 20*3600, mid + 20*3600 - 1, mid - 4*3600} {
						for r := 0; r < 3; r++ {
							h = append(h, Entry{T: t, Mood: moods[(rnd(len(moods)))]})
						}
					}
				default: // many questions over the last two weeks and a bit
					n := []int{3, 9, 25, 60, 200}[k-1]
					bias := []string{Low, Tense, Upbeat}[(zi+ni+k)%3]
					for i := 0; i < n; i++ {
						age := int64(rnd(16*86400)) - 3600
						mood := moods[rnd(len(moods))]
						if rnd(3) == 0 {
							mood = bias
						}
						h = append(h, Entry{T: sec - age, Mood: mood})
					}
				}
				var hs []any
				for _, e := range h {
					hs = append(hs, []any{e.T, e.Mood})
				}
				cases = append(cases, parity.Case{Fn: "pattern",
					In:  map[string]any{"zone": fmt.Sprintf("%x", data), "now": sec, "nanos": now.Nanosecond(), "history": hs},
					Out: Pattern(h, now)})
			}
		}
	}
	parity.Fixture(t, "tone", cases)
}

// short is a file's bytes in hex, or their SHA-256 when there are many.
func short(b []byte) string {
	if len(b) > 4000 {
		return fmt.Sprintf("sha256:%x", sha256.Sum256(b))
	}
	return fmt.Sprintf("%x", b)
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	for i := 1; i < len(keys); i++ {
		for j := i; j > 0 && keys[j] < keys[j-1]; j-- {
			keys[j], keys[j-1] = keys[j-1], keys[j]
		}
	}
	return keys
}
