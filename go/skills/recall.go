package skills

// Finding passages in long skills by meaning. Each section is cut into passages of about a paragraph; every passage becomes a vector
// (the same local embedding model and compressed index as the vault's notes), made the first time a skill is searched and kept under
// $XDG_STATE_HOME/sarina/skill-index, so later searches only embed the question. Without the embedding model (Ollama not running, the
// model not pulled) the search is by words instead, so it always answers.

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"hash/fnv"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"

	"github.com/Artifex-Technologies/Sarina/go/vecindex"
)

// Embedder turns texts into vectors (vault.DefaultEmbedder is one).
type Embedder interface {
	Embed(texts []string) ([][]float32, error)
	Name() string
}

// Passage is one found piece of a long skill.
type Passage struct {
	Skill   string
	Section int // 1 first
	Title   string
	Text    string
	Score   float64
}

const (
	passageBytes = 1200
	// minPassageScore is the cosine similarity below which a passage is not offered by meaning.
	minPassageScore = 0.45
)

// Library searches the sections of the long skills in a catalog.
type Library struct {
	cat *Catalog
	emb Embedder // nil: by words only
	dir string   // where indexes are kept; "" = not kept

	mu       sync.Mutex
	indexes  map[string]*skillIndex
	noModel  bool // the embedder failed once: words only from then on
	lastNote string
	started  map[string]bool // skills the operator ran in this session (lowercase names)
}

