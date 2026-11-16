package stego

import (
	"context"
	"strings"
	"testing"
)

// An empty file is inspected like any other: FindEmbedded searched from offset 1 of an empty slice and panicked
// (stego_inspect answered "failed internally"; `sarina stego inspect` of an empty file ended with a Go trace).
func TestEmptyFileIsInspected(t *testing.T) {
	t.Setenv("PATH", t.TempDir()) // no exiftool or binwalk
	defer func() {
		if p := recover(); p != nil {
			t.Fatalf("panicked: %v", p)
		}
	}()
	if got := FindEmbedded(nil, 10); len(got) != 0 {
		t.Fatalf("embedded in nothing: %v", got)
	}
	if out := Inspect(context.Background(), []byte{}, "empty.png"); !strings.Contains(out, "0 bytes") {
		t.Fatalf("report: %q", out)
	}
}
