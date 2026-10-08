package fleet

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

var snow = time.Date(2026, 9, 26, 18, 0, 0, 0, time.UTC)
var fresh = snow.Add(-5 * time.Minute).Format(time.RFC3339)

func rec(id string, over map[string]any) map[string]any {
	base := map[string]any{
		"schemaVersion":    float64(1),
		"id":               id,
		"name":             strings.ToUpper(id[:1]) + id[1:],
		"updatedAt":        fresh,
		"ready":            true,
		"todayTotalTokens": float64(1200),
		"totalSessions":    float64(4),
		"recentDays": []any{
			map[string]any{"date": "2026-09-25", "messageCount": float64(800)},
			map[string]any{"date": "2026-09-26", "messageCount": float64(1200)},
		},
		"limits": []any{
			map[string]any{"label": "Session (5-hour)", "percent": float64(30), "resetsAt": "2026-09-26T20:00:00Z"},
			map[string]any{"label": "Weekly (7-day)", "percent": float64(55), "resetsAt": "2026-09-30T00:00:00Z"},
		},
	}
	for k, v := range over {
		base[k] = v
	}
	return base
}

func parse(t *testing.T, id string, over map[string]any) AgentStatus {
	t.Helper()
	s := ParseRecord(rec(id, over), snow, 0)
	if s == nil {
		t.Fatal("nil status")
	}
	return *s
}

func TestRealShapedRecordParses(t *testing.T) {
	s := parse(t, "claude", nil)
	if s.AgentID != "claude" || s.TodayTokens != 1200 || s.WeekTokens != 2000 || len(s.Limits) != 2 || s.Stale {
		t.Fatalf("%+v", s)
	}
}

func TestHeadroomIsTightestWindow(t *testing.T) {
	if h := parse(t, "claude", nil).HeadroomPercent(); h == nil || *h != 45 {
		t.Fatal(h)
	}
}

func TestNoLimitsMeansUnknownHeadroomNotUnlimited(t *testing.T) {
	if h := parse(t, "local", map[string]any{"limits": []any{}}).HeadroomPercent(); h != nil {
		t.Fatal(*h)
	}
}

func TestOldRecordIsStale(t *testing.T) {
	old := snow.Add(-3 * time.Hour).Format(time.RFC3339)
	if !parse(t, "claude", map[string]any{"updatedAt": old}).Stale {
		t.Fatal("not stale")
	}
}

func TestMissingOrBadTimestampIsStale(t *testing.T) {
	for _, v := range []any{nil, "yesterday", float64(12345)} {
		if !parse(t, "claude", map[string]any{"updatedAt": v}).Stale {
			t.Errorf("%v not stale", v)
		}
	}
}

func TestTimestampFormats(t *testing.T) {
	for _, v := range []string{
		snow.Add(-time.Minute).Format("2006-01-02T15:04:05Z"),
		snow.Add(-time.Minute).Format("2006-01-02T15:04:05.000000+00:00"),
		snow.Add(-time.Minute).Format("2006-01-02T15:04:05"), // naive = UTC
	} {
		if parse(t, "claude", map[string]any{"updatedAt": v}).Stale {
			t.Errorf("%q should be fresh", v)
		}
	}
}

func TestRejectsNonRecordsAndBadIDs(t *testing.T) {
	if ParseRecord([]any{}, snow, 0) != nil || ParseRecord("x", snow, 0) != nil || ParseRecord(nil, snow, 0) != nil {
		t.Fatal("non-record accepted")
	}
	for _, bad := range []any{nil, "", "Claude", "../etc", "a b", strings.Repeat("x", 40), "-lead", float64(5)} {
		if ParseRecord(rec("ok", map[string]any{"id": bad}), snow, 0) != nil {
			t.Errorf("accepted id %#v", bad)
		}
	}
	for _, good := range []string{"a", "claude", "cursor-agent", "x_y", strings.Repeat("x", 32)} {
		if ParseRecord(rec("ok", map[string]any{"id": good}), snow, 0) == nil {
			t.Errorf("rejected id %q", good)
		}
	}
}

func TestControlCharactersStrippedFromText(t *testing.T) {
	s := parse(t, "claude", map[string]any{
		"name":            "Cl\x1b[31maude\x07‮",
		"usageStatusText": "ok\x00\nline2",
	})
	if s.Name != "Cl[31maude" || strings.ContainsAny(s.Name, "\x1b\x07") || strings.Contains(s.StatusText, "\n") {
		t.Fatalf("%q %q", s.Name, s.StatusText)
	}
}

