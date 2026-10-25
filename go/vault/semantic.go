package vault

// Recall by meaning: every note is turned into a vector by a small local embedding model (through Ollama) and kept in a compressed index
// (package vecindex, 4 bits a number), so "which program opens PDFs" finds the note about Default apps even when no word matches. The index
// lives outside the vault, is updated only for notes that changed, and the model is loaded only while it is used (Ollama unloads it after
// two minutes idle), so nothing stays in memory while you game or work. Off unless SARINA_SEMANTIC=on; if the model or Ollama is missing,
// search falls back to keywords.

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

	"github.com/Artifex-Technologies/Sarina/go/embed"
	"github.com/Artifex-Technologies/Sarina/go/vecindex"
)

// Embedder turns texts into vectors.
type Embedder interface {
	Embed(texts []string) ([][]float32, error)
	Name() string
}

// SemanticEnabled reports SARINA_SEMANTIC=on.
func SemanticEnabled() bool { return os.Getenv("SARINA_SEMANTIC") == "on" }

// DefaultEmbedder talks to Ollama (package embed: OLLAMA_HOST ..., model SARINA_EMBED_MODEL, else nomic-embed-text).
func DefaultEmbedder() Embedder { return embed.Default() }

type metaEntry struct {
	Sum string   `json:"sum"`
	IDs []uint64 `json:"ids"` // one vector per chunk (paragraph or list item)
}

type semMeta struct {
	Model  string               `json:"model"`
	Dim    int                  `json:"dim"`
	Notes  map[string]metaEntry `json:"notes"`  // note path -> its chunks
	Chunks map[string]string    `json:"chunks"` // chunk id (decimal) -> note path
}

// indexDir is where the index of one vault lives: $XDG_STATE_HOME/sarina/vault-index/<hash of the vault's path>.
func indexDir(v *Vault) string {
	base := os.Getenv("XDG_STATE_HOME")
	if base == "" {
		h, _ := os.UserHomeDir()
		base = filepath.Join(h, ".local", "state")
	}
	sum := sha256.Sum256([]byte(v.Root))
	return filepath.Join(base, "sarina", "vault-index", hex.EncodeToString(sum[:])[:16])
}

func chunkID(path string, i int) uint64 {
	h := fnv.New64a()
	fmt.Fprintf(h, "%s#%d", path, i)
	return h.Sum64()
}

func noteText(n Note) string { return n.Title + "\n" + n.Body }

// chunks splits a note into the pieces that are embedded: paragraphs, and each list item on its own (a guide is mostly lists, and a whole
// note's vector blurs its items together). Each piece carries the note's title for context.
func chunks(n Note) []string {
	var out []string
	var para []string
	flush := func() {
		if t := strings.TrimSpace(strings.Join(para, " ")); t != "" {
			out = append(out, t)
		}
		para = nil
	}
	for _, l := range strings.Split(n.Body, "\n") {
		t := strings.TrimSpace(l)
		switch {
		case t == "" || strings.HasPrefix(t, "#"):
			flush()
		case strings.HasPrefix(t, "- ") || strings.HasPrefix(t, "* "):
			flush()
			para = []string{strings.TrimSpace(t[2:])}
			flush()
		default:
			para = append(para, t)
		}
	}
	flush()
	if len(out) == 0 {
		out = []string{n.Title}
	}
	for i, c := range out {
		if r := []rune(c); len(r) > 600 {
			c = string(r[:600])
		}
		out[i] = n.Title + ": " + c
	}
	if len(out) > 60 {
		out = out[:60]
	}
	return out
}

func noteSum(n Note) string {
	s := sha256.Sum256([]byte(noteText(n)))
	return hex.EncodeToString(s[:8])
}

func normalize(v []float32) []float32 {
	var s float64
	for _, x := range v {
		s += float64(x) * float64(x)
	}
	if s == 0 {
		return v
	}
	inv := float32(1 / math.Sqrt(s))
	out := make([]float32, len(v))
	for i, x := range v {
		out[i] = x * inv
	}
	return out
}

