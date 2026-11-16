package stego

import (
	"context"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

// A helper program that leaves a child holding its output open is not waited for (it held stego_inspect, the "stego"
// prefetch and `sarina stego inspect` for as long as the child lived), and a helper's output is not kept whole (only
// 2,500 bytes of it are shown).
func TestHelpersReportIsBounded(t *testing.T) {
	bin := t.TempDir()
	write := func(name, script string) {
		if err := os.WriteFile(filepath.Join(bin, name), []byte("#!/bin/sh\n"+script), 0o755); err != nil {
			t.Fatal(err)
		}
	}
	write("exiftool", "echo 'Comment: hi'\nsleep 30 &\n")
	write("binwalk", "head -c 67108864 /dev/zero | tr '\\0' 'x'\n")
	t.Setenv("PATH", bin+":/usr/bin:/bin")
	var before, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&before)
	start := time.Now()
	out := helpersReport(context.Background(), filepath.Join(t.TempDir(), "x.png"))
	if d := time.Since(start); d > 10*time.Second {
		t.Fatalf("took %v: a child holding the output was waited for", d)
	}
	runtime.ReadMemStats(&after)
	if grew := after.TotalAlloc - before.TotalAlloc; grew > 32<<20 {
		t.Fatalf("allocated %d MiB for 64 MiB of output", grew>>20)
	}
	if !strings.Contains(out, "exiftool was run:\nComment: hi") || !strings.Contains(out, "binwalk was run:\nxxx") || !strings.HasSuffix(out, "x\n[…]") {
		t.Fatalf("report: %.300q", out)
	}
}
