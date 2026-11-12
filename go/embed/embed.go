// Package embed turns texts into vectors with a small local embedding model through Ollama (nomic-embed-text by default). Used by the
// vault's recall by meaning and by the search in long skills; the model stays loaded for KeepAlive after its last use.
package embed

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"
)

// Embedder turns texts into vectors.
type Embedder interface {
	Embed(texts []string) ([][]float32, error)
	Name() string
}

// KeepAlive is how long Ollama keeps the embedding model loaded after a request. Thirty minutes (the owner's decision D5, 2026-11-02;
// it was two): the model is small (~0.3 GB, partly on the CPU beside qwen3:8b), and reloading it made the first recall of a
// conversation slower than the rest.
const KeepAlive = "30m"

// MaxAnswerBytes bounds what is read of one answer from Ollama (64 MiB). The largest plausible answer is far below it:
// Sarina embeds at most 32 texts at a time (vault, skills; desktop's prototypes are ~13), and even 256 texts of a
// 4096-number model (the biggest embedding models today) at 24 bytes a number in JSON is about 24 MiB. A program answering on
// Ollama's port in its place must not make Sarina read without end.
const MaxAnswerBytes = 64 << 20

type ollamaEmbedder struct {
	base, model string
	limit       int64 // bytes read of an answer at most; 0 = MaxAnswerBytes (tests set it lower)
}

// Default talks to Ollama: OLLAMA_HOST, else OPENAI_BASE_URL without its /v1, else 127.0.0.1:11434. Model: SARINA_EMBED_MODEL, else nomic-embed-text.
func Default() Embedder {
	base := os.Getenv("OLLAMA_HOST")
	if base == "" {
		base = strings.TrimSuffix(strings.TrimSuffix(os.Getenv("OPENAI_BASE_URL"), "/"), "/v1")
	}
	if base == "" {
		base = "http://127.0.0.1:11434"
	}
	if !strings.HasPrefix(base, "http") {
		base = "http://" + base
	}
	model := os.Getenv("SARINA_EMBED_MODEL")
	if model == "" {
		model = "nomic-embed-text"
	}
	return ollamaEmbedder{base: strings.TrimSuffix(base, "/"), model: model}
}

func (o ollamaEmbedder) Name() string { return o.model }

func (o ollamaEmbedder) Embed(texts []string) ([][]float32, error) {
	body, _ := json.Marshal(map[string]any{"model": o.model, "input": texts, "keep_alive": KeepAlive})
	c := &http.Client{Timeout: 120 * time.Second}
	resp, err := c.Post(o.base+"/api/embed", "application/json", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	limit := o.limit
	if limit <= 0 {
		limit = MaxAnswerBytes
	}
	raw, rerr := io.ReadAll(io.LimitReader(resp.Body, limit+1))
	if int64(len(raw)) > limit {
		return nil, fmt.Errorf("embedder answer is larger than %d bytes; not read", limit)
	}
	var out struct {
		Embeddings [][]float32 `json:"embeddings"`
		Error      string      `json:"error"`
	}
	// the first JSON value of the answer; when the answer was cut short before that value ended, the read's error
	if err := json.NewDecoder(bytes.NewReader(raw)).Decode(&out); err != nil {
		if rerr != nil && (errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF)) {
			return nil, rerr
		}
		return nil, err
	}
	if out.Error != "" {
		return nil, errors.New(out.Error)
	}
	if len(out.Embeddings) != len(texts) {
		return nil, fmt.Errorf("embedder returned %d vectors for %d texts", len(out.Embeddings), len(texts))
	}
	return out.Embeddings, nil
}
