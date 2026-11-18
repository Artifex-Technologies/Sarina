package insight

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
	"unicode/utf8"
)

// Found while porting reflect and vault-ingest to Rust: byte cuts that split a letter, a capital made from half a letter,
// a state file of "null" that crashed vault-ingest, and a --batch that never ended.

func TestNoteTitleIsCutOnALetter(t *testing.T) {
	got := noteTitle(strings.Repeat("a", 69) + "éé more words")
	if !utf8.ValidString(got) || len(got) > 70 {
		t.Fatalf("noteTitle = %q", got)
	}
}

func TestMergeCutsAnObservationOnALetter(t *testing.T) {
	lines := []Line{{When: time.Date(2020, 1, 1, 10, 0, 0, 0, time.Local), Said: "I have a cat named Pixel"}}
	var n Note
	n.Merge([]Proposal{{Observation: strings.Repeat("a", 199) + "é and more", Quote: "I have a cat named Pixel"}}, lines, nil)
	if len(n.Observations) != 1 || !utf8.ValidString(n.Observations[0].Text) {
		t.Fatalf("observations %+v", n.Observations)
	}
}

func TestCapitalOfANameThatStartsWithAnAccent(t *testing.T) {
	t.Setenv("SARINA_OWNER_NAME", "émile")
	if p := prompt(); !utf8.ValidString(p) || !strings.Contains(p, "Émile has a cat") {
		t.Fatalf("prompt: %q", p[:200])
	}
	if r := (Note{}).Render(); !utf8.ValidString(r) || !strings.Contains(r, "Émile can edit it") {
		t.Fatalf("render: %q", r)
	}
}

func TestIngestStateOfNullDoesNotCrash(t *testing.T) {
	vault, state := t.TempDir(), t.TempDir()
	write(t, filepath.Join(vault, "Inbox", "article.md"), "Something to summarise.\n")
	write(t, filepath.Join(state, "i.json"), "null")
	var out, errb bytes.Buffer
	var calls int32
	rc := ingestCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "i.json"), "--force"})
	if rc != 0 || calls != 1 {
		t.Fatalf("rc %d, %d model calls, out %q, err %q", rc, calls, out.String(), errb.String())
	}
	if b, _ := os.ReadFile(filepath.Join(state, "i.json")); !strings.Contains(string(b), "Inbox/article.md") {
		t.Fatalf("state %q", b)
	}
}

func TestReflectBatchBelowOneIsRefused(t *testing.T) {
	for _, batch := range []string{"0", "-3"} {
		vault, state := t.TempDir(), t.TempDir()
		write(t, filepath.Join(vault, "Talks", "2020-01.md"), catLine)
		var out, errb bytes.Buffer
		var calls int32
		rc := within(t, "reflect --batch "+batch, func() (rc int) {
			defer func() {
				if recover() != nil {
					rc = -2
				}
			}()
			return reflectCommand(plantedEnv(&out, &errb, &calls), []string{"--path", vault, "--state", filepath.Join(state, "r.json"), "--force", "--batch", batch})
		})
		if rc != 2 || calls != 0 {
			t.Fatalf("--batch %s: rc %d, %d model calls, err %q", batch, rc, calls, errb.String())
		}
	}
}

// An inbox file whose name is not UTF-8 was summarised again every day: its name went into ingest.json with U+FFFD in place of
// the bad byte, so it never matched, and each run wrote another Sources/ note ("(2)", "(3)", ...). Now it is skipped, with why.
func TestInboxNameThatIsNotUTF8IsSkipped(t *testing.T) {
	vault := t.TempDir()
	write(t, filepath.Join(vault, "Inbox", "bad\xff.md"), "Something to summarise.\n")
	files, skipped := NewInboxFiles(vault, map[string]string{})
	if len(files) != 0 || len(skipped) != 1 || !strings.Contains(skipped[0], "not valid UTF-8") {
		t.Fatalf("files %d, skipped %q", len(files), skipped)
	}
}
