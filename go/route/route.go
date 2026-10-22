// Package route sorts a spoken question into what to look up before the model starts (the idea of Laya's "System 1" decisions: one fast pass,
// no text generated), using the small embedding model Sarina already has for her notes instead of a 400M-parameter classifier. Each topic is a
// few example questions; a question goes to the topics it is closest to in meaning. A miss costs nothing but a lookup she does herself later.
package route

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// Embedder turns texts into vectors (vault.DefaultEmbedder satisfies it).
type Embedder interface {
	Embed(texts []string) ([][]float32, error)
	Name() string
}

// Topics: what a question can need. The names are system_status topics, plus "guides" (her notes about this desktop) and "chat" (nothing).
var Topics = map[string][]string{
	"performance": {"why is my computer so slow", "the system feels laggy and sluggish", "my games stutter and drop frames", "something is using all the cpu", "the fans are loud and it is hot", "how much memory is free"},
	"network":     {"is my internet working", "the wifi keeps disconnecting", "I can't reach any websites", "my connection is slow", "am I online"},
	"disk":        {"is my disk full", "how much storage space do I have left", "I am running out of space", "which drive is almost full"},
	"crashes":     {"did anything crash", "an app keeps closing by itself", "a program crashed when I opened it", "why did my game quit suddenly", "something froze and closed"},
	"boot":        {"why does it take so long to start", "booting is slow", "what slows down startup"},
	"audio":       {"there is no sound", "my microphone doesn't work", "the speakers are silent", "audio crackles"},
	"services":    {"are there any errors on my system", "something is broken since the update", "is everything working properly", "any problems I should know about"},
	"guides":      {"how do I change the default app for pdf files", "where is the setting for the wallpaper", "how do I sign in to proton drive", "what is the shortcut to talk to you", "how do I lock the screen", "where do I change the power mode", "how do I install an app"},
	"chat":        {"tell me a joke", "how are you today", "what is the capital of france", "thank you", "good morning", "write me a short poem"},
}

const (
	floor = 0.62 // below this a topic is not offered
	band  = 0.05 // a second topic must be this close to the best
)

func cacheFile(model string) string {
	base := os.Getenv("XDG_STATE_HOME")
	if base == "" {
		h, _ := os.UserHomeDir()
		base = filepath.Join(h, ".local", "state")
	}
	return filepath.Join(base, "sarina", "route-"+model+".json")
}

func signature() string {
	keys := make([]string, 0, len(Topics))
	for k := range Topics {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	h := sha256.New()
	for _, k := range keys {
		h.Write([]byte(k + "\x00" + strings.Join(Topics[k], "\x00") + "\x01"))
	}
	return hex.EncodeToString(h.Sum(nil))[:16]
}

type cache struct {
	Sig  string                 `json:"sig"`
	Vecs map[string][][]float32 `json:"vecs"`
}

func unit(v []float32) []float32 {
	var s float64
	for _, x := range v {
		s += float64(x) * float64(x)
	}
	if s == 0 {
		return v
	}
	out := make([]float32, len(v))
	inv := float32(1 / math.Sqrt(s))
	for i, x := range v {
		out[i] = x * inv
	}
	return out
}

func dot(a, b []float32) float64 {
	var s float64
	for i := range a {
		if i < len(b) {
			s += float64(a[i]) * float64(b[i])
		}
	}
	return s
}

// prototypes embeds the example questions once and keeps them in a small cache file.
func prototypes(emb Embedder) (map[string][][]float32, error) {
	path := cacheFile(emb.Name())
	var c cache
	if b, err := os.ReadFile(path); err == nil && json.Unmarshal(b, &c) == nil && c.Sig == signature() {
		return c.Vecs, nil
	}
	c = cache{Sig: signature(), Vecs: map[string][][]float32{}}
	for topic, ex := range Topics {
		texts := make([]string, len(ex))
		for i, e := range ex {
			texts[i] = "search_query: " + e
		}
		vecs, err := emb.Embed(texts)
		if err != nil {
			return nil, err
		}
		for _, v := range vecs {
			c.Vecs[topic] = append(c.Vecs[topic], unit(v))
		}
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err == nil {
		if b, err := json.Marshal(c); err == nil {
			_ = os.WriteFile(path, b, 0o600)
		}
	}
	return c.Vecs, nil
}

// Score is one topic and how close the question is to it.
type Score struct {
	Topic string
	Value float64
}

// Scores returns every topic's best similarity to the question, best first.
func Scores(question string, emb Embedder) ([]Score, error) {
	protos, err := prototypes(emb)
	if err != nil {
		return nil, err
	}
	qv, err := emb.Embed([]string{"search_query: " + question})
	if err != nil {
		return nil, err
	}
	q := unit(qv[0])
	var out []Score
	for topic, vs := range protos {
		best := -1.0
		for _, v := range vs {
			if d := dot(q, v); d > best {
				best = d
			}
		}
		out = append(out, Score{topic, best})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Value > out[j].Value })
	return out, nil
}

// Route returns at most two topics to look up for the question ("chat" and no topic both mean nothing to look up).
func Route(question string, emb Embedder) ([]string, error) {
	scores, err := Scores(question, emb)
	if err != nil {
		return nil, err
	}
	return pick(scores), nil
}

func pick(scores []Score) []string {
	if len(scores) == 0 || scores[0].Value < floor {
		return nil
	}
	var out []string
	for i, s := range scores {
		if i >= 2 || s.Value < floor || s.Value < scores[0].Value-band {
			break
		}
		if s.Topic == "chat" {
			if i == 0 {
				return nil // small talk: nothing to look up
			}
			break
		}
		out = append(out, s.Topic)
	}
	return out
}
