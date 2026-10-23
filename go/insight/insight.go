// Package insight is how Sarina learns about the person she talks with, not only remembers what was said (the idea of
// vectorize-io/hindsight's observations and mental models; no code taken). Once a day `sarina reflect` reads the talk log
// her desktop keeps in the vault (Talks/YYYY-MM.md: what they said, what she said) since the last time, asks the model what
// those lines show about them, and folds the answer into one note, Entities/About Zlothra.md:
//
//   - every observation must come with a quote of their own words, and the quote is checked against the talk log: an
//     observation whose quote is not really there is thrown away (the model cannot invent what they said)
//   - an observation already in the note is refined, not repeated: it is "seen" once more, its newest evidence kept
//   - the owner can edit the note; an observation moved under "## Forgotten" is never added again
//   - nothing that looks like a secret passes (the vault's own gate checks every line)
//
// Reading what she has learned is reading a note: no model call while she answers.
package insight

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// NoteName is the note she keeps about the owner.
const NoteName = "Entities/About Zlothra.md"

// Line is one exchange from the talk log.
type Line struct {
	When time.Time
	Said string // what the owner said
	Me   string // what she answered
}

var talkRx = regexp.MustCompile(`^- (\d{4}-\d{2}-\d{2} \d{2}:\d{2}) \| they said: (.*?) \| I said: (.*)$`)

// ReadTalks returns the exchanges in Talks/*.md after `since`, oldest first.
func ReadTalks(vault string, since time.Time) []Line {
	files, _ := filepath.Glob(filepath.Join(vault, "Talks", "20*.md"))
	sort.Strings(files)
	var out []Line
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err != nil {
			continue
		}
		for _, l := range strings.Split(string(b), "\n") {
			m := talkRx.FindStringSubmatch(strings.TrimSpace(l))
			if m == nil {
				continue
			}
			t, err := time.ParseInLocation("2006-01-02 15:04", m[1], time.Local)
			if err != nil || !t.After(since) {
				continue
			}
			out = append(out, Line{When: t, Said: m[2], Me: m[3]})
		}
	}
	return out
}

// Observation is one thing learned, with how often it has been seen and the words that show it.
type Observation struct {
	Text     string
	Seen     int
	Last     string   // YYYY-MM-DD
	Evidence []string // their own words, newest first, at most three
}

// Note is the parsed note.
type Note struct {
	Observations []Observation
	Forgotten    []string
}

var obsRx = regexp.MustCompile(`^- (.+?) — seen (\d+)×, last (\d{4}-\d{2}-\d{2})\.(?: Evidence: (.*))?$`)
var quoteRx = regexp.MustCompile(`"([^"]*)"`)

// ParseNote reads the note (an empty one when the text is empty).
func ParseNote(text string) Note {
	var n Note
	section := ""
	for _, l := range strings.Split(text, "\n") {
		l = strings.TrimSpace(l)
		if strings.HasPrefix(l, "## ") {
			section = strings.ToLower(strings.TrimSpace(l[3:]))
			continue
		}
		if !strings.HasPrefix(l, "- ") {
			continue
		}
		switch section {
		case "forgotten":
			t := strings.TrimSpace(l[2:])
			if m := obsRx.FindStringSubmatch(l); m != nil {
				t = m[1]
			}
			n.Forgotten = append(n.Forgotten, t)
		case "observations":
			m := obsRx.FindStringSubmatch(l)
			if m == nil {
				n.Observations = append(n.Observations, Observation{Text: strings.TrimSpace(l[2:]), Seen: 1}) // the owner wrote it by hand
				continue
			}
			seen, _ := strconv.Atoi(m[2])
			o := Observation{Text: m[1], Seen: seen, Last: m[3]}
			for _, q := range quoteRx.FindAllStringSubmatch(m[4], 3) {
				o.Evidence = append(o.Evidence, q[1])
			}
			n.Observations = append(n.Observations, o)
		}
	}
	return n
}

// Render writes the note's body (front matter is added by the vault).
func (n Note) Render() string {
	var b strings.Builder
	b.WriteString("# About Zlothra\n\nWhat I have learned from talking with Zlothra, each with how often I have seen it and their own words that show it. ")
	b.WriteString("I update this myself once a day. Zlothra can edit it: anything moved under Forgotten I will not add again.\n\n## Observations\n\n")
	obs := append([]Observation(nil), n.Observations...)
	sort.SliceStable(obs, func(i, j int) bool {
		if obs[i].Seen != obs[j].Seen {
			return obs[i].Seen > obs[j].Seen
		}
		return obs[i].Last > obs[j].Last
	})
	for _, o := range obs {
		fmt.Fprintf(&b, "- %s — seen %d×, last %s.", o.Text, o.Seen, o.Last)
		if len(o.Evidence) > 0 {
			var q []string
			for _, e := range o.Evidence {
				q = append(q, `"`+e+`"`)
			}
			b.WriteString(" Evidence: " + strings.Join(q, "; "))
		}
		b.WriteString("\n")
	}
	b.WriteString("\n## Forgotten\n\n")
	for _, f := range n.Forgotten {
		b.WriteString("- " + f + "\n")
	}
	return b.String()
}

// Proposal is what the model offered.
type Proposal struct {
	Observation string `json:"observation"`
	Quote       string `json:"quote"`
}

var objRx = regexp.MustCompile(`\{[^{}]*\}`)

