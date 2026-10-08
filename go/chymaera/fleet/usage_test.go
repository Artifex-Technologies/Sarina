package fleet

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

var now = time.Date(2026, 9, 26, 15, 0, 0, 0, time.Local)

type sessOpt struct {
	model                                 string
	turns, in, out, cacheRead, cacheWrite int
}

func writeSession(t *testing.T, dir, id string, day time.Time, o sessOpt) {
	t.Helper()
	if o.model == "" {
		o.model = "qwen3-coder"
	}
	if o.turns == 0 {
		o.turns = 4
	}
	body, _ := json.Marshal(map[string]any{
		"session_id":   id,
		"model_config": map[string]any{"model": o.model},
		"turns":        o.turns,
		"usage": map[string]any{
			"input_tokens": o.in, "output_tokens": o.out,
			"cache_read_input_tokens": o.cacheRead, "cache_creation_input_tokens": o.cacheWrite,
		},
	})
	path := filepath.Join(dir, id+".json")
	if err := os.WriteFile(path, body, 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(path, day, day); err != nil {
		t.Fatal(err)
	}
}

func recordMap(t *testing.T, r Record) map[string]any {
	t.Helper()
	text, err := r.JSON()
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	if err := json.Unmarshal([]byte(text), &m); err != nil {
		t.Fatal(err)
	}
	return m
}

func days(r Record) map[string]int {
	m := map[string]int{}
	for _, d := range r.RecentDays {
		m[d.Date] = d.MessageCount
	}
	return m
}

func TestEmptyDirectoryGivesValidZeroRecord(t *testing.T) {
	r := CollectRecord([]string{t.TempDir()}, now)
	if r.ID != "sarina" || r.Name != "Sarina" || r.SchemaVersion != 1 || !r.Ready || r.TotalSessions != 0 {
		t.Fatalf("%+v", r)
	}
	if len(r.RecentDays) != 7 || r.RecentDays[6].Date != "2026-09-26" || r.RecentDays[0].Date != "2026-09-20" {
		t.Fatalf("%+v", r.RecentDays)
	}
}

func TestRecordCarriesEveryKeyThePanelReads(t *testing.T) {
	m := recordMap(t, CollectRecord([]string{t.TempDir()}, now))
	for _, k := range []string{"id", "name", "ready", "hasLocalStats", "todayPrompts", "todaySessions",
		"todayTotalTokens", "todayTokensByModel", "recentDays", "totalPrompts", "totalSessions",
		"activeDays", "activeDates", "modelUsage", "schemaVersion", "updatedAt"} {
		if _, ok := m[k]; !ok {
			t.Errorf("missing key %s", k)
		}
	}
	if dates, ok := m["activeDates"].([]any); !ok || len(dates) != 0 {
		t.Errorf("activeDates must be an empty array, got %#v", m["activeDates"])
	}
	if mu, ok := m["modelUsage"].(map[string]any); !ok || len(mu) != 0 {
		t.Errorf("modelUsage must be an empty object, got %#v", m["modelUsage"])
	}
}

func TestOutputIsCompactSingleLineJSON(t *testing.T) {
	text, _ := CollectRecord([]string{t.TempDir()}, now).JSON()
	if strings.ContainsAny(text, "\n ") || !strings.HasPrefix(text, `{"schemaVersion":1,"id":"sarina","name":"Sarina","updatedAt":`) {
		t.Fatalf("not compact / wrong key order: %s", text)
	}
}

func TestTodayAndEarlierDaysBucketedSeparately(t *testing.T) {
	d := t.TempDir()
	writeSession(t, d, "a", time.Date(2026, 9, 26, 9, 0, 0, 0, time.Local), sessOpt{in: 100, out: 50, turns: 3})
	writeSession(t, d, "b", time.Date(2026, 9, 24, 9, 0, 0, 0, time.Local), sessOpt{in: 200, out: 100, turns: 2})
	r := CollectRecord([]string{d}, now)
	if r.TodayTotalTokens != 150 || r.TodaySessions != 1 || r.TodayPrompts != 3 || r.TotalSessions != 2 || r.TotalPrompts != 5 {
		t.Fatalf("%+v", r)
	}
	by := days(r)
	if by["2026-09-26"] != 150 || by["2026-09-24"] != 300 {
		t.Fatalf("%v", by)
	}
	if r.ActiveDays != 2 || !reflect.DeepEqual(r.ActiveDates, []string{"2026-09-24", "2026-09-26"}) {
		t.Fatalf("%v", r.ActiveDates)
	}
}

func TestSessionsOlderThanAWeekCountOnlyTowardTotals(t *testing.T) {
	d := t.TempDir()
	writeSession(t, d, "old", time.Date(2026, 8, 1, 9, 0, 0, 0, time.Local), sessOpt{in: 999, out: 1})
	r := CollectRecord([]string{d}, now)
	sum := 0
	for _, v := range days(r) {
		sum += v
	}
	if sum != 0 || r.TotalSessions != 1 || r.ModelUsage.Get("qwen3-coder").InputTokens != 999 {
		t.Fatalf("%+v", r)
	}
}

func TestCacheTokensSummedAndKeptPerModel(t *testing.T) {
	d := t.TempDir()
	writeSession(t, d, "c", now, sessOpt{in: 10, out: 20, cacheRead: 30, cacheWrite: 40})
	r := CollectRecord([]string{d}, now)
	u := r.ModelUsage.Get("qwen3-coder")
	if r.TodayTotalTokens != 100 || u.CacheReadInputTokens != 30 || u.CacheCreationInputTokens != 40 {
		t.Fatalf("%+v %+v", r, u)
	}
}

func TestTokensSplitByModel(t *testing.T) {
	d := t.TempDir()
	writeSession(t, d, "a", now, sessOpt{model: "m1", in: 100})
	writeSession(t, d, "b", now, sessOpt{model: "m2", in: 300})
	r := CollectRecord([]string{d}, now)
	if !reflect.DeepEqual(r.TodayTokensByModel.Map(), map[string]int{"m1": 100, "m2": 300}) {
		t.Fatalf("%v", r.TodayTokensByModel.Map())
	}
}

func TestBrokenAndForeignFilesSkipped(t *testing.T) {
	d := t.TempDir()
	os.WriteFile(filepath.Join(d, "garbage.json"), []byte("not json"), 0o644)
	os.WriteFile(filepath.Join(d, "list.json"), []byte("[1, 2]"), 0o644)
	os.WriteFile(filepath.Join(d, "nousage.json"), []byte(`{"session_id":"x"}`), 0o644)
	os.Mkdir(filepath.Join(d, "dir.json"), 0o755)
	writeSession(t, d, "good", now, sessOpt{})
	got := ReadSessions([]string{d})
	if len(got) != 1 || got[0].SessionID != "good" {
		t.Fatalf("%+v", got)
	}
}

func TestHostileTokenValuesBecomeZero(t *testing.T) {
	d := t.TempDir()
	os.WriteFile(filepath.Join(d, "evil.json"), []byte(`{"session_id":"evil","turns":-5,
		"usage":{"input_tokens":-1,"output_tokens":"lots","cache_read_input_tokens":true,"cache_creation_input_tokens":null}}`), 0o644)
	got := ReadSessions([]string{d})
	if len(got) != 1 {
		t.Fatalf("%+v", got)
	}
	s := got[0]
	if s.TotalTokens() != 0 || s.Prompts != 0 || s.Model != "unknown" {
		t.Fatalf("%+v", s)
	}
}

func TestSessionIDFallsBackToFileStem(t *testing.T) {
	d := t.TempDir()
	os.WriteFile(filepath.Join(d, "stem.json"), []byte(`{"usage":{}}`), 0o644)
	got := ReadSessions([]string{d})
	if len(got) != 1 || got[0].SessionID != "stem" {
		t.Fatalf("%+v", got)
	}
}

func TestSameSessionInTwoDirectoriesCountedOnceNewestWins(t *testing.T) {
	d, other := t.TempDir(), t.TempDir()
	writeSession(t, d, "dup", time.Date(2026, 9, 25, 9, 0, 0, 0, time.Local), sessOpt{in: 1})
	writeSession(t, other, "dup", time.Date(2026, 9, 26, 9, 0, 0, 0, time.Local), sessOpt{in: 500})
	r := CollectRecord([]string{d, other}, now)
	if r.TotalSessions != 1 || r.TodayTotalTokens != 500 {
		t.Fatalf("%+v", r)
	}
}

func TestMissingDirectoryIsNotAnError(t *testing.T) {
	if got := ReadSessions([]string{filepath.Join(t.TempDir(), "nope")}); len(got) != 0 {
		t.Fatalf("%+v", got)
	}
}

func env(m map[string]string) func(string) string { return func(k string) string { return m[k] } }

func TestUsageDirectoryHonoursXDGStateHome(t *testing.T) {
	got := UsageDirectory(env(map[string]string{"XDG_STATE_HOME": "/s"}), "/h")
	if got != filepath.Join("/s", "omarchy", "agents", "usage") {
		t.Fatal(got)
	}
}

func TestUsageDirectoryDefaultsUnderHome(t *testing.T) {
	got := UsageDirectory(env(nil), "/h")
	if got != filepath.Join("/h", ".local", "state", "omarchy", "agents", "usage") {
		t.Fatal(got)
	}
}

func TestSessionDirsEnvReplacesDefaults(t *testing.T) {
	v := strings.Join([]string{"/a", "/b"}, string(os.PathListSeparator))
	got := DefaultSessionDirectories(env(map[string]string{SessionDirsEnv: v}), "/work", "/h")
	if !reflect.DeepEqual(got, []string{"/a", "/b"}) {
		t.Fatal(got)
	}
}

func TestDefaultSessionDirsIncludeCwdHomeAndService(t *testing.T) {
	got := DefaultSessionDirectories(env(nil), "/work", "/home/u")
	want := []string{
		filepath.Join("/work", ".port_sessions", "agent"),
		filepath.Join("/home/u", ".port_sessions", "agent"),
		"/var/lib/sarina/sessions",
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatal(got)
	}
}

func TestWriteIsAtomicAndReadableBack(t *testing.T) {
	target := filepath.Join(t.TempDir(), "nested", "usage")
	path, err := WriteRecord(BuildRecord(nil, now), target)
	if err != nil {
		t.Fatal(err)
	}
	if path != filepath.Join(target, "sarina.json") {
		t.Fatal(path)
	}
	raw, _ := os.ReadFile(path)
	var m map[string]any
	if json.Unmarshal(raw, &m) != nil || m["id"] != "sarina" || !strings.HasSuffix(string(raw), "}\n") {
		t.Fatalf("%s", raw)
	}
	entries, _ := os.ReadDir(target)
	if len(entries) != 1 || entries[0].Name() != "sarina.json" {
		t.Fatalf("temp files left behind: %v", entries)
	}
}

func TestSecondWriteReplacesTheFirst(t *testing.T) {
	d := t.TempDir()
	WriteRecord(map[string]any{"id": "sarina", "n": 1}, d)
	WriteRecord(map[string]any{"id": "sarina", "n": 2}, d)
	raw, _ := os.ReadFile(filepath.Join(d, "sarina.json"))
	var m map[string]any
	json.Unmarshal(raw, &m)
	if m["n"] != float64(2) {
		t.Fatalf("%s", raw)
	}
}

func TestCompactJSONEscapesNonASCIILikePython(t *testing.T) {
	s, _ := compactJSON(map[string]string{"k": "é<>&😀"})
	if want := "{\"k\":\"\\u00e9<>&\\ud83d\\ude00\"}"; s != want {
		t.Fatal(s)
	}
}

func TestFleetUsageCommandPrintsOneLineAndWrites(t *testing.T) {
	d, out := t.TempDir(), t.TempDir()
	writeSession(t, d, "a", time.Now(), sessOpt{in: 7})
	stdout, _, code := runCLI(t, nil, "fleet-usage", "--session-dir", d, "--force", "--limits-only")
	if code != 0 || strings.Count(stdout, "\n") != 1 || !strings.Contains(stdout, `"todayTotalTokens":7`) {
		t.Fatalf("%d %q", code, stdout)
	}
	stdout, _, code = runCLI(t, nil, "fleet-usage", "--session-dir", d, "--write", "--usage-dir", out)
	if code != 0 || strings.TrimSpace(stdout) != filepath.Join(out, "sarina.json") {
		t.Fatalf("%d %q", code, stdout)
	}
	if _, err := os.Stat(filepath.Join(out, "sarina.json")); err != nil {
		t.Fatal(err)
	}
}
