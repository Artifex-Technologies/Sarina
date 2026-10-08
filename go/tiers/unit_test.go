package tiers

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

func TestDefaultConfigIsOneLocalTier(t *testing.T) {
	home := t.TempDir()
	c, err := Load(home, envOf(nil))
	if err != nil || len(c.Tiers) != 1 || c.Multi() || !c.Tiers[0].AllowData.All || c.Path != "" {
		t.Fatalf("%+v %v", c, err)
	}
}

func TestLoadFromHomeAndEnvOverride(t *testing.T) {
	home := t.TempDir()
	if err := os.MkdirAll(filepath.Join(home, ".sarina"), 0o755); err != nil {
		t.Fatal(err)
	}
	doc := `{"tiers":[{"name":"local"},{"name":"desktop","base_url":"http://10.0.0.5:8080/v1","model":"big","max_context":32768,"wake":"wakeonlan aa:bb"}]}`
	os.WriteFile(filepath.Join(home, ".sarina", "tiers.json"), []byte(doc), 0o600)
	c, err := Load(home, envOf(nil))
	if err != nil || !c.Multi() || c.Tiers[1].MaxContext != 32768 || c.Tiers[1].Wake == "" {
		t.Fatalf("%+v %v", c, err)
	}
	// the env override wins and a missing override file means the default, not ~/.sarina
	c, err = Load(home, envOf(map[string]string{"SARINA_TIERS_FILE": filepath.Join(home, "nope.json")}))
	if err != nil || c.Multi() {
		t.Fatalf("%+v %v", c, err)
	}
	other := filepath.Join(home, "other.json")
	os.WriteFile(other, []byte(`{"tiers":[{"name":"a"},{"name":"b","base_url":"http://x/v1"}]}`), 0o600)
	c, _ = Load(home, envOf(map[string]string{"SARINA_TIERS_FILE": other}))
	if c.Tiers[1].Name != "b" || c.Path != other {
		t.Fatalf("%+v", c)
	}
}

func TestAllowDataDefaultsAndParsing(t *testing.T) {
	c, err := Parse([]byte(`{"tiers":[
		{"name":"local"},
		{"name":"desk","base_url":"http://x/v1"},
		{"name":"open","base_url":"http://y/v1","allow_data":"all"},
		{"name":"cl","kind":"claude","model":"m","api_key_env":"K"},
		{"name":"odd","base_url":"http://z/v1","allow_data":["no_captures","bogus"]}]}`))
	if err != nil {
		t.Fatal(err)
	}
	if !c.Tiers[0].AllowData.All {
		t.Error("first tier defaults to all")
	}
	if !c.Tiers[1].AllowData.Has(ClassNoSecrets) || c.Tiers[1].AllowData.Has(ClassNoCaptures) {
		t.Errorf("desk: %v", c.Tiers[1].AllowData)
	}
	if !c.Tiers[2].AllowData.All {
		t.Error("explicit all")
	}
	if !c.Tiers[3].AllowData.Has(ClassNoSecrets) || !c.Tiers[3].AllowData.Has(ClassNoCaptures) {
		t.Errorf("claude default: %v", c.Tiers[3].AllowData)
	}
	if !c.Tiers[4].AllowData.Has(ClassNoCaptures) || len(c.Warnings) != 1 {
		t.Errorf("odd: %v %v", c.Tiers[4].AllowData, c.Warnings)
	}
}

func TestInvalidConfigIsRejectedAndFallsBackToLocalOnly(t *testing.T) {
	bad := []string{
		`{`,
		`{"tiers":[{"name":"a"},{"name":"a","base_url":"http://x"}]}`,
		`{"tiers":[{"name":"a"},{"name":"b","kind":"gpt5","base_url":"http://x"}]}`,
		`{"tiers":[{"name":"a"},{"name":"b"}]}`,
		`{"tiers":[{"name":"a"},{"name":"c","kind":"claude","model":"m"}]}`,
		`{"tiers":[{"name":"a"},{"name":"c","kind":"claude","model":"m","api_key_env":"sk-ant-api03-abcdef"}]}`,
		`{"tiers":[{"name":"a"},{"name":"c","kind":"claude","model":"m","api_key_env":"has space"}]}`,
		`{"tiers":[{"name":"a"},{"name":"b","base_url":"http://x","allow_data":"some"}]}`,
		`{"tiers":[{"name":"a"},{"name":"b","base_url":"http://x","max_context":-5}]}`,
	}
	for _, doc := range bad {
		if _, err := Parse([]byte(doc)); err == nil {
			t.Errorf("accepted: %s", doc)
		}
	}
	home := t.TempDir()
	os.MkdirAll(filepath.Join(home, ".sarina"), 0o755)
	os.WriteFile(filepath.Join(home, ".sarina", "tiers.json"), []byte(bad[3]), 0o600)
	c, err := Load(home, envOf(nil))
	if err == nil || c.Multi() {
		t.Fatalf("a broken file must warn and stay local-only: %+v %v", c, err)
	}
}

func TestConfigNeverReadFromWorkspace(t *testing.T) {
	ws := t.TempDir()
	os.MkdirAll(filepath.Join(ws, ".sarina"), 0o755)
	os.WriteFile(filepath.Join(ws, ".sarina", "tiers.json"), []byte(`{"tiers":[{"name":"a"},{"name":"evil","base_url":"http://evil/v1","allow_data":"all"}]}`), 0o600)
	oldWd, _ := os.Getwd()
	defer os.Chdir(oldWd)
	os.Chdir(ws)
	c, err := Load(t.TempDir(), envOf(nil))
	if err != nil || c.Multi() {
		t.Fatalf("a workspace file must be ignored: %+v %v", c, err)
	}
}

