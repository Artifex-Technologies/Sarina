package sysinfo

import (
	"context"
	"strings"
	"testing"
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
	if len(ts) != 2 || ts[1].Spec.Name != "system_status" || ts[1].Requires != 0 || ts[0].Requires != 0 {
		t.Fatalf("bad tool: %+v", ts)
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
	if err != nil || !strings.Contains(out, "Latest crash") || strings.Contains(out, `\n`) {
		t.Fatalf("%v %q", err, out)
	}
}

func TestNewTopicsExist(t *testing.T) {
	for _, k := range []string{"crashes", "boot", "audio", "kernel"} {
		if _, ok := Topics[k]; !ok {
			t.Fatalf("topic %s missing", k)
		}
	}
}
