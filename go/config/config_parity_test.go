package config

import (
	"encoding/json"
	"strconv"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// envText are values a person may put in sarina.env.
var envText = []string{"", "0", "1", "-1", " 7 ", "+3", "08", "1e3", "2.5", "0.6", "2", "2.0000001", "-0", "NaN", "inf",
	"x", "off", "OFF", " no ", "false", "False", "yes", "9223372036854775807", "9223372036854775808", "0x10", "1_0", "\u00a07"}

// TestParityConfig records go/config (testdata/parity/config.json): ModelFromEnv, the integer settings, ContextConfig's
// Settings against the environment, AsInt and AsFloat, the JSON of a default runtime and a model.
func TestParityConfig(t *testing.T) {
	var cases []parity.Case
	envs := []map[string]string{{}, {"OPENAI_MODEL": "m", "OPENAI_BASE_URL": "http://x/v1", "OPENAI_API_KEY": "k"},
		{"SARINA_FALLBACK_MODELS": " a, ,b,a,Qwen/Qwen3-Coder-30B-A3B-Instruct,c,d,e,f"}, {"OPENAI_MODEL": "a", "SARINA_FALLBACK_MODELS": "a,b"}}
	for _, v := range envText {
		envs = append(envs, map[string]string{"SARINA_TEMPERATURE": v})
	}
	for _, env := range envs {
		m := ModelFromEnv(func(k string) string { return env[k] })
		b, err := json.Marshal(m)
		out := map[string]any{"json": string(b), "key": m.APIKey}
		if err != nil {
			out["json"] = err.Error()
		}
		cases = append(cases, parity.Case{Fn: "ModelFromEnv", In: env, Out: out})
	}
	for _, v := range envText {
		get := func(string) string { return v }
		cases = append(cases, parity.Case{Fn: "intEnv", In: v, Out: []int{RequestMaxTokensFromEnv(get), ContextWindowFromEnv(get)}})
	}
	configs := []*ContextConfig{nil, {}, {DisableNotes: true}, {NotesEveryTurns: 3, NotesWindowPercent: 101, ClearWindowPercent: 100},
		{NotesWindowPercent: -1, ClearWindowPercent: 1, ClearKeepRecent: 2, ClearMinTokens: 9}, {DisableClearing: true, DisableLLMCompaction: true, DisableDiscovery: true}}
	settingEnvs := []map[string]string{{}, {"SARINA_SESSION_NOTES": "0", "SARINA_NOTES_EVERY": "9"}, {"SARINA_CLEAR_TOOL_OUTPUTS": " Off ",
		"SARINA_LLM_COMPACTION": "no", "SARINA_CONTEXT_DISCOVERY": "FALSE"}, {"SARINA_NOTES_EVERY": "-2", "SARINA_SESSION_NOTES": "1"}, {"": "5"}}
	for _, c := range configs {
		for _, env := range settingEnvs {
			s := c.Settings(func(k string) string { return env[k] })
			var in any
			if c != nil {
				b, _ := json.Marshal(c)
				in = string(b)
			}
			cases = append(cases, parity.Case{Fn: "Settings", In: []any{in, env}, Out: []any{s.Notes, s.NotesEveryTurns,
				s.NotesWindowFraction, s.Clear, s.ClearWindowFraction, s.ClearKeepRecent, s.ClearMinTokens, s.LLMCompaction, s.Discovery, c.IsZero()}})
		}
	}
	values := []any{nil, true, false, 0.0, -0.5, 2.9, -2.9, 1e300, -1e300, 9.223372036854775807e18, -9.223372036854775808e18,
		4.5e15, 7, int64(8), []any{}, map[string]any{}}
	for _, v := range envText {
		values = append(values, v)
	}
	for _, v := range values {
		n, ok := AsInt(v)
		f, fok := AsFloat(v)
		fj, err := json.Marshal(f)
		if err != nil {
			fj = []byte(err.Error())
		}
		typed := parity.Typed(v)
		if i, isInt64 := v.(int64); isInt64 {
			typed = map[string]any{"i": strconv.FormatInt(i, 10)}
		}
		cases = append(cases, parity.Case{Fn: "As", In: typed, Out: []any{n, ok, string(fj), fok}})
	}
	for _, cwd := range []string{"/w", "/a/../b/", "/", "/x y/é<&>"} {
		b, err := json.Marshal(DefaultRuntime(cwd).Normalized())
		if err != nil {
			t.Fatal(err)
		}
		cases = append(cases, parity.Case{Fn: "DefaultRuntime", In: cwd, Out: string(b)})
	}
	parity.Fixture(t, "config", cases)
}
