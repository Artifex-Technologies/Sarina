package firmware

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

const healthy = `
Chymaera Zero
firmware_origin_fork : Chymaera
firmware_origin_git  : https://github.com/Artifex-Technologies/Argus_Firmware.git
firmware_version     : 1.4.3-CHY-0.1.0
firmware_commit      : abc1234
hardware_model       : Flipper Zero
enclave_valid        : true
radio_alive          : true
`

func healthyFields() map[string]string { return ParseDeviceInfo(healthy) }

func hasSub(list []string, sub string) bool {
	for _, s := range list {
		if strings.Contains(s, sub) {
			return true
		}
	}
	return false
}

func TestParseDeviceInfo(t *testing.T) {
	f := healthyFields()
	if f["firmware_origin_fork"] != "Chymaera" || f["firmware_commit"] != "abc1234" || f["radio_alive"] != "true" {
		t.Fatalf("%v", f)
	}
	if _, ok := f["Chymaera Zero"]; ok {
		t.Fatal("banner parsed")
	}
	if f["firmware_origin_git"] != "https://github.com/Artifex-Technologies/Argus_Firmware.git" {
		t.Fatalf("colon in value lost: %q", f["firmware_origin_git"])
	}
	if got := ParseDeviceInfo("a b: c\r\nkey: v\r\n: x"); len(got) != 1 || got["key"] != "v" {
		t.Fatalf("%v", got)
	}
}

func TestHealthyHasNoWarnings(t *testing.T) {
	r := CheckDeviceInfo(healthyFields(), "")
	if !r.OK() {
		t.Fatal(r.Warnings)
	}
	if !hasSub(r.Notes, "tracks upstream release 1.4.3") || hasSub(r.Notes, "development build") {
		t.Fatal(r.Notes)
	}
}

func TestWarnings(t *testing.T) {
	f := healthyFields()
	f["firmware_origin_fork"] = "flipperdevices"
	r := CheckDeviceInfo(f, "")
	if r.OK() || r.Warnings[0] != "firmware_origin_fork is 'flipperdevices', expected 'Chymaera'" {
		t.Fatal(r.Warnings)
	}
	f = healthyFields()
	f["enclave_valid"] = "false"
	if !hasSub(CheckDeviceInfo(f, "").Warnings, "enclave_valid") {
		t.Fatal("enclave not flagged")
	}
	f = healthyFields()
	f["firmware_version"] = "unknown"
	r = CheckDeviceInfo(f, "")
	if r.OK() || !hasSub(r.Warnings, "firmware_version") {
		t.Fatal(r.Warnings)
	}
	if r := CheckDeviceInfo(map[string]string{}, ""); !hasSub(r.Warnings, "missing") || len(r.Warnings) != 3 {
		t.Fatal(r.Warnings)
	}
	if got := CheckDeviceInfo(map[string]string{}, "").AsText(); got != "# Chymaera Zero device\n\nNo device_info fields parsed." {
		t.Fatal(got)
	}
}

func TestVersionNotes(t *testing.T) {
	cases := []struct{ version, note string }{
		{"1.4.3-CHY-0.1.0+8", "8 commit(s) since chy-0.1.0"},
		{"1.4.3-CHY-dev", "no chy-* tag"},
		{"chy-0.1.0-2-g540d03c7", "pre-2026-09-12"},
		{"something-else", "not a recognised"},
	}
	for _, c := range cases {
		f := healthyFields()
		f["firmware_version"] = c.version
		r := CheckDeviceInfo(f, "")
		if !r.OK() {
			t.Fatalf("%s: %v", c.version, r.Warnings)
		}
		if !hasSub(r.Notes, c.note) {
			t.Fatalf("%s: %v", c.version, r.Notes)
		}
	}
	f := healthyFields()
	f["firmware_version"] = "1.4.3-CHY-0.1.0+8"
	if !strings.Contains(CheckDeviceInfo(f, "").AsText(), "## Notes") {
		t.Fatal("notes section missing")
	}
}

