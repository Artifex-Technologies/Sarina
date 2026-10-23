package insight

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

func talks(t *testing.T) string {
	d := t.TempDir()
	os.MkdirAll(filepath.Join(d, "Talks"), 0o755)
	os.WriteFile(filepath.Join(d, "Talks", "2026-10.md"), []byte(`# Talks 2026-10

- 2026-10-20 21:14 | they said: my cat Pixel knocked my coffee off the desk again | I said: Pixel is running gravity tests.
- 2026-10-21 22:40 | they said: I'm thinking about picking up drawing but I'm honestly terrible at it | I said: Everyone starts somewhere.
- 2026-10-22 23:10 | they said: Pixel woke me up at five again, that cat | I said: A dedicated alarm.
`), 0o644)
	return d
}

type fakeModel struct{ reply string }

func (f fakeModel) Complete(context.Context, llm.Request, func(llm.StreamEvent)) (*llm.Turn, error) {
	return &llm.Turn{Content: f.reply}, nil
}

type noGate struct{}

func (noGate) Blocked(text string) bool { return strings.Contains(text, "password") }

func TestOnlyGroundedObservationsAreKeptAndRepeatsAreRefined(t *testing.T) {
	lines := ReadTalks(talks(t), time.Date(2026, 10, 20, 0, 0, 0, 0, time.Local))
	if len(lines) != 3 || lines[0].Said != "my cat Pixel knocked my coffee off the desk again" {
		t.Fatalf("talks: %+v", lines)
	}
	reply := `<think>hmm</think>Here you go:
{"observation": "Zlothra has a cat named Pixel", "quote": "my cat Pixel knocked my coffee"}
{"observation": "Zlothra wants to start drawing", "quote": "thinking about picking up drawing"}
{"observation": "Zlothra lives in Lisbon", "quote": "I live in Lisbon"}
{"observation": "Zlothra's password is hunter2", "quote": "my cat Pixel knocked"}
{"observation": "Zlothra has a cat called Pixel.", "quote": "Pixel woke me up at five again"}`
	props, err := Ask(context.Background(), fakeModel{reply}, lines)
	if err != nil || len(props) != 5 {
		t.Fatalf("proposals: %v %v", props, err)
	}
	var n Note
	added, refined, dropped := n.Merge(props, lines, noGate{})
	if added != 2 || refined != 1 || len(dropped) != 2 {
		t.Fatalf("added %d refined %d dropped %v", added, refined, dropped)
	}
	if !strings.Contains(strings.Join(dropped, " "), "Lisbon") {
		t.Errorf("an invented quote was kept: %v", dropped)
	}
	cat := n.Observations[0]
	if cat.Seen != 2 || cat.Last != "2026-10-22" || len(cat.Evidence) != 2 || cat.Evidence[0] != "Pixel woke me up at five again" {
		t.Errorf("the cat: %+v", cat)
	}

	// the note round-trips, and the same words again are not new evidence
	n2 := ParseNote(n.Render())
	if len(n2.Observations) != 2 || n2.Observations[0].Seen != 2 || len(n2.Observations[0].Evidence) != 2 {
		t.Fatalf("round trip: %+v", n2)
	}
	if _, r, _ := n2.Merge([]Proposal{{"Zlothra has a cat named Pixel", "Pixel woke me up at five again"}}, lines, noGate{}); r != 0 {
		t.Error("the same quote counted twice")
	}

	// what the owner moves under Forgotten does not come back
	edited := strings.Replace(n.Render(), "## Forgotten\n", "## Forgotten\n\n- Zlothra wants to start drawing\n", 1)
	edited = strings.Replace(edited, "- Zlothra wants to start drawing — seen", "- (moved) x — seen", 1)
	n3 := ParseNote(edited)
	if a, _, d := n3.Merge([]Proposal{{"Zlothra wants to start drawing", "thinking about picking up drawing"}}, lines, noGate{}); a != 0 || len(d) != 1 || !strings.Contains(d[0], "forget") {
		t.Errorf("forgotten came back: %d %v", a, d)
	}
}

func TestShortOrMissingQuotesAreNotEvidence(t *testing.T) {
	lines := []Line{{When: time.Now(), Said: "I love rainy days"}}
	if _, ok := grounded("rainy", lines); ok {
		t.Error("one word is not a quote")
	}
	if _, ok := grounded("I love rainy days", lines); !ok {
		t.Error("the whole sentence")
	}
	if _, ok := grounded("I “love” rainy  days!", lines); !ok {
		t.Error("quotes and punctuation are not differences")
	}
	if got := ParseProposals("no json here"); len(got) != 0 {
		t.Error(got)
	}
}
