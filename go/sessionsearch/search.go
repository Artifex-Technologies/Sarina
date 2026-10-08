// Package sessionsearch is lexical search over saved sessions (the Go session
// files, messages as objects). The index is rebuilt in memory per call: session
// directories are single-operator scale and a persistent index is not worth its
// upkeep. No SQLite: the query is matched as one literal phrase (whole words,
// case-insensitive) and ranked with a small BM25; if no message contains the
// phrase, a term-count scan over the whitespace-split query is used instead.
//
// Deviations from the Python original, on purpose: system messages and
// <system-reminder> context envelopes are not indexed (every session carries
// the same system prompt, so searching for a common word would just return it).
package sessionsearch

import (
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

const snippetRadius = 160

// Hit is one matching message.
type Hit struct {
	SessionID string
	Role      string
	TurnIndex int
	Snippet   string
	// Score: lower is better (BM25 value, or the negated match count).
	Score float64
}

type row struct {
	session string
	turn    int
	role    string
	content string
}

// Search returns up to topK hits for query over every *.json in dir.
func Search(query, dir string, topK int) []Hit {
	query = strings.TrimSpace(query)
	if query == "" {
		return nil
	}
	if topK <= 0 {
		topK = 10
	}
	rows := load(dir)
	if len(rows) == 0 {
		return nil
	}
	if hits := phrase(query, rows, topK); len(hits) > 0 {
		return hits
	}
	return terms(query, rows, topK)
}

func load(dir string) []row {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	type f struct {
		name string
		mod  int64
	}
	var files []f
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		if info, err := e.Info(); err == nil {
			files = append(files, f{e.Name(), info.ModTime().UnixNano()})
		}
	}
	sort.SliceStable(files, func(i, j int) bool { return files[i].mod > files[j].mod })
	var rows []row
	for _, fl := range files {
		raw, err := os.ReadFile(filepath.Join(dir, fl.name))
		if err != nil {
			continue
		}
		// Only the messages are needed, so read them leniently rather than through
		// the strict session loader: a file that lacks some other key still searches.
		var file struct {
			Messages []struct {
				Role    string `json:"role"`
				Content any    `json:"content"`
			} `json:"messages"`
		}
		if json.Unmarshal(raw, &file) != nil {
			continue
		}
		id := strings.TrimSuffix(fl.name, ".json")
		for i, m := range file.Messages {
			content, _ := m.Content.(string)
			if m.Role == "" || m.Role == "system" || strings.HasPrefix(content, session.ReminderOpen) || strings.TrimSpace(content) == "" {
				continue
			}
			rows = append(rows, row{id, i, m.Role, content})
		}
	}
	return rows
}

func words(s string) []string {
	return strings.FieldsFunc(strings.ToLower(s), func(r rune) bool { return !unicode.IsLetter(r) && !unicode.IsDigit(r) })
}

func countPhrase(doc, q []string) int {
	n := 0
	for i := 0; i+len(q) <= len(doc); i++ {
		ok := true
		for j := range q {
			if doc[i+j] != q[j] {
				ok = false
				break
			}
		}
		if ok {
			n++
		}
	}
	return n
}

func phrase(query string, rows []row, topK int) []Hit {
	q := words(query)
	if len(q) == 0 {
		return nil
	}
	counts := make([]int, len(rows))
	lens := make([]int, len(rows))
	total, matching := 0, 0
	for i, r := range rows {
		d := words(r.content)
		lens[i] = len(d)
		total += len(d)
		counts[i] = countPhrase(d, q)
		if counts[i] > 0 {
			matching++
		}
	}
	if matching == 0 {
		return nil
	}
	avg := float64(total) / float64(len(rows))
	if avg == 0 {
		avg = 1
	}
	idf := math.Log((float64(len(rows))-float64(matching)+0.5)/(float64(matching)+0.5) + 1)
	const k1, b = 1.2, 0.75
	var hits []Hit
	for i, r := range rows {
		if counts[i] == 0 {
			continue
		}
		tf := float64(counts[i])
		score := -idf * tf * (k1 + 1) / (tf + k1*(1-b+b*float64(lens[i])/avg))
		hits = append(hits, Hit{r.session, r.role, r.turn, snippet(r.content, query), score})
	}
	sort.SliceStable(hits, func(i, j int) bool { return hits[i].Score < hits[j].Score })
	if len(hits) > topK {
		hits = hits[:topK]
	}
	return hits
}

func terms(query string, rows []row, topK int) []Hit {
	ts := strings.Fields(strings.ToLower(query))
	var hits []Hit
	for _, r := range rows {
		low := strings.ToLower(r.content)
		n := 0
		for _, t := range ts {
			n += strings.Count(low, t)
		}
		if n > 0 {
			hits = append(hits, Hit{r.session, r.role, r.turn, snippet(r.content, query), -float64(n)})
		}
	}
	sort.SliceStable(hits, func(i, j int) bool { return hits[i].Score < hits[j].Score })
	if len(hits) > topK {
		hits = hits[:topK]
	}
	return hits
}

func lowerRunes(s string) []rune {
	r := []rune(s)
	for i := range r {
		r[i] = unicode.ToLower(r[i])
	}
	return r
}

func indexRunes(hay, needle []rune) int {
	if len(needle) == 0 {
		return -1
	}
	for i := 0; i+len(needle) <= len(hay); i++ {
		match := true
		for j := range needle {
			if hay[i+j] != needle[j] {
				match = false
				break
			}
		}
		if match {
			return i
		}
	}
	return -1
}

// snippet shows snippetRadius characters either side of the first query term
// found (terms tried in order); with no match, the first 2*radius characters.
func snippet(content, query string) string {
	c := []rune(content)
	low := lowerRunes(content)
	pos := -1
	for _, t := range strings.Fields(strings.ToLower(query)) {
		if pos = indexRunes(low, lowerRunes(t)); pos != -1 {
			break
		}
	}
	if pos == -1 {
		n := min(len(c), snippetRadius*2)
		if len(c) <= n {
			return string(c[:n])
		}
		return string(c[:n]) + "..."
	}
	start := max(0, pos-snippetRadius)
	end := min(len(c), pos+snippetRadius)
	out := strings.TrimSpace(string(c[start:end]))
	if start > 0 {
		out = "..." + out
	}
	if end < len(c) {
		out += "..."
	}
	return out
}
