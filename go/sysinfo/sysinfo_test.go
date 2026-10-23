package sysinfo

import (
	"context"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/tools"
)

func TestReportPerformance(t *testing.T) {
	out, err := Report(context.Background(), "performance")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "Memory") || !strings.Contains(out, "Evidence only") {
		t.Fatalf("unexpected report: %s", out)
	}
}

func TestUnknownTopic(t *testing.T) {
	if _, err := Report(context.Background(), "rm -rf /"); err == nil {
		t.Fatal("an unknown topic must be refused")
	}
}

func TestToolIsReadOnly(t *testing.T) {
	t.Setenv("SARINA_SYSTEM", "on")
	ts := Tools()
	if len(ts) != 3 || ts[2].Spec.Name != "system_status" || ts[2].Requires != 0 || ts[0].Requires != 0 {
		t.Fatalf("bad tool: %+v", ts)
	}
	if ts[1].Spec.Name != "system_fix" || ts[1].Requires != tools.PermShell || ts[1].Class != tools.ClassMutating {
		t.Fatalf("system_fix must need --allow-shell and count as mutating: %+v", ts[1])
	}
}

func TestOffByDefault(t *testing.T) {
	t.Setenv("SARINA_SYSTEM", "")
	if len(Tools()) != 0 {
		t.Fatal("the tool must be off unless SARINA_SYSTEM=on")
	}
}

func TestCrashDetailRefusesOddNames(t *testing.T) {
	for _, bad := range []string{"", "-1", "a b", "../x", "$(id)", "a;b", "x\ny", "--help"} {
		if _, err := CrashDetail(context.Background(), bad); err == nil {
			t.Fatalf("%q must be refused", bad)
		}
	}
}

func TestCrashDetailRuns(t *testing.T) {
	out, err := CrashDetail(context.Background(), "python3")
	if err != nil || !strings.Contains(out, "## Summary") || strings.Contains(out, `\n`) {
		t.Fatalf("%v %q", err, out)
	}
}

func TestNewTopicsExist(t *testing.T) {
	for _, k := range []string{"crashes", "boot", "audio", "kernel", "gaming"} {
		if _, ok := Topics[k]; !ok {
			t.Fatalf("topic %s missing", k)
		}
	}
}

func TestGamingReportHasSummary(t *testing.T) {
	out, err := Report(context.Background(), "gaming")
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"## Summary", "game is running", "graphics memory", "Proton versions", "Evidence only"} {
		if !strings.Contains(out, want) {
			t.Fatalf("gaming report lacks %q:\n%s", want, out)
		}
	}
}

func TestFixesAreNamedAndFixed(t *testing.T) {
	old := execFix
	execFix = func(ctx context.Context, argv []string) string {
		t.Fatalf("a refused name must run nothing: %v", argv)
		return ""
	}
	defer func() { execFix = old }()
	for _, bad := range []string{"", "rm -rf /", "restart_audio; id", "Restart Audio", "../x", "restart_audio x", "unknown_fix"} {
		if _, err := ApplyFix(context.Background(), bad); err == nil {
			t.Fatalf("%q must be refused", bad)
		}
	}
	for _, f := range Fixes {
		if !fixNameRE.MatchString(f.Name) || len(f.Steps) == 0 || f.What == "" || len(f.Check) == 0 {
			t.Fatalf("a repair needs a plain name, steps, a description and a check: %+v", f)
		}
		for _, argv := range f.Steps {
			if argv[0] == "sudo" || argv[0] == "sh" || argv[0] == "bash" || strings.Contains(strings.Join(argv, " "), "rm ") {
				t.Fatalf("a repair runs at user level, with no shell and nothing deleted: %v", argv)
			}
		}
	}
}

func TestApplyFixRunsStepsThenCheck(t *testing.T) {
	var ran [][]string
	old := execFix
	execFix = func(ctx context.Context, argv []string) string {
		ran = append(ran, argv)
		return "active\nactive\nactive"
	}
	defer func() { execFix = old }()
	out, err := ApplyFix(context.Background(), "restart_audio")
	if err != nil {
		t.Fatal(err)
	}
	if len(ran) != 2 || ran[0][0] != "systemctl" || ran[1][2] != "is-active" {
		t.Fatalf("expected the restart then the check: %v", ran)
	}
	if !strings.Contains(out, "## Check afterwards") || !strings.Contains(out, "Evidence only") {
		t.Fatalf("report: %s", out)
	}
}

func TestBluetoothFixOnlyWhenOn(t *testing.T) {
	f, ok := findFix("restart_bluetooth")
	if !ok || f.Only == nil {
		t.Fatal("restart_bluetooth must check that Bluetooth is on first")
	}
}

func TestGameRunningIsQuietWithoutAGame(t *testing.T) {
	// no game runs in a test; the check must say so without error (and never report its own pgrep as a game)
	if why, yes := GameRunning(context.Background()); yes && !strings.Contains(why, "game mode") && !strings.Contains(why, "gamescope") {
		t.Fatalf("a game reported in a test environment: %s", why)
	}
}
