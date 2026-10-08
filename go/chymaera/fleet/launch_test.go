package fleet

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// The spellings Omarchy uses to start each agent without permission prompts
// (bin/omarchy-agent at v4.0.4). None may ever appear in a proposal.
var autoApproveFlags = []string{
	"--auto", "--yolo", "--allow-all", "--permission-mode", "bypassPermissions",
	"--approve-for-me", "--auto-approve", "--approval-mode", "--trust", "never",
}

func found(name string) string { return "/usr/bin/" + name }
func notFound(string) string   { return "" }

func plan(t *testing.T, agent, prompt string) LaunchProposal {
	t.Helper()
	p, err := PlanLaunch(agent, prompt, t.TempDir(), found)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func TestNoSupportedAgentGetsAnAutoApproveFlag(t *testing.T) {
	for _, agent := range SupportedAgents() {
		argv := plan(t, agent, "fix the build").Argv
		for _, a := range argv {
			for _, bad := range autoApproveFlags {
				if a == bad {
					t.Errorf("%s carries %s", agent, bad)
				}
			}
		}
	}
}

func TestArgumentForms(t *testing.T) {
	cases := map[string][]string{
		"claude":       {"claude", "--", "p"},
		"codex":        {"codex", "--", "p"},
		"opencode":     {"opencode", "--prompt", "p"},
		"gemini":       {"gemini", "--prompt-interactive", "p"},
		"copilot":      {"copilot", "--interactive", "p"},
		"cursor-agent": {"cursor-agent", "agent", "--", "p"},
		"hermes":       {"hermes", "chat", "--tui", "--query=p"},
		"pi":           {"pi", "p"},
	}
	for agent, want := range cases {
		if got := plan(t, agent, "p").Argv; !reflect.DeepEqual(got, want) {
			t.Errorf("%s: %v", agent, got)
		}
	}
}

func TestSupportedAgentList(t *testing.T) {
	want := []string{"claude", "codex", "copilot", "cursor-agent", "gemini", "grok", "hermes", "muse", "omp", "opencode", "pi"}
	if got := SupportedAgents(); !reflect.DeepEqual(got, want) {
		t.Fatal(got)
	}
}

func TestPromptStartingWithDashCannotBecomeAnOption(t *testing.T) {
	for _, agent := range []string{"claude", "codex", "grok", "muse", "omp", "cursor-agent"} {
		argv := plan(t, agent, "--dangerously-skip-permissions").Argv
		if argv[len(argv)-2] != "--" || argv[len(argv)-1] != "--dangerously-skip-permissions" {
			t.Errorf("%s: %v", agent, argv)
		}
	}
}

func TestNoPromptMeansBareInteractiveLaunch(t *testing.T) {
	if got := plan(t, "claude", "").Argv; !reflect.DeepEqual(got, []string{"claude"}) {
		t.Fatal(got)
	}
}

func TestUnsupportedAgentsRefusedWithReason(t *testing.T) {
	for agent, reason := range Unsupported {
		_, err := PlanLaunch(agent, "x", "", nil)
		var le *LaunchError
		if !errors.As(err, &le) || !strings.Contains(err.Error(), reason) {
			t.Errorf("%s: %v", agent, err)
		}
	}
}

func TestUnknownAgentRefused(t *testing.T) {
	if _, err := PlanLaunch("rm", "x", "", nil); err == nil {
		t.Fatal("accepted")
	}
}

func TestPromptLimits(t *testing.T) {
	if _, err := PlanLaunch("claude", strings.Repeat("x", 9000), "", found); err == nil {
		t.Error("long prompt accepted")
	}
	if _, err := PlanLaunch("claude", strings.Repeat("x", 8000), "", found); err != nil {
		t.Error("8000 chars refused:", err)
	}
	// the limit counts characters, not bytes
	if _, err := PlanLaunch("claude", strings.Repeat("é", 8000), "", found); err != nil {
		t.Error("8000 multibyte chars refused:", err)
	}
	if _, err := PlanLaunch("claude", "a\x00b", "", found); err == nil {
		t.Error("NUL accepted")
	}
}

func TestShellMetacharactersStayOneArgument(t *testing.T) {
	prompt := "$(touch /tmp/pwned); `id` | rm -rf ~ && echo \"x\""
	if got := plan(t, "claude", prompt).Argv; !reflect.DeepEqual(got, []string{"claude", "--", prompt}) {
		t.Fatal(got)
	}
}

func TestPiFlaggedAsUnattended(t *testing.T) {
	p := plan(t, "pi", "x")
	ok := false
	for _, w := range p.Warnings {
		ok = ok || strings.Contains(w, "no approval prompts")
	}
	if !ok || strings.Contains(p.AsText(), "the agent will ask") {
		t.Fatalf("%v\n%s", p.Warnings, p.AsText())
	}
}

func TestOthersSayTheAgentWillAsk(t *testing.T) {
	text := plan(t, "claude", "x").AsText()
	if !strings.Contains(text, "the agent will ask before acting") || !strings.Contains(text, "claude -- <prompt>") {
		t.Fatal(text)
	}
}

func TestMissingBinaryReported(t *testing.T) {
	p, _ := PlanLaunch("claude", "x", "", notFound)
	if p.BinaryPath != "" || !strings.Contains(p.AsText(), "not found on PATH") {
		t.Fatal(p.AsText())
	}
}

func TestProposalNeverContainsPromptText(t *testing.T) {
	if strings.Contains(plan(t, "claude", "my secret token abc123").AsText(), "abc123") {
		t.Fatal("prompt leaked into proposal")
	}
}

// ---- Launch ----------------------------------------------------------------

type fakeRun struct {
	calls [][]string
	dirs  []string
}

func (f *fakeRun) run(argv []string, dir string) (int, error) {
	f.calls = append(f.calls, argv)
	f.dirs = append(f.dirs, dir)
	return 7, nil
}

func auditEntries(t *testing.T, path string) []map[string]any {
	t.Helper()
	raw, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var out []map[string]any
	for _, line := range strings.Split(strings.TrimSpace(string(raw)), "\n") {
		var m map[string]any
		if err := json.Unmarshal([]byte(line), &m); err != nil {
			t.Fatalf("bad audit line %q: %v", line, err)
		}
		out = append(out, m)
	}
	return out
}

func logPath(t *testing.T) string { return filepath.Join(t.TempDir(), "log", "agent-launch.jsonl") }

func TestUnconfirmedLaunchRunsNothingAndIsAudited(t *testing.T) {
	log, f := logPath(t), &fakeRun{}
	_, err := Launch(plan(t, "claude", "go"), "go", false, log, f.run)
	var le *LaunchError
	if !errors.As(err, &le) || len(f.calls) != 0 || auditEntries(t, log)[0]["outcome"] != "refused" {
		t.Fatalf("%v %v", err, f.calls)
	}
}

func TestConfirmedLaunchRunsProposedArgvAndReturnsStatus(t *testing.T) {
	log, f := logPath(t), &fakeRun{}
	p := plan(t, "claude", "go")
	status, err := Launch(p, "go", true, log, f.run)
	if err != nil || status != 7 || !reflect.DeepEqual(f.calls, [][]string{{"claude", "--", "go"}}) || f.dirs[0] != p.Cwd {
		t.Fatalf("%d %v %v", status, err, f.calls)
	}
	if auditEntries(t, log)[0]["outcome"] != "launched" {
		t.Fatal("not audited as launched")
	}
}

func TestPromptSwappedAfterProposalRefused(t *testing.T) {
	log, f := logPath(t), &fakeRun{}
	_, err := Launch(plan(t, "claude", "harmless"), "something else", true, log, f.run)
	if err == nil || len(f.calls) != 0 || !strings.Contains(auditEntries(t, log)[0]["detail"].(string), "differs") {
		t.Fatalf("%v %v", err, f.calls)
	}
}

func TestMissingBinaryRefusedEvenWhenConfirmed(t *testing.T) {
	log, f := logPath(t), &fakeRun{}
	p, _ := PlanLaunch("claude", "go", t.TempDir(), notFound)
	if _, err := Launch(p, "go", true, log, f.run); err == nil || len(f.calls) != 0 {
		t.Fatalf("%v %v", err, f.calls)
	}
	if e := auditEntries(t, log)[0]; e["outcome"] != "refused" {
		t.Fatal(e)
	}
}

func TestAuditStoresHashAndShortPreviewNotThePrompt(t *testing.T) {
	log, f := logPath(t), &fakeRun{}
	prompt := "line one\n" + strings.Repeat("x", 500) + "SECRET-TAIL"
	if _, err := Launch(plan(t, "claude", prompt), prompt, true, log, f.run); err != nil {
		t.Fatal(err)
	}
	e := auditEntries(t, log)[0]
	preview := e["promptPreview"].(string)
	raw, _ := os.ReadFile(log)
	if e["promptLength"] != float64(len([]rune(prompt))) || len(e["promptSha256"].(string)) != 64 ||
		len([]rune(preview)) > 60 || strings.Contains(preview, "\n") || strings.Contains(string(raw), "SECRET-TAIL") {
		t.Fatalf("%v", e)
	}
	for _, k := range []string{"time", "agent", "argv0", "cwd", "promptSha256", "promptLength", "promptPreview", "outcome", "detail"} {
		if _, ok := e[k]; !ok {
			t.Errorf("audit entry lacks %s", k)
		}
	}
}

func TestAuditLogAppendsAndIsOneLinePerAttempt(t *testing.T) {
	log, f := logPath(t), &fakeRun{}
	p := plan(t, "claude", "go")
	for i := 0; i < 3; i++ {
		Launch(p, "go", true, log, f.run)
	}
	raw, _ := os.ReadFile(log)
	if len(auditEntries(t, log)) != 3 || strings.Count(string(raw), "\n") != 3 {
		t.Fatalf("%s", raw)
	}
}

func TestAuditPathHonoursXDGStateHome(t *testing.T) {
	if got := AuditLogPath(env(map[string]string{"XDG_STATE_HOME": "/s"}), "/h"); got != filepath.Join("/s", "sarina", "agent-launch.jsonl") {
		t.Fatal(got)
	}
	if got := AuditLogPath(env(nil), "/h"); got != filepath.Join("/h", ".local", "state", "sarina", "agent-launch.jsonl") {
		t.Fatal(got)
	}
}

func TestLaunchDoesNotRunWhenAuditCannotBeWritten(t *testing.T) {
	blocker := filepath.Join(t.TempDir(), "file")
	os.WriteFile(blocker, []byte("x"), 0o644)
	f := &fakeRun{}
	// the log's parent is a regular file, so it can never be created
	if _, err := Launch(plan(t, "claude", "go"), "go", true, filepath.Join(blocker, "log.jsonl"), f.run); err == nil || len(f.calls) != 0 {
		t.Fatalf("launched without an audit trail: %v %v", err, f.calls)
	}
}

// ---- command ---------------------------------------------------------------

type cliRun struct{ stdin string }

func runCLI(t *testing.T, opts *cliRun, args ...string) (stdout, stderr string, code int) {
	t.Helper()
	var out, errb bytes.Buffer
	var in io.Reader = strings.NewReader("")
	if opts != nil {
		in = strings.NewReader(opts.stdin)
	}
	state := t.TempDir()
	code = cli.Main(args, cli.Env{
		Stdin: in, Stdout: &out, Stderr: &errb, Home: state,
		Getenv:     func(k string) string { return map[string]string{"XDG_STATE_HOME": state}[k] },
		Interrupts: make(chan os.Signal),
	})
	return out.String(), errb.String(), code
}

func withSeams(t *testing.T, terminal bool, f *fakeRun) {
	t.Helper()
	oRun, oLook, oTTY := runner, lookPath, isTerminal
	runner, lookPath = f.run, found
	isTerminal = func(io.Reader) bool { return terminal }
	t.Cleanup(func() { runner, lookPath, isTerminal = oRun, oLook, oTTY })
}

func TestLaunchCommandNonTTYWithoutYesIsExit2AndRunsNothing(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	out, _, code := runCLI(t, nil, "fleet-launch", "claude", "hello")
	if code != 2 || len(f.calls) != 0 || !strings.Contains(out, "Proposed launch: claude") || !strings.Contains(out, "Not launching") {
		t.Fatalf("%d %q %v", code, out, f.calls)
	}
}

func TestLaunchCommandYesRunsAndReturnsAgentStatus(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	out, _, code := runCLI(t, nil, "fleet-launch", "claude", "hello", "--yes")
	if code != 7 || !reflect.DeepEqual(f.calls, [][]string{{"claude", "--", "hello"}}) || strings.Contains(out, "Launch? [y/N]") {
		t.Fatalf("%d %q %v", code, out, f.calls)
	}
}

func TestLaunchCommandFlagsMayPrecedeOrFollowPositionals(t *testing.T) {
	for _, args := range [][]string{
		{"fleet-launch", "--yes", "claude", "hello"},
		{"fleet-launch", "claude", "--yes", "hello"},
		{"fleet-launch", "claude", "hello", "--yes"},
	} {
		f := &fakeRun{}
		withSeams(t, false, f)
		if _, _, code := runCLI(t, nil, args...); code != 7 || len(f.calls) != 1 {
			t.Errorf("%v: code %d calls %v", args, code, f.calls)
		}
	}
}

func TestLaunchCommandDashPromptViaDoubleDash(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	_, _, code := runCLI(t, nil, "fleet-launch", "claude", "--yes", "--", "--dangerously-skip-permissions")
	if code != 7 || !reflect.DeepEqual(f.calls, [][]string{{"claude", "--", "--dangerously-skip-permissions"}}) {
		t.Fatalf("%d %v", code, f.calls)
	}
}

func TestLaunchCommandAnswerYesLaunches(t *testing.T) {
	for _, ans := range []string{"y\n", "YES\n", "  y  \n", "y"} {
		f := &fakeRun{}
		withSeams(t, true, f)
		out, _, code := runCLI(t, &cliRun{ans}, "fleet-launch", "claude", "hi")
		if code != 7 || len(f.calls) != 1 || !strings.Contains(out, "Launch? [y/N]") {
			t.Errorf("%q: %d %q", ans, code, out)
		}
	}
}

func TestLaunchCommandAnythingElseCancelsWithExit1AndIsAudited(t *testing.T) {
	for _, ans := range []string{"\n", "n\n", "no\n", "yep\n", "maybe\n"} {
		f := &fakeRun{}
		withSeams(t, true, f)
		var out, errb bytes.Buffer
		state := t.TempDir()
		code := cli.Main([]string{"fleet-launch", "claude", "hi"}, cli.Env{
			Stdin: strings.NewReader(ans), Stdout: &out, Stderr: &errb, Home: state,
			Getenv:     func(k string) string { return map[string]string{"XDG_STATE_HOME": state}[k] },
			Interrupts: make(chan os.Signal),
		})
		if code != 1 || len(f.calls) != 0 || !strings.Contains(out.String(), "Cancelled.") {
			t.Errorf("%q: %d %q %v", ans, code, out.String(), f.calls)
			continue
		}
		e := auditEntries(t, filepath.Join(state, "sarina", "agent-launch.jsonl"))
		if len(e) != 1 || e[0]["outcome"] != "refused" {
			t.Errorf("%q: not audited as refused: %v", ans, e)
		}
	}
}

func TestLaunchCommandEOFAtPromptIsExit2(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, true, f)
	out, _, code := runCLI(t, &cliRun{""}, "fleet-launch", "claude", "hi")
	if code != 2 || len(f.calls) != 0 || !strings.Contains(out, "Not launching") {
		t.Fatalf("%d %q", code, out)
	}
}

