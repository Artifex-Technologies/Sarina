// Package tone reads HOW something was said, not what: loudness, pitch movement, speaking pace and pauses of one spoken question, compared with how
// this person usually sounds, to give Sarina a quiet hint ("a bit rushed", "sounds tired") that only changes her style. Plain signal processing, no model.
//
// Privacy by design: it works on a recording that is deleted right after, keeps only a handful of running averages (numbers, no audio, no words) in one
// private file, runs on this computer, and is never shown or sent anywhere. It cannot tell emotions, only how a voice differs from its own usual self,
// and it says nothing until it has heard enough of that voice (MinSamples) to know what "usual" is.
package tone

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
)

// Features of one recording.
type Features struct {
	LoudDB     float64 `json:"loud_db"`     // mean level of the speaking frames, dB below full scale
	F0Median   float64 `json:"f0_median"`   // Hz
	F0RangeSt  float64 `json:"f0_range_st"` // spread of pitch (10th to 90th percentile) in semitones
	Rate       float64 `json:"rate"`        // syllable-like beats per second of speech
	PauseRatio float64 `json:"pause_ratio"` // time in pauses (250 ms or more) over speaking time
	VoicedSec  float64 `json:"voiced_sec"`
}

// ReadWAV reads 16-bit PCM (mono, or stereo mixed down) and returns samples in -1..1 and the sample rate.
func ReadWAV(path string) ([]float64, int, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, 0, err
	}
	if len(b) < 44 || string(b[:4]) != "RIFF" || string(b[8:12]) != "WAVE" {
		return nil, 0, errors.New("not a WAV file")
	}
	var rate, channels, bits int
	pos := 12
	for pos+8 <= len(b) {
		id, size := string(b[pos:pos+4]), int(binary.LittleEndian.Uint32(b[pos+4:pos+8]))
		body := pos + 8
		if id == "fmt " && body+16 <= len(b) {
			channels = int(binary.LittleEndian.Uint16(b[body+2:]))
			rate = int(binary.LittleEndian.Uint32(b[body+4:]))
			bits = int(binary.LittleEndian.Uint16(b[body+14:]))
		}
		if id == "data" {
			if size <= 0 || body+size > len(b) { // a recording still being written, or a streamed file: take what is there
				size = len(b) - body
			}
			if bits != 16 || channels < 1 || rate <= 0 {
				return nil, 0, fmt.Errorf("only 16-bit PCM is read (got %d bits, %d channels)", bits, channels)
			}
			n := size / 2 / channels
			out := make([]float64, n)
			for i := 0; i < n; i++ {
				var s float64
				for c := 0; c < channels; c++ {
					s += float64(int16(binary.LittleEndian.Uint16(b[body+(i*channels+c)*2:])))
				}
				out[i] = s / float64(channels) / 32768
			}
			return out, rate, nil
		}
		pos = body + size + size%2
	}
	return nil, 0, errors.New("no audio data")
}

// Analyze measures one recording. ok is false when there is too little speech to say anything (under about a second and a half).
func Analyze(x []float64, rate int) (f Features, ok bool) {
	frame, hop := rate*25/1000, rate*10/1000
	if frame < 16 || len(x) < frame*10 {
		return f, false
	}
	var rms []float64
	for i := 0; i+frame <= len(x); i += hop {
		var s float64
		for _, v := range x[i : i+frame] {
			s += v * v
		}
		rms = append(rms, math.Sqrt(s/float64(frame)))
	}
	db := make([]float64, len(rms))
	for i, r := range rms {
		db[i] = 20 * math.Log10(math.Max(r, 1e-6))
	}
	sorted := append([]float64(nil), db...)
	sort.Float64s(sorted)
	floor := sorted[len(sorted)/10]
	peak := sorted[len(sorted)*95/100]
	if peak-floor < 12 {
		return f, false // nothing stands out from the room: no speech
	}
	thr := math.Max(floor+9, peak-28)
	voiced := make([]bool, len(db))
	var nv int
	var sumDB float64
	for i, d := range db {
		if d > thr {
			voiced[i] = true
			nv++
			sumDB += d
		}
	}
	f.VoicedSec = float64(nv*hop) / float64(rate)
	if f.VoicedSec < 1.2 {
		return f, false
	}
	f.LoudDB = sumDB / float64(nv)

	// pauses: gaps of 250 ms or more between speaking frames, inside the speech (not the quiet before and after)
	first, last := -1, -1
	for i, v := range voiced {
		if v {
			if first < 0 {
				first = i
			}
			last = i
		}
	}
	minGap := 250 * rate / 1000 / hop
	var pauseFrames, run int
	for i := first; i <= last; i++ {
		if voiced[i] {
			if run >= minGap {
				pauseFrames += run
			}
			run = 0
		} else {
			run++
		}
	}
	span := float64(last - first + 1)
	f.PauseRatio = float64(pauseFrames) / math.Max(span-float64(pauseFrames), 1)

	// pace: peaks in the smoothed level (one per syllable, roughly)
	sm := make([]float64, len(db))
	for i := range db {
		var s float64
		var n int
		for j := i - 2; j <= i+2; j++ {
			if j >= 0 && j < len(db) {
				s += db[j]
				n++
			}
		}
		sm[i] = s / float64(n)
	}
	peaks := 0
	lastPeak := -100
	for i := 2; i < len(sm)-2; i++ {
		if voiced[i] && sm[i] > sm[i-1] && sm[i] >= sm[i+1] && sm[i] > thr+3 && i-lastPeak >= 8 {
			// it must rise clearly above the dip before it (a syllable, not wobble)
			lo := sm[i]
			for j := i - 1; j >= 0 && i-j <= 12; j-- {
				lo = math.Min(lo, sm[j])
			}
			if sm[i]-lo > 2.5 {
				peaks++
				lastPeak = i
			}
		}
	}
	f.Rate = float64(peaks) / (float64(last-first+1-pauseFrames) * float64(hop) / float64(rate))

	// pitch: autocorrelation over the speaking frames, 75 to 350 Hz
	lagMin, lagMax := rate/350, rate/75
	var f0s []float64
	for i := first; i <= last; i++ {
		if !voiced[i] || i%2 == 1 {
			continue
		}
		seg := x[i*hop : i*hop+frame]
		if i*hop+frame > len(x) || frame <= lagMax {
			continue
		}
		var e0 float64
		for _, v := range seg {
			e0 += v * v
		}
		if e0 < 1e-9 {
			continue
		}
		best, bestLag := 0.0, 0
		for lag := lagMin; lag <= lagMax && lag < frame/2+frame/4; lag++ {
			var s float64
			for k := 0; k+lag < frame; k++ {
				s += seg[k] * seg[k+lag]
			}
			c := s / e0
			if c > best {
				best, bestLag = c, lag
			}
		}
		if best >= 0.4 && bestLag > 0 {
			f0s = append(f0s, float64(rate)/float64(bestLag))
		}
	}
	if len(f0s) < 15 {
		return f, false // too little voiced speech to know the pitch
	}
	sort.Float64s(f0s)
	f.F0Median = f0s[len(f0s)/2]
	lo, hi := f0s[len(f0s)/10], f0s[len(f0s)*9/10]
	f.F0RangeSt = 12 * math.Log2(hi/lo)
	return f, true
}

