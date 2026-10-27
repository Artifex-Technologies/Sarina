package insight

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/vault"
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

func TestInboxFilesBecomeLinkedSourceNotes(t *testing.T) {
	root := t.TempDir()
	os.MkdirAll(filepath.Join(root, "Inbox"), 0o755)
	os.MkdirAll(filepath.Join(root, "Entities"), 0o755)
	os.WriteFile(filepath.Join(root, "Entities", "Gardening.md"), []byte("# Gardening\n\nTomatoes and basil, watered on Sundays.\n"), 0o644)
	os.WriteFile(filepath.Join(root, "Inbox", "tomatoes.md"), []byte("---\ntags: x\n---\nHow to grow tomatoes on a balcony: sun, water, basil nearby.\n"), 0o644)
	os.WriteFile(filepath.Join(root, "Inbox", "photo.png"), []byte("x"), 0o644)
	os.WriteFile(filepath.Join(root, "Inbox", ".hidden.md"), []byte("x"), 0o644)
	files, skipped := NewInboxFiles(root, map[string]string{})
	if len(files) != 1 || files[0].Rel != "Inbox/tomatoes.md" || strings.Contains(files[0].Text, "tags:") || len(skipped) != 1 {
		t.Fatalf("files %+v skipped %v", files, skipped)
	}
	if again, _ := NewInboxFiles(root, map[string]string{"Inbox/tomatoes.md": files[0].Hash}); len(again) != 0 {
		t.Error("read again although unchanged")
	}
	reply := `<think>x</think>{"title": "Growing tomatoes on a [[balcony]]/../x", "points": ["Tomatoes need sun.", "Basil grows well next to them."], "matters": "They garden on Sundays."}`
	s, err := Summarize(context.Background(), fakeModel{reply}, "tomatoes.md", files[0].Text)
	if err != nil {
		t.Fatal(err)
	}
	v, _ := vaultNotes(root)
	name := FreeName(root, s.Title)
	if name != "Sources/Growing tomatoes on a balcony x.md" {
		t.Errorf("name: %q", name)
	}
	body := s.Render(files[0].Rel, false, Related(v, s, name))
	if !strings.Contains(body, "From [[Inbox/tomatoes]]") || !strings.Contains(body, "- [[Entities/Gardening]]") || strings.Contains(body, "[[balcony]]") {
		t.Errorf("note:\n%s", body)
	}
	if _, err := Summarize(context.Background(), fakeModel{"I cannot do that."}, "x", "y"); err == nil {
		t.Error("no JSON accepted as a summary")
	}
}

func vaultNotes(root string) ([]vault.Note, error) {
	v, err := vault.Open(root)
	if err != nil {
		return nil, err
	}
	return v.Notes()
}

func TestHerOwnVoiceHeardBackIsNotTheOwner(t *testing.T) {
	// the 2026-10-23 morning: the speakers were on, and her answers came back through the microphone as "what they said"
	lines := []Line{
		{Said: "Hey, Sarina, I have...", Me: "Hey Zlothra, what's up? Are you ready to tackle whatever you need help with?"},
		{Said: "I am completely out of the loop on that conversation, but I am always ready to help you tackle whatever is on your mind.", Me: "Hey Zlothra, no worries—sometimes conversations take a few loops to catch up! What’s on your mind?"},
		{Said: "Hear your custom AI respond with such natural cadence and support is a seriously impressive technical achievement. I'm right here whenever you're ready to dive back into your projects or just keep chatting.", Me: "Your audio system is stable and ready—no hiccups detected."},
		{Said: "I'm standing by whenever you want to return to our conversation or dive into your creative projects.", Me: "Your network is stable and connected."},
		{Said: "This is actually called me Matt as Lothra is actually just my social media name.", Me: "Oh, I see! What's on your mind, Matt?"},
		{Said: "my cat Pixel knocked my coffee off the desk again", Me: "Pixel is running gravity tests."},
	}
	kept, echoes := WithoutEchoes(lines)
	if len(echoes) != 3 || len(kept) != 3 {
		t.Fatalf("kept %d, echoes %d", len(kept), len(echoes))
	}
	for _, k := range kept {
		if strings.Contains(k.Said, "standing by") || strings.Contains(k.Said, "custom AI") || strings.Contains(k.Said, "out of the loop") {
			t.Errorf("kept an echo: %q", k.Said)
		}
	}
	if !strings.Contains(kept[1].Said, "Matt") || !strings.Contains(kept[2].Said, "Pixel") {
		t.Errorf("the owner's own words were dropped: %+v", kept)
	}
	// a proposal that quotes an echo line is not grounded once the echoes are gone
	if _, ok := grounded("always ready to help you tackle", kept); ok {
		t.Error("an echo line grounded a quote")
	}
}

