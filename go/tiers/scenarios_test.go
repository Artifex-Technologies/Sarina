package tiers

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// Scenario 1: local down -> the desktop runs the turn, the operator sees one
// line, and the session file records it. Claude is never contacted.
func TestLocalDownDesktopRuns(t *testing.T) {
	desktop := newBackend(t, false, "answer from desktop")
	claude := newBackend(t, true, "should never be asked")
	ct := claudeTier(claude.URL)
	ct.Enabled = false
	g := newRig(t, localTier(deadURL(t)), desktopTier(desktop.URL), ct)

	res, _ := runAgent(t, g, "hello")
	if res.StopReason != "stop" || res.FinalOutput != "answer from desktop" {
		t.Fatalf("stop=%q out=%q", res.StopReason, res.FinalOutput)
	}
	esc := eventsOfType(res, "tier_escalation")
	if len(esc) != 1 {
		t.Fatalf("want one escalation event, got %v", res.Events)
	}
	line, _ := esc[0]["line"].(string)
	if !strings.Contains(line, "local -> desktop") || !strings.Contains(line, "unreachable") || strings.Contains(line, "\n") {
		t.Fatalf("bad operator line: %q", line)
	}
	if claude.calls() != 0 || claude.healthChecks() != 0 {
		t.Fatal("a disabled Claude tier must not be contacted at all")
	}

	rec := tierRecord(t, res.SessionPath)
	if rec["current_tier"] != "desktop" || rec["highest_tier"] != "desktop" {
		t.Fatalf("record: %v", rec)
	}
	list, _ := rec["escalations"].([]any)
	if len(list) != 1 {
		t.Fatalf("escalations: %v", list)
	}
	first, _ := list[0].(map[string]any)
	if first["from"] != "local" || first["to"] != "desktop" || first["kind"] != KindOpenAICompat {
		t.Fatalf("escalation record: %v", first)
	}
	// the session file must still load with the existing loader
	s, err := session.Load(res.SessionPath)
	if err != nil {
		t.Fatal(err)
	}
	if _, ok := s.PluginState[SessionKey]; !ok {
		t.Fatal("loader dropped the tier record")
	}
}

// Scenario 2: everything down and Claude disabled: a clear failure, nothing sent.
func TestAllDownClaudeDisabledSendsNothing(t *testing.T) {
	claude := newBackend(t, true, "x")
	ct := claudeTier(claude.URL)
	ct.Enabled = false
	g := newRig(t, localTier(deadURL(t)), desktopTier(deadURL(t)), ct)

	res, _ := runAgent(t, g, "please do the thing")
	if res.StopReason != agent.StopBackendError {
		t.Fatalf("stop=%q", res.StopReason)
	}
	for _, want := range []string{"no compute tier could run this request", "local: ", "desktop: ", "claude: disabled in config", "Nothing was sent to any tier"} {
		if !strings.Contains(res.FinalOutput, want) {
			t.Fatalf("message lacks %q: %s", want, res.FinalOutput)
		}
	}
	if claude.calls() != 0 || claude.healthChecks() != 0 {
		t.Fatal("nothing may reach Claude while it is disabled")
	}
}

// Scenario 3: a secret-shaped string is not sent to a tier whose allow_data forbids it.
func TestSecretsRedactedBeforeRestrictedTier(t *testing.T) {
	desktop := newBackend(t, false, "ok")
	g := newRig(t, localTier(deadURL(t)), desktopTier(desktop.URL))
	ghp := "ghp_" + strings.Repeat("a1B2c3D4e5", 4)
	res, _ := runAgent(t, g, "deploy with token "+ghp+" and password=hunter2hunter2 then report")
	if res.StopReason != "stop" {
		t.Fatalf("stop=%q out=%s", res.StopReason, res.FinalOutput)
	}
	body := desktop.lastBody()
	if body == "" {
		t.Fatal("desktop got no request")
	}
	for _, leak := range []string{ghp, "hunter2hunter2"} {
		if strings.Contains(body, leak) {
			t.Fatalf("secret %q reached the restricted tier", leak)
		}
	}
	if !strings.Contains(body, "[REDACTED:") || !strings.Contains(body, "deploy with token") {
		t.Fatalf("expected redaction markers around intact text: %s", body)
	}
	if len(eventsOfType(res, "tier_redaction")) == 0 {
		t.Fatal("the redaction must be visible to the operator")
	}
	rec := tierRecord(t, res.SessionPath)
	reds, _ := rec["redactions"].(map[string]any)
	if n, _ := reds["desktop"].(float64); n < 2 {
		t.Fatalf("redaction count not recorded: %v", rec)
	}
}