// ParseProposals takes the JSON objects out of a reply (a small model wraps them in prose or a list as often as not).
func ParseProposals(reply string) []Proposal {
	reply = regexp.MustCompile(`(?s)<think>.*?</think>`).ReplaceAllString(reply, "")
	var out []Proposal
	for _, m := range objRx.FindAllString(reply, 40) {
		var p Proposal
		if json.Unmarshal([]byte(m), &p) == nil && strings.TrimSpace(p.Observation) != "" {
			out = append(out, p)
		}
	}
	return out
}

func norm(s string) string {
	s = strings.ToLower(s)
	s = strings.NewReplacer("’", "'", "‘", "'", "“", "", "”", "", `"`, "").Replace(s)
	return strings.Join(strings.FieldsFunc(s, func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || r == '\'')
	}), " ")
}

// grounded returns the talk line whose owner's words contain the quote, if any.
func grounded(quote string, lines []Line) (Line, bool) {
	q := norm(quote)
	if len(strings.Fields(q)) < 3 {
		return Line{}, false
	}
	for i := len(lines) - 1; i >= 0; i-- {
		if strings.Contains(norm(lines[i].Said), q) {
			return lines[i], true
		}
	}
	return Line{}, false
}

var stop = map[string]bool{"the": true, "a": true, "an": true, "and": true, "or": true, "of": true, "to": true, "in": true, "is": true, "are": true,
	"they": true, "their": true, "them": true, "zlothra": true, "has": true, "have": true, "with": true, "for": true, "on": true, "at": true,
	"likes": true, "like": true, "it": true, "that": true, "was": true, "be": true, "about": true}

func words(s string) map[string]bool {
	m := map[string]bool{}
	for _, w := range strings.Fields(norm(s)) {
		if !stop[w] && len(w) > 2 {
			m[strings.TrimSuffix(w, "s")] = true
		}
	}
	return m
}

// similar says whether two observations say the same thing (most of the shorter one's words in the other).
func similar(a, b string) bool {
	wa, wb := words(a), words(b)
	if len(wa) == 0 || len(wb) == 0 {
		return false
	}
	both := 0
	for w := range wa {
		if wb[w] {
			both++
		}
	}
	return float64(both) >= 0.6*float64(min(len(wa), len(wb)))
}

// Scanner vets one line before it is kept (the vault's gate).
type Scanner interface {
	Blocked(text string) bool
}

// Merge folds checked proposals into the note and says what happened to each.
func (n *Note) Merge(props []Proposal, lines []Line, gate Scanner) (added, refined int, dropped []string) {
	for _, p := range props {
		text := strings.TrimSpace(strings.TrimSuffix(strings.TrimSpace(p.Observation), "."))
		text = strings.NewReplacer("—", "-", "\n", " ").Replace(text)
		if len(text) > 200 {
			text = text[:200]
		}
		line, ok := grounded(p.Quote, lines)
		if !ok {
			dropped = append(dropped, fmt.Sprintf("%q: its quote is not in what they said", text))
			continue
		}
		quote := strings.ReplaceAll(strings.TrimSpace(p.Quote), `"`, "'")
		if gate != nil && (gate.Blocked(text) || gate.Blocked(quote)) {
			dropped = append(dropped, fmt.Sprintf("%q: the vault's gate refused it", text))
			continue
		}
		forgotten := false
		for _, f := range n.Forgotten {
			if similar(f, text) {
				forgotten = true
			}
		}
		if forgotten {
			dropped = append(dropped, fmt.Sprintf("%q: the owner asked me to forget this", text))
			continue
		}
		day := line.When.Format("2006-01-02")
		found := false
		for i := range n.Observations {
			o := &n.Observations[i]
			if !similar(o.Text, text) {
				continue
			}
			found = true
			dup := false
			for _, e := range o.Evidence {
				if norm(e) == norm(quote) {
					dup = true
				}
			}
			if dup {
				break // the same words seen again are not new evidence
			}
			o.Seen++
			if day > o.Last {
				o.Last = day
			}
			o.Evidence = append([]string{quote}, o.Evidence...)
			if len(o.Evidence) > 3 {
				o.Evidence = o.Evidence[:3]
			}
			refined++
			break
		}
		if !found {
			n.Observations = append(n.Observations, Observation{Text: text, Seen: 1, Last: day, Evidence: []string{quote}})
			added++
		}
	}
	return added, refined, dropped
}

// Completer is the model.
type Completer interface {
	Complete(ctx context.Context, req llm.Request, on func(llm.StreamEvent)) (*llm.Turn, error)
}

const prompt = `Below are things Zlothra said to you in conversation, with the date, and what you answered.
Write what these show about Zlothra as a person: what they like or dislike, people and pets in their life, what they are working on or planning, what worries them, what makes them laugh, how they like to be talked to.
Rules:
- Only what Zlothra said about themselves, not what you said.
- Each observation is one short sentence in the third person ("Zlothra has a cat named Pixel").
- Each needs a quote: Zlothra's exact words copied from the lines below, at least three words, that show it.
- Never anything about passwords, keys, account numbers or other secrets.
- At most 8. If nothing is worth keeping, write nothing.
Answer only with one JSON object per line: {"observation": "...", "quote": "..."}

`

// Ask asks the model for proposals about these lines.
func Ask(ctx context.Context, c Completer, lines []Line) ([]Proposal, error) {
	var b strings.Builder
	b.WriteString(prompt)
	for _, l := range lines {
		fmt.Fprintf(&b, "%s Zlothra: %s\n   You: %s\n", l.When.Format("2006-01-02 15:04"), l.Said, l.Me)
	}
	turn, err := c.Complete(ctx, llm.Request{Messages: []llm.Message{{Role: "user", Content: b.String()}}, MaxTokens: 3000}, nil)
	if err != nil {
		return nil, err
	}
	return ParseProposals(turn.Content), nil
}
