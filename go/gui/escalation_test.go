package gui

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/fakeclaude"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

func TestMain(m *testing.M) {
	fakeclaude.MaybeRun()
	os.Exit(m.Run())
}

func deadURL(t *testing.T) string {
	t.Helper()
	d := httptest.NewServer(http.NotFoundHandler())
	u := d.URL
	d.Close()
	return u
}

func fakeClaude(t *testing.T) (exe, out string) {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	out = filepath.Join(t.TempDir(), "child.json")
	t.Setenv(fakeclaude.EnvMode, "ok")
	t.Setenv(fakeclaude.EnvOut, out)
	return exe, out
}

func TestOperatorEscalationHandsOffAndShowsInTierStatus(t *testing.T) {
	exe, out := fakeClaude(t)
	dead := deadURL(t)
	r := newRig(t, nil, func(c *Config) {
		c.Initial.BaseURL = dead
		c.Escalation = &tiers.EscalationOptions{Enabled: true, CLIPath: exe}
	})
	code, res := r.do("POST", "/api/chat", map[string]any{"prompt": "do the thing"})
	if code != 200 || res["final_output"] != "done by claude" {
		t.Fatalf("%d %v", code, res)
	}
	if _, err := os.Stat(out); err != nil {
		t.Fatal("the fake claude did not run")
	}
	_, tr := r.do("GET", "/api/tiers", nil)
	rows, _ := tr["tiers"].([]any)
	if len(rows) != 2 {
		t.Fatalf("tiers: %v", tr)
	}
	row := rows[1].(map[string]any)
	if row["kind"] != "claude_cli" || row["cli_found"] != true || row["cli_path"] != exe {
		t.Fatalf("row: %v", row)
	}
}

func TestPageCannotEnableEscalation(t *testing.T) {
	exe, out := fakeClaude(t)
	dead := deadURL(t)
	r := newRig(t, nil, func(c *Config) { c.Initial.BaseURL = dead })
	for _, patch := range []map[string]any{
		{"escalate_to_claude_code": true}, {"claude_code_cli_path": exe}, {"escalation_config": map[string]any{"enabled": true, "cli_path": exe}},
	} {
		r.do("POST", "/api/state", patch) // rejected or ignored; either way it must not take effect
	}
	r.do("POST", "/api/chat", map[string]any{"prompt": "do the thing", "escalate_to_claude_code": true, "claude_code_cli_path": exe})
	if _, err := os.Stat(out); err == nil {
		t.Fatal("a request enabled the claude_cli tier")
	}
	_, tr := r.do("GET", "/api/tiers", nil)
	if strings.Contains(strings.ToLower(toString(tr)), "claude_cli") {
		t.Fatalf("tier status lists claude_cli: %v", tr)
	}
}

func toString(m map[string]any) string {
	var b strings.Builder
	var walk func(any)
	walk = func(v any) {
		switch x := v.(type) {
		case map[string]any:
			for k, e := range x {
				b.WriteString(k + " ")
				walk(e)
			}
		case []any:
			for _, e := range x {
				walk(e)
			}
		case string:
			b.WriteString(x + " ")
		}
	}
	walk(m)
	return b.String()
}
