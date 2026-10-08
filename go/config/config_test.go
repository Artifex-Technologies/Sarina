package config

import (
	"encoding/json"
	"path/filepath"
	"strings"
	"testing"
)

func env(m map[string]string) func(string) string { return func(k string) string { return m[k] } }

func TestModelFromEnv(t *testing.T) {
	d := ModelFromEnv(env(nil))
	if d.Model != DefaultModel || d.BaseURL != DefaultBaseURL || d.APIKey != "local-token" || d.TimeoutSeconds != 120 || d.Temperature != 0 {
		t.Fatalf("defaults = %+v", d)
	}
	m := ModelFromEnv(env(map[string]string{"OPENAI_MODEL": "m", "OPENAI_BASE_URL": "http://h/v1", "OPENAI_API_KEY": "k"}))
	if m.Model != "m" || m.BaseURL != "http://h/v1" || m.APIKey != "k" {
		t.Fatalf("env = %+v", m)
	}
}

func TestContextWindowFromEnv(t *testing.T) {
	for in, want := range map[string]int{"": 0, "8192": 8192, " 4096 ": 4096, "abc": 0, "-5": 0, "0": 0} {
		if got := ContextWindowFromEnv(env(map[string]string{"SARINA_CONTEXT_WINDOW": in})); got != want {
			t.Errorf("%q -> %d, want %d", in, got, want)
		}
	}
}

func TestModelNeverSerialisesAPIKey(t *testing.T) {
	b, _ := json.Marshal(ModelConfig{Model: "m", APIKey: "hush-hush"})
	if strings.Contains(string(b), "hush") || strings.Contains(string(b), "api_key") {
		t.Fatalf("serialised: %s", b)
	}
}

func TestDefaultRuntimeAnchorsAtWorkspace(t *testing.T) {
	dir := t.TempDir()
	r := DefaultRuntime(dir)
	if r.SessionDirectory != filepath.Join(dir, ".port_sessions", "agent") || r.ScratchpadRoot != filepath.Join(dir, ".port_sessions", "scratchpad") {
		t.Fatalf("dirs = %s %s", r.SessionDirectory, r.ScratchpadRoot)
	}
	if r.MaxTurns != 12 || r.CommandTimeoutSeconds != 30 || r.MaxOutputChars != 12000 || r.CompactPreserveMessages != 4 || r.StreamModelResponses {
		t.Fatalf("defaults = %+v", r)
	}
	if !filepath.IsAbs(DefaultRuntime(".").CWD) {
		t.Fatal("cwd must be absolute")
	}
}

func TestRuntimeJSONShape(t *testing.T) {
	b, err := json.Marshal(DefaultRuntime(t.TempDir()).Normalized())
	if err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	json.Unmarshal(b, &m)
	want := []string{"cwd", "max_turns", "command_timeout_seconds", "max_output_chars", "stream_model_responses",
		"auto_snip_threshold_tokens", "auto_compact_threshold_tokens", "compact_preserve_messages", "permissions",
		"additional_working_directories", "disable_claude_md_discovery", "budget_config", "escalation_config",
		"output_schema", "session_directory", "scratchpad_root"}
	for _, k := range want {
		if _, ok := m[k]; !ok {
			t.Errorf("missing key %s", k)
		}
	}
	if m["additional_working_directories"] == nil || m["auto_snip_threshold_tokens"] != nil {
		t.Fatalf("nil slices must be [] and unset limits null: %s", b)
	}
	bc := m["budget_config"].(map[string]any)
	if len(bc) != 9 {
		t.Fatalf("budget_config = %v", bc)
	}
}

func TestCoercions(t *testing.T) {
	if n, ok := AsInt("12"); !ok || n != 12 {
		t.Fatal("numeric string")
	}
	if n, ok := AsInt(7.9); !ok || n != 7 {
		t.Fatal("float truncation")
	}
	if _, ok := AsInt(true); ok {
		t.Fatal("booleans are not numbers")
	}
	if _, ok := AsInt("x"); ok {
		t.Fatal("garbage string")
	}
	if f, ok := AsFloat("0.5"); !ok || f != 0.5 {
		t.Fatal("float string")
	}
	if len(AsMap("nope")) != 0 {
		t.Fatal("non-object becomes empty")
	}
}

