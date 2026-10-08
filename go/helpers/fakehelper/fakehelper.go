// Package fakehelper is a stand-in for `python -m sarina_helpers`, for Go tests.
//
// A test package calls MaybeRun from TestMain and points SARINA_HELPERS_PYTHON
// at its own test binary with SARINA_FAKE_HELPER=1; the binary then behaves as
// the helper instead of running tests. No Python is needed.
package fakehelper

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"
)

// Environment switches read by the fake.
const (
	EnvOn          = "SARINA_FAKE_HELPER"      // "1": act as the helper
	EnvFingerprint = "SARINA_FAKE_FINGERPRINT" // ping's prototypes value
	EnvLabel       = "SARINA_FAKE_LABEL"       // classify label (empty: null)
	EnvLog         = "SARINA_FAKE_LOG"         // file receiving every request line
	EnvExit        = "SARINA_FAKE_EXIT"        // exit code for memory-search / voice-chat
)

// MaybeRun runs the fake and exits if this process was started as the helper.
func MaybeRun() {
	if os.Getenv(EnvOn) == "1" {
		os.Exit(Run(os.Args[1:], os.Stdin, os.Stdout))
	}
}

// Run is the fake helper; args are what follows the interpreter name.
func Run(args []string, in io.Reader, out io.Writer) int {
	if len(args) < 2 || args[0] != "-m" || args[1] != "sarina_helpers" {
		fmt.Fprintln(out, "not the helper")
		return 2
	}
	rest := args[2:]
	if len(rest) == 0 {
		fmt.Fprintln(out, "usage: python -m sarina_helpers {serve|memory-search|voice-chat} ...")
		return 2
	}
	if rest[0] == "serve" {
		return serve(rest[1:], in, out)
	}
	fmt.Fprintln(out, "fake-helper "+strings.Join(rest, "|"))
	code := 0
	fmt.Sscan(os.Getenv(EnvExit), &code)
	return code
}

func serve(args []string, in io.Reader, out io.Writer) int {
	if len(args) >= 2 && args[1] == "bad" {
		fmt.Fprintln(out, `{"ok":false,"error":"no model.onnx in bad"}`)
		return 1
	}
	var logf *os.File
	if p := os.Getenv(EnvLog); p != "" {
		logf, _ = os.OpenFile(p, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
		defer logf.Close()
	}
	sc := bufio.NewScanner(in)
	sc.Buffer(make([]byte, 1<<20), 1<<24)
	for sc.Scan() {
		if logf != nil {
			fmt.Fprintln(logf, sc.Text())
		}
		var req map[string]any
		_ = json.Unmarshal(sc.Bytes(), &req)
		var resp map[string]any
		switch req["op"] {
		case "ping":
			fp := os.Getenv(EnvFingerprint)
			if fp == "" {
				fp = "fake"
			}
			resp = map[string]any{"ok": true, "protocol": 1, "prototypes": fp}
		case "classify":
			var label any
			if l := os.Getenv(EnvLabel); l != "" {
				label = l
			}
			resp = map[string]any{"ok": true, "label": label, "score": 0.9}
		case "group":
			resp = map[string]any{"ok": true, "groups": [][]string{{"a.nfc", "b.nfc"}}}
		default:
			resp = map[string]any{"ok": false, "error": "unknown op"}
		}
		b, _ := json.Marshal(resp)
		fmt.Fprintln(out, string(b))
	}
	return 0
}