// refresh brings the index up to date with the notes: the chunks of new and changed notes are embedded (in batches), those of deleted or
// changed notes removed.
func refresh(v *Vault, notes []Note, emb Embedder) (*vecindex.Index, map[uint64]chunkRef, error) {
	dir := indexDir(v)
	var meta semMeta
	var ix *vecindex.Index
	if b, err := os.ReadFile(filepath.Join(dir, "meta.json")); err == nil && json.Unmarshal(b, &meta) == nil && meta.Model == emb.Name() && meta.Chunks != nil {
		if loaded, err := vecindex.Load(filepath.Join(dir, "notes.vec")); err == nil && loaded.Dim() == meta.Dim {
			ix = loaded
		}
	}
	if ix == nil {
		meta = semMeta{Model: emb.Name(), Notes: map[string]metaEntry{}, Chunks: map[string]string{}}
	}
	type job struct {
		note  Note
		parts []string
	}
	var todo []job
	live := map[string]bool{}
	changed := false
	for _, n := range notes {
		live[n.Path] = true
		e, ok := meta.Notes[n.Path]
		if ok && e.Sum == noteSum(n) && ix != nil {
			continue
		}
		if ok && ix != nil {
			for _, id := range e.IDs {
				ix.Remove(id)
				delete(meta.Chunks, fmt.Sprint(id))
			}
		}
		delete(meta.Notes, n.Path)
		todo = append(todo, job{n, chunks(n)})
		changed = true
	}
	for p, e := range meta.Notes {
		if !live[p] {
			for _, id := range e.IDs {
				if ix != nil {
					ix.Remove(id)
				}
				delete(meta.Chunks, fmt.Sprint(id))
			}
			delete(meta.Notes, p)
			changed = true
		}
	}
	// flatten the chunks to embed, 32 at a time
	type piece struct {
		note Note
		i    int
		text string
	}
	var pieces []piece
	for _, j := range todo {
		for i, c := range j.parts {
			pieces = append(pieces, piece{j.note, i, c})
		}
		meta.Notes[j.note.Path] = metaEntry{Sum: noteSum(j.note)}
	}
	for i := 0; i < len(pieces); i += 32 {
		batch := pieces[i:min(i+32, len(pieces))]
		texts := make([]string, len(batch))
		for k, p := range batch {
			texts[k] = "search_document: " + p.text
		}
		vecs, err := emb.Embed(texts)
		if err != nil {
			return nil, nil, err
		}
		for k, p := range batch {
			if ix == nil {
				if ix, err = vecindex.New(len(vecs[k]), 4, 1); err != nil {
					return nil, nil, err
				}
				meta.Dim = len(vecs[k])
			}
			id := chunkID(p.note.Path, p.i)
			if err := ix.Add(id, normalize(vecs[k])); err != nil {
				return nil, nil, err
			}
			e := meta.Notes[p.note.Path]
			e.IDs = append(e.IDs, id)
			meta.Notes[p.note.Path] = e
			meta.Chunks[fmt.Sprint(id)] = p.note.Path
		}
	}
	refs := map[uint64]chunkRef{}
	for p, e := range meta.Notes {
		for i, id := range e.IDs {
			refs[id] = chunkRef{path: p, index: i}
		}
	}
	if ix == nil {
		return nil, refs, nil
	}
	if changed {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return nil, nil, err
		}
		if err := ix.Save(filepath.Join(dir, "notes.vec")); err != nil {
			return nil, nil, err
		}
		b, _ := json.Marshal(meta)
		tmp := filepath.Join(dir, ".meta.tmp")
		if err := os.WriteFile(tmp, b, 0o600); err != nil {
			return nil, nil, err
		}
		if err := os.Rename(tmp, filepath.Join(dir, "meta.json")); err != nil {
			return nil, nil, err
		}
	}
	return ix, refs, nil
}

type chunkRef struct {
	path  string
	index int
}

