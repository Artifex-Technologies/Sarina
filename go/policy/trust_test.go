package policy

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type refuseAll struct{}

func (refuseAll) Refuse(string) string { return "not approved" }

func TestAnUnapprovedOperatorPolicyStopsSarina(t *testing.T) {
	home := t.TempDir()
	os.MkdirAll(filepath.Join(home, ".sarina"), 0o755)
	os.WriteFile(filepath.Join(home, ".sarina", "policy.json"), []byte(`{"deny":["bash"]}`), 0o644)
	p := Load(Options{Home: home, Getenv: func(string) string { return "" }, Trust: refuseAll{}})
	if len(p.Fatal) != 1 || !strings.Contains(p.Fatal[0], "not approved") {
		t.Fatalf("%v", p.Fatal)
	}
	if p := Load(Options{Home: home, Getenv: func(string) string { return "" }}); len(p.Fatal) != 0 {
		t.Fatal("without trust, as before")
	}
}
