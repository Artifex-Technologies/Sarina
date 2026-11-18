package analysiscache

import (
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// A named pipe planted at the cache's name held New (and so desktop-audit --model-dir) for ever (found by the Rust
// port's differential, 2026-11-18); now it reads as an empty cache, and the next Set replaces it.
func TestNewDoesNotWaitOnAPlantedPipe(t *testing.T) {
	p := filepath.Join(t.TempDir(), "c.json")
	if err := syscall.Mkfifo(p, 0o644); err != nil {
		t.Skip("no named pipes here:", err)
	}
	done := make(chan *Cache)
	go func() { done <- New(p, "ns") }()
	select {
	case c := <-done:
		if _, ok := c.Get("x"); ok {
			t.Fatal("a pipe is an empty cache")
		}
		if err := c.Set("x", 1.0); err != nil {
			t.Fatal(err)
		}
		if v, ok := New(p, "ns").Get("x"); !ok || v != 1.0 {
			t.Fatalf("%v %v", v, ok)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a named pipe at the cache's name held it")
	}
}
