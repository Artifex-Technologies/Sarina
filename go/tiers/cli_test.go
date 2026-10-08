package tiers

import (
	"context"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// The fake `claude` executable is this very test binary: TestMain looks at
// FAKE_CLAUDE_MODE before the test framework starts and, when it is set, acts
// as the CLI instead of running tests.
func TestMain(m *testing.M) {
	if mode := os.Getenv("FAKE_CLAUDE_MODE"); mode != "" {
		os.Exit(fakeClaude(mode))
	}
	os.Exit(m.Run())
}

func fakeClaude(mode string) int {
	if out := os.Getenv("FAKE_CLAUDE_OUT"); out != "" {
		dump := map[string]any{"args": os.Args[1:], "env": os.Environ()}
		b, _ := json.Marshal(dump)
		os.WriteFile(out, b, 0o600)
	}
	switch mode {
	case "ok":
		os.Stdout.WriteString(`{"type":"result","is_error":false,"result":"done by claude","session_id":"cli-1","total_cost_usd":0.25}`)
	case "is_error":
		os.Stdout.WriteString(`{"is_error":true,"result":"nope","session_id":"cli-2"}`)
	case "badjson":
		os.Stdout.WriteString("not json at all")
	case "exit1":
		os.Stdout.WriteString(`{"result":"x"}`)
		return 1
	case "tree":
		// a grandchild that keeps a heartbeat file fresh, then hang
		c := exec.Command(os.Args[0])
		c.Env = append(os.Environ(), "FAKE_CLAUDE_MODE=beat")
		c.Start()
		time.Sleep(time.Minute)
	case "beat":
		beat := os.Getenv("FAKE_CLAUDE_BEAT")
		for i := 0; i < 400; i++ {
			os.WriteFile(beat, []byte(time.Now().Format(time.RFC3339Nano)), 0o600)
			time.Sleep(50 * time.Millisecond)
		}
	}
	return 0
}

func cliTier(path string) Tier {
	return Tier{Name: "claude-cli", Kind: KindClaudeCLI, CLIPath: path, TimeoutSeconds: 30, MaxEscalations: 1,
		Triggers:  []string{TriggerBackendError, TriggerResourceExhausted},
		AllowData: AllowData{Classes: []string{ClassNoSecrets, ClassNoCaptures}}, Enabled: true, Auto: true}
}

type cliRig struct {
	*rig
	out  string
	work string
}

// newCLIRig: the first tier at localURL, the second the fake claude run in the given mode.
func newCLIRig(t *testing.T, localURL, mode string, mod func(*Tier), extraEnv ...string) *cliRig {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	ct := cliTier(exe)
	if mod != nil {
		mod(&ct)
	}
	dir := t.TempDir()
	c := &cliRig{out: filepath.Join(dir, "child.json"), work: t.TempDir()}
	g := &rig{clk: newClock(), smp: &fakeSampler{Sample{MemPercent: 20, MemOK: true}}}
	g.cfg = Config{Tiers: []Tier{localTier(localURL), ct}, Resource: Thresholds{MaxMemoryPercent: 90, MaxGPUMemoryPercent: 90}}
	env := append(os.Environ(), "FAKE_CLAUDE_MODE="+mode, "FAKE_CLAUDE_OUT="+c.out,
		"ANTHROPIC_TEST_KEY="+claudeKey, "OPENAI_API_KEY="+localKey, "GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
		"MY_SECRET_THING=hush", "SAFE_VAR=visible", "WRAPPED_VALUE=prefix-"+localKey+"-suffix")
	env = append(env, extraEnv...)
	g.r, err = New(Options{Config: g.cfg,
		Base:    config.ModelConfig{Model: "local-model", BaseURL: localURL, APIKey: localKey, TimeoutSeconds: 5},
		Getenv:  envOf(map[string]string{"ANTHROPIC_TEST_KEY": claudeKey}),
		Sampler: g.smp, Now: g.clk.Now, HealthTimeout: time.Second, WakePoll: 5 * time.Millisecond,
		Workdir: c.work, Environ: func() []string { return env }})
	if err != nil {
		t.Fatal(err)
	}
	c.rig = g
	return c
}

func (c *cliRig) child(t *testing.T) (args, env []string) {
	t.Helper()
	b, err := os.ReadFile(c.out)
	if err != nil {
		t.Fatalf("the fake claude was not run: %v", err)
	}
	var d struct{ Args, Env []string }
	if err := json.Unmarshal(b, &d); err != nil {
		t.Fatal(err)
	}
	return d.Args, d.Env
}

func (c *cliRig) ran() bool { _, err := os.Stat(c.out); return err == nil }

func TestCLIHandedOverWhenLocalUnavailable(t *testing.T) {
	c := newCLIRig(t, deadURL(t), "ok", func(t *Tier) { t.ExtraArgs = []string{"--model", "x"} })
	res, dir := runAgent(t, c.rig, "refactor the parser please")
	if res.StopReason != "stop" || res.FinalOutput != "done by claude" {
		t.Fatalf("stop=%q out=%q", res.StopReason, res.FinalOutput)
	}
	args, _ := c.child(t)
	if args[0] != "-p" || !strings.Contains(args[1], "refactor the parser please") || !strings.Contains(args[1], "unreachable") {
		t.Fatalf("args: %q", args)
	}
	joined := strings.Join(args[2:], " ")
	if !strings.HasPrefix(joined, "--output-format json --cwd "+c.work+" --bare") || !strings.HasSuffix(joined, "--model x") {
		t.Fatalf("flag set: %q", joined)
	}
	if strings.Contains(joined, "bypass") || strings.Contains(joined, "--permission-mode") {
		t.Fatalf("read-only agent must get no permission flag: %q", joined)
	}
	esc := eventsOfType(res, "tier_escalation")
	if len(esc) != 1 {
		t.Fatalf("events: %v", res.Events)
	}
	line, _ := esc[0]["line"].(string)
	if !strings.Contains(line, "local -> claude-cli") || !strings.Contains(line, "claude CLI") || strings.Contains(line, "\n") {
		t.Fatalf("operator line: %q", line)
	}
	rec := tierRecord(t, res.SessionPath)
	if rec["highest_tier"] != "claude-cli" {
		t.Fatalf("record: %v", rec)
	}
	var hist map[string]any
	for _, e := range rec["escalations"].([]any) {
		if m := e.(map[string]any); m["type"] == "tier_cli_result" {
			hist = m
		}
	}
	if hist == nil || hist["ok"] != true || hist["action"] != "escalate_claude_code" || hist["cli_session_id"] != "cli-1" || hist["total_cost_usd"] != 0.25 {
		t.Fatalf("history entry: %v", hist)
	}
	_ = dir
}

func TestCLIHandedOverOnResourcePressure(t *testing.T) {
	local := newBackend(t, false, "local answer")
	c := newCLIRig(t, local.URL, "ok", nil)
	c.smp.s = Sample{MemPercent: 97, MemOK: true}
	turn, err := c.r.Complete(context.Background(), userReq("hello"), nil)
	if err != nil || turn.Content != "done by claude" {
		t.Fatalf("%v %v", turn, err)
	}
	if local.calls() != 0 {
		t.Fatal("the local backend must not be asked under pressure when the CLI can take it")
	}
	args, _ := c.child(t)
	if !strings.Contains(args[1], "memory at 97%") {
		t.Fatalf("the prompt must say why: %q", args[1])
	}
}

func TestCLIRespectsTriggerList(t *testing.T) {
	local := newBackend(t, false, "local answer")
	c := newCLIRig(t, local.URL, "ok", func(t *Tier) { t.Triggers = []string{TriggerBackendError} })
	c.smp.s = Sample{MemPercent: 97, MemOK: true}
	turn, err := c.r.Complete(context.Background(), userReq("hello"), nil)
	if err != nil || turn.Content != "local answer" || c.ran() {
		t.Fatalf("pressure must not trigger a hand-off when resource_exhausted is not a trigger: %v %v", turn, err)
	}
}

func TestCLINotHandedOverWhenDisabled(t *testing.T) {
	c := newCLIRig(t, deadURL(t), "ok", func(t *Tier) { t.Enabled = false })
	_, err := c.r.Complete(context.Background(), userReq("hello"), nil)
	if err == nil || c.ran() {
		t.Fatalf("a disabled claude_cli tier must never run: %v", err)
	}
	// and the original default: escalation not enabled -> no tier is added at all
	cfg := FromEscalation(Default(), config.EscalationConfig{}, EscalationOptions{}, nil)
	if cfg.Multi() {
		t.Fatalf("escalation is off by default: %+v", cfg.Tiers)
	}
}

func TestCLIMissingBinaryIsSkipped(t *testing.T) {
	c := newCLIRig(t, deadURL(t), "ok", func(t *Tier) { t.CLIPath = filepath.Join(t.CLIPath, "..", "no-such-claude") })
	_, err := c.r.Complete(context.Background(), userReq("hello"), nil)
	if err == nil || !strings.Contains(err.Error(), "not found") {
		t.Fatalf("%v", err)
	}
}

func TestCLISecretsRedactedAndCapturesRefused(t *testing.T) {
	c := newCLIRig(t, deadURL(t), "ok", nil)
	const leak = "sk-abcdefghijklmnopqrstuvwxyz0123456789"
	if _, err := c.r.Complete(context.Background(), userReq("use key "+leak+" and "+localKey), nil); err != nil {
		t.Fatal(err)
	}
	args, _ := c.child(t)
	if strings.Contains(args[1], leak) || strings.Contains(args[1], localKey) || !strings.Contains(args[1], "[REDACTED") {
		t.Fatalf("secret reached the child: %q", args[1])
	}
	if got := c.r.Record(nil)["redactions"]; got == nil {
		t.Fatal("redactions must be counted in the record")
	}

	c2 := newCLIRig(t, deadURL(t), "ok", nil)
	_, err := c2.r.Complete(context.Background(), userReq("look at this\nFiletype: Flipper SubGhz RAW File\nRAW_Data: 100 -200 300"), nil)
	if err == nil || c2.ran() || !strings.Contains(err.Error(), "capture") {
		t.Fatalf("capture data must refuse the tier: %v", err)
	}
}

func TestCLIEnvironmentIsScrubbed(t *testing.T) {
	c := newCLIRig(t, deadURL(t), "ok", nil)
	if _, err := c.r.Complete(context.Background(), userReq("hello"), nil); err != nil {
		t.Fatal(err)
	}
	_, env := c.child(t)
	all := strings.Join(env, "\n")
	for _, bad := range []string{"ANTHROPIC_TEST_KEY", "OPENAI_API_KEY", "GITHUB_TOKEN", "MY_SECRET_THING", claudeKey, localKey, "hush"} {
		if strings.Contains(all, bad) {
			t.Fatalf("%q reached the child environment", bad)
		}
	}
	if !strings.Contains(all, "SAFE_VAR=visible") {
		t.Fatal("ordinary variables must still be passed")
	}
}

func TestCLITimeoutKillsProcessTree(t *testing.T) {
	beat := filepath.Join(t.TempDir(), "beat.txt")
	c := newCLIRig(t, deadURL(t), "tree", func(t *Tier) { t.TimeoutSeconds = 1.5 }, "FAKE_CLAUDE_BEAT="+beat)
	start := time.Now()
	_, err := c.r.Complete(context.Background(), userReq("hello"), nil)
	if err == nil || !strings.Contains(err.Error(), "timed out") {
		t.Fatalf("want a timeout error, got %v", err)
	}
	if time.Since(start) > 15*time.Second {
		t.Fatal("the hand-off was not cut off at the timeout")
	}
	if _, err := os.Stat(beat); err != nil {
		t.Fatal("the grandchild never started, so the test proves nothing")
	}
	time.Sleep(400 * time.Millisecond)
	first, _ := os.ReadFile(beat)
	time.Sleep(400 * time.Millisecond)
	second, _ := os.ReadFile(beat)
	if string(first) != string(second) {
		t.Fatal("the grandchild survived the timeout: the process tree was not killed")
	}
}

func TestCLICancelKillsProcessTree(t *testing.T) {
	beat := filepath.Join(t.TempDir(), "beat.txt")
	c := newCLIRig(t, deadURL(t), "tree", nil, "FAKE_CLAUDE_BEAT="+beat)
	ctx, cancel := context.WithCancel(context.Background())
	go func() { time.Sleep(1200 * time.Millisecond); cancel() }()
	_, err := c.r.Complete(ctx, userReq("hello"), nil)
	if err == nil {
		t.Fatal("cancel must fail the hand-off")
	}
	time.Sleep(400 * time.Millisecond)
	first, _ := os.ReadFile(beat)
	time.Sleep(400 * time.Millisecond)
	second, _ := os.ReadFile(beat)
	if len(first) == 0 || string(first) != string(second) {
		t.Fatal("the process tree survived the cancel")
	}
}

func TestCLIFailuresNeverPanicAndAreRecorded(t *testing.T) {
	for _, mode := range []string{"is_error", "badjson", "exit1"} {
		c := newCLIRig(t, deadURL(t), mode, nil)
		_, err := c.r.Complete(context.Background(), userReq("hello"), nil)
		if err == nil {
			t.Fatalf("%s: want an error", mode)
		}
		if _, ok := err.(*CLIError); !ok {
			t.Fatalf("%s: want a CLIError (never retried), got %T", mode, err)
		}
		var found bool
		for _, e := range c.r.Record(nil)["escalations"].([]any) {
			if m := e.(map[string]any); m["type"] == "tier_cli_result" && m["ok"] == false && m["error"] != "" {
				found = true
			}
		}
		if !found {
			t.Fatalf("%s: the failed hand-off must be recorded", mode)
		}
	}
}

func TestCLIEscalationCap(t *testing.T) {
	c := newCLIRig(t, deadURL(t), "ok", nil)
	if _, err := c.r.Complete(context.Background(), userReq("one"), nil); err != nil {
		t.Fatal(err)
	}
	c.r.BeginTurn(nil)
	_, err := c.r.Complete(context.Background(), userReq("two"), nil)
	if err == nil || !strings.Contains(err.Error(), "cap reached") {
		t.Fatalf("a second hand-off must hit max_escalations=1: %v", err)
	}
}

func TestCLIPermissionFlagsNeverBypass(t *testing.T) {
	tr := cliTier("claude")
	if got := strings.Join(tr.Command("p", "/w"), " "); strings.Contains(got, "--permission-mode") {
		t.Fatal(got)
	}
	tr.AcceptEdits, tr.ExtraDirs = true, []string{"/extra"}
	got := tr.Command("p", "/w")
	j := strings.Join(got, " ")
	if !strings.Contains(j, "--add-dir /extra --permission-mode acceptEdits") || strings.Contains(j, "bypass") {
		t.Fatal(j)
	}
}

func TestBuildPromptDigest(t *testing.T) {
	var msgs []llm.Message
	msgs = append(msgs, llm.Message{Role: "system", Content: "sys"})
	msgs = append(msgs, llm.Message{Role: "user", Content: "the real task"})
	for i := 0; i < 12; i++ {
		msgs = append(msgs, llm.Message{Role: "assistant", Content: strings.Repeat("x", 500)})
	}
	msgs = append(msgs, llm.Message{Role: "user", Content: "<system-reminder>boilerplate</system-reminder>"})
	p := BuildPrompt(msgs, "memory at 97%")
	if !strings.Contains(p, "Original task:\nthe real task") || !strings.Contains(p, "Escalation reason: memory at 97%.") {
		t.Fatal(p)
	}
	if strings.Count(p, "[assistant] ") != 7 || !strings.Contains(p, strings.Repeat("x", 400)+"...") || strings.Contains(p, strings.Repeat("x", 401)) {
		t.Fatalf("digest: %q", p)
	}
	if !strings.Contains(BuildPrompt(nil, "r"), "(no prior progress)") {
		t.Fatal("empty digest text")
	}
}

func TestFromEscalationMapsOriginalConfig(t *testing.T) {
	mem, cpu := 80.0, 70.0
	esc := config.EscalationConfig{Enabled: true, CLIPath: "claude", TriggerStopReasons: []string{"backend_error", "budget_exceeded"},
		MaxEscalations: 2, TimeoutSeconds: 60, ExtraCLIArgs: []string{"--model", "m"},
		ResourceThresholds: config.ResourceThresholds{MaxMemoryPercent: &mem, MaxCPUPercent: &cpu}}
	cfg := FromEscalation(Default(), esc, EscalationOptions{AcceptEdits: true, ExtraDirs: []string{"/d"}},
		envOf(map[string]string{"CLAUDE_CODE_CLI_PATH": "/opt/claude"}))
	if len(cfg.Tiers) != 2 {
		t.Fatalf("%+v", cfg.Tiers)
	}
	ct := cfg.Tiers[1]
	if ct.Kind != KindClaudeCLI || ct.CLIPath != "/opt/claude" || ct.MaxEscalations != 2 || ct.TimeoutSeconds != 60 || !ct.Auto ||
		!ct.AcceptEdits || len(ct.Triggers) != 1 || ct.Triggers[0] != TriggerBackendError || strings.Join(ct.ExtraArgs, " ") != "--model m" {
		t.Fatalf("%+v", ct)
	}
	if cfg.Resource.MaxMemoryPercent != 80 || cfg.Resource.MaxCPUPercent != 70 || cfg.Resource.MaxGPUMemoryPercent != 90 {
		t.Fatalf("%+v", cfg.Resource)
	}
	if len(cfg.Warnings) != 1 || !strings.Contains(cfg.Warnings[0], "budget_exceeded") {
		t.Fatalf("%v", cfg.Warnings)
	}
	// the flag form, with the resource check disabled
	off := FromEscalation(Default(), config.EscalationConfig{}, EscalationOptions{Enabled: true, DisableResourceCheck: true, CLIPath: "/x/claude"}, nil)
	if off.Tiers[1].CLIPath != "/x/claude" || off.Resource != (Thresholds{}) || len(off.Tiers[1].Triggers) != 2 {
		t.Fatalf("%+v", off)
	}
}

func TestParseClaudeCLITier(t *testing.T) {
	c, err := Parse([]byte(`{"resource":{"max_cpu_percent":85},"tiers":[{"name":"local"},
		{"name":"cli","kind":"claude_cli","cli_path":"/bin/claude","timeout_seconds":12,"max_escalations":3,
		 "trigger_stop_reasons":["resource_exhausted"],"extra_cli_args":["--x"],"accept_edits":true}]}`))
	if err != nil {
		t.Fatal(err)
	}
	ct := c.Tiers[1]
	if ct.Kind != KindClaudeCLI || ct.CLIPath != "/bin/claude" || ct.TimeoutSeconds != 12 || ct.MaxEscalations != 3 || !ct.AcceptEdits ||
		!ct.AllowData.Has(ClassNoSecrets) || !ct.AllowData.Has(ClassNoCaptures) || !ct.Auto || c.Resource.MaxCPUPercent != 85 {
		t.Fatalf("%+v", ct)
	}
	if _, err := Parse([]byte(`{"tiers":[{"name":"cli","kind":"claude_cli"}]}`)); err == nil {
		t.Fatal("claude_cli as the first tier must be rejected")
	}
}

func TestPressureOrderIncludesCPU(t *testing.T) {
	th := Thresholds{MaxMemoryPercent: 90, MaxCPUPercent: 80, MaxGPUMemoryPercent: 90}
	if r := th.Pressure(Sample{MemOK: true, MemPercent: 10, CPUOK: true, CPUPercent: 85, GPUOK: true, GPUPercent: 95}); !strings.HasPrefix(r, "CPU") {
		t.Fatal(r)
	}
	if r := (Thresholds{MaxMemoryPercent: 90}).Pressure(Sample{CPUOK: true, CPUPercent: 100}); r != "" {
		t.Fatal("an unset CPU threshold never trips")
	}
}
