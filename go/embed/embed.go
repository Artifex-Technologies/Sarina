// Package embed turns texts into vectors with a small local embedding model through Ollama (nomic-embed-text by default). Used by the
// vault's recall by meaning and by the search in long skills; the model is loaded only while it is used (keep_alive two minutes).
package embed

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
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

type ollamaEmbedder struct {
	base, model string
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
	body, _ := json.Marshal(map[string]any{"model": o.model, "input": texts, "keep_alive": "2m"})
	c := &http.Client{Timeout: 120 * time.Second}
	resp, err := c.Post(o.base+"/api/embed", "application/json", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	var out struct {
		Embeddings [][]float32 `json:"embeddings"`
		Error      string      `json:"error"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
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