func TestContextSettingsDefaults(t *testing.T) {
	st := (*ContextConfig)(nil).Settings(env(nil))
	if !st.Notes || !st.Clear || !st.LLMCompaction || !st.Discovery ||
		st.NotesEveryTurns != 6 || st.NotesWindowFraction != 0.5 ||
		st.ClearWindowFraction != 0.6 || st.ClearKeepRecent != 4 || st.ClearMinTokens != 500 {
		t.Fatalf("defaults = %+v", st)
	}
}

func TestContextSettingsConfigAndEnvSwitches(t *testing.T) {
	for _, off := range []string{"0", "false", "OFF", " no "} {
		st := (&ContextConfig{}).Settings(env(map[string]string{
			"SARINA_SESSION_NOTES": off, "SARINA_CLEAR_TOOL_OUTPUTS": off, "SARINA_LLM_COMPACTION": off, "SARINA_CONTEXT_DISCOVERY": off}))
		if st.Notes || st.Clear || st.LLMCompaction || st.Discovery {
			t.Errorf("env %q left something on: %+v", off, st)
		}
	}
	if st := (&ContextConfig{}).Settings(env(map[string]string{"SARINA_SESSION_NOTES": "1"})); !st.Notes {
		t.Error("1 must not disable")
	}
	st := (&ContextConfig{DisableNotes: true, DisableClearing: true, DisableLLMCompaction: true, DisableDiscovery: true}).Settings(env(nil))
	if st.Notes || st.Clear || st.LLMCompaction || st.Discovery {
		t.Errorf("config left something on: %+v", st)
	}
}

func TestContextSettingsNumbers(t *testing.T) {
	st := (&ContextConfig{NotesEveryTurns: 2, ClearKeepRecent: 1, ClearMinTokens: 50, NotesWindowPercent: 30, ClearWindowPercent: 80}).
		Settings(env(map[string]string{"SARINA_NOTES_EVERY": "9"}))
	if st.NotesEveryTurns != 2 || st.ClearKeepRecent != 1 || st.ClearMinTokens != 50 || st.NotesWindowFraction != 0.3 || st.ClearWindowFraction != 0.8 {
		t.Fatalf("config must win: %+v", st)
	}
	if st := (&ContextConfig{}).Settings(env(map[string]string{"SARINA_NOTES_EVERY": "9"})); st.NotesEveryTurns != 9 {
		t.Fatalf("env = %d", st.NotesEveryTurns)
	}
	if st := (&ContextConfig{NotesWindowPercent: 500}).Settings(env(map[string]string{"SARINA_NOTES_EVERY": "x"})); st.NotesWindowFraction != 0.5 || st.NotesEveryTurns != 6 {
		t.Fatalf("bad values must fall back: %+v", st)
	}
}

func TestContextConfigRoundTripAndOmission(t *testing.T) {
	r := DefaultRuntime(t.TempDir())
	b, _ := json.Marshal(r)
	if strings.Contains(string(b), "context_management") {
		t.Fatalf("default runtime serialises context_management: %s", b)
	}
	r.ContextManagement = &ContextConfig{DisableNotes: true, NotesEveryTurns: 4}
	b, _ = json.Marshal(r)
	var m map[string]any
	json.Unmarshal(b, &m)
	back, ok := RuntimeFromMap(m)
	if !ok || back.ContextManagement == nil || *back.ContextManagement != *r.ContextManagement {
		t.Fatalf("round trip = %+v", back.ContextManagement)
	}
	m["context_management"] = "junk"
	if back, _ := RuntimeFromMap(m); back.ContextManagement != nil {
		t.Fatal("junk must load as defaults")
	}
}
