// Package analysiscache is a content-hash-keyed cache for expensive,
// deterministic analysis: {sha256(content): value} as JSON on disk, so repeated
// runs over unchanged input skip recomputing it.
//
// It is invalidated wholesale by a namespace fingerprint the caller supplies:
// if whatever produces the cached values changes shape, the whole cache is
// discarded rather than served under a namespace that no longer matches.
package analysiscache

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

// DefaultDir is ~/.sarina/cache (empty if the home directory is unknown).
func DefaultDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".sarina", "cache")
}

// Cache is one cache file. Safe for concurrent use.
type Cache struct {
	path      string
	namespace string
	mu        sync.Mutex
	entries   map[string]any
}

// New opens the cache at path. A missing, corrupt or other-namespace file is
// treated as empty, never as an error.
func New(path, namespace string) *Cache {
	c := &Cache{path: path, namespace: namespace, entries: map[string]any{}}
	raw, err := os.ReadFile(path)
	if err != nil {
		return c
	}
	var file struct {
		Namespace string         `json:"namespace"`
		Entries   map[string]any `json:"entries"`
	}
	if json.Unmarshal(raw, &file) != nil || file.Namespace != namespace || file.Entries == nil {
		return c
	}
	c.entries = file.Entries
	return c
}

func key(content string) string {
	sum := sha256.Sum256([]byte(content))
	return hex.EncodeToString(sum[:])
}

// Get returns the cached value for content.
func (c *Cache) Get(content string) (any, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	v, ok := c.entries[key(content)]
	return v, ok
}

// Set stores a JSON-encodable value and persists the cache.
func (c *Cache) Set(content string, value any) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.entries[key(content)] = value
	data, err := session.EncodeJSON(map[string]any{"namespace": c.namespace, "entries": c.entries})
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(c.path), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(c.path), ".cache-*.tmp")
	if err != nil {
		return err
	}
	name := tmp.Name()
	_, werr := tmp.Write(data)
	cerr := tmp.Close()
	if werr != nil || cerr != nil {
		os.Remove(name)
		if werr != nil {
			return werr
		}
		return cerr
	}
	return os.Rename(name, c.path)
}
