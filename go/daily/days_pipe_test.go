package daily

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// ReadDays reads the daily notes as bounded regular files: a named pipe planted under a day's name held `sarina trends`
// for ever.
func TestReadDaysSkipsAPlantedPipe(t *testing.T) {
	vault := t.TempDir()
	if err := os.MkdirAll(filepath.Join(vault, "Daily"), 0o755); err != nil {
		t.Fatal(err)
	}
	line := "## Numbers\n\n- 09:00 | disk / 40% | crashes 0 | failed 0 | memory 30% | load 0.5\n"
	if err := os.WriteFile(filepath.Join(vault, "Daily", "2026-11-01.md"), []byte(line), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(filepath.Join(vault, "Daily", "2026-11-02.md"), 0o644); err != nil {
		t.Fatal(err)
	}
	done := make(chan []Day, 1)
	go func() { done <- ReadDays(vault, 30) }()
	select {
	case days := <-done:
		if len(days) != 1 || days[0].Date != "2026-11-01" {
			t.Fatalf("days %+v", days)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("ReadDays waited on a named pipe")
	}
}