func TestCommitChecks(t *testing.T) {
	if !hasSub(CheckDeviceInfo(healthyFields(), "deadbee").Warnings, "firmware_commit") {
		t.Fatal("mismatch not flagged")
	}
	if r := CheckDeviceInfo(healthyFields(), "abc1234567890"); !r.OK() {
		t.Fatal(r.Warnings)
	}
}

func TestAsTextLayout(t *testing.T) {
	want := "# Chymaera Zero device\n\nfirmware_origin_fork: Chymaera\nfirmware_version: 1.4.3-CHY-0.1.0\nfirmware_commit: abc1234\n" +
		"firmware_origin_git: https://github.com/Artifex-Technologies/Argus_Firmware.git\nhardware_model: Flipper Zero\nenclave_valid: true\nradio_alive: true\n\nNo warnings.\n\n## Notes\n- tracks upstream release 1.4.3"
	if got := CheckDeviceInfo(healthyFields(), "").AsText(); got != want {
		t.Fatalf("got:\n%s", got)
	}
}

func TestClassifyPath(t *testing.T) {
	cases := map[string]string{
		"assets/protobuf/application.proto": "wire-format",
		"applications_user/chymaera/main.c": "external-app",
		"furi/core/thread.c":                "core",
		"targets/f7/api_symbols.csv":        "core",
		"lib/nfc/nfc.c":                     "core",
		"applications/main/gui.c":           "core",
		"applications_user/x.c":             "external-app",
		"ReadMe.md":                         "docs",
		".editorconfig":                     "other",
	}
	for p, want := range cases {
		if got, _ := ClassifyPath(p); got != want {
			t.Errorf("%s: %s, want %s", p, got, want)
		}
	}
}

// ---- merge impact against real temp git repos ----

func run(t *testing.T, repo string, args ...string) {
	t.Helper()
	cmd := exec.Command("git", args...)
	cmd.Dir = repo
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("git %v: %v\n%s", args, err, out)
	}
}

