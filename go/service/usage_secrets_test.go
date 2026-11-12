package service

import (
	"bytes"
	"io"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// The usage printed for -h or a mistyped flag must never show the token or the API key: both were flag defaults, so a
// mistyped flag in sarina-service's unit put them into the journal.
func TestUsageShowsNoSecrets(t *testing.T) {
	const key, token = "sk-made-up-key-0123456789", "made-up-token-abcdef"
	getenv := func(k string) string {
		switch k {
		case "OPENAI_API_KEY":
			return key
		case "SARINA_SERVICE_TOKEN":
			return token
		}
		return ""
	}
	for _, args := range [][]string{{"-h"}, {"--no-such-flag"}} {
		var errOut bytes.Buffer
		command(&cli.Env{Stdout: io.Discard, Stderr: &errOut, Getenv: getenv}, args)
		if strings.Contains(errOut.String(), key) || strings.Contains(errOut.String(), token) {
			t.Errorf("%v: the usage shows a secret:\n%s", args, errOut.String())
		}
		if !strings.Contains(errOut.String(), "-token") {
			t.Errorf("%v: no usage printed:\n%s", args, errOut.String())
		}
	}
	// the token still comes from the environment: a non-loopback address is not refused for want of one (192.0.2.1 is
	// a documentation address no machine has, so the listen fails instead: nothing is bound)
	var errOut bytes.Buffer
	if code := command(&cli.Env{Stdout: io.Discard, Stderr: &errOut, Getenv: getenv}, []string{"--host", "192.0.2.1", "--port", "0"}); code != 1 ||
		strings.Contains(errOut.String(), "without a token") {
		t.Fatalf("%d %s", code, errOut.String())
	}
}