func TestCapturesRefuseTierWithNoCaptures(t *testing.T) {
	claude := newBackend(t, true, "x")
	g := newRig(t, localTier(deadURL(t)), claudeTier(claude.URL))
	dump := "Filetype: Flipper SubGhz RAW File\nFrequency: 433920000\nRAW_Data: 350 -120 340 -130\n"
	res, _ := runAgent(t, g, "analyse this\n"+dump)
	if res.StopReason != agent.StopBackendError {
		t.Fatalf("stop=%q", res.StopReason)
	}
	if claude.calls() != 0 {
		t.Fatal("capture data reached Claude")
	}
	ref := eventsOfType(res, "tier_refused")
	if len(ref) != 1 || !strings.Contains(ref[0]["line"].(string), "no_captures") {
		t.Fatalf("refusal not shown with a reason: %v", res.Events)
	}
	rec := tierRecord(t, res.SessionPath)
	list, _ := rec["escalations"].([]any)
	if len(list) == 0 {
		t.Fatalf("refusal not recorded: %v", rec)
	}
}

// Scenario 4: Claude as the last resort, and the key nowhere except the x-api-key header.
func TestClaudeLastResortAndKeyNeverPersisted(t *testing.T) {
	desktop := newBackend(t, false, "x")
	desktop.down.Store(true)
	claude := newBackend(t, true, "answer from claude")
	g := newRig(t, localTier(deadURL(t)), desktopTier(desktop.URL), claudeTier(claude.URL))

	res, dir := runAgent(t, g, "hello there")
	if res.FinalOutput != "answer from claude" || res.StopReason != "stop" {
		t.Fatalf("stop=%q out=%q", res.StopReason, res.FinalOutput)
	}
	if got := claude.headers[0].Get("x-api-key"); got != claudeKey {
		t.Fatalf("x-api-key = %q", got)
	}
	if claude.headers[0].Get("anthropic-version") == "" {
		t.Fatal("missing anthropic-version")
	}
	esc := eventsOfType(res, "tier_escalation")
	if len(esc) != 1 || esc[0]["to"] != "claude" {
		t.Fatalf("events: %v", res.Events)
	}
	// neither key may be in any file or in any event.
	for _, key := range []string{claudeKey, localKey} {
		if hits := filesContain(t, dir, key); len(hits) > 0 {
			t.Fatalf("key written to %v", hits)
		}
		ev, _ := json.Marshal(res.Events)
		tr, _ := json.Marshal(res.Transcript)
		if strings.Contains(string(ev)+string(tr)+res.FinalOutput, key) {
			t.Fatal("key appears in events/transcript")
		}
	}
}

func TestErrorsNeverEchoTheKey(t *testing.T) {
	claude := newBackend(t, true, "x")
	claude.down.Store(true)
	g := newRig(t, localTier(deadURL(t)), claudeTier(claude.URL))
	_, err := g.r.Complete(context.Background(), userReq("hi"), nil)
	if err == nil {
		t.Fatal("expected failure")
	}
	if strings.Contains(err.Error(), claudeKey) || strings.Contains(err.Error(), localKey) {
		t.Fatalf("error leaks a key: %v", err)
	}
}

// Tools and permissions are untouched by escalation.
func TestEscalationDoesNotChangeTools(t *testing.T) {
	local := newBackend(t, false, "from local")
	desktop := newBackend(t, false, "from desktop")
	g := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	runAgent(t, g, "first") // on local; records its tool list
	local.down.Store(true)
	g.clk.Advance(time.Minute)
	res, _ := runAgent(t, g, "second")
	if res.FinalOutput != "from desktop" {
		t.Fatalf("out=%q", res.FinalOutput)
	}
	names := func(body string) []string {
		var m struct {
			Tools []struct {
				Function struct{ Name string } `json:"function"`
			} `json:"tools"`
		}
		json.Unmarshal([]byte(body), &m)
		var out []string
		for _, tl := range m.Tools {
			out = append(out, tl.Function.Name)
		}
		return out
	}
	a, b := names(local.lastBody()), names(desktop.lastBody())
	if len(a) == 0 || strings.Join(a, ",") != strings.Join(b, ",") {
		t.Fatalf("tool lists differ: %v vs %v", a, b)
	}
	s, _ := session.Load(res.SessionPath)
	if s.Runtime.Permissions.AllowFileWrite || s.Runtime.Permissions.AllowShellCommands {
		t.Fatal("permissions changed")
	}
}

