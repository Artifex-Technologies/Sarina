package trust

import (
	"os"
	"path/filepath"
	"testing"
)

func TestContained(t *testing.T) {
	p := filepath.Join(t.TempDir(), "contained")
	t.Setenv(EnvContained, p)
	if on, _ := Contained(); on {
		t.Fatal("no lock: not contained")
	}
	os.WriteFile(p, []byte(`{"reason":"planted skill","level":"contained"}`), 0o644)
	if on, why := Contained(); !on || why != "planted skill" {
		t.Fatal(on, why)
	}
	os.WriteFile(p, []byte(`garbage`), 0o644)
	if on, _ := Contained(); !on {
		t.Fatal("an unreadable lock is still a lock")
	}
}
