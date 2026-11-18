package desktop

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// Things planted under the names desktop-audit reads and writes (found by the Rust port's differential, 2026-11-18): a
// named pipe named *.desktop held the scan and --remove-generated for ever, and --apply wrote an entry through a link
// planted at its name (into whatever file the link named).

func within(t *testing.T, what string, f func()) {
	t.Helper()
	done := make(chan struct{})
	go func() { f(); close(done) }()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatalf("%s: a named pipe held it", what)
	}
}

func TestScanAndRemoveSkipAPlantedPipe(t *testing.T) {
	dir := t.TempDir()
	if err := syscall.Mkfifo(filepath.Join(dir, "a.desktop"), 0o644); err != nil {
		t.Skip("no named pipes here:", err)
	}
	write(t, dir, "b.desktop", "[Desktop Entry]\nName=B\nExec=b\nX-Sarina-Generated=sarina\n")
	var got []DesktopEntry
	within(t, "ScanDesktopEntries", func() { got = ScanDesktopEntries([]string{dir}) })
	if len(got) != 1 || got[0].EntryID != "b" {
		t.Fatalf("%+v", got)
	}
	var removed []string
	within(t, "RemoveGenerated", func() { removed = RemoveGenerated(dir) })
	if len(removed) != 1 || filepath.Base(removed[0]) != "b.desktop" {
		t.Fatalf("%v", removed)
	}
}

func TestApplyWritesNoEntryThroughAPlantedLinkOrPipe(t *testing.T) {
	dir := t.TempDir()
	elsewhere := filepath.Join(dir, "bashrc")
	if err := os.WriteFile(elsewhere, []byte("keep\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(dir, "apps")
	if err := os.MkdirAll(target, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(elsewhere, filepath.Join(target, "sarina-nmap.desktop")); err != nil {
		t.Fatal(err)
	}
	p := CategoryProposal{Action: "create", Target: "nmap", Name: "nmap", Categories: []string{"Network"}, Confidence: 1, ExecCommand: "nmap"}
	written, err := ApplyProposals([]CategoryProposal{p}, target, 0)
	if err == nil || len(written) != 0 {
		t.Fatalf("wrote through a planted link: %v %v", written, err)
	}
	if b, _ := os.ReadFile(elsewhere); string(b) != "keep\n" {
		t.Fatalf("the entry went where the link leads: %q", b)
	}
	if err := syscall.Mkfifo(filepath.Join(target, "sarina-tcpdump.desktop"), 0o644); err != nil {
		t.Skip("no named pipes here:", err)
	}
	p.Target = "tcpdump"
	within(t, "ApplyProposals", func() { written, err = ApplyProposals([]CategoryProposal{p}, target, 0) })
	if err == nil || len(written) != 0 {
		t.Fatalf("wrote into a planted pipe: %v %v", written, err)
	}
}