// De-escalation: the next turn goes back to the lowest healthy tier.
func TestDeescalationAtNextTurn(t *testing.T) {
	local := newBackend(t, false, "from local")
	desktop := newBackend(t, false, "from desktop")
	g := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	local.down.Store(true)
	g.r.BeginTurn(nil)
	turn, err := g.r.Complete(context.Background(), userReq("one"), nil)
	if err != nil || turn.Content != "from desktop" || g.r.Current() != "desktop" {
		t.Fatalf("turn1: %v %v cur=%s", turn, err, g.r.Current())
	}
	// still inside the same turn: stays up
	turn, _ = g.r.Complete(context.Background(), userReq("two"), nil)
	if turn.Content != "from desktop" {
		t.Fatal("must stay on the reached tier within a turn")
	}
	local.down.Store(false)
	g.clk.Advance(31 * time.Second) // health cache expires
	g.r.BeginTurn(nil)
	turn, err = g.r.Complete(context.Background(), userReq("three"), nil)
	if err != nil || turn.Content != "from local" || g.r.Current() != "local" {
		t.Fatalf("turn3: %v %v cur=%s", turn, err, g.r.Current())
	}
}

func TestHealthCheckCachedThirtySeconds(t *testing.T) {
	local := newBackend(t, false, "ok")
	desktop := newBackend(t, false, "ok")
	g := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	for i := 0; i < 3; i++ {
		g.r.BeginTurn(nil)
		if _, err := g.r.Complete(context.Background(), userReq("x"), nil); err != nil {
			t.Fatal(err)
		}
	}
	if n := local.healthChecks(); n != 1 {
		t.Fatalf("health probed %d times within 30s, want 1", n)
	}
	g.clk.Advance(31 * time.Second)
	g.r.BeginTurn(nil)
	g.r.Complete(context.Background(), userReq("x"), nil)
	if n := local.healthChecks(); n != 2 {
		t.Fatalf("health probed %d times after expiry, want 2", n)
	}
}

func firstEventReason(g *rig) string {
	g.r.mu.Lock()
	defer g.r.mu.Unlock()
	if len(g.r.events) == 0 {
		return ""
	}
	return g.r.events[0]["reason"].(string)
}

func TestRequestTooBigForTierEscalates(t *testing.T) {
	local := newBackend(t, false, "from local")
	desktop := newBackend(t, false, "from desktop")
	lt := localTier(local.URL)
	lt.MaxContext = 200
	g := newRig(t, lt, desktopTier(desktop.URL))
	g.r.BeginTurn(nil)
	turn, err := g.r.Complete(context.Background(), userReq(strings.Repeat("word ", 1000)), nil)
	if err != nil || turn.Content != "from desktop" || local.calls() != 0 {
		t.Fatalf("turn=%v err=%v localCalls=%d", turn, err, local.calls())
	}
	if r := firstEventReason(g); !strings.Contains(r, "does not fit") {
		t.Fatalf("reason: %s", r)
	}
}

func TestRequestFitsNoTierSaysContextWindow(t *testing.T) {
	lt := localTier(newBackend(t, false, "x").URL)
	lt.MaxContext = 100
	dt := desktopTier(newBackend(t, false, "x").URL)
	dt.MaxContext = 100
	g := newRig(t, lt, dt)
	g.r.BeginTurn(nil)
	_, err := g.r.Complete(context.Background(), userReq(strings.Repeat("word ", 1000)), nil)
	if err == nil || !strings.Contains(err.Error(), "context window") || !strings.Contains(err.Error(), "Nothing was sent") {
		t.Fatalf("err = %v", err)
	}
}

