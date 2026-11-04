package vault

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDigestHasOnlyActiveRules(t *testing.T) {
	d := t.TempDir()
	os.MkdirAll(filepath.Join(d, "Lessons"), 0o755)
	w := func(name, text string) { os.WriteFile(filepath.Join(d, "Lessons", name), []byte(text), 0o644) }
	w("Read first.md", "---\nstatus: active\nrule: Never say what code does until a tool has shown it.\n---\n# Read first\n")
	w("New one.md", "---\nrule: Say how a message was hidden.\n---\n# New one\n")
	w("Old.md", "---\nstatus: retired\nrule: An old rule.\n---\n")
	w("Plain note.md", "# No rule here\n")
	v, _ := Open(d)
	ls, err := v.Lessons()
	if err != nil || len(ls) != 3 {
		t.Fatalf("%v %+v", err, ls)
	}
	dg := Digest(ls)
	if !strings.Contains(dg, "- Never say what code does until a tool has shown it.\n") || strings.Contains(dg, "hidden") || strings.Contains(dg, "old rule") {
		t.Fatalf("digest:\n%s", dg)
	}
}
