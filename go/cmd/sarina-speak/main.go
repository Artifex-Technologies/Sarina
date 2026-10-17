// Command sarina-speak says text with a Piper voice, in Go (no Python): the plain-text answer goes in as arguments or on standard input,
// a WAV file comes out (or plays, with --play).
//
//	sarina-speak --model ~/.local/share/labyrinth/tts/voices/en_US-amy-medium.onnx "Hello" --out hello.wav
//	echo "Hello" | sarina-speak --model VOICE.onnx --play
package main

import (
	"flag"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/voice"
)

func main() {
	model := flag.String("model", "", "path of the voice's .onnx file (its .onnx.json must sit next to it)")
	out := flag.String("out", "", "write a WAV file here")
	play := flag.Bool("play", false, "play it (pw-play) instead of writing a file")
	length := flag.Float64("length", 0, "speaking speed: bigger is slower (default: the voice's own, usually 1)")
	noise := flag.Float64("noise", 0, "how much the voice varies (default: the voice's own, usually 0.667)")
	noiseW := flag.Float64("noise-w", 0, "how much the rhythm varies (default: the voice's own, usually 0.8)")
	flag.Parse()
	text := strings.Join(flag.Args(), " ")
	if text == "" {
		b, _ := io.ReadAll(os.Stdin)
		text = string(b)
	}
	if *model == "" || strings.TrimSpace(text) == "" || (*out == "" && !*play) {
		fmt.Fprintln(os.Stderr, "usage: sarina-speak --model VOICE.onnx [--out FILE.wav | --play] [TEXT]")
		os.Exit(2)
	}
	v, err := voice.Load(*model)
	if err != nil {
		fmt.Fprintln(os.Stderr, "sarina-speak:", err)
		os.Exit(1)
	}
	defer v.Close()
	v.LengthScale, v.NoiseScale, v.NoiseW = float32(*length), float32(*noise), float32(*noiseW)
	audio, err := v.Speak(text)
	if err != nil {
		fmt.Fprintln(os.Stderr, "sarina-speak:", err)
		os.Exit(1)
	}
	wav := voice.WAV(audio, v.SampleRate())
	if *out != "" {
		if err := os.WriteFile(*out, wav, 0o600); err != nil {
			fmt.Fprintln(os.Stderr, "sarina-speak:", err)
			os.Exit(1)
		}
		return
	}
	f, err := os.CreateTemp("", "sarina-speak-*.wav")
	if err != nil {
		fmt.Fprintln(os.Stderr, "sarina-speak:", err)
		os.Exit(1)
	}
	defer os.Remove(f.Name())
	_, _ = f.Write(wav)
	f.Close()
	if err := exec.Command("pw-play", f.Name()).Run(); err != nil {
		fmt.Fprintln(os.Stderr, "sarina-speak: playing:", err)
		os.Exit(1)
	}
}
