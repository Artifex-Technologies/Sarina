package tone

import (
	"encoding/binary"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

const sr = 16000

// speech makes a stand-in for talking: a voice-like tone (harmonics) at `f0` Hz that glides by up to `glide` semitones, shaped into
// syllables at `rate` per second with a pause of `pause` seconds after every `perPhrase` syllables, at loudness `amp`, over a faint noise floor.
func speech(seed int64, seconds, f0, glide, rate, amp float64, perPhrase int, pause float64) []float64 {
	r := rand.New(rand.NewSource(seed))
	n := int(seconds * sr)
	x := make([]float64, n)
	t0 := 0.2
	syl := 1 / rate
	phase := 0.0
	t := t0
	count := 0
	for t < seconds-0.2 {
		for i := int(t * sr); i < int((t+syl*0.75)*sr) && i < n; i++ {
			u := (float64(i)/sr - t) / (syl * 0.75)
			env := math.Sin(math.Pi * u) // 0 to 1 to 0
			g := glide * math.Sin(2*math.Pi*0.7*float64(i)/sr+float64(seed))
			f := f0 * math.Pow(2, g/12)
			phase += 2 * math.Pi * f / sr
			var v float64
			for h := 1; h <= 6; h++ {
				v += math.Sin(float64(h)*phase) / float64(h)
			}
			x[i] += amp * env * v / 2
		}
		t += syl
		count++
		if count%perPhrase == 0 {
			t += pause
		}
	}
	for i := range x {
		x[i] += 0.002 * r.NormFloat64()
	}
	return x
}

func TestPitchPaceAndPausesAreMeasured(t *testing.T) {
	low, ok := Analyze(speech(1, 6, 120, 0.5, 4, 0.3, 5, 0.3), sr)
	if !ok {
		t.Fatal("no speech found")
	}
	if low.F0Median < 110 || low.F0Median > 132 {
		t.Errorf("120 Hz voice measured as %.0f Hz", low.F0Median)
	}
	high, _ := Analyze(speech(2, 6, 220, 0.5, 4, 0.3, 5, 0.3), sr)
	if high.F0Median < 200 || high.F0Median > 242 {
		t.Errorf("220 Hz voice measured as %.0f Hz", high.F0Median)
	}
	slow, _ := Analyze(speech(3, 8, 140, 0.5, 3, 0.3, 50, 0), sr)
	fast, _ := Analyze(speech(4, 8, 140, 0.5, 6.5, 0.3, 50, 0), sr)
	if !(fast.Rate > slow.Rate*1.5) {
		t.Errorf("pace: slow %.1f fast %.1f", slow.Rate, fast.Rate)
	}
	few, _ := Analyze(speech(5, 8, 140, 0.5, 5, 0.3, 50, 0), sr)
	many, _ := Analyze(speech(6, 8, 140, 0.5, 5, 0.3, 4, 0.5), sr)
	if !(many.PauseRatio > few.PauseRatio+0.15) {
		t.Errorf("pauses: few %.2f many %.2f", few.PauseRatio, many.PauseRatio)
	}
	wide, _ := Analyze(speech(7, 8, 140, 4, 5, 0.3, 50, 0), sr)
	flat, _ := Analyze(speech(8, 8, 140, 0.3, 5, 0.3, 50, 0), sr)
	if !(wide.F0RangeSt > flat.F0RangeSt+2) {
		t.Errorf("pitch range: wide %.1f flat %.1f", wide.F0RangeSt, flat.F0RangeSt)
	}
	loud, _ := Analyze(speech(9, 6, 140, 0.5, 5, 0.5, 50, 0), sr)
	quiet, _ := Analyze(speech(10, 6, 140, 0.5, 5, 0.1, 50, 0), sr)
	if !(loud.LoudDB > quiet.LoudDB+8) {
		t.Errorf("loudness: loud %.1f quiet %.1f dB", loud.LoudDB, quiet.LoudDB)
	}
}

func TestSilenceNoiseAndFragmentsSaySNothing(t *testing.T) {
	r := rand.New(rand.NewSource(1))
	noise := make([]float64, 6*sr)
	for i := range noise {
		noise[i] = 0.003 * r.NormFloat64()
	}
	if _, ok := Analyze(noise, sr); ok {
		t.Error("room noise was taken for speech")
	}
	if _, ok := Analyze(speech(1, 1.0, 140, 0.5, 5, 0.3, 50, 0), sr); ok {
		t.Error("a second of sound was enough")
	}
	if _, ok := Analyze(nil, sr); ok {
		t.Error("nothing was taken for speech")
	}
}

func baseline(t *testing.T, n int) Baseline {
	t.Helper()
	var b Baseline
	for i := 0; i < n; i++ {
		// the usual: a mid voice, medium pace, some pauses, with natural variation between questions
		f, ok := Analyze(speech(int64(100+i), 6, 130+float64(i%5)*3, 1.5+float64(i%3)*0.3, 4.5+float64(i%4)*0.2, 0.25+float64(i%3)*0.02, 6, 0.3+float64(i%3)*0.05), sr)
		if !ok {
			t.Fatal("baseline sample not read")
		}
		b.Update(f)
	}
	return b
}

func TestHintsAppearOnlyForRealDifferencesAndOnlyAfterEnoughHistory(t *testing.T) {
	usual, _ := Analyze(speech(300, 6, 133, 1.6, 4.6, 0.26, 6, 0.32), sr)
	young := baseline(t, 6)
	rushed, _ := Analyze(speech(301, 6, 135, 0.8, 7.5, 0.33, 40, 0), sr)
	if h := young.Hint(rushed); h != "" {
		t.Errorf("a hint after only %d samples: %q", young.N, h)
	}
	b := baseline(t, 20)
	if h := b.Hint(usual); h != "" {
		t.Errorf("an ordinary question got a hint: %q", h)
	}
	if h := b.Hint(rushed); !strings.Contains(h, "rushed") {
		t.Errorf("fast and pauseless: %q", h)
	}
	tired, _ := Analyze(speech(302, 8, 118, 0.6, 3.0, 0.10, 3, 0.7), sr)
	if h := b.Hint(tired); !strings.Contains(h, "tired") {
		t.Errorf("slow, quiet, flat, many pauses: %q (z %v)", h, b.Z(tired))
	}
	animated, _ := Analyze(speech(303, 6, 160, 4.5, 5.2, 0.45, 6, 0.2), sr)
	if h := b.Hint(animated); !strings.Contains(h, "animated") {
		t.Errorf("loud, high, wide pitch: %q (z %v)", h, b.Z(animated))
	}
}

func TestTheBaselineFollowsSlowChangeAndIsPrivate(t *testing.T) {
	var b Baseline
	for i := 0; i < 200; i++ {
		b.Update(Features{LoudDB: -30, F0Median: 130, F0RangeSt: 5, Rate: 4.5, PauseRatio: 0.2})
	}
	for i := 0; i < 200; i++ {
		b.Update(Features{LoudDB: -24, F0Median: 130, F0RangeSt: 5, Rate: 4.5, PauseRatio: 0.2})
	}
	if b.Mean["loud"] < -25 {
		t.Errorf("a new usual loudness was not followed: %.1f", b.Mean["loud"])
	}
	p := filepath.Join(t.TempDir(), "sub", "b.json")
	if err := b.Save(p); err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(p)
	if st.Mode().Perm() != 0o600 {
		t.Errorf("the baseline file is %o", st.Mode().Perm())
	}
	if got := Load(p); got.N != b.N || got.Mean["loud"] != b.Mean["loud"] {
		t.Error("save and load disagree")
	}
	raw, _ := os.ReadFile(p)
	if strings.Contains(string(raw), "RIFF") || len(raw) > 2000 {
		t.Error("the file holds more than a few numbers")
	}
}

func TestReadWAV(t *testing.T) {
	x := speech(1, 2, 140, 0.5, 5, 0.3, 50, 0)
	p := filepath.Join(t.TempDir(), "a.wav")
	hdr := make([]byte, 44)
	copy(hdr, "RIFF")
	copy(hdr[8:], "WAVEfmt ")
	binary.LittleEndian.PutUint32(hdr[16:], 16)
	binary.LittleEndian.PutUint16(hdr[20:], 1)
	binary.LittleEndian.PutUint16(hdr[22:], 1)
	binary.LittleEndian.PutUint32(hdr[24:], sr)
	binary.LittleEndian.PutUint32(hdr[28:], sr*2)
	binary.LittleEndian.PutUint16(hdr[32:], 2)
	binary.LittleEndian.PutUint16(hdr[34:], 16)
	copy(hdr[36:], "data")
	body := make([]byte, len(x)*2)
	for i, v := range x {
		binary.LittleEndian.PutUint16(body[i*2:], uint16(int16(v*32767)))
	}
	binary.LittleEndian.PutUint32(hdr[40:], 0xFFFFFFFF) // a length still being written is read to the end of the file
	if err := os.WriteFile(p, append(hdr, body...), 0o600); err != nil {
		t.Fatal(err)
	}
	got, rate, err := ReadWAV(p)
	if err != nil || rate != sr || len(got) != len(x) || math.Abs(got[5000]-x[5000]) > 0.001 {
		t.Errorf("ReadWAV: %d samples, %d Hz, %v", len(got), rate, err)
	}
	if _, _, err := ReadWAV(filepath.Join(t.TempDir(), "none.wav")); err == nil {
		t.Error("a missing file was read")
	}
}

func TestMoodHistoryNoticesAPatternOverDays(t *testing.T) {
	now := time.Date(2026, 10, 22, 21, 0, 0, 0, time.Local)
	at := func(daysAgo, hour int) int64 {
		return time.Date(2026, 10, 22-daysAgo, hour, 0, 0, 0, time.Local).Unix()
	}
	var h []Entry
	for d := 7; d < 14; d++ { // the week before: usual
		h = append(h, Entry{at(d, 15), Usual}, Entry{at(d, 21), Usual})
	}
	if p := Pattern(h, now); p != "" {
		t.Errorf("nothing unusual, yet: %q", p)
	}
	for d := 1; d < 7; d++ { // this week: low in the evenings
		h = append(h, Entry{at(d, 14), Usual}, Entry{at(d, 22), Low})
	}
	p := Pattern(h, now)
	if !strings.Contains(p, "low or tired in 6 of 12") || !strings.Contains(p, "more than the week before") || !strings.Contains(p, "late in the day") {
		t.Errorf("an evening pattern this week: %q", p)
	}
	h = append(h, Entry{at(0, 9), Tense}, Entry{at(0, 12), Tense}, Entry{at(0, 18), Usual})
	if p := Pattern(h, now); !strings.HasPrefix(p, "Today they have sounded rushed or tense in 2 of 3") {
		t.Errorf("today first: %q", p)
	}
	if p := Pattern(h[:5], now); p != "" {
		t.Errorf("too little to go on: %q", p)
	}

	path := filepath.Join(t.TempDir(), "s", "mood.jsonl")
	for i := 0; i < keepEntries+5; i++ {
		if err := AppendHistory(path, Entry{int64(1000 + i), Usual}); err != nil {
			t.Fatal(err)
		}
	}
	got := ReadHistory(path)
	if len(got) != keepEntries || got[0].T != 1005 {
		t.Errorf("kept %d, oldest %d", len(got), got[0].T)
	}
	if st, _ := os.Stat(path); st.Mode().Perm() != 0o600 {
		t.Errorf("the history file is %o", st.Mode().Perm())
	}
}

func TestBrightnessIsMeasuredAndJoinsAnOldBaselineGently(t *testing.T) {
	low, _ := Analyze(speech(310, 6, 110, 1.0, 4.5, 0.25, 6, 0.2), sr)
	high, _ := Analyze(speech(311, 6, 220, 1.0, 4.5, 0.25, 6, 0.2), sr)
	if low.Bright <= 0 || high.Bright <= low.Bright {
		t.Errorf("brightness: low voice %.0f, high voice %.0f", low.Bright, high.Bright)
	}
	// a baseline saved before brightness existed: 30 recordings, no counts
	old := Baseline{N: 30, Mean: map[string]float64{"loud": -30, "f0": 130, "range": 5, "rate": 4.5, "pause": 0.2},
		Var: map[string]float64{"loud": 4, "f0": 100, "range": 2, "rate": 0.3, "pause": 0.01}}
	f := Features{LoudDB: -30, F0Median: 130, F0RangeSt: 5, Rate: 4.5, PauseRatio: 0.2, Bright: 9000}
	if z := old.Z(f); z["bright"] != 0 || old.Mood(f) != Usual {
		t.Errorf("brightness without its own history moved the mood: %v %s", z, old.Mood(f))
	}
	old.Update(f)
	if old.Count["loud"] != 31 || old.Count["bright"] != 1 || old.Mean["bright"] != 9000 {
		t.Errorf("counts after one update: %v mean %v", old.Count, old.Mean["bright"])
	}
}