// Baseline is how this person usually sounds: running averages that follow slow changes (a floor on the update weight).
type Baseline struct {
	N    int                `json:"n"`
	Mean map[string]float64 `json:"mean"`
	Var  map[string]float64 `json:"var"`
}

// MinSamples is how many recordings must have been heard before any hint is given.
const MinSamples = 12

func (f Features) vec() map[string]float64 {
	return map[string]float64{"loud": f.LoudDB, "f0": f.F0Median, "range": f.F0RangeSt, "rate": f.Rate, "pause": f.PauseRatio}
}

// Update folds one recording into the baseline.
func (b *Baseline) Update(f Features) {
	if b.Mean == nil {
		b.Mean, b.Var = map[string]float64{}, map[string]float64{}
	}
	b.N++
	alpha := math.Max(1/float64(b.N), 0.03)
	for k, v := range f.vec() {
		if b.N == 1 {
			b.Mean[k], b.Var[k] = v, 0
			continue
		}
		d := v - b.Mean[k]
		b.Mean[k] += alpha * d
		b.Var[k] = (1 - alpha) * (b.Var[k] + alpha*d*d)
	}
}

// minSD keeps a very steady voice from turning small differences into big z-scores.
var minSD = map[string]float64{"loud": 2.0, "f0": 12.0, "range": 1.5, "rate": 0.6, "pause": 0.08}

// Z says how far each feature is from this person's usual, in their own standard deviations.
func (b *Baseline) Z(f Features) map[string]float64 {
	z := map[string]float64{}
	for k, v := range f.vec() {
		sd := math.Max(math.Sqrt(b.Var[k]), minSD[k])
		z[k] = (v - b.Mean[k]) / sd
	}
	return z
}

// Hint is a short note for Sarina's style, or "" (nothing unusual, or not enough history yet). It is never shown to the person.
func (b *Baseline) Hint(f Features) string {
	if b.N < MinSamples {
		return ""
	}
	z := b.Z(f)
	arousal := (z["loud"] + z["rate"] + z["range"] + z["f0"]) / 4
	switch {
	case z["rate"] >= 1.0 && z["pause"] <= -0.3 && z["range"] < 0.6:
		return "The owner sounds rushed or tense: be brief and direct, no pleasantries."
	case arousal >= 0.8 && z["pause"] <= 0.4:
		return "The owner sounds animated or in a good mood: matching that energy is fine, and a light joke would land."
	case arousal <= -0.8 && z["pause"] >= 0.3:
		return "The owner sounds low or tired: be gentle and brief, no jokes unless they make one, and do not point this out."
	}
	return ""
}

// Load reads the baseline file (an empty baseline if there is none).
func Load(path string) Baseline {
	var b Baseline
	if raw, err := os.ReadFile(path); err == nil {
		_ = json.Unmarshal(raw, &b)
	}
	return b
}

// Save writes it privately (owner only), through a temporary file.
func (b Baseline) Save(path string) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	raw, _ := json.Marshal(b)
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, raw, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}
