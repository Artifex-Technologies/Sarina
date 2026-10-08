package helpers

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/helpers/fakehelper"
)

func TestMain(m *testing.M) {
	fakehelper.MaybeRun()
	os.Exit(m.Run())
}

// useFake points the helper at this test binary acting as the Python helper.
func useFake(t *testing.T) {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv(PythonEnv, exe)
	t.Setenv(fakehelper.EnvOn, "1")
}

func run(t *testing.T, args ...string) (int, string, string) {
	t.Helper()
	var out, errb bytes.Buffer
	code := cli.Main(args, cli.Env{Stdin: strings.NewReader(""), Stdout: &out, Stderr: &errb})
	return code, out.String(), errb.String()
}

func TestMemorySearchForwardsArgs(t *testing.T) {
	useFake(t)
	code, out, _ := run(t, "memory-search", "--model-dir", "m", "find", "the", "bug")
	if code != 0 || strings.TrimSpace(out) != "fake-helper memory-search|--model-dir|m|find|the|bug" {
		t.Fatalf("code %d out %q", code, out)
	}
}

func TestPassthroughExitCode(t *testing.T) {
	useFake(t)
	t.Setenv(fakehelper.EnvExit, "3")
	if code, _, _ := run(t, "memory-search", "q"); code != 3 {
		t.Fatalf("code %d", code)
	}
}

func TestVoiceChatAddsOwnBinaryUnlessGiven(t *testing.T) {
	useFake(t)
	exe, _ := os.Executable()
	_, out, _ := run(t, "voice-chat", "--tts-model", "v.onnx")
	if want := "fake-helper voice-chat|--sarina-bin|" + exe + "|--tts-model|v.onnx"; strings.TrimSpace(out) != want {
		t.Fatalf("got %q want %q", out, want)
	}
	_, out, _ = run(t, "voice-chat", "--sarina-bin", "other")
	if strings.TrimSpace(out) != "fake-helper voice-chat|--sarina-bin|other" {
		t.Fatalf("got %q", out)
	}
}

func TestMissingHelperGivesClearMessage(t *testing.T) {
	// An interpreter that runs but has no sarina_helpers module: `true`-like behaviour via the test binary without the fake switch.
	exe, _ := os.Executable()
	t.Setenv(PythonEnv, exe)
	t.Setenv(fakehelper.EnvOn, "")
	code, _, errOut := run(t, "memory-search", "q")
	if code != 1 || !strings.Contains(errOut, "not importable") || !strings.Contains(errOut, "install:") {
		t.Fatalf("code %d err %q", code, errOut)
	}
	t.Setenv(PythonEnv, filepath.Join(t.TempDir(), "no-such-python"))
	code, _, errOut = run(t, "memory-search", "q")
	if code != 1 || !strings.Contains(errOut, "not importable") {
		t.Fatalf("code %d err %q", code, errOut)
	}
}

func TestServerRoundTripAndFailure(t *testing.T) {
	useFake(t)
	t.Setenv(fakehelper.EnvLabel, "Graphics")
	t.Setenv(fakehelper.EnvFingerprint, "abc")
	s, err := StartServer("model")
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if s.Prototypes != "abc" {
		t.Fatalf("fingerprint %q", s.Prototypes)
	}
	label, score, err := s.Classify("draw")
	if err != nil || label != "Graphics" || score != 0.9 {
		t.Fatalf("%q %v %v", label, score, err)
	}
	groups, err := s.Group([]map[string]any{{"name": "a.nfc"}}, 0.7)
	if err != nil || len(groups) != 1 || groups[0][1] != "b.nfc" {
		t.Fatalf("%v %v", groups, err)
	}

	if _, err := StartServer("bad"); err == nil || !strings.Contains(err.Error(), "no model.onnx in bad") {
		t.Fatalf("err %v", err)
	}
}

func TestNullLabelIsEmpty(t *testing.T) {
	useFake(t)
	s, err := StartServer("model")
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if label, _, err := s.Classify("x"); err != nil || label != "" {
		t.Fatalf("%q %v", label, err)
	}
}
