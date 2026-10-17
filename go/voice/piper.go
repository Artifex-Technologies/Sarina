// Package voice speaks: it runs a Piper text-to-speech voice (a VITS model in ONNX form, the .onnx plus .onnx.json pair every Piper voice
// ships as) without Python. Text goes through espeak-ng to phonemes, the phonemes become the ids the model was trained on, and the ONNX model
// (run by onnxruntime) turns them into audio. A voice trained for Sarina is just another .onnx file here.
//
// Needs the espeak-ng program and onnxruntime's shared library (Arch: espeak-ng, onnxruntime-cpu).
package voice

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"math"
	"os"
	"os/exec"
	"strings"
	"sync"

	ort "github.com/yalue/onnxruntime_go"
)

// Config is the part of a voice's .onnx.json this package needs.
type Config struct {
	Audio struct {
		SampleRate int `json:"sample_rate"`
	} `json:"audio"`
	Espeak struct {
		Voice string `json:"voice"`
	} `json:"espeak"`
	Inference struct {
		NoiseScale  float32 `json:"noise_scale"`
		LengthScale float32 `json:"length_scale"`
		NoiseW      float32 `json:"noise_w"`
	} `json:"inference"`
	PhonemeIDMap map[string][]int64 `json:"phoneme_id_map"`
	NumSpeakers  int                `json:"num_speakers"`
}

// Voice is a loaded voice.
type Voice struct {
	Cfg     Config
	model   string
	session *ort.DynamicAdvancedSession
	hasSID  bool
	mu      sync.Mutex
	// How the voice is delivered; zero values mean the model's own settings.
	NoiseScale, LengthScale, NoiseW float32
	Speaker                         int64
}

var (
	initOnce sync.Once
	initErr  error
)

// LibraryPath is where onnxruntime's shared library is looked for; set before the first Load to change it.
var LibraryPath = "/usr/lib/libonnxruntime.so"

func initRuntime() error {
	initOnce.Do(func() {
		if p := os.Getenv("ONNXRUNTIME_LIB"); p != "" {
			LibraryPath = p
		}
		ort.SetSharedLibraryPath(LibraryPath)
		initErr = ort.InitializeEnvironment()
	})
	return initErr
}

// Load opens a voice: model is the .onnx path; its .json sits next to it as model+".json".
func Load(model string) (*Voice, error) {
	raw, err := os.ReadFile(model + ".json")
	if err != nil {
		return nil, fmt.Errorf("voice config: %w", err)
	}
	var cfg Config
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return nil, fmt.Errorf("voice config: %w", err)
	}
	if cfg.Audio.SampleRate == 0 || len(cfg.PhonemeIDMap) == 0 {
		return nil, fmt.Errorf("%s.json is not a Piper voice config", model)
	}
	if err := initRuntime(); err != nil {
		return nil, fmt.Errorf("onnxruntime (%s): %w", LibraryPath, err)
	}
	inputs := []string{"input", "input_lengths", "scales"}
	hasSID := cfg.NumSpeakers > 1
	if hasSID {
		inputs = append(inputs, "sid")
	}
	sess, err := ort.NewDynamicAdvancedSession(model, inputs, []string{"output"}, nil)
	if err != nil {
		return nil, fmt.Errorf("loading %s: %w", model, err)
	}
	return &Voice{Cfg: cfg, model: model, session: sess, hasSID: hasSID}, nil
}

// Close frees the model.
func (v *Voice) Close() {
	if v.session != nil {
		_ = v.session.Destroy()
		v.session = nil
	}
}

// SampleRate of the audio Speak returns.
func (v *Voice) SampleRate() int { return v.Cfg.Audio.SampleRate }

// Phonemize turns text into one phoneme string per sentence or clause (espeak-ng's IPA, without the tie marks the Piper voices were not trained on).
func Phonemize(text, espeakVoice string) ([]string, error) {
	text = strings.TrimSpace(text)
	if text == "" {
		return nil, nil
	}
	if espeakVoice == "" {
		espeakVoice = "en-us"
	}
	cmd := exec.Command("espeak-ng", "-q", "--ipa=3", "-v", espeakVoice, "--", text)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	if err := cmd.Run(); err != nil {
		return nil, fmt.Errorf("espeak-ng: %v: %s", err, strings.TrimSpace(errb.String()))
	}
	return splitPhonemes(out.String()), nil
}

func splitPhonemes(s string) []string {
	var res []string
	for _, line := range strings.Split(s, "\n") {
		line = strings.ReplaceAll(line, "‍", "") // zero-width joiner: espeak's tie, not a Piper symbol
		line = strings.Join(strings.Fields(line), " ")
		if line != "" {
			res = append(res, line)
		}
	}
	return res
}

