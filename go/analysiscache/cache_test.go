package analysiscache

import (
	"os"
	"path/filepath"
	"testing"
)

func TestEmptyAndRoundTrip(t *testing.T) {
	p := filepath.Join(t.TempDir(), "sub", "c.json")
	c := New(p, "ns")
	if _, ok := c.Get("x"); ok {
		t.Fatal("empty cache must miss")
	}
	if err := c.Set("x", map[string]any{"label": "Graphics", "score": 0.62}); err != nil {
		t.Fatal(err)
	}
	v, ok := c.Get("x")
	if !ok || v.(map[string]any)["label"] != "Graphics" {
		t.Fatalf("%v %v", v, ok)
	}
	if _, ok := c.Get("y"); ok {
		t.Fatal("different content is a miss")
	}
}

func TestPersistsAcrossInstancesAndNamespaceInvalidates(t *testing.T) {
	p := filepath.Join(t.TempDir(), "c.json")
	New(p, "v1").Set("x", "value")
	if v, ok := New(p, "v1").Get("x"); !ok || v != "value" {
		t.Fatal("matching namespace keeps entries")
	}
	if _, ok := New(p, "v2").Get("x"); ok {
		t.Fatal("a different namespace must discard the cache")
	}
}

func TestCorruptOrMissingFileIsEmpty(t *testing.T) {
	p := filepath.Join(t.TempDir(), "c.json")
	if _, ok := New(p, "").Get("x"); ok {
		t.Fatal("missing")
	}
	os.WriteFile(p, []byte("{nope"), 0o600)
	c := New(p, "")
	if _, ok := c.Get("x"); ok {
		t.Fatal("corrupt")
	}
	if err := c.Set("x", 1); err != nil {
		t.Fatalf("a corrupt file must be overwritable: %v", err)
	}
}

func TestOnDiskShapeMatchesThePythonCache(t *testing.T) {
	p := filepath.Join(t.TempDir(), "c.json")
	New(p, "ns").Set("hello", []any{"a", 1.5})
	raw, _ := os.ReadFile(p)
	// sha256("hello")
	want := "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
	if !contains(string(raw), `"namespace": "ns"`) || !contains(string(raw), want) {
		t.Fatalf("%s", raw)
	}
}

func contains(s, sub string) bool {
	for i := 0; i+len(sub) <= len(s); i++ {
		if s[i:i+len(sub)] == sub {
			return true
		}
	}
	return false
}
