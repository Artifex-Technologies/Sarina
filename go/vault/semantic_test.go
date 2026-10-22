package vault

import (
	"hash/fnv"
	"strings"
	"testing"
)

// bagEmbedder: words hashed into 64 buckets, enough to make "default apps pdf" close to a note about default apps and pdf.
type bagEmbedder struct{ calls, texts int }

func (b *bagEmbedder) Name() string { return "bag" }
func (b *bagEmbedder) Embed(texts []string) ([][]float32, error) {
	b.calls++
	b.texts += len(texts)
	out := make([][]float32, len(texts))
	for i, t := range texts {
		v := make([]float32, 64)
		for _, w := range strings.Fields(strings.ToLower(t)) {
			w = strings.Trim(w, ".,:;!?\"'()")
			if len(w) < 3 || w == "search_document" || w == "search_query" {
				continue
			}
			h := fnv.New32a()
			h.Write([]byte(w))
			v[h.Sum32()%64]++
		}
		out[i] = v
	}
	return out, nil
}

func TestSemanticFindsByMeaningAndUpdatesIncrementally(t *testing.T) {
	t.Setenv("XDG_STATE_HOME", t.TempDir())
	v := tmpVault(t)
	put(t, v, "Guides/Settings pages.md", "# Settings pages\n\nDefault apps: which program opens pdf files, pictures and music.\n", 0)
	put(t, v, "Guides/Shortcuts.md", "# Shortcuts\n\nSuper key opens the start menu; lock with meta l.\n", 0)
	put(t, v, "Lessons/Disk.md", "# Disk\n\nCheck free space with df before copies.\n", 0)
	notes, _ := v.Notes()
	emb := &bagEmbedder{}
	hits, err := SemanticHits(v, notes, "which program opens pdf files", 3, emb)
	if err != nil || len(hits) == 0 || hits[0].Path != "Guides/Settings pages.md" {
		t.Fatalf("%v %+v", err, hits)
	}
	first := emb.texts
	// nothing changed: only the query is embedded again
	SemanticHits(v, notes, "start menu", 3, emb)
	if emb.texts != first+1 {
		t.Fatalf("re-embedded unchanged notes: %d -> %d", first, emb.texts)
	}
	// one note changes: only it (and the query) are embedded
	put(t, v, "Lessons/Disk.md", "# Disk\n\nCheck free space with df before big copies, always.\n", 0)
	notes, _ = v.Notes()
	SemanticHits(v, notes, "free space", 3, emb)
	if emb.texts != first+1+2 {
		t.Fatalf("expected one note and one query embedded, got %d new", emb.texts-first-1)
	}
}

func TestMergeKeepsBothReasons(t *testing.T) {
	kw := []Hit{{Path: "a.md", Why: []string{"keywords"}}, {Path: "b.md", Why: []string{"keywords"}}}
	sem := []Hit{{Path: "b.md", Why: []string{"meaning"}}, {Path: "c.md", Why: []string{"meaning"}}}
	m := Merge(kw, sem, 5)
	if len(m) != 3 || m[0].Path != "b.md" || len(m[0].Why) != 2 {
		t.Fatalf("%+v", m)
	}
}
