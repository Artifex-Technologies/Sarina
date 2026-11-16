package reverse

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// reaStandIn makes a stand-in labyrinth-rea and an analysis folder with one file; it returns the tool and the file.
func reaStandIn(t *testing.T, script string) (func(map[string]any) (string, error), string) {
	t.Helper()
	bin := filepath.Join(t.TempDir(), "labyrinth-rea")
	if err := os.WriteFile(bin, []byte("#!/bin/sh\n"+script), 0o755); err != nil {
		t.Fatal(err)
	}
	samples := t.TempDir()
	file := filepath.Join(samples, "demo.bin")
	if err := os.WriteFile(file, []byte("\x7fELF"), 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv("LABYRINTH_REA", bin)
	t.Setenv("LABYRINTH_REA_SAMPLES", samples)
	t.Setenv("SARINA_REVERSE", "on")
	t.Setenv("PATH", t.TempDir()) // no heimdalld
	ts := Tools()
	if len(ts) != 3 {
		t.Fatalf("tools: %d", len(ts))
	}
	return func(a map[string]any) (string, error) {
		out, _, err := ts[2].Handler(context.Background(), a, nil)
		return out, err
	}, file
}

// An analysis that prints a great deal (a large program's strings for network-map) is read only to a bound: before, its
// whole output was held (and the error output too).
func TestAnalysisOutputIsBounded(t *testing.T) {
	if _, err := os.Stat("/usr/bin/head"); err != nil {
		t.Skip("no /usr/bin/head")
	}
	call, file := reaStandIn(t, "/usr/bin/head -c 134217728 /dev/zero | /usr/bin/tr '\\000' 'a'\n/usr/bin/head -c 134217728 /dev/zero >&2\n")
	for _, cmd := range []string{"inspect-binary-layout", "network-map"} {
		var before, after runtime.MemStats
		runtime.GC()
		runtime.ReadMemStats(&before)
		out, err := call(map[string]any{"command": cmd, "path": file})
		runtime.ReadMemStats(&after)
		if err != nil {
			t.Fatal(err)
		}
		if got := after.TotalAlloc - before.TotalAlloc; got > 96<<20 {
			t.Fatalf("%s: 128 MiB of output and 128 MiB of errors took %d MiB to read: they must be bounded", cmd, got>>20)
		}
		if cmd == "inspect-binary-layout" && !strings.Contains(out, "[cut]") {
			t.Fatalf("%s: not cut: %.80q", cmd, out)
		}
	}
}

// An analysis that leaves a child holding its output open does not hold the answer for the six minutes it may run:
// before, the tool waited for the child.
func TestAnalysisDoesNotWaitForAChildHoldingTheOutput(t *testing.T) {
	if _, err := os.Stat("/usr/bin/sleep"); err != nil {
		t.Skip("no /usr/bin/sleep")
	}
	call, file := reaStandIn(t, "echo layout\n/usr/bin/sleep 20 &\n")
	start := time.Now()
	out, err := call(map[string]any{"command": "inspect-binary-layout", "path": file})
	if d := time.Since(start); d > 10*time.Second {
		t.Fatalf("the tool waited %v for a child holding the pipe", d)
	}
	if err != nil || !strings.HasPrefix(out, "layout\n") {
		t.Fatalf("got %q %v", out, err)
	}
}