func TestLaunchCommandPlanningErrorsAreExit2(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	out, _, code := runCLI(t, nil, "fleet-launch", "crush", "x", "--yes")
	if code != 2 || !strings.HasPrefix(out, "fleet-launch: crush is not supported") || len(f.calls) != 0 {
		t.Fatalf("%d %q", code, out)
	}
	out, _, code = runCLI(t, nil, "fleet-launch", "claude", "a\x00b", "--yes")
	if code != 2 || !strings.Contains(out, "NUL") || len(f.calls) != 0 {
		t.Fatalf("%d %q", code, out)
	}
	out, _, code = runCLI(t, nil, "fleet-launch", "claude", "x", "--yes", "--cwd", filepath.Join(t.TempDir(), "nope"))
	if code != 2 || len(f.calls) != 0 {
		t.Fatalf("%d %q", code, out)
	}
}

func TestLaunchCommandBadArgumentsAreExit2(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	for _, args := range [][]string{
		{"fleet-launch"},
		{"fleet-launch", "rm", "-rf"},
		{"fleet-launch", "rm", "x", "--yes"},
		{"fleet-launch", "claude", "a", "b", "--yes"},
		{"fleet-launch", "claude", "--bogus"},
	} {
		if _, _, code := runCLI(t, nil, args...); code != 2 {
			t.Errorf("%v: exit %d", args, code)
		}
	}
	if len(f.calls) != 0 {
		t.Fatal("something ran", f.calls)
	}
}

func TestLaunchCommandMissingBinaryIsRefusedEvenWithYes(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	lookPath = notFound
	out, _, code := runCLI(t, nil, "fleet-launch", "claude", "x", "--yes")
	if code != 2 || len(f.calls) != 0 || !strings.Contains(out, "not found on PATH") {
		t.Fatalf("%d %q", code, out)
	}
}

func TestLaunchCommandHasNoAutoApproveOption(t *testing.T) {
	f := &fakeRun{}
	withSeams(t, false, f)
	for _, flagName := range []string{"--auto", "--yolo", "--auto-approve", "--allow-all", "--permission-mode", "--approve"} {
		if _, _, code := runCLI(t, nil, "fleet-launch", "claude", "x", flagName); code != 2 {
			t.Errorf("%s accepted (exit %d)", flagName, code)
		}
	}
	if len(f.calls) != 0 {
		t.Fatal("something ran")
	}
}
