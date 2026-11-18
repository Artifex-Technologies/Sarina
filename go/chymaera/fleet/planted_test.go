package fleet

import (
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// Things planted under the names fleet reads and writes (found by the Rust port's differential, 2026-11-18): a named pipe
// held a reader for ever, and a link at the audit log's name had every launch attempt written wherever it led.

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

func fifo(t *testing.T, path string) {
	t.Helper()
	if err := syscall.Mkfifo(path, 0o644); err != nil {
		t.Skip("no named pipes here:", err)
	}
}

func TestReadSessionsSkipsAPlantedPipe(t *testing.T) {
	dir := t.TempDir()
	fifo(t, filepath.Join(dir, "a.json"))
	if err := os.WriteFile(filepath.Join(dir, "b.json"), []byte(`{"session_id":"b","usage":{"input_tokens":5}}`), 0o644); err != nil {
		t.Fatal(err)
	}
	var got []SessionUsage
	within(t, "ReadSessions", func() { got = ReadSessions([]string{dir}) })
	if len(got) != 1 || got[0].SessionID != "b" || got[0].InputTokens != 5 {
		t.Fatalf("%+v", got)
	}
}

func TestReadAgentStatusSkipsAPlantedPipe(t *testing.T) {
	dir := t.TempDir()
	fifo(t, filepath.Join(dir, "a.json"))
	if err := os.WriteFile(filepath.Join(dir, "b.json"), []byte(`{"id":"codex","name":"Codex"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	var got []AgentStatus
	within(t, "ReadAgentStatus", func() { got = ReadAgentStatus(dir, time.Time{}, 0) })
	if len(got) != 1 || got[0].AgentID != "codex" {
		t.Fatalf("%+v", got)
	}
}

func TestLaunchWritesTheAuditLogOnlyAsARegularFileOfItsOwn(t *testing.T) {
	// a link at the log's name: nothing is written where it leads, and nothing runs
	dir := t.TempDir()
	elsewhere := filepath.Join(dir, "elsewhere")
	if err := os.WriteFile(elsewhere, []byte("keep\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	log := filepath.Join(dir, "agent-launch.jsonl")
	if err := os.Symlink(elsewhere, log); err != nil {
		t.Fatal(err)
	}
	f := &fakeRun{}
	if _, err := Launch(plan(t, "claude", "go"), "go", true, log, f.run); err == nil || len(f.calls) != 0 {
		t.Fatalf("launched through a planted link: %v %v", err, f.calls)
	}
	if b, _ := os.ReadFile(elsewhere); string(b) != "keep\n" {
		t.Fatalf("the audit entry went where the link leads: %q", b)
	}
	// a named pipe at the log's name: refused at once, nothing runs
	pipeLog := filepath.Join(t.TempDir(), "agent-launch.jsonl")
	fifo(t, pipeLog)
	f = &fakeRun{}
	var err error
	within(t, "Launch", func() { _, err = Launch(plan(t, "claude", "go"), "go", true, pipeLog, f.run) })
	if err == nil || len(f.calls) != 0 {
		t.Fatalf("launched with a pipe for an audit log: %v %v", err, f.calls)
	}
	// a refusal is not written through a link either
	if _, err := Launch(plan(t, "claude", "go"), "go", false, log, f.run); err == nil {
		t.Fatal("an unconfirmed launch is an error")
	}
	if b, _ := os.ReadFile(elsewhere); string(b) != "keep\n" {
		t.Fatalf("the refusal went where the link leads: %q", b)
	}
}
