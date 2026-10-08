package service

import (
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
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

func TestOperatorEscalationHandsOffWhenBackendIsDown(t *testing.T) {
	exe, out := fakeClaude(t)
	dead := deadURL(t)
	r := newRig(t, func(c *Config) {
		c.BaseURL = dead
		c.Escalation = &tiers.EscalationOptions{Enabled: true, CLIPath: exe}
	})
	id := r.mustCreate(nil)
	code, res := r.chat(id, map[string]any{"prompt": "do the thing"})
	if code != 200 || res["final_output"] != "done by claude" {
		t.Fatalf("%d %v", code, res)
	}
	if _, err := os.Stat(out); err != nil {
		t.Fatal("the fake claude did not run")
	}
}

func TestCallerCannotEnableEscalationOrChooseTheExecutable(t *testing.T) {
	exe, out := fakeClaude(t)
	dead := deadURL(t)
	r := newRig(t, func(c *Config) { c.BaseURL = dead }) // operator did not enable it
	code, id := r.create(map[string]any{"escalate_to_claude_code": true, "claude_code_cli_path": exe,
		"escalation_config": map[string]any{"enabled": true, "cli_path": exe}})
	if code == 200 {
		r.chat(id["session_id"].(string), map[string]any{"prompt": "do the thing"})
	}
	if _, err := os.Stat(out); err == nil {
		t.Fatal("a caller enabled the claude_cli tier")
	}
}

func TestCallerCannotRedirectOperatorEscalationPath(t *testing.T) {
	exe, out := fakeClaude(t)
	dead := deadURL(t)
	other := filepath.Join(t.TempDir(), "evil-claude")
	r := newRig(t, func(c *Config) {
		c.BaseURL = dead
		c.Escalation = &tiers.EscalationOptions{Enabled: true, CLIPath: exe}
	})
	code, id := r.create(map[string]any{"claude_code_cli_path": other, "escalation_config": map[string]any{"cli_path": other}})
	if code != 200 {
		t.Skipf("unknown fields rejected outright (%d): nothing to redirect", code)
	}
	code, res := r.chat(id["session_id"].(string), map[string]any{"prompt": "do the thing"})
	if code != 200 || res["final_output"] != "done by claude" {
		t.Fatalf("%d %v", code, res)
	}
	if _, err := os.Stat(out); err != nil {
		t.Fatal("the operator's claude did not run")
	}
}
