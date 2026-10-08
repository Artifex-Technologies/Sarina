package desktop

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"math"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/analysiscache"
)

// Prototype is one category bucket the semantic classifier may emit, with the
// sentence a description is compared against.
type Prototype struct {
	Categories []string
	Sentence   string
}

// CategoryPrototypes are prototype sentences, not a trained classifier: adding a
// bucket costs one line.
var CategoryPrototypes = []Prototype{
	{cats("Network", "Security"), "network scanning, port scanning, packet capture, sniffing, wireless and wifi auditing, spoofing, proxies and remote network attacks"},
	{cats("System", "Security"), "password cracking, hash cracking, digital forensics, malware and rootkit analysis, privilege escalation and host exploitation"},
	{cats("Development", "Security"), "disassembly, decompilation, debugging, fuzzing and reverse engineering of binaries and executables"},
	{cats("Utility", "Security"), "encryption, decryption, cryptography, hashing, encoding and steganography"},
	{cats("Development"), "software development, programming languages, compilers, build systems, version control and source code editing"},
	{cats("Network"), "web browsing, downloading, file transfer, email, chat and general internet client software"},
	{cats("Graphics"), "image editing, photo manipulation, drawing, vector graphics and viewing pictures"},
	{cats("AudioVideo"), "playing, recording and editing audio, music and video"},
	{cats("Office"), "word processing, spreadsheets, presentations, documents and note taking"},
	{cats("Science"), "scientific computing, mathematics, statistics, data analysis and plotting"},
	{cats("Settings"), "configuring the desktop, appearance, input devices and system preferences"},
	{cats("System"), "system administration, managing processes, services, disks, filesystems and monitoring the machine"},
	{cats("Utility"), "small general purpose desktop utilities, archiving, clipboard and text tools"},
}

func label(c []string) string { return strings.Join(c, ";") }

// PrototypesFingerprint identifies the exact prototype set cached scores were
// computed against; if the set changes, old cache entries must not be served.
func PrototypesFingerprint() string {
	parts := make([]string, len(CategoryPrototypes))
	for i, p := range CategoryPrototypes {
		parts[i] = label(p.Categories) + "|" + p.Sentence
	}
	sum := sha256.Sum256([]byte(strings.Join(parts, "\n")))
	return hex.EncodeToString(sum[:])
}

// DefaultClassificationCache opens the on-disk classification cache, namespaced
// to the current prototype set.
func DefaultClassificationCache(path string) *analysiscache.Cache {
	return analysiscache.New(path, PrototypesFingerprint())
}

// Scorer is the expensive, threshold-independent half of semantic
// classification: the nearest prototype label (";"-joined categories, empty if
// none) and its similarity score. Implementations need a model (ONNX embeddings
// are Python-only), so the core never provides one itself.
type Scorer interface {
	Score(text string) (label string, score float64, err error)
}

// Classifier turns a description into categories, or none.
type Classifier interface {
	Classify(text string) (categories []string, score float64)
}

// SemanticClassifier wraps a Scorer with the content-hash cache and a
// similarity threshold. The cache holds the expensive (label, score) pair; the
// threshold is applied fresh on every call, cached or not, so a changed
// --similarity-threshold still changes the outcome for already-cached text.
type SemanticClassifier struct {
	Scorer    Scorer
	Threshold float64
	Cache     *analysiscache.Cache // optional
}

var byLabel = func() map[string][]string {
	m := map[string][]string{}
	for _, p := range CategoryPrototypes {
		m[label(p.Categories)] = p.Categories
	}
	return m
}()

// Classify implements Classifier. A scorer error counts as "no signal".
func (c *SemanticClassifier) Classify(text string) ([]string, float64) {
	if strings.TrimSpace(text) == "" {
		return nil, 0
	}
	lbl, score, cached := "", 0.0, false
	if c.Cache != nil {
		if v, ok := c.Cache.Get(text); ok {
			lbl, score, cached = decodeCached(v)
		}
	}
	if !cached {
		var err error
		lbl, score, err = c.Scorer.Score(text)
		if err != nil {
			return nil, 0
		}
		if c.Cache != nil {
			_ = c.Cache.Set(text, map[string]any{"label": lbl, "score": score})
		}
	}
	cats, ok := byLabel[lbl]
	if lbl == "" || !ok || score < c.Threshold {
		return nil, score
	}
	return cats, score
}

func decodeCached(v any) (string, float64, bool) {
	m, ok := v.(map[string]any)
	if !ok {
		return "", 0, false
	}
	score, ok := m["score"].(float64)
	if !ok {
		return "", 0, false
	}
	l, _ := m["label"].(string) // null label -> no match
	return l, score, true
}

// Embedder produces one vector per text. A real one lives outside this package
// (the Python ONNX helper); tests use fakes.
type Embedder interface {
	Embed(texts []string) ([][]float64, error)
}

// EmbeddingScorer scores text by cosine similarity to the prototype sentences.
type EmbeddingScorer struct {
	model  Embedder
	labels []string
	vecs   [][]float64
}

// NewEmbeddingScorer embeds the prototype set once.
func NewEmbeddingScorer(model Embedder) (*EmbeddingScorer, error) {
	sentences := make([]string, len(CategoryPrototypes))
	labels := make([]string, len(CategoryPrototypes))
	for i, p := range CategoryPrototypes {
		sentences[i], labels[i] = p.Sentence, label(p.Categories)
	}
	vecs, err := model.Embed(sentences)
	if err != nil {
		return nil, err
	}
	if len(vecs) != len(sentences) {
		return nil, fmt.Errorf("embedder returned %d vectors for %d prototypes", len(vecs), len(sentences))
	}
	return &EmbeddingScorer{model: model, labels: labels, vecs: vecs}, nil
}

// Score implements Scorer.
func (s *EmbeddingScorer) Score(text string) (string, float64, error) {
	out, err := s.model.Embed([]string{text})
	if err != nil {
		return "", 0, err
	}
	if len(out) != 1 {
		return "", 0, fmt.Errorf("embedder returned %d vectors for 1 text", len(out))
	}
	best, bestScore := "", math.Inf(-1)
	for i, v := range s.vecs {
		if sc := cosine(out[0], v); sc > bestScore {
			best, bestScore = s.labels[i], sc
		}
	}
	if best == "" {
		return "", 0, nil
	}
	return best, bestScore, nil
}

func cosine(a, b []float64) float64 {
	var dot, na, nb float64
	for i := 0; i < len(a) && i < len(b); i++ {
		dot += a[i] * b[i]
	}
	for _, x := range a {
		na += x * x
	}
	for _, x := range b {
		nb += x * x
	}
	if na == 0 || nb == 0 {
		return 0
	}
	return dot / (math.Sqrt(na) * math.Sqrt(nb))
}