func TestBidiAndZeroWidthStripped(t *testing.T) {
	s := parse(t, "claude", map[string]any{"name": "a​b‏c‪d⁦e⁩f\u0085g\u009fh"})
	if s.Name != "abcdefgh" {
		t.Fatalf("%q", s.Name)
	}
}

func TestTextIsTruncated(t *testing.T) {
	if n := len([]rune(parse(t, "claude", map[string]any{"name": strings.Repeat("N", 500)}).Name)); n != 80 {
		t.Fatal(n)
	}
}

func TestMissingNameFallsBackToID(t *testing.T) {
	if s := parse(t, "claude", map[string]any{"name": "\x1b\x07"}); s.Name != "claude" {
		t.Fatal(s.Name)
	}
}

func TestHostileNumbersNeutralised(t *testing.T) {
	s := parse(t, "claude", map[string]any{
		"todayTotalTokens": math.Inf(1),
		"totalSessions":    float64(-3),
		"recentDays": []any{
			map[string]any{"messageCount": "lots"}, map[string]any{"messageCount": true}, "junk",
			map[string]any{"messageCount": float64(5)},
		},
		"limits": []any{
			map[string]any{"label": "x", "percent": float64(9999)},
			map[string]any{"label": "y", "percent": "high"},
			"junk",
		},
	})
	if s.TodayTokens != 0 || s.TotalSessions != 0 || s.WeekTokens != 5 {
		t.Fatalf("%+v", s)
	}
	if len(s.Limits) != 2 || s.Limits[0].PercentUsed == nil || *s.Limits[0].PercentUsed != 100 || s.Limits[1].PercentUsed != nil {
		t.Fatalf("%+v", s.Limits)
	}
}

func TestNaNAndBooleanPercentsAreUnknown(t *testing.T) {
	s := parse(t, "claude", map[string]any{"limits": []any{
		map[string]any{"label": "a", "percent": math.NaN()},
		map[string]any{"label": "b", "percent": true},
		map[string]any{"label": "c", "percent": float64(-4)},
		map[string]any{"percent": float64(1)},
	}})
	if s.Limits[0].PercentUsed != nil || s.Limits[1].PercentUsed != nil || *s.Limits[2].PercentUsed != 0 || s.Limits[3].Label != "limit" {
		t.Fatalf("%+v", s.Limits)
	}
}

func TestReadyMustBeTrueBoolean(t *testing.T) {
	for _, v := range []any{"true", float64(1), nil} {
		if parse(t, "claude", map[string]any{"ready": v}).Ready {
			t.Errorf("%#v read as ready", v)
		}
	}
}

func TestLimitListIsCapped(t *testing.T) {
	var many []any
	for i := 0; i < 500; i++ {
		many = append(many, map[string]any{"label": "l", "percent": float64(1)})
	}
	if n := len(parse(t, "claude", map[string]any{"limits": many}).Limits); n != 16 {
		t.Fatal(n)
	}
}

