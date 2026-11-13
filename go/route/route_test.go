package route

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

func TestPick(t *testing.T) {
	if got := pick([]Score{{"performance", 0.80}, {"crashes", 0.78}, {"disk", 0.70}}); len(got) != 2 || got[0] != "performance" || got[1] != "crashes" {
		t.Fatalf("%v", got)
	}
	if got := pick([]Score{{"chat", 0.85}, {"performance", 0.6}}); got != nil {
		t.Fatalf("small talk must route nowhere: %v", got)
	}
	if got := pick([]Score{{"network", 0.50}}); got != nil {
		t.Fatalf("below the floor nothing is offered: %v", got)
	}
	if got := pick([]Score{{"guides", 0.75}, {"chat", 0.74}}); len(got) != 1 || got[0] != "guides" {
		t.Fatalf("%v", got)
	}
}

func TestCacheFileNeverRelative(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", "")
	t.Setenv("HOME", "")
	if p := cacheFile("m"); !filepath.IsAbs(p) {
		t.Fatalf("relative cache path %q", p)
	}
	t.Setenv("XDG_STATE_HOME", "/tmp/x")
	if p := cacheFile("m"); p != "/tmp/x/sarina/route-m.json" {
		t.Fatal(p)
	}
}

type fixedEmbedder struct{ n int }

func (f *fixedEmbedder) Name() string { return "made-up" }
func (f *fixedEmbedder) Embed(texts []string) ([][]float32, error) {
	f.n++
	out := make([][]float32, len(texts))
	for i := range texts {
		out[i] = []float32{1, float32(i)}
	}
	return out, nil
}

// A named pipe planted at the cache's name (or a huge file) is not read: before, Route waited for ever for a writer.
func TestCacheNamedPipeIsNotWaitedFor(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("XDG_STATE_HOME", dir)
	p := cacheFile("made-up")
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(p, 0o600); err != nil {
		t.Skip(err)
	}
	done := make(chan error, 1)
	go func() { _, err := Scores("is my disk full", &fixedEmbedder{}); done <- err }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Scores waited on a named pipe planted at the route cache")
	}
}