func TestResourcePressureEscalatesAndIsAdvisoryOtherwise(t *testing.T) {
	local := newBackend(t, false, "from local")
	desktop := newBackend(t, false, "from desktop")
	g := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	g.smp.s = Sample{MemPercent: 95, MemOK: true}
	g.r.BeginTurn(nil)
	turn, err := g.r.Complete(context.Background(), userReq("x"), nil)
	if err != nil || turn.Content != "from desktop" {
		t.Fatalf("%v %v", turn, err)
	}
	if r := firstEventReason(g); !strings.Contains(r, "memory at 95%") {
		t.Fatalf("reason: %s", r)
	}

	// GPU pressure counts too.
	g2 := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	g2.smp.s = Sample{GPUPercent: 97, GPUOK: true}
	g2.r.BeginTurn(nil)
	turn, _ = g2.r.Complete(context.Background(), userReq("x"), nil)
	if turn.Content != "from desktop" {
		t.Fatal("GPU pressure did not escalate")
	}

	// With nowhere to go the run continues on the pressured tier instead of aborting.
	desktop.down.Store(true)
	g3 := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	g3.smp.s = Sample{MemPercent: 99, MemOK: true}
	g3.r.BeginTurn(nil)
	turn, err = g3.r.Complete(context.Background(), userReq("x"), nil)
	if err != nil || turn.Content != "from local" {
		t.Fatalf("advisory fallback: %v %v", turn, err)
	}
}

func TestRepeatedTimeoutsEscalate(t *testing.T) {
	local := newBackend(t, false, "slow")
	desktop := newBackend(t, false, "from desktop")
	local.delay.Store(int64(400 * time.Millisecond))
	g := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	g.r.opt.Base.TimeoutSeconds = 0.1
	g.r.BeginTurn(nil)
	turn, err := g.r.Complete(context.Background(), userReq("x"), nil)
	if err != nil || turn.Content != "from desktop" {
		t.Fatalf("%v %v", turn, err)
	}
	if local.calls() != 2 {
		t.Fatalf("local was tried %d times, want exactly 2 before escalating", local.calls())
	}
	if r := firstEventReason(g); !strings.Contains(r, "repeated timeouts") {
		t.Fatalf("reason: %v", r)
	}
}

func TestOperatorTierUpAndDown(t *testing.T) {
	local := newBackend(t, false, "from local")
	desktop := newBackend(t, false, "from desktop")
	g := newRig(t, localTier(local.URL), desktopTier(desktop.URL))
	g.r.BeginTurn(nil)
	g.r.Complete(context.Background(), userReq("x"), nil)

	name, err := g.r.RequestUp()
	if err != nil || name != "desktop" {
		t.Fatalf("%s %v", name, err)
	}
	var lines []string
	g.r.BeginTurn(func(e map[string]any) { lines = append(lines, e["line"].(string)) })
	turn, _ := g.r.Complete(context.Background(), userReq("y"), nil)
	if turn.Content != "from desktop" || len(lines) != 1 || !strings.Contains(lines[0], "operator ran /tier up") {
		t.Fatalf("turn=%v lines=%v", turn, lines)
	}
	// only one turn: the next goes back down
	g.r.BeginTurn(nil)
	turn, _ = g.r.Complete(context.Background(), userReq("z"), nil)
	if turn.Content != "from local" {
		t.Fatalf("did not de-escalate: %s", turn.Content)
	}
	if _, err := g.r.RequestUp(); err != nil {
		t.Fatal(err)
	}
	if _, err := g.r.RequestUp(); err == nil {
		t.Fatal("going above the top tier must fail")
	}
	if g.r.RequestDown() != "local" {
		t.Fatal("down")
	}
	g.r.BeginTurn(nil)
	turn, _ = g.r.Complete(context.Background(), userReq("w"), nil)
	if turn.Content != "from local" {
		t.Fatal("/tier down did not cancel /tier up")
	}
}