func TestTidyDropsOldSmallTalkAndEchoesKeepsTheRest(t *testing.T) {
	text := `# Talks 2026-09

What we talked about when it was not about the computer.

- 2026-09-01 08:00 | they said: Hey Sarina, just wanted to check and see how things were looking for you. | I said: Hey Zlothra, everything's looking good here! How can I assist you today?
- 2026-09-01 08:01 | they said: Yeah, thank you too. | I said: You're welcome! Have a great day!
- 2026-09-01 08:02 | they said: What time is it? | I said: It's 08:02 on Monday.
- 2026-09-01 08:03 | they said: This is actually called me Matt as Lothra is actually just my social media name. | I said: Oh, I see! What's on your mind, Matt?
- 2026-09-01 08:04 | they said: haha that was a good one | I said: I try.
- 2026-09-01 08:05 | they said: I really like the new dock, keep it | I said: Noted.
- 2026-09-01 08:06 | they said: I'm standing by whenever you want to return to our conversation. | I said: Your network is stable and connected.
- 2026-09-01 08:07 | they said: I was thinking about picking up drawing but I'm honestly terrible at it, any thoughts on where to start | I said: Everyone starts somewhere.
`
	now := time.Date(2026, 10, 25, 12, 0, 0, 0, time.Local)
	kept, dropped, why := Tidy(text, now, 30*24*time.Hour)
	if len(dropped) != 4 {
		t.Fatalf("dropped %d: %v", len(dropped), dropped)
	}
	joined := strings.Join(kept, "\n")
	for _, must := range []string{"Matt", "haha", "really like the new dock", "picking up drawing", "# Talks 2026-09"} {
		if !strings.Contains(joined, must) {
			t.Errorf("lost %q", must)
		}
	}
	for i, d := range dropped {
		if strings.Contains(d, "standing by") && why[i] != "my own voice heard back" {
			t.Errorf("echo reason: %s", why[i])
		}
	}
	// nothing older than the limit: only the echo goes
	_, dropped, _ = Tidy(text, time.Date(2026, 9, 2, 0, 0, 0, 0, time.Local), 30*24*time.Hour)
	if len(dropped) != 1 || !strings.Contains(dropped[0], "standing by") {
		t.Errorf("fresh small talk was dropped: %v", dropped)
	}
}

func TestTheOwnersNameAndPronouns(t *testing.T) {
	t.Setenv("SARINA_OWNER_NAME", "Matt")
	t.Setenv("SARINA_OWNER_PRONOUNS", "he/him")
	if NoteName() != "Entities/About Matt.md" {
		t.Fatalf("note: %s", NoteName())
	}
	p := prompt()
	if strings.Contains(p, "Zlothra") || !strings.Contains(p, "what he likes") || !strings.Contains(p, "Matt said") && !strings.Contains(p, "things Matt said") {
		t.Fatalf("prompt: %s", p[:300])
	}
	if r := (Note{}).Render(); !strings.HasPrefix(r, "# About Matt") || strings.Contains(r, "their own words") {
		t.Fatalf("render: %s", r[:200])
	}
	t.Setenv("SARINA_OWNER_NAME", "")
	t.Setenv("SARINA_OWNER_PRONOUNS", "")
	if o := OwnerFromEnv(); o.Name != "the owner" || o.Subject != "they" {
		t.Fatalf("default: %+v", o)
	}
}