func write(t *testing.T, repo, rel, text string) {
	t.Helper()
	p := filepath.Join(repo, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(p, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

func newRepo(t *testing.T) string {
	t.Helper()
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("git not installed")
	}
	repo := t.TempDir()
	run(t, repo, "init", "-b", "main")
	run(t, repo, "config", "user.email", "test@example.com")
	run(t, repo, "config", "user.name", "Test")
	run(t, repo, "config", "commit.gpgsign", "false")
	write(t, repo, "furi/core.c", "base\n")
	write(t, repo, "applications_user/app.c", "base\n")
	write(t, repo, "assets/protobuf/application.proto", "base\n")
	run(t, repo, "add", "-A")
	run(t, repo, "commit", "-m", "base")

	run(t, repo, "checkout", "-b", "upstream-dev")
	write(t, repo, "furi/core.c", "upstream\n")
	write(t, repo, "assets/protobuf/application.proto", "upstream\n")
	run(t, repo, "add", "-A")
	run(t, repo, "commit", "-m", "upstream work")

	// Fork: edits an upstream file both sides touch (high), an upstream file
	// only we touch (medium), and adds a new file inside an upstream directory
	// (low - the menu_wii.c case).
	run(t, repo, "checkout", "main")
	write(t, repo, "furi/core.c", "chymaera\n")
	write(t, repo, "applications_user/app.c", "chymaera\n")
	write(t, repo, "applications/services/gui/modules/menu_wii.c", "new\n")
	run(t, repo, "add", "-A")
	run(t, repo, "commit", "-m", "fork work")
	return repo
}

func tiers(t *testing.T, repo, ref string) (MergeImpactReport, map[string]string) {
	t.Helper()
	r, err := MergeImpact(repo, ref, "")
	if err != nil {
		t.Fatal(err)
	}
	m := map[string]string{}
	for _, k := range r.Risks {
		m[k.Path] = k.Tier
	}
	return r, m
}

func TestMergeImpactTiers(t *testing.T) {
	repo := newRepo(t)
	r, m := tiers(t, repo, "upstream-dev")
	if m["furi/core.c"] != "high" {
		t.Error("both-sides file must be high")
	}
	if m["applications_user/app.c"] != "medium" {
		t.Error("our edit to an upstream file must be medium")
	}
	if m["applications/services/gui/modules/menu_wii.c"] != "low" {
		t.Error("new file in an upstream directory must be low")
	}
	if _, ok := m["assets/protobuf/application.proto"]; ok {
		t.Error("upstream-only change reported")
	}
	areas := map[string]string{}
	for _, k := range r.Risks {
		areas[k.Path] = k.Area
	}
	if areas["furi/core.c"] != "core" || areas["applications_user/app.c"] != "external-app" {
		t.Errorf("areas: %v", areas)
	}
	if len(r.Conflicting()) != 1 || r.Conflicting()[0].Path != "furi/core.c" {
		t.Errorf("conflicting: %v", r.Conflicting())
	}
	var na []string
	for _, k := range r.NonAdditive() {
		na = append(na, k.Path)
	}
	if strings.Join(na, ",") != "applications_user/app.c,furi/core.c" {
		t.Errorf("non-additive: %v", na)
	}
	if len(r.UpstreamChanged) != 2 || len(r.LocalChanged) != 3 {
		t.Errorf("counts: %d %d", len(r.UpstreamChanged), len(r.LocalChanged))
	}
	text := r.AsText()
	for _, want := range []string{"# Merge impact: upstream-dev -> local firmware tree", "furi/core.c [core]", "## high (1)", "## low (1)", "would conflict (high): 1"} {
		if !strings.Contains(text, want) {
			t.Errorf("missing %q in\n%s", want, text)
		}
	}
	if r.PolicyBreached() || strings.Contains(text, "POLICY ALARM") {
		t.Error("small fork tripped the alarm")
	}
	if strings.Contains(text, "should apply cleanly") {
		t.Error("conflict report claims a clean merge")
	}
}

func TestPolicyAlarm(t *testing.T) {
	repo := newRepo(t)
	run(t, repo, "checkout", "main")
	for i := 0; i <= RebasePolicyBudget; i++ {
		write(t, repo, fmt.Sprintf("lib/vendor%d.c", i), "base\n")
	}
	run(t, repo, "add", "-A")
	run(t, repo, "commit", "-m", "add vendored files")
	run(t, repo, "branch", "up2", "main")
	for i := 0; i <= RebasePolicyBudget; i++ {
		write(t, repo, fmt.Sprintf("lib/vendor%d.c", i), "ours\n")
	}
	run(t, repo, "add", "-A")
	run(t, repo, "commit", "-m", "edit them all")
	r, _ := tiers(t, repo, "up2")
	if !r.PolicyBreached() {
		t.Fatalf("alarm did not fire: %d non-additive", len(r.NonAdditive()))
	}
	if !strings.Contains(r.AsText(), "## POLICY ALARM: 13 first-party files exist upstream, over the budget of 12") {
		t.Fatal(r.AsText())
	}
}

func TestCleanMergeReportedClean(t *testing.T) {
	repo := newRepo(t)
	run(t, repo, "checkout", "-b", "clean-branch", "main")
	write(t, repo, "documentation/notes.md", "docs only\n")
	run(t, repo, "add", "-A")
	run(t, repo, "commit", "-m", "docs")
	run(t, repo, "checkout", "main")
	r, _ := tiers(t, repo, "clean-branch")
	if len(r.Conflicting()) != 0 || !strings.Contains(r.AsText(), "should apply cleanly") {
		t.Fatal(r.AsText())
	}
}

func TestNothingChangedLocally(t *testing.T) {
	repo := newRepo(t)
	run(t, repo, "checkout", "-b", "same", "upstream-dev")
	r, _ := tiers(t, repo, "upstream-dev")
	if len(r.Risks) != 0 || !strings.HasSuffix(r.AsText(), "Nothing changed locally since the merge base - the merge should apply cleanly.") {
		t.Fatal(r.AsText())
	}
}

func TestUnknownRefIsAnError(t *testing.T) {
	repo := newRepo(t)
	if _, err := MergeImpact(repo, "does/not/exist", ""); err == nil || !strings.HasPrefix(err.Error(), "git merge-base HEAD does/not/exist failed:") {
		t.Fatalf("err = %v", err)
	}
}
