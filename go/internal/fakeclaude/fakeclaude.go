// Package fakeclaude lets a test binary act as the claude executable: a
// package's TestMain calls MaybeRun first, and when FAKE_CLAUDE_MODE is set the
// binary behaves as the CLI (writing its arguments to FAKE_CLAUDE_OUT) instead
// of running tests. Tests point claude_cli's cli_path at os.Executable().
package fakeclaude

import (
	"encoding/json"
	"os"
)

// Env names read by the fake.
const (
	EnvMode = "FAKE_CLAUDE_MODE" // "ok" prints a success result
	EnvOut  = "FAKE_CLAUDE_OUT"  // file receiving {"args": [...]}
)

// MaybeRun acts as the fake claude and exits when EnvMode is set; otherwise it returns.
func MaybeRun() {
	mode := os.Getenv(EnvMode)
	if mode == "" {
		return
	}
	if out := os.Getenv(EnvOut); out != "" {
		b, _ := json.Marshal(map[string]any{"args": os.Args[1:]})
		os.WriteFile(out, b, 0o600)
	}
	if mode == "ok" {
		os.Stdout.WriteString(`{"type":"result","is_error":false,"result":"done by claude","session_id":"cli-1","total_cost_usd":0.25}`)
	}
	os.Exit(0)
}
