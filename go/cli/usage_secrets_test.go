package cli

import (
	"bytes"
	"io"
	"strings"
	"testing"
)

// The usage printed for -h or a mistyped flag must never show the API key: it was the --api-key flag's default value,
// so `sarina chat -h` printed OPENAI_API_KEY's value (into a terminal, a log, a screenshot).
func TestChatUsageShowsNoKey(t *testing.T) {
	const key = "sk-made-up-key-0123456789"
	for _, args := range [][]string{{"chat", "-h"}, {"chat", "--no-such-flag"}, {"resume", "-h"}} {
		var errOut bytes.Buffer
		env := Env{Stdin: strings.NewReader(""), Stdout: io.Discard, Stderr: &errOut, Home: t.TempDir(),
			Getenv: func(k string) string {
				if k == "OPENAI_API_KEY" {
					return key
				}
				return ""
			}}
		Main(args, env)
		if strings.Contains(errOut.String(), key) {
			t.Errorf("%v: the usage shows the key:\n%s", args, errOut.String())
		}
		if !strings.Contains(errOut.String(), "-api-key") {
			t.Errorf("%v: no usage printed:\n%s", args, errOut.String())
		}
	}
}
