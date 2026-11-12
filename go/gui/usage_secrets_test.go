package gui

import (
	"bytes"
	"io"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// The usage printed for -h or a mistyped flag must never show the API key (it was the --api-key flag's default).
func TestUsageShowsNoKey(t *testing.T) {
	const key = "sk-made-up-key-0123456789"
	getenv := func(k string) string {
		if k == "OPENAI_API_KEY" {
			return key
		}
		return ""
	}
	for _, args := range [][]string{{"-h"}, {"--no-such-flag"}} {
		var errOut bytes.Buffer
		command(&cli.Env{Stdout: io.Discard, Stderr: &errOut, Getenv: getenv}, args)
		if strings.Contains(errOut.String(), key) || !strings.Contains(errOut.String(), "-api-key") {
			t.Errorf("%v: %s", args, errOut.String())
		}
	}
}
