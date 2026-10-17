package vault

import (
	"math"
	"sort"
	"strings"
	"unicode"
)

// hubLinks: a note with more links than this is a hub (an index), not evidence that its neighbours are related.
const hubLinks = 8

// Hit is one search result.
type Hit struct {
	Path    string
	Title   string
	Score   float64
	Snippet string
	// Why says which strategies found it: "keywords", "linked" (a neighbour of a keyword hit), "recent".
	Why []string
}

func tokens(s string) []string {
	return strings.FieldsFunc(strings.ToLower(s), func(r rune) bool { return !unicode.IsLetter(r) && !unicode.IsDigit(r) })
}

var stop = map[string]bool{"the": true, "a": true, "an": true, "and": true, "or": true, "of": true, "to": true, "in": true, "on": true, "is": true, "it": true, "for": true, "with": true, "that": true, "this": true, "was": true, "are": true, "be": true, "as": true, "at": true, "by": true}

func terms(s string) []string {
	var out []string
	for _, t := range tokens(s) {
		if len(t) > 1 && !stop[t] {
			out = append(out, t)
		}
	}
	return out
}

// Search finds notes for a query: keyword ranking (BM25, titles and tags count more), the link graph (notes next to the best keyword hits),
// and recency, merged by reciprocal rank fusion, the way Hindsight merges its retrieval strategies.
func Search(notes []Note, query string, limit int) []Hit {
	q := terms(query)
	if len(q) == 0 || len(notes) == 0 {
		return nil
	}
	if limit <= 0 {
		limit = 8
	}
	// term frequencies, with titles and tags counted three and two times
	type doc struct {
		tf  map[string]float64
		len float64
	}
	docs := make([]doc, len(notes))
	df := map[string]int{}
	var total float64
	for i, n := range notes {
		d := doc{tf: map[string]float64{}}
		for _, t := range terms(n.Title) {
			d.tf[t] += 3
			d.len += 3
		}
		for _, tg := range n.Tags {
			for _, t := range terms(tg) {
				d.tf[t] += 2
				d.len += 2
			}
		}
		for _, t := range terms(n.Body) {
			d.tf[t]++
			d.len++
		}
		for t := range d.tf {
			df[t]++
		}
		docs[i] = d
		total += d.len
	}
	avg := total / float64(len(notes))
	const k1, b = 1.2, 0.75
	bm := make([]float64, len(notes))
	for i, d := range docs {
		for _, t := range q {
			f := d.tf[t]
			if f == 0 {
				continue
			}
			idf := math.Log(1 + (float64(len(notes))-float64(df[t])+0.5)/(float64(df[t])+0.5))
			bm[i] += idf * f * (k1 + 1) / (f + k1*(1-b+b*d.len/math.Max(avg, 1)))
		}
	}
	// a note whose TITLE holds the query words is almost certainly what was asked for
	for i, n := range notes {
		if bm[i] == 0 {
			continue
		}
		in := map[string]bool{}
		for _, t := range terms(n.Title) {
			in[t] = true
		}
		hit := 0
		for _, t := range q {
			if in[t] {
				hit++
			}
		}
		bm[i] *= 1 + 2*float64(hit)/float64(len(q))
	}
	kw := rank(bm)
	// the link graph: notes the five best keyword hits link to or are linked from, that the keywords did not already find (an expansion)
	index := map[string]int{}
	for i, n := range notes {
		index[n.Path] = i
	}
	top := kw
	if len(top) > 5 {
		top = top[:5]
	}
	topSet := map[int]bool{}
	for _, i := range top {
		topSet[i] = true
	}
	graph := make([]float64, len(notes))
	hub := hubLinks
	if f := len(notes) / 5; f > hub {
		hub = f
	}
	for i, n := range notes {
		if len(n.Links) > hub {
			continue // an index note links to everything: that says nothing about which notes are related
		}
		for _, l := range n.Links {
			if j, ok := index[Resolve(notes, l)]; ok {
				if len(notes[j].Links) > hub {
					continue // and a link TO an index says nothing either
				}
				if topSet[j] && !topSet[i] && i != j {
					graph[i]++ // i links to a top hit
				}
				if topSet[i] && !topSet[j] && i != j {
					graph[j]++ // a top hit links to j
				}
			}
		}
	}
	gr := rank(graph)
	// recency among notes that matched at all (the list is already newest first)
	matched := []int{}
	for i := range notes {
		if bm[i] > 0 || graph[i] > 0 {
			matched = append(matched, i)
		}
	}
	score := map[int]float64{}
	why := map[int]map[string]bool{}
	note := func(i int, s float64, w string) {
		score[i] += s
		if why[i] == nil {
			why[i] = map[string]bool{}
		}
		why[i][w] = true
	}
	for r, i := range kw {
		note(i, 1/float64(60+r), "keywords")
	}
	// keywords count most; a neighbour of a hit is supporting evidence; recency only breaks near-ties
	for r, i := range gr {
		note(i, 0.5/float64(60+r), "linked")
	}
	for r, i := range matched {
		note(i, 0.05/float64(60+r), "recent")
	}
	var order []int
	for i := range score {
		order = append(order, i)
	}
	sort.Slice(order, func(a, c int) bool {
		if score[order[a]] != score[order[c]] {
			return score[order[a]] > score[order[c]]
		}
		return notes[order[a]].Path < notes[order[c]].Path
	})
	if len(order) > limit {
		order = order[:limit]
	}
	var hits []Hit
	for _, i := range order {
		h := Hit{Path: notes[i].Path, Title: notes[i].Title, Score: score[i], Snippet: snippet(notes[i].Body, q)}
		for _, w := range []string{"keywords", "linked", "recent"} {
			if why[i][w] {
				h.Why = append(h.Why, w)
			}
		}
		hits = append(hits, h)
	}
	return hits
}

// rank returns the indexes with a positive score, best first.
func rank(scores []float64) []int {
	var idx []int
	for i, s := range scores {
		if s > 0 {
			idx = append(idx, i)
		}
	}
	sort.SliceStable(idx, func(a, b int) bool { return scores[idx[a]] > scores[idx[b]] })
	return idx
}

// snippet is the first line of the note that holds a query word (or its first line), cut to a sensible length.
func snippet(body string, q []string) string {
	lines := strings.Split(body, "\n")
	pick := ""
	for _, l := range lines {
		l = strings.TrimSpace(l)
		if l == "" || strings.HasPrefix(l, "# ") {
			continue
		}
		if pick == "" {
			pick = l
		}
		low := strings.ToLower(l)
		for _, t := range q {
			if strings.Contains(low, t) {
				return cut(l, 200)
			}
		}
	}
	return cut(pick, 200)
}

func cut(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + "…"
}

// Backlinks lists the notes that link to the given note.
func Backlinks(notes []Note, path string) []string {
	var out []string
	for _, n := range notes {
		for _, l := range n.Links {
			if Resolve(notes, l) == path && n.Path != path {
				out = append(out, n.Path)
				break
			}
		}
	}
	sort.Strings(out)
	return out
}
