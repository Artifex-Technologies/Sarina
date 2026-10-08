package cli

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/memory"
)

const proposalReply = `[{"scope":"agent","action":"add","content":"Builds need GOFLAGS=-mod=vendor","rationale":"seen twice"}]`

func TestAutoMemoryReviewIsOffAtTheLoopLayer(t *testing.T) {
	b := newBackend(t, "answer", proposalReply)
	h := newHarness(t, strings.NewReader("a question\n/exit\n"))
	o := newOpts("chat", &h.env)
	if o.autoReview {
		t.Fatal("the library layer must default to off")
	}
	o.fs.Parse(h.commonAuto(b))
	a, err := o.agent(&h.env, nil)
	if err != nil {
		t.Fatal(err)
	}
	if code := conversation(&h.env, a, o, nil, ""); code != 0 {
		t.Fatalf("code %d", code)
	}
	if b.count() != 1 || strings.Contains(h.out.String(), "memory_review") {
		t.Fatalf("no review call expected: requests=%d out=%q", b.count(), h.out.String())
	}
}

func TestFlagDisablesAutoReview(t *testing.T) {
	b := newBackend(t, "answer")
	h := newHarness(t, strings.NewReader("a question\n/exit\n"))
	if code := h.run(append([]string{"chat"}, h.common(b)...)...); code != 0 || b.count() != 1 {
		t.Fatalf("code=%d requests=%d out=%q", code, b.count(), h.out.String())
	}
	o := newOpts("chat", &h.env)
	o.fs.Parse([]string{"--no-auto-memory-review"})
	if !o.noAutoReview {
		t.Fatal("flag not parsed")
	}
}

func TestExitTimeReviewStagesProposalsAndWritesNothing(t *testing.T) {
	b := newBackend(t, "the answer", proposalReply)
	h := newHarness(t, strings.NewReader("we vendor deps\n/exit\n"))
	if code := h.run(append([]string{"chat"}, h.commonAuto(b)...)...); code != 0 {
		t.Fatalf("code=%d err=%q", code, h.errb.String())
	}
	if b.count() != 2 {
		t.Fatalf("want chat + one review call, got %d", b.count())
	}
	if !strings.Contains(h.out.String(), "memory_review: proposed 1 entry (1 pending, 0 blocked) -- run 'sarina memory-review-list'") {
		t.Fatalf("status line missing: %q", h.out.String())
	}
	review := b.reqs[1]
	if _, has := review["tools"]; has {
		t.Fatal("the review call must offer no tools")
	}
	raw, _ := json.Marshal(review["messages"])
	if !strings.Contains(string(raw), "we vendor deps") || !strings.Contains(string(raw), "the answer") {
		t.Fatalf("transcript missing: %s", raw)
	}
	q := memory.QueueFromWorkspace(h.dir)
	if pend := q.Pending(); len(pend) != 1 || pend[0].Status != memory.StatusPending {
		t.Fatalf("queue = %+v", q.Load())
	}
	mem := filepath.Join(h.dir, ".port_sessions", "memory", "AGENT_MEMORY.md")
	if _, err := os.Stat(mem); err == nil {
		t.Fatal("nothing may be written to memory automatically")
	}
	// approval is a separate command
	if code := h.run("memory-review-approve", "--cwd", h.dir, q.Pending()[0].EntryID); code != 0 {
		t.Fatalf("approve: %d %s", code, h.errb.String())
	}
	got, _ := os.ReadFile(mem)
	if string(got) != "Builds need GOFLAGS=-mod=vendor" {
		t.Fatalf("memory = %q", got)
	}
}

func TestExitTimeReviewFailureIsReportedNotFatalAndRunsOnEOF(t *testing.T) {
	b := newBackend(t, "the answer") // second response: backend error
	h := newHarness(t, strings.NewReader("hello there\n"))
	if code := h.run(append([]string{"chat"}, h.commonAuto(b)...)...); code != 0 {
		t.Fatalf("a failed review must not change the exit code: %d", code)
	}
	if !strings.Contains(h.out.String(), "memory_review_skipped=error (") {
		t.Fatalf("out = %q", h.out.String())
	}
}

func TestNoReviewWithoutASavedSessionOrInOneShotMode(t *testing.T) {
	b := newBackend(t, "answer")
	h := newHarness(t, strings.NewReader("/exit\n"))
	if code := h.run(append([]string{"chat"}, h.commonAuto(b)...)...); code != 0 || b.count() != 0 {
		t.Fatalf("no session id, no review: code=%d requests=%d", code, b.count())
	}
	h2 := newHarness(t, strings.NewReader(""))
	h2.run(append([]string{"chat"}, append(h2.commonAuto(b), "one", "shot")...)...)
	if b.count() != 1 {
		t.Fatalf("one-shot chat must not review: %d", b.count())
	}
}

func TestManualMemoryReviewListRejectAndStatus(t *testing.T) {
	b := newBackend(t, "the answer", proposalReply)
	h := newHarness(t, strings.NewReader(""))
	h.run(append([]string{"chat"}, append(h.common(b), "remember the vendor flag")...)...)
	files := h.sessionFiles(t)
	if len(files) != 1 {
		t.Fatal(files)
	}
	id := strings.TrimSuffix(filepath.Base(files[0]), ".json")
	h.out.Reset()
	if code := h.run(append([]string{"memory-review"}, append(h.commonAuto(b), id)...)...); code != 0 || !strings.Contains(h.out.String(), "proposed 1 entry") {
		t.Fatalf("%d %q %q", code, h.out.String(), h.errb.String())
	}
	h.out.Reset()
	h.run("memory-review-list", "--cwd", h.dir)
	if list := h.out.String(); !strings.Contains(list, "status=pending") || !strings.Contains(list, "GOFLAGS=-mod=vendor") {
		t.Fatal(list)
	}
	eid := memory.QueueFromWorkspace(h.dir).Pending()[0].EntryID
	if code := h.run("memory-review-reject", "--cwd", h.dir, eid); code != 0 {
		t.Fatal(code)
	}
	if code := h.run("memory-review-reject", "--cwd", h.dir, eid); code == 0 {
		t.Fatal("rejecting twice must fail")
	}
	h.out.Reset()
	if h.run("memory-status", "--cwd", h.dir) != 0 || !strings.Contains(h.out.String(), "## agent") {
		t.Fatal(h.out.String())
	}
}

func TestSessionSearchCommand(t *testing.T) {
	b := newBackend(t, "the vault token rotates monthly")
	h := newHarness(t, strings.NewReader(""))
	h.run(append([]string{"chat"}, append(h.commonAuto(b), "when", "does", "it", "rotate")...)...)
	h.out.Reset()
	if code := h.run("session-search", "--cwd", h.dir, "rotates monthly"); code != 0 || !strings.Contains(h.out.String(), "role=assistant") {
		t.Fatalf("%d %q", code, h.out.String())
	}
}
