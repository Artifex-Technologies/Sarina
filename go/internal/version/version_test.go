package version

import (
	"os"
	"strings"
	"testing"
)

func TestVersionIsTheVersionFile(t *testing.T) {
	b, err := os.ReadFile("../../../VERSION")
	if err != nil {
		t.Fatal(err)
	}
	if want := strings.TrimSpace(string(b)); Version != want {
		t.Fatalf("Version is %q, VERSION says %q: scripts/release.sh in Labyrinth_OS writes both", Version, want)
	}
}
