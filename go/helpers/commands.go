package helpers

import (
	"os"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

func init() {
	cli.Register("memory-search", "semantic search over saved sessions (needs the Python helper): memory-search --model-dir D <query...>",
		func(env *cli.Env, args []string) int {
			return Passthrough(env.Stdin, env.Stdout, env.Stderr, append([]string{"memory-search"}, args...)...)
		})
	cli.Register("voice-chat", "push-to-talk voice conversation (needs the Python helper): voice-chat --stt-model-dir D --tts-model F",
		voiceChat)
}

// voiceChat hands the audio loop to the helper and tells it which binary to run
// per turn: this one, unless the operator chose another with --sarina-bin.
func voiceChat(env *cli.Env, args []string) int {
	have := false
	for _, a := range args {
		if a == "--sarina-bin" || strings.HasPrefix(a, "--sarina-bin=") {
			have = true
		}
	}
	if exe, err := os.Executable(); err == nil && !have {
		args = append([]string{"--sarina-bin", exe}, args...)
	}
	return Passthrough(env.Stdin, env.Stdout, env.Stderr, append([]string{"voice-chat"}, args...)...)
}
