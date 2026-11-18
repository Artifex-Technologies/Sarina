package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// toolDefs is every tool the binary offers under one environment: name, permission, class and the definition as the
// model receives it (json.Marshal of the spec, the form inside every request body).
func toolDefs(t *testing.T, env map[string]string) []any {
	t.Helper()
	for _, kv := range os.Environ() {
		if k, _, _ := strings.Cut(kv, "="); (strings.HasPrefix(k, "SARINA_") || strings.HasPrefix(k, "HEIMDALL_")) && k != parity.UpdateEnv {
			t.Setenv(k, "")
		}
	}
	t.Setenv("HOME", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	for k, v := range env {
		t.Setenv(k, v)
	}
	var out []any
	for _, tool := range tools.Builtins() {
		b, err := json.Marshal(tool.Spec)
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, []any{tool.Spec.Name, tool.Requires.String(), int(tool.Class), string(b)})
	}
	return out
}

func TestParityToolDefs(t *testing.T) {
	var cases []parity.Case
	// a council with one member, for council_ask
	councilFile := filepath.Join(t.TempDir(), "council.json")
	if err := os.WriteFile(councilFile, []byte(`{"members":[{"name":"m","base_url":"http://127.0.0.1:1/v1","model":"m"}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, env := range []map[string]string{{}, {"SARINA_SYSTEM": "on"},
		{"SARINA_CRYPTO": "on", "SARINA_MATH": "on", "SARINA_QUANTUM": "on", "SARINA_SYSTEM": "on", "SARINA_REVERSE": "on",
			"LABYRINTH_REA": "/bin/true", "SARINA_COUNCIL_FILE": councilFile, "SARINA_VAULT": "/made-up/vault", "SARINA_FORGE": "on"}} {
		in := map[string]string{}
		for k, v := range env {
			in[k] = strings.ReplaceAll(v, councilFile, "{TEMP}/council.json") // the same fixture on every run
		}
		cases = append(cases, parity.Case{Fn: "Builtins", In: in, Out: toolDefs(t, env)})
	}
	parity.Fixture(t, "tooldefs", cases)
	// the definitions themselves, for the Rust port (rust/sarina/src/tools/defs.json): every tool of the fullest set,
	// one per line, its parameters as Go writes the map
	var buf bytes.Buffer
	buf.WriteString("[\n")
	all := tools.Builtins()
	for i, tool := range all {
		params, err := json.Marshal(tool.Spec.Parameters)
		if err != nil {
			t.Fatal(err)
		}
		line, err := json.Marshal(map[string]any{"name": tool.Spec.Name, "requires": tool.Requires.String(), "class": int(tool.Class),
			"description": tool.Spec.Description, "parameters": json.RawMessage(params)})
		if err != nil {
			t.Fatal(err)
		}
		buf.Write(line)
		if i < len(all)-1 {
			buf.WriteByte(',')
		}
		buf.WriteByte('\n')
	}
	buf.WriteString("]\n")
	parity.Generated(t, "rust/sarina/src/tools/defs.json", buf.Bytes())
}