func put(t *testing.T, dir, name string, data any) {
	t.Helper()
	text, ok := data.(string)
	if !ok {
		b, _ := json.Marshal(data)
		text = string(b)
	}
	if err := os.WriteFile(filepath.Join(dir, name), []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestReadsSortedAndSkipsJunk(t *testing.T) {
	d := t.TempDir()
	put(t, d, "codex.json", rec("codex", nil))
	put(t, d, "claude.json", rec("claude", nil))
	put(t, d, "broken.json", "{nope")
	put(t, d, "list.json", []int{1})
	put(t, d, "notes.txt", "ignored")
	got := ReadAgentStatus(d, snow, 0)
	if len(got) != 2 || got[0].AgentID != "claude" || got[1].AgentID != "codex" {
		t.Fatalf("%+v", got)
	}
}

func TestOversizedFileSkipped(t *testing.T) {
	d := t.TempDir()
	put(t, d, "big.json", strings.Repeat(" ", MaxRecordBytes+1))
	if got := ReadAgentStatus(d, snow, 0); len(got) != 0 {
		t.Fatal(got)
	}
}

func TestMissingStatusDirectoryIsEmpty(t *testing.T) {
	if got := ReadAgentStatus(filepath.Join(t.TempDir(), "nope"), snow, 0); len(got) != 0 {
		t.Fatal(got)
	}
}

func suggestStatus(t *testing.T, id string, pct *float64, over map[string]any) AgentStatus {
	t.Helper()
	limits := []any{}
	if pct != nil {
		limits = append(limits, map[string]any{"label": "w", "percent": *pct})
	}
	o := map[string]any{"limits": limits}
	for k, v := range over {
		o[k] = v
	}
	return parse(t, id, o)
}

func fp(f float64) *float64 { return &f }

func TestSuggestPicksMostHeadroom(t *testing.T) {
	pick := SuggestAgent([]AgentStatus{suggestStatus(t, "claude", fp(80), nil), suggestStatus(t, "codex", fp(20), nil)}, nil)
	if pick == nil || pick.AgentID != "codex" {
		t.Fatal(pick)
	}
}

func TestSuggestTieGoesToFewerTokensToday(t *testing.T) {
	pick := SuggestAgent([]AgentStatus{
		suggestStatus(t, "claude", fp(50), map[string]any{"todayTotalTokens": float64(9000)}),
		suggestStatus(t, "codex", fp(50), map[string]any{"todayTotalTokens": float64(100)}),
	}, nil)
	if pick == nil || pick.AgentID != "codex" {
		t.Fatal(pick)
	}
}

func TestSuggestIgnoresStaleUnreadyUnmeasuredAndSarina(t *testing.T) {
	old := snow.Add(-5 * time.Hour).Format(time.RFC3339)
	pick := SuggestAgent([]AgentStatus{
		suggestStatus(t, "stale", fp(0), map[string]any{"updatedAt": old}),
		suggestStatus(t, "unready", fp(0), map[string]any{"ready": false}),
		suggestStatus(t, "unknown", nil, nil),
		suggestStatus(t, "sarina", fp(0), nil),
	}, nil)
	if pick != nil {
		t.Fatal(pick)
	}
}

func TestNoAgentsNoSuggestion(t *testing.T) {
	if SuggestAgent(nil, nil) != nil {
		t.Fatal("suggested from nothing")
	}
}

func TestRenderEmpty(t *testing.T) {
	if RenderStatus(nil) != "No agent usage records found." {
		t.Fatal(RenderStatus(nil))
	}
}

func TestRenderShowsLimitsAndFlags(t *testing.T) {
	old := snow.Add(-5 * time.Hour).Format(time.RFC3339)
	text := RenderStatus([]AgentStatus{
		parse(t, "claude", nil),
		parse(t, "codex", map[string]any{"updatedAt": old, "limits": []any{}}),
	})
	for _, want := range []string{"Claude (claude): 1.2k today", "Session (5-hour): 30% used, resets 2026-09-26T20:00:00Z", "[stale]", "no limit data"} {
		if !strings.Contains(text, want) {
			t.Errorf("missing %q in:\n%s", want, text)
		}
	}
}

func TestRenderStripsEscapesEndToEnd(t *testing.T) {
	text := RenderStatus([]AgentStatus{parse(t, "claude", map[string]any{"name": "\x1b]0;pwned\x07Evil", "usageStatusText": "\x1b[2J"})})
	if strings.ContainsRune(text, 0x1b) || strings.ContainsRune(text, 7) {
		t.Fatalf("escape reached output: %q", text)
	}
}

func TestFleetStatusCommand(t *testing.T) {
	d := t.TempDir()
	put(t, d, "claude.json", rec("claude", map[string]any{"updatedAt": time.Now().UTC().Format(time.RFC3339)}))
	stdout, _, code := runCLI(t, nil, "fleet-status", "--usage-dir", d, "--suggest")
	if code != 0 || !strings.Contains(stdout, "Claude (claude)") || !strings.Contains(stdout, "\n\nMost headroom: Claude (45% left in its tightest window)") {
		t.Fatalf("%d %q", code, stdout)
	}
	stdout, _, code = runCLI(t, nil, "fleet-status", "--usage-dir", t.TempDir(), "--suggest")
	if code != 0 || !strings.HasPrefix(stdout, "No agent usage records found.\n\n") {
		t.Fatalf("%d %q", code, stdout)
	}
}
