package embed

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

// The request asks Ollama to keep the model loaded for KeepAlive (30 minutes, decision D5) and returns one vector per text.
func TestEmbedSendsKeepAliveAndModel(t *testing.T) {
	var got map[string]any
	s := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/embed" {
			http.NotFound(w, r)
			return
		}
		json.NewDecoder(r.Body).Decode(&got)
		w.Write([]byte(`{"embeddings":[[0.1,0.2],[0.3,0.4]]}`))
	}))
	defer s.Close()
	t.Setenv("OLLAMA_HOST", s.URL)
	t.Setenv("SARINA_EMBED_MODEL", "")
	vs, err := Default().Embed([]string{"a", "b"})
	if err != nil || len(vs) != 2 {
		t.Fatalf("embed: %v %v", vs, err)
	}
	if got["keep_alive"] != "30m" || got["model"] != "nomic-embed-text" {
		t.Fatalf("request = %v", got)
	}
}

// An answer larger than the limit is refused with a clear error, not read on without end.
func TestEmbedRefusesAnAnswerPastTheLimit(t *testing.T) {
	body := `{"embeddings":[[` + strings.Repeat("0.125,", 200) + `1]]}`
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, body) }))
	defer srv.Close()
	if _, err := (ollamaEmbedder{base: srv.URL, model: "m", limit: int64(len(body))}).Embed([]string{"x"}); err != nil {
		t.Fatalf("an answer of exactly the limit must be read: %v", err)
	}
	_, err := (ollamaEmbedder{base: srv.URL, model: "m", limit: int64(len(body) - 1)}).Embed([]string{"x"})
	if err == nil || !strings.Contains(err.Error(), "larger than") {
		t.Fatalf("got %v", err)
	}
	if MaxAnswerBytes < 256*4096*24 {
		t.Fatal("the limit must leave room for the largest plausible answer")
	}
}
