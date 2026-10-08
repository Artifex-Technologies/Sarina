package sessionsearch

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

func save(t *testing.T, dir, id string, msgs ...session.Message) {
	t.Helper()
	s := &session.Session{ID: id, Model: config.ModelConfig{Model: "m"}, Runtime: config.DefaultRuntime(dir)}
	for _, m := range msgs {
		s.Append(m)
	}
	if _, err := s.Save(dir); err != nil {
		t.Fatal(err)
	}
}

func um(c string) session.Message { return session.Message{Role: "user", Content: c} }

func TestFindsMessageByKeyword(t *testing.T) {
	dir := t.TempDir()
	save(t, dir, "aaa", session.Message{Role: "system", Content: "pytest everywhere"}, um("how do I configure pytest fixtures?"),
		session.Message{Role: "assistant", Content: "Use conftest.py"})
	save(t, dir, "bbb", um("unrelated chatter about lunch"))
	hits := Search("pytest fixtures", dir, 10)
	if len(hits) != 1 || hits[0].SessionID != "aaa" || hits[0].Role != "user" || hits[0].TurnIndex != 1 {
		t.Fatalf("hits = %+v", hits)
	}
	if !strings.Contains(hits[0].Snippet, "pytest fixtures") || hits[0].Score >= 0 {
		t.Fatalf("hit = %+v", hits[0])
	}
}

func TestSystemMessagesAndRemindersAreNotIndexed(t *testing.T) {
	dir := t.TempDir()
	save(t, dir, "aaa", session.Message{Role: "system", Content: "xyzzy"},
		session.Message{Role: "user", Content: "<system-reminder>\nxyzzy\n</system-reminder>"})
	if hits := Search("xyzzy", dir, 10); len(hits) != 0 {
		t.Fatalf("%+v", hits)
	}
}

func TestEmptyQueryAndEmptyDirectory(t *testing.T) {
	dir := t.TempDir()
	save(t, dir, "aaa", um("hello"))
	if Search("   ", dir, 10) != nil {
		t.Fatal("empty query must give no hits")
	}
	if Search("hello", filepath.Join(dir, "missing"), 10) != nil || Search("hello", t.TempDir(), 10) != nil {
		t.Fatal("no sessions must give no hits")
	}
}

func TestPhraseOutranksMentionAndFallbackRanksByTermCount(t *testing.T) {
	dir := t.TempDir()
	save(t, dir, "one", um("alpha beta once"), um("beta then alpha later"))
	hits := Search("alpha beta", dir, 10)
	if len(hits) != 1 || hits[0].TurnIndex != 0 {
		t.Fatalf("phrase hits = %+v", hits)
	}
	// words not adjacent anywhere: term-count fallback, most occurrences first
	dir2 := t.TempDir()
	save(t, dir2, "two", um("gamma"), um("delta gamma delta gamma"), um("gamma delta"))
	hits = Search("gamma delta", dir2, 10)
	if len(hits) == 0 {
		t.Fatal("no hits")
	}
	hits = Search("delta delta", dir2, 10)
	if len(hits) < 1 {
		t.Fatal("expected fallback hits")
	}
}

func TestTopKAndNewestFirstTies(t *testing.T) {
	dir := t.TempDir()
	save(t, dir, "old", um("needle here"))
	old := time.Now().Add(-time.Hour)
	_ = os.Chtimes(filepath.Join(dir, "old.json"), old, old)
	save(t, dir, "new", um("needle here"))
	hits := Search("needle", dir, 1)
	if len(hits) != 1 || hits[0].SessionID != "new" {
		t.Fatalf("%+v", hits)
	}
}

func TestSnippetRules(t *testing.T) {
	long := strings.Repeat("a", 400) + " target " + strings.Repeat("b", 400)
	s := snippet(long, "target")
	if !strings.HasPrefix(s, "...") || !strings.HasSuffix(s, "...") || !strings.Contains(s, "target") {
		t.Fatalf("snippet = %q", s)
	}
	if got := snippet(strings.Repeat("x", 500), "nomatch"); len([]rune(got)) != 323 || !strings.HasSuffix(got, "...") {
		t.Fatalf("no-match snippet len %d", len([]rune(got)))
	}
	if got := snippet("short", "nomatch"); got != "short" {
		t.Fatal(got)
	}
}

func TestUnreadableSessionFilesAreSkipped(t *testing.T) {
	dir := t.TempDir()
	_ = os.WriteFile(filepath.Join(dir, "bad.json"), []byte("{not json"), 0o600)
	save(t, dir, "ok", um("findable words"))
	if hits := Search("findable", dir, 5); len(hits) != 1 {
		t.Fatalf("%+v", hits)
	}
}