// minMeaning is the cosine similarity below which a note is not offered as a match by meaning; meaningBand keeps only notes close to the best.
const (
	minMeaning  = 0.55
	meaningBand = 0.08
)

// SemanticHits finds notes close in meaning to the query: each note scores by its closest chunk, and that chunk is the snippet.
func SemanticHits(v *Vault, notes []Note, query string, limit int, emb Embedder) ([]Hit, error) {
	if strings.TrimSpace(query) == "" || len(notes) == 0 {
		return nil, nil
	}
	ix, refs, err := refresh(v, notes, emb)
	if err != nil || ix == nil {
		return nil, err
	}
	qv, err := emb.Embed([]string{"search_query: " + query})
	if err != nil {
		return nil, err
	}
	found, err := ix.Search(normalize(qv[0]), limit*6, nil)
	if err != nil {
		return nil, err
	}
	byPath := map[string]Note{}
	for _, n := range notes {
		byPath[n.Path] = n
	}
	var out []Hit
	seen := map[string]bool{}
	top := 0.0
	for _, f := range found {
		ref, ok := refs[f.ID]
		n, ok2 := byPath[ref.path]
		if !ok || !ok2 || seen[ref.path] {
			continue
		}
		score := float64(f.Score)
		if top == 0 {
			top = score
		}
		// only notes near the best one, and above a floor: below that everything is "somewhat related"
		if score < minMeaning || score < top-meaningBand {
			break
		}
		seen[ref.path] = true
		snip := ""
		if parts := chunks(n); ref.index < len(parts) {
			snip = strings.TrimPrefix(parts[ref.index], n.Title+": ")
			if r := []rune(snip); len(r) > 200 {
				snip = string(r[:200]) + "…"
			}
		}
		out = append(out, Hit{Path: n.Path, Title: n.Title, Score: score, Snippet: snip, Why: []string{"meaning"}})
		if len(out) == limit {
			break
		}
	}
	return out, nil
}

func firstLine(body string) string {
	for _, l := range strings.Split(body, "\n") {
		l = strings.TrimSpace(l)
		if l != "" && !strings.HasPrefix(l, "#") {
			if r := []rune(l); len(r) > 200 {
				return string(r[:200]) + "…"
			}
			return l
		}
	}
	return ""
}

// Merge joins keyword hits and meaning hits by reciprocal rank (the same way Search joins its own strategies), keeping every reason.
func Merge(keyword, meaning []Hit, limit int) []Hit {
	type acc struct {
		h   Hit
		rrf float64
	}
	all := map[string]*acc{}
	add := func(list []Hit, weight float64) {
		for rank, h := range list {
			a, ok := all[h.Path]
			if !ok {
				a = &acc{h: h}
				all[h.Path] = a
			} else {
				for _, w := range h.Why {
					if !contains(a.h.Why, w) {
						a.h.Why = append(a.h.Why, w)
					}
				}
				if a.h.Snippet == "" {
					a.h.Snippet = h.Snippet
				}
			}
			a.rrf += weight / (60 + float64(rank+1))
		}
	}
	// a match by meaning that passed its floor counts double: keyword ranks also carry "linked" and "recent", which find neighbours, not answers
	add(keyword, 1)
	add(meaning, 2)
	out := make([]*acc, 0, len(all))
	for _, a := range all {
		out = append(out, a)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].rrf > out[j].rrf })
	res := make([]Hit, 0, min(limit, len(out)))
	for i := 0; i < len(out) && i < limit; i++ {
		h := out[i].h
		h.Score = out[i].rrf
		res = append(res, h)
	}
	return res
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

// Recall is what the tool and the command use: keywords, plus meaning when it is switched on and works.
func Recall(v *Vault, notes []Note, query string, limit int) []Hit {
	kw := Search(notes, query, limit)
	if !SemanticEnabled() {
		return kw
	}
	sem, err := SemanticHits(v, notes, query, limit, DefaultEmbedder())
	if err != nil || len(sem) == 0 {
		return kw
	}
	return Merge(kw, sem, limit)
}