func TestAutoFalseNeedsConfirmation(t *testing.T) {
	mk := func(confirm func(string) bool) (*rig, *fakeBackend) {
		claude := newBackend(t, true, "from claude")
		ct := claudeTier(claude.URL)
		ct.Auto = false
		g := newRig(t, localTier(deadURL(t)), ct)
		g.r.SetConfirm(confirm)
		return g, claude
	}
	// nobody to ask: refused, nothing sent
	g, claude := mk(nil)
	g.r.BeginTurn(nil)
	if _, err := g.r.Complete(context.Background(), userReq("x"), nil); err == nil || claude.calls() != 0 {
		t.Fatalf("err=%v calls=%d", err, claude.calls())
	}
	// declined: remembered for the session, asked once
	asked := 0
	g, claude = mk(func(string) bool { asked++; return false })
	for i := 0; i < 3; i++ {
		g.r.BeginTurn(nil)
		g.r.Complete(context.Background(), userReq("x"), nil)
	}
	if asked != 1 || claude.calls() != 0 {
		t.Fatalf("asked=%d calls=%d", asked, claude.calls())
	}
	// approved: asked once, used
	asked = 0
	g, claude = mk(func(string) bool { asked++; return true })
	for i := 0; i < 2; i++ {
		g.r.BeginTurn(nil)
		if _, err := g.r.Complete(context.Background(), userReq("x"), nil); err != nil {
			t.Fatal(err)
		}
	}
	if asked != 1 || claude.calls() != 2 {
		t.Fatalf("asked=%d calls=%d", asked, claude.calls())
	}
}

func TestWakeRunsOnlyWhenConfiguredAndDown(t *testing.T) {
	desktop := newBackend(t, false, "from desktop")
	desktop.down.Store(true)
	dt := desktopTier(desktop.URL)
	dt.Wake = "wake-the-desktop"
	dt.WakeWait = 2
	g := newRig(t, localTier(deadURL(t)), dt)
	woken := 0
	g.r.opt.Wake = func(ctx context.Context, tr Tier) error {
		woken++
		if tr.Wake != "wake-the-desktop" {
			t.Errorf("wake got %q", tr.Wake)
		}
		desktop.down.Store(false)
		return nil
	}
	var lines []string
	g.r.BeginTurn(func(e map[string]any) { lines = append(lines, e["line"].(string)) })
	turn, err := g.r.Complete(context.Background(), userReq("x"), nil)
	if err != nil || turn.Content != "from desktop" || woken != 1 {
		t.Fatalf("turn=%v err=%v woken=%d", turn, err, woken)
	}
	if !strings.Contains(strings.Join(lines, "\n"), "wake sent to desktop") {
		t.Fatalf("wake not shown: %v", lines)
	}

	// no wake entry: the runner is never called
	d2 := newBackend(t, false, "x")
	d2.down.Store(true)
	g2 := newRig(t, localTier(deadURL(t)), desktopTier(d2.URL))
	g2.r.opt.Wake = func(context.Context, Tier) error { t.Error("wake ran without being configured"); return nil }
	g2.r.BeginTurn(nil)
	g2.r.Complete(context.Background(), userReq("x"), nil)
}

func TestRecordKeepsFileIdenticalWhenNothingHappened(t *testing.T) {
	g := newRig(t, localTier(deadURL(t)), desktopTier(deadURL(t)))
	if g.r.Record(nil) != nil {
		t.Fatal("a session that never escalated must record nothing")
	}
}

func TestRecordMergesAcrossResume(t *testing.T) {
	desktop := newBackend(t, false, "ok")
	g := newRig(t, localTier(deadURL(t)), desktopTier(desktop.URL))
	g.r.BeginTurn(nil)
	g.r.Complete(context.Background(), userReq("x"), nil)
	rec1 := g.r.Record(nil)
	b, _ := json.Marshal(rec1) // as the loader would see it
	var prev any
	json.Unmarshal(b, &prev)

	g.clk.Advance(time.Minute)
	g.r.BeginTurn(nil)
	g.r.Complete(context.Background(), userReq("y"), nil)
	rec2 := g.r.Record(prev)
	if n := len(rec2["escalations"].([]any)); n != 2 {
		t.Fatalf("want 2 recorded escalations after resume, got %d", n)
	}
}

// the request given to the router is never modified by the guard.
func TestSanitizeDoesNotMutateCaller(t *testing.T) {
	req := userReq("key sk-ant-api03-ABCDEFGHIJKLMNOP1234")
	out, rep, err := Sanitize(req, AllowData{Classes: []string{ClassNoSecrets}}, nil)
	if err != nil || rep.Redactions == 0 {
		t.Fatal(rep, err)
	}
	if !strings.Contains(req.Messages[1].Content, "sk-ant-api03") || strings.Contains(out.Messages[1].Content, "sk-ant-api03") {
		t.Fatal("guard must copy, not edit in place")
	}
}