// Start records that the operator ran this skill, so its sections may be read in this session even when the model may not invoke it.
func (l *Library) Start(name string) {
	if l == nil {
		return
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	l.started[strings.ToLower(name)] = true
}

// Readable reports whether the model may read this skill's sections: it may invoke the skill itself, or the operator ran it in this
// session. A skill only the operator may start (most collections downloaded from elsewhere) stays out of ordinary conversations.
func (l *Library) Readable(s *Skill) bool {
	if s == nil {
		return false
	}
	if s.ModelInvocable {
		return true
	}
	if l == nil {
		return false
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.started[strings.ToLower(s.Name)]
}

type skillIndex struct {
	ix   *vecindex.Index
	refs map[uint64]passageRef
}

type passageRef struct {
	section int // 0-based
	text    string
}

// NewLibrary searches c's long skills. stateDir is where the indexes are kept (normally $XDG_STATE_HOME/sarina/skill-index).
func NewLibrary(c *Catalog, emb Embedder, stateDir string) *Library {
	return &Library{cat: c, emb: emb, dir: stateDir, indexes: map[string]*skillIndex{}, started: map[string]bool{}}
}

// DefaultIndexDir is $XDG_STATE_HOME/sarina/skill-index (~/.local/state/... without it).
func DefaultIndexDir(getenv func(string) string, home string) string {
	base := ""
	if getenv != nil {
		base = getenv("XDG_STATE_HOME")
	}
	if base == "" {
		base = filepath.Join(home, ".local", "state")
	}
	return filepath.Join(base, "sarina", "skill-index")
}

// passages cuts a section into pieces of about a paragraph (short paragraphs joined, long ones cut).
func passages(sec Section) []string {
	var out []string
	var cur strings.Builder
	flush := func() {
		if t := strings.TrimSpace(cur.String()); t != "" {
			out = append(out, t)
		}
		cur.Reset()
	}
	for _, para := range strings.Split(sec.Text, "\n\n") {
		para = strings.TrimSpace(para)
		if para == "" {
			continue
		}
		if cur.Len() > 0 && cur.Len()+len(para) > passageBytes {
			flush()
		}
		for _, p := range cutParts(para, passageBytes) {
			if cur.Len() > 0 {
				cur.WriteString("\n\n")
			}
			cur.WriteString(p)
			if cur.Len() >= passageBytes {
				flush()
			}
		}
	}
	flush()
	return out
}

func passageID(skill string, section, i int) uint64 {
	h := fnv.New64a()
	fmt.Fprintf(h, "%s#%d#%d", skill, section, i)
	return h.Sum64()
}

// sum identifies a skill's text, so an index is made again when the skill changes.
func sum(s *Skill, model string) string {
	h := sha256.New()
	fmt.Fprintf(h, "%s\x00%s\x00", model, s.Name)
	for _, sec := range s.Sections {
		fmt.Fprintf(h, "%s\x00%s\x00", sec.Title, sec.Text)
	}
	return hex.EncodeToString(h.Sum(nil))[:24]
}

func unit(v []float32) []float32 {
	var t float64
	for _, x := range v {
		t += float64(x) * float64(x)
	}
	if t == 0 {
		return v
	}
	inv := float32(1 / math.Sqrt(t))
	out := make([]float32, len(v))
	for i, x := range v {
		out[i] = x * inv
	}
	return out
}

// index is the skill's vector index, loaded or made.
func (l *Library) index(s *Skill) (*skillIndex, error) {
	if si, ok := l.indexes[s.Name]; ok {
		return si, nil
	}
	refs := map[uint64]passageRef{}
	type piece struct {
		id   uint64
		text string
		sec  int
	}
	var pieces []piece
	for si, sec := range s.Sections {
		for i, p := range passages(sec) {
			id := passageID(s.Name, si, i)
			refs[id] = passageRef{section: si, text: p}
			pieces = append(pieces, piece{id, sec.Title + ": " + p, si})
		}
	}
	key := sum(s, l.emb.Name())
	dir := ""
	if l.dir != "" {
		dir = filepath.Join(l.dir, key)
		var meta struct {
			Model string `json:"model"`
			Count int    `json:"count"`
		}
		if b, err := os.ReadFile(filepath.Join(dir, "meta.json")); err == nil && json.Unmarshal(b, &meta) == nil && meta.Model == l.emb.Name() && meta.Count == len(pieces) {
			if ix, err := vecindex.Load(filepath.Join(dir, "passages.vec")); err == nil {
				si := &skillIndex{ix, refs}
				l.indexes[s.Name] = si
				return si, nil
			}
		}
	}
	var ix *vecindex.Index
	for i := 0; i < len(pieces); i += 32 {
		batch := pieces[i:min(i+32, len(pieces))]
		texts := make([]string, len(batch))
		for k, p := range batch {
			texts[k] = "search_document: " + p.text
		}
		vecs, err := l.emb.Embed(texts)
		if err != nil {
			return nil, err
		}
		for k, p := range batch {
			if ix == nil {
				if ix, err = vecindex.New(len(vecs[k]), 4, 1); err != nil {
					return nil, err
				}
			}
			if err := ix.Add(p.id, unit(vecs[k])); err != nil {
				return nil, err
			}
		}
	}
	if ix == nil {
		return nil, fmt.Errorf("skill %s has nothing to index", s.Name)
	}
	si := &skillIndex{ix, refs}
	l.indexes[s.Name] = si
	if dir != "" && os.MkdirAll(dir, 0o700) == nil {
		if ix.Save(filepath.Join(dir, "passages.vec")) == nil {
			b, _ := json.Marshal(map[string]any{"model": l.emb.Name(), "count": len(pieces), "skill": s.Name})
			_ = os.WriteFile(filepath.Join(dir, "meta.json"), b, 0o600)
		}
	}
	return si, nil
}

// Search finds up to limit passages for query, in one long skill (by name) or in all of them ("" ). The note says how it searched
// ("by meaning" or why by words).
func (l *Library) Search(skill, query string, limit int) ([]Passage, string, error) {
	if limit <= 0 {
		limit = 5
	}
	var list []*Skill
	if skill != "" {
		s, ok := l.cat.Get(skill)
		if !ok {
			return nil, "", fmt.Errorf("no skill named %q", skill)
		}
		if !s.IsLibrary() {
			return nil, "", fmt.Errorf("skill %q is short: all of it is in its prompt, there is nothing to search", s.Name)
		}
		if !l.Readable(s) {
			return nil, "", fmt.Errorf("skill %q can only be used when the operator runs it", s.Name)
		}
		list = []*Skill{s}
	} else if l.cat != nil {
		for _, s := range l.cat.Skills {
			if s.IsLibrary() && l.Readable(s) {
				list = append(list, s)
			}
		}
	}
	if len(list) == 0 || strings.TrimSpace(query) == "" {
		return nil, "", nil
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.emb != nil && !l.noModel {
		found, err := l.byMeaning(list, query, limit)
		if err == nil {
			return found, "by meaning", nil
		}
		l.noModel = true
		l.lastNote = "by words (the embedding model is not available: " + err.Error() + ")"
	}
	note := l.lastNote
	if note == "" {
		note = "by words"
	}
	return byWords(list, query, limit), note, nil
}

func (l *Library) byMeaning(list []*Skill, query string, limit int) ([]Passage, error) {
	qv, err := l.emb.Embed([]string{"search_query: " + query})
	if err != nil {
		return nil, err
	}
	q := unit(qv[0])
	var out []Passage
	for _, s := range list {
		si, err := l.index(s)
		if err != nil {
			return nil, err
		}
		hits, err := si.ix.Search(q, limit, nil)
		if err != nil {
			return nil, err
		}
		for _, h := range hits {
			ref, ok := si.refs[h.ID]
			if !ok || float64(h.Score) < minPassageScore {
				continue
			}
			out = append(out, Passage{Skill: s.Name, Section: ref.section + 1, Title: s.Sections[ref.section].Title, Text: ref.text, Score: float64(h.Score)})
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Score > out[j].Score })
	if len(out) > limit {
		out = out[:limit]
	}
	return out, nil
}

// byWords scores passages by how many of the question's words (three letters or more) they contain, rarer words counting more.
func byWords(list []*Skill, query string, limit int) []Passage {
	var words []string
	for _, w := range strings.FieldsFunc(strings.ToLower(query), func(r rune) bool {
		return !(r == '_' || r == '-' || r >= '0' && r <= '9' || r >= 'a' && r <= 'z' || r > 127)
	}) {
		if len(w) >= 3 {
			words = append(words, w)
		}
	}
	if len(words) == 0 {
		return nil
	}
	type cand struct {
		p     Passage
		lower string
	}
	var all []cand
	df := map[string]int{}
	for _, s := range list {
		for si, sec := range s.Sections {
			for _, p := range passages(sec) {
				low := strings.ToLower(sec.Title + " " + p)
				all = append(all, cand{Passage{Skill: s.Name, Section: si + 1, Title: sec.Title, Text: p}, low})
				for _, w := range words {
					if strings.Contains(low, w) {
						df[w]++
					}
				}
			}
		}
	}
	for i := range all {
		for _, w := range words {
			if n := strings.Count(all[i].lower, w); n > 0 {
				all[i].p.Score += (1 + math.Log(float64(n))) * math.Log(1+float64(len(all))/float64(1+df[w]))
			}
		}
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].p.Score > all[j].p.Score })
	var out []Passage
	for _, c := range all {
		if c.p.Score <= 0 || len(out) == limit {
			break
		}
		out = append(out, c.p)
	}
	return out
}

// passageBudget is how much of found passages goes into a long skill's prompt when it is run with a task; passageBand keeps only
// passages scoring close to the best (by words, scores are on another scale, so the band is relative there too).
const (
	passageBudget = 6 << 10
	passageBand   = 0.08
)

// contextBudget is the most that prefetched context adds to a prompt.
const contextBudget = 8 << 10

var (
	contextMu sync.RWMutex
	contexts  = map[string]func(task string) string{}
)

// RegisterContext makes a kind of context a skill can ask for with "prefetch" (e.g. "status", registered by package sysinfo): given
// the task, it returns text to put in front of the model, or "". Context is looked up before the model starts because the small local
// model answers from what is in front of it far more reliably than from rules telling it to go and look.
func RegisterContext(name string, fn func(task string) string) {
	contextMu.Lock()
	defer contextMu.Unlock()
	contexts[name] = fn
}

func (s *Skill) prefetched(task string) string {
	if len(s.Prefetch) == 0 || strings.TrimSpace(task) == "" {
		return ""
	}
	var b strings.Builder
	for _, name := range s.Prefetch {
		contextMu.RLock()
		fn := contexts[name]
		contextMu.RUnlock()
		if fn == nil {
			continue
		}
		if t := strings.TrimSpace(fn(task)); t != "" {
			b.WriteString(t)
			b.WriteString("\n\n")
		}
	}
	out := b.String()
	if len(out) > contextBudget {
		out = cutParts(out, contextBudget)[0] + "\n[…]\n\n"
	}
	return out
}

// ExpandWith is Expand for a run with a task: context the skill asks for (prefetch) goes first, and for a long skill the passages that
// fit the task best follow (up to passageBudget), so the model starts from what the computer says and the right part of the skill.
// Without a task it is Expand.
func (s *Skill) ExpandWith(args string, lib *Library) string {
	lib.Start(s.Name) // run by the operator (or invoked by the model, which may): its sections may be read now
	text := s.prefetched(args) + s.Expand(args)
	if lib == nil || !s.IsLibrary() || strings.TrimSpace(args) == "" {
		return text
	}
	found, _, err := lib.Search(s.Name, args, 8)
	if err != nil || len(found) == 0 {
		return text
	}
	// only passages close to the best one (the rest is noise to a small model), within the budget, then in reading order with one
	// heading per section
	keep := found[:0:0]
	used := 0
	for _, p := range found {
		if p.Score < found[0].Score-passageBand {
			break
		}
		if used+len(p.Text) > passageBudget {
			break
		}
		keep = append(keep, p)
		used += len(p.Text)
	}
	order := map[string]int{}
	for si, sec := range s.Sections {
		for i, t := range passages(sec) {
			order[fmt.Sprintf("%d\x00%s", si+1, t)] = i
		}
	}
	sort.SliceStable(keep, func(i, j int) bool {
		if keep[i].Section != keep[j].Section {
			return keep[i].Section < keep[j].Section
		}
		return order[fmt.Sprintf("%d\x00%s", keep[i].Section, keep[i].Text)] < order[fmt.Sprintf("%d\x00%s", keep[j].Section, keep[j].Text)]
	})
	var b strings.Builder
	b.WriteString(text)
	b.WriteString("\n\nThe parts of this skill that fit the task (open a whole section with skill_section when you need more):\n")
	last := -1
	for _, p := range keep {
		if p.Section != last {
			fmt.Fprintf(&b, "\n--- section %d: %s ---\n", p.Section, p.Title)
			last = p.Section
		}
		b.WriteString(p.Text)
		b.WriteString("\n")
	}
	return b.String()
}