// IDs turns a phoneme string into the model's input: start, then each phoneme followed by the pad id, then end. Phonemes the voice does not know are skipped.
func (c *Config) IDs(phonemes string) []int64 {
	pad, bos, eos := c.PhonemeIDMap["_"], c.PhonemeIDMap["^"], c.PhonemeIDMap["$"]
	ids := append([]int64{}, bos...)
	for _, r := range phonemes {
		if m, ok := c.PhonemeIDMap[string(r)]; ok {
			ids = append(ids, m...)
			ids = append(ids, pad...)
		}
	}
	return append(ids, eos...)
}

// Speak makes speech for the text: 16-bit mono samples at SampleRate(), with a short pause between sentences.
func (v *Voice) Speak(text string) ([]int16, error) {
	sentences, err := Phonemize(text, v.Cfg.Espeak.Voice)
	if err != nil {
		return nil, err
	}
	var all []int16
	pause := make([]int16, v.SampleRate()/6)
	for i, s := range sentences {
		audio, err := v.synth(v.Cfg.IDs(s))
		if err != nil {
			return nil, err
		}
		if i > 0 {
			all = append(all, pause...)
		}
		all = append(all, audio...)
	}
	return all, nil
}

func (v *Voice) synth(ids []int64) ([]int16, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	ns, ls, nw := v.Cfg.Inference.NoiseScale, v.Cfg.Inference.LengthScale, v.Cfg.Inference.NoiseW
	if v.NoiseScale > 0 {
		ns = v.NoiseScale
	}
	if v.LengthScale > 0 {
		ls = v.LengthScale
	}
	if v.NoiseW > 0 {
		nw = v.NoiseW
	}
	in, err := ort.NewTensor(ort.NewShape(1, int64(len(ids))), ids)
	if err != nil {
		return nil, err
	}
	defer in.Destroy()
	ln, err := ort.NewTensor(ort.NewShape(1), []int64{int64(len(ids))})
	if err != nil {
		return nil, err
	}
	defer ln.Destroy()
	sc, err := ort.NewTensor(ort.NewShape(3), []float32{ns, ls, nw})
	if err != nil {
		return nil, err
	}
	defer sc.Destroy()
	inputs := []ort.Value{in, ln, sc}
	if v.hasSID {
		sid, err := ort.NewTensor(ort.NewShape(1), []int64{v.Speaker})
		if err != nil {
			return nil, err
		}
		defer sid.Destroy()
		inputs = append(inputs, sid)
	}
	outputs := []ort.Value{nil}
	if err := v.session.Run(inputs, outputs); err != nil {
		return nil, fmt.Errorf("running the voice: %w", err)
	}
	defer outputs[0].Destroy()
	t, ok := outputs[0].(*ort.Tensor[float32])
	if !ok {
		return nil, fmt.Errorf("the voice returned something other than audio")
	}
	return toPCM(t.GetData()), nil
}

// toPCM scales float audio to 16-bit the way Piper does: relative to the loudest sample (but never boosting a very quiet clip too far).
func toPCM(f []float32) []int16 {
	peak := float32(0.01)
	for _, x := range f {
		if a := float32(math.Abs(float64(x))); a > peak {
			peak = a
		}
	}
	scale := 32767 / peak
	out := make([]int16, len(f))
	for i, x := range f {
		v := x * scale
		if v > 32767 {
			v = 32767
		} else if v < -32768 {
			v = -32768
		}
		out[i] = int16(v)
	}
	return out
}

// WAV is a 16-bit mono WAV file of the samples.
func WAV(samples []int16, rate int) []byte {
	var b bytes.Buffer
	dataLen := uint32(len(samples) * 2)
	b.WriteString("RIFF")
	_ = binary.Write(&b, binary.LittleEndian, 36+dataLen)
	b.WriteString("WAVEfmt ")
	_ = binary.Write(&b, binary.LittleEndian, uint32(16))
	_ = binary.Write(&b, binary.LittleEndian, uint16(1)) // PCM
	_ = binary.Write(&b, binary.LittleEndian, uint16(1)) // mono
	_ = binary.Write(&b, binary.LittleEndian, uint32(rate))
	_ = binary.Write(&b, binary.LittleEndian, uint32(rate*2))
	_ = binary.Write(&b, binary.LittleEndian, uint16(2))
	_ = binary.Write(&b, binary.LittleEndian, uint16(16))
	b.WriteString("data")
	_ = binary.Write(&b, binary.LittleEndian, dataLen)
	_ = binary.Write(&b, binary.LittleEndian, samples)
	return b.Bytes()
}
