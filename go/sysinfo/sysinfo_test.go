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
	if len(ts) != 1 || ts[0].Spec.Name != "system_status" || ts[0].Requires != 0 {
		t.Fatalf("bad tool: %+v", ts)
	}
}

func TestOffByDefault(t *testing.T) {
	t.Setenv("SARINA_SYSTEM", "")
	if len(Tools()) != 0 {
		t.Fatal("the tool must be off unless SARINA_SYSTEM=on")
	}
}
