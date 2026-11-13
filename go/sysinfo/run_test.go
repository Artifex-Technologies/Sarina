package sysinfo

import (
	"context"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// standIn puts an executable script under the name on a PATH of its own.
func standIn(t *testing.T, name, script string) {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, name), []byte("#!/bin/sh\n"+script), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir)
}

// A program that prints without end (or a great deal) is read only to the bound: before, its whole output was held.
func TestRunHoldsABoundedOutput(t *testing.T) {
	if _, err := os.Stat("/usr/bin/head"); err != nil {
		t.Skip("no /usr/bin/head")
	}
	standIn(t, "uptime", "exec /usr/bin/head -c 67108864 /dev/zero\n")
	var before, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&before)
	out := run(context.Background(), step{argv: []string{"uptime"}, lines: 1})
	runtime.ReadMemStats(&after)
	if got := after.TotalAlloc - before.TotalAlloc; got > 16<<20 {
		t.Fatalf("64 MiB of output took %d MiB to read: it must be bounded", got>>20)
	}
	if out != "(nothing to report)" && !strings.HasPrefix(out, "\x00") {
		t.Fatalf("unexpected: %q", out[:min(len(out), 40)])
	}
}

// A program that leaves a child holding its output open does not hold the report: before, run waited for the child.
func TestRunDoesNotWaitForAChildHoldingTheOutput(t *testing.T) {
	if _, err := os.Stat("/usr/bin/sleep"); err != nil {
		t.Skip("no /usr/bin/sleep")
	}
	standIn(t, "uptime", "echo up\n/usr/bin/sleep 20 &\n")
	start := time.Now()
	out := run(context.Background(), step{argv: []string{"uptime"}})
	if d := time.Since(start); d > 10*time.Second {
		t.Fatalf("run waited %v for a child holding the pipe", d)
	}
	if out != "up" {
		t.Fatalf("got %q", out)
	}
}

// A held file whose id is shorter than the twelve characters shown does not crash the report (the skill's prefetch has
// no recover: the whole command died).
func TestShortHeldIDDoesNotPanic(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/audit":
			w.Write([]byte(`{"packages":[],"kernels":[],"hardening":[]}`))
		case "/v1/quarantine":
			w.Write([]byte(`[{"sha256":"abc","original":"/home/x/a","status":"held","kind":"elf"}]`))
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()
	t.Setenv("HEIMDALL_REVIEW_URL", srv.URL+"/v1/review")
	out := securitySummary(context.Background())
	if !strings.Contains(out, "id abc)") || !strings.Contains(out, "HELD /home/x/a") {
		t.Fatal(out)
	}
}
