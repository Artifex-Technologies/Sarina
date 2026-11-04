package embed

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
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
