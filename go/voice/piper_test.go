package voice

import (
	"encoding/binary"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func testConfig() *Config {
	c := &Config{PhonemeIDMap: map[string][]int64{"_": {0}, "^": {1}, "$": {2}, " ": {3}, "h": {20}, "ə": {59}, "l": {24}, "o": {27}, "ʊ": {100}}}
	return c
}

func TestIDsWrapPhonemesWithPadsAndSkipUnknown(t *testing.T) {
	got := testConfig().IDs("hə lˈoʊ")
	// ^ h _ ə _ space _ l _ (ˈ unknown, skipped) o _ ʊ _ $
	want := []int64{1, 20, 0, 59, 0, 3, 0, 24, 0, 27, 0, 100, 0, 2}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("IDs = %v, want %v", got, want)
	}
}

func TestSplitPhonemesDropsTiesAndEmptyLines(t *testing.T) {
	got := splitPhonemes("həlˈo‍ʊ\n\n  a‍ɪɐm   sɑː‍ɹɹˈiːnə \n")
	want := []string{"həlˈoʊ", "aɪɐm sɑːɹɹˈiːnə"}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("splitPhonemes = %q, want %q", got, want)
	}
}

func TestToPCMScalesToTheLoudestSampleAndClips(t *testing.T) {
	pcm := toPCM([]float32{0, 0.5, -1, 0.25})
	if pcm[2] != -32767 || pcm[1] <= 16000 || pcm[0] != 0 {
		t.Errorf("pcm = %v", pcm)
	}
	quiet := toPCM([]float32{0.001, -0.001}) // never boosted past the 0.01 floor
	if quiet[0] > 4000 {
		t.Errorf("a near-silent clip was boosted too far: %v", quiet)
	}
}

func TestWAVHeader(t *testing.T) {
	b := WAV([]int16{1, -2, 3}, 22050)
	if string(b[:4]) != "RIFF" || string(b[8:12]) != "WAVE" || string(b[36:40]) != "data" {
		t.Fatalf("bad header: %q", b[:44])
	}
	if binary.LittleEndian.Uint32(b[24:28]) != 22050 || binary.LittleEndian.Uint32(b[40:44]) != 6 || len(b) != 44+6 {
		t.Errorf("sample rate or length wrong: rate %d, data %d, total %d", binary.LittleEndian.Uint32(b[24:28]), binary.LittleEndian.Uint32(b[40:44]), len(b))
	}
}

// With a real voice and onnxruntime installed, it speaks. Skipped otherwise (set SARINA_TEST_VOICE to a .onnx file).
func TestSpeakWithARealVoice(t *testing.T) {
	model := os.Getenv("SARINA_TEST_VOICE")
	if model == "" {
		model = filepath.Join(os.Getenv("HOME"), ".local/share/labyrinth/tts/voices/en_US-amy-medium.onnx")
	}
	if _, err := os.Stat(model); err != nil {
		t.Skip("no voice installed")
	}
	if _, err := os.Stat(LibraryPath); err != nil {
		t.Skip("onnxruntime is not installed")
	}
	v, err := Load(model)
	if err != nil {
		t.Fatal(err)
	}
	defer v.Close()
	audio, err := v.Speak("Hello. This is a test.")
	if err != nil {
		t.Fatal(err)
	}
	secs := float64(len(audio)) / float64(v.SampleRate())
	if secs < 1.0 || secs > 6.0 {
		t.Errorf("speech of %.1f s for two short sentences", secs)
	}
}