func TestRedactSecrets(t *testing.T) {
	cases := map[string]string{
		"sk-ant-api03-ABCDEFGHIJKLMNOP":                                        "sk-ant",
		"ghp_" + strings.Repeat("x9", 20):                                      "ghp_",
		"AKIAABCDEFGHIJKLMNOP":                                                 "AKIA",
		"Authorization: Bearer abcdefghijklmnop1234":                           "abcdefghijklmnop1234",
		"api_key = \"s3cr3t-value-123\"":                                       "s3cr3t-value-123",
		"-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----": "MIIE",
		"my-custom-literal-secret-9999":                                        "my-custom-literal-secret-9999",
	}
	for in, leak := range cases {
		out, n := RedactSecrets("before "+in+" after", []string{"my-custom-literal-secret-9999"})
		if strings.Contains(out, leak) || n == 0 || !strings.Contains(out, "before") || !strings.Contains(out, "after") {
			t.Errorf("%q -> %q (n=%d)", in, out, n)
		}
	}
	for _, benign := range []string{"token_count: 12345678", "the password policy is documented", "ordinary text with sk- prefix"} {
		if out, n := RedactSecrets(benign, nil); out != benign || n != 0 {
			t.Errorf("over-redacted %q -> %q", benign, out)
		}
	}
}

func TestSamplerParsersAndPressure(t *testing.T) {
	pct, ok := parseMeminfo("MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    4000000 kB\n")
	if !ok || pct != 75 {
		t.Fatalf("%v %v", pct, ok)
	}
	if _, ok := parseMeminfo("garbage"); ok {
		t.Fatal("garbage parsed")
	}
	g, ok := parseNvidia("9000, 12000\n4000, 12000\n")
	if !ok || g != 75 {
		t.Fatalf("%v %v", g, ok)
	}
	if _, ok := parseNvidia("N/A, N/A"); ok {
		t.Fatal("N/A parsed")
	}
	th := Thresholds{MaxMemoryPercent: 90, MaxGPUMemoryPercent: 90}
	if th.Pressure(Sample{MemPercent: 89, MemOK: true, GPUPercent: 89, GPUOK: true}) != "" {
		t.Error("below thresholds must not trip")
	}
	if th.Pressure(Sample{MemPercent: 99}) != "" {
		t.Error("an unmeasured value must never trip")
	}
	if r := th.Pressure(Sample{MemPercent: 91, MemOK: true}); !strings.Contains(r, "memory") {
		t.Error(r)
	}
	if (Thresholds{}).Pressure(Sample{MemPercent: 100, MemOK: true, GPUPercent: 100, GPUOK: true}) != "" {
		t.Error("zero disables")
	}
}

// the real sampler works without cgo on the platforms the project targets.
func TestSystemSamplerMeasuresMemory(t *testing.T) {
	if runtime.GOOS != "linux" && runtime.GOOS != "windows" {
		t.Skip("native memory reader is Linux and Windows only")
	}
	s := (&SystemSampler{}).Sample(context.Background())
	if !s.MemOK || s.MemPercent <= 0 || s.MemPercent > 100 {
		t.Fatalf("%+v", s)
	}
}

func TestWakeUnderCommandPermissionModel(t *testing.T) {
	dir := t.TempDir()
	marker := filepath.Join(dir, "woke")
	tr := Tier{Name: "desk", Wake: "echo hi > " + filepath.ToSlash(marker)}
	run := NewWake(config.Permissions{}, dir, nil)
	if err := run(context.Background(), tr); err == nil || !strings.Contains(err.Error(), "shell commands are disabled") {
		t.Fatalf("a wake command must need the shell permission: %v", err)
	}
	if _, err := os.Stat(marker); err == nil {
		t.Fatal("the command ran without permission")
	}
	// destructive commands are refused by the same check bash uses
	run = NewWake(config.Permissions{AllowShellCommands: true}, dir, nil)
	if err := run(context.Background(), Tier{Name: "d", Wake: "rm -rf /"}); err == nil || !strings.Contains(err.Error(), "wake command not run") {
		t.Fatalf("destructive wake must be refused: %v", err)
	}
	// permitted: runs (needs a POSIX shell; skipped on Windows, where sh mangles
	// the absolute marker path and leaks a stray file into the package dir)
	if runtime.GOOS == "windows" {
		t.Skip("marker path is mangled by sh on Windows")
	}
	if _, err := exec.LookPath("sh"); err != nil {
		t.Skip("no sh available")
	}
	if err := run(context.Background(), tr); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(marker); err != nil {
		t.Fatal("permitted wake command did not run")
	}
}

func TestWakeURLAndEnvFiltering(t *testing.T) {
	var hits atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { hits.Add(1) }))
	defer srv.Close()
	run := NewWake(config.Permissions{}, t.TempDir(), nil)
	if err := run(context.Background(), Tier{Name: "d", Wake: srv.URL + "/wake"}); err != nil || hits.Load() != 1 {
		t.Fatalf("%v hits=%d", err, hits.Load())
	}
	if err := run(context.Background(), Tier{Name: "d"}); err == nil {
		t.Fatal("no wake entry must not run anything")
	}
	env := filteredEnv([]string{"PATH=/bin", "ANTHROPIC_API_KEY=sk-x", "MY_TOKEN=1", "HOME=/h"})
	if len(env) != 2 {
		t.Fatalf("%v", env)
	}
}
