package cli

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/fakeclaude"
)

func TestMain(m *testing.M) {
	fakeclaude.MaybeRun()
	os.Exit(m.Run())
}

func deadBackendURL(t *testing.T) string {
	t.Helper()
	dead := httptest.NewServer(http.NotFoundHandler())
	u := dead.URL
	dead.Close()
	return u
}

// useFakeClaude points the child at this test binary and returns its path and
// the file the fake writes its arguments to.
func useFakeClaude(t *testing.T) (exe, out string) {
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

func childArgs(t *testing.T, out string) []string {
	t.Helper()
	b, err := os.ReadFile(out)
	if err != nil {
		t.Fatalf("the fake claude was not run: %v", err)
	}
	var d struct{ Args []string }
	json.Unmarshal(b, &d)
	return d.Args
}

func TestEscalateFlagHandsOffToClaudeAndUsesWorkspaceAndPermissions(t *testing.T) {
	exe, out := useFakeClaude(t)
	h := newHarness(t, strings.NewReader(""))
	code := Main(chatArgs(h, deadBackendURL(t), "--escalate-to-claude-code", "--claude-code-cli-path", exe, "--allow-write", "fix the bug"), h.env)
	if code != 0 || !strings.Contains(h.out.String(), "done by claude") {
		t.Fatalf("exit %d out=%s err=%s", code, h.out.String(), h.errb.String())
	}
	args := childArgs(t, out)
	joined := strings.Join(args, " ")
	if !strings.Contains(args[1], "fix the bug") || !strings.Contains(joined, "--cwd "+h.dir) ||
		!strings.Contains(joined, "--permission-mode acceptEdits") || strings.Contains(joined, "bypass") {
		t.Fatalf("child args: %q", args)
	}
	if !strings.Contains(h.errb.String(), "[tier] escalated local -> claude-cli") {
		t.Fatalf("stderr: %s", h.errb.String())
	}
}

func TestEscalationEnvPathAndNoFlagMeansOff(t *testing.T) {
	exe, out := useFakeClaude(t)
	h := newHarness(t, strings.NewReader(""))
	base := h.env.Getenv
	h.env.Getenv = func(k string) string {
		if k == "CLAUDE_CODE_CLI_PATH" {
			return exe
		}
		return base(k)
	}
	// without the flag the env alone enables nothing
	if code := Main(chatArgs(h, deadBackendURL(t), "hi"), h.env); code == 0 {
		t.Fatalf("expected failure with no tier to fall to; out=%s", h.out.String())
	}
	if _, err := os.Stat(out); err == nil {
		t.Fatal("claude ran without --escalate-to-claude-code")
	}
	h2 := newHarness(t, strings.NewReader(""))
	h2.env.Getenv = h.env.Getenv
	if code := Main(chatArgs(h2, deadBackendURL(t), "--escalate-to-claude-code", "hi"), h2.env); code != 0 {
		t.Fatalf("exit %d err=%s", code, h2.errb.String())
	}
	childArgs(t, out)
}

func TestEscalationTriggerRestrictsHandOff(t *testing.T) {
	exe, out := useFakeClaude(t)
	h := newHarness(t, strings.NewReader(""))
	// only resource_exhausted may hand off, so a dead backend must not
	code := Main(chatArgs(h, deadBackendURL(t), "--escalate-to-claude-code", "--claude-code-cli-path", exe,
		"--escalation-trigger", "resource_exhausted", "hi"), h.env)
	if code == 0 {
		t.Fatalf("handed off on backend_error although only resource_exhausted was allowed: %s", h.out.String())
	}
	if _, err := os.Stat(out); err == nil {
		t.Fatal("claude ran")
	}
}

func TestResumeUsesEscalationFlags(t *testing.T) {
	exe, out := useFakeClaude(t)
	local := newTierBackend(t, "from local")
	h := newHarness(t, strings.NewReader(""))
	if code := Main(chatArgs(h, local.URL, "first"), h.env); code != 0 {
		t.Fatalf("chat exit %d: %s", code, h.errb.String())
	}
	files := h.sessionFiles(t)
	if len(files) != 1 {
		t.Fatalf("session files: %v", files)
	}
	id := strings.TrimSuffix(filepath.Base(files[0]), ".json")
	h2 := newHarness(t, strings.NewReader(""))
	h2.env.Home = h.env.Home
	code := Main([]string{"resume", "--base-url", deadBackendURL(t), "--cwd", h.dir, "--disable-claude-md", "--no-auto-memory-review",
		"--escalate-to-claude-code", "--claude-code-cli-path", exe, id, "second"}, h2.env)
	if code != 0 || !strings.Contains(h2.out.String(), "done by claude") {
		t.Fatalf("exit %d out=%s err=%s", code, h2.out.String(), h2.errb.String())
	}
	childArgs(t, out)
}

func TestTierSlashShowsClaudeCLITier(t *testing.T) {
	exe, _ := useFakeClaude(t)
	local := newTierBackend(t, "x")
	h := newHarness(t, strings.NewReader("/tier status\n/exit\n"))
	Main(chatArgs(h, local.URL, "--interactive", "--escalate-to-claude-code", "--claude-code-cli-path", exe), h.env)
	if !strings.Contains(h.out.String(), "2. claude-cli [claude_cli] healthy") || !strings.Contains(h.out.String(), "(found)") {
		t.Fatalf("out: %s", h.out.String())
	}
}

func TestTiersStatusReportsClaudeExecutable(t *testing.T) {
	exe, _ := useFakeClaude(t)
	local := newTierBackend(t, "x")
	run := func(extra ...string) string {
		h := newHarness(t, strings.NewReader(""))
		base := h.env.Getenv
		h.env.Getenv = func(k string) string {
			if k == "OPENAI_BASE_URL" {
				return local.URL
			}
			return base(k)
		}
		if code := Main(append([]string{"tiers", "status"}, extra...), h.env); code != 0 {
			t.Fatalf("exit %d: %s", code, h.errb.String())
		}
		return h.out.String()
	}
	out := run("--escalate-to-claude-code", "--claude-code-cli-path", exe, "--disable-resource-check")
	for _, want := range []string{"claude-cli [claude_cli] healthy", "claude executable " + exe + " (found)", "(no limit)"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	missing := filepath.Join(t.TempDir(), "no-such-claude")
	out = run("--escalate-to-claude-code", "--claude-code-cli-path", missing)
	if !strings.Contains(out, "(not found)") || !strings.Contains(out, "unavailable") {
		t.Errorf("missing executable not reported:\n%s", out)
	}
	if strings.Contains(run(), "claude_cli") {
		t.Error("escalation must be off by default")
	}
}

func TestStoredSessionCannotEnableEscalation(t *testing.T) {
	exe, out := useFakeClaude(t)
	local := newTierBackend(t, "from local")
	h := newHarness(t, strings.NewReader(""))
	if code := Main(chatArgs(h, local.URL, "first"), h.env); code != 0 {
		t.Fatal(h.errb.String())
	}
	files := h.sessionFiles(t)
	raw, _ := os.ReadFile(files[0])
	var doc map[string]any
	json.Unmarshal(raw, &doc)
	rt := doc["runtime_config"].(map[string]any)
	rt["escalation_config"] = map[string]any{"enabled": true, "cli_path": exe}
	b, _ := json.Marshal(doc)
	os.WriteFile(files[0], b, 0o600)
	id := strings.TrimSuffix(filepath.Base(files[0]), ".json")
	h2 := newHarness(t, strings.NewReader(""))
	h2.env.Home = h.env.Home
	Main([]string{"resume", "--base-url", deadBackendURL(t), "--cwd", h.dir, "--disable-claude-md", "--no-auto-memory-review", id, "second"}, h2.env)
	if _, err := os.Stat(out); err == nil {
		t.Fatal("a session file enabled the claude_cli tier")
	}
}
