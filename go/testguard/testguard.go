// Package testguard keeps an eval run out of the owner's own notes. Under SARINA_TEST=1 (the eval runner sets it for every case)
// the places Sarina keeps things between conversations (the vault and its Daily note, Talks and Jokes, mood history, reflections,
// remembered facts) refuse a write outside the test's own folders: SARINA_TEST_ROOT (colon-separated; the runner's work folder)
// and the system's temporary folder. A case that writes there works as before; a case that would have written into the real
// vault gets an error instead, and the owner's notes stay as they were.
package testguard

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const (
	Env     = "SARINA_TEST"
	RootEnv = "SARINA_TEST_ROOT"
)

// Active reports an eval run.
func Active() bool { return os.Getenv(Env) == "1" }

// Allow returns nil when not under test, or when path lies inside a test root; otherwise an error naming the refusal.
func Allow(path string) error {
	if !Active() {
		return nil
	}
	p := resolve(path)
	roots := []string{os.TempDir()}
	for _, r := range strings.Split(os.Getenv(RootEnv), ":") {
		if r != "" {
			roots = append(roots, r)
		}
	}
	for _, r := range roots {
		r = resolve(r)
		if p == r || strings.HasPrefix(p, r+string(filepath.Separator)) {
			return nil
		}
	}
	return fmt.Errorf("not written: this is a test run (%s=1), and %s is outside the test's folders", Env, path)
}

// resolve makes path absolute and follows links in the part that exists (a link must not lead a test out of its folder).
func resolve(path string) string {
	p, err := filepath.Abs(path)
	if err != nil {
		return filepath.Clean(path)
	}
	var rest []string
	for q := p; ; q = filepath.Dir(q) {
		if r, err := filepath.EvalSymlinks(q); err == nil {
			return filepath.Join(append([]string{r}, rest...)...)
		}
		if q == filepath.Dir(q) {
			return p
		}
		rest = append([]string{filepath.Base(q)}, rest...)
	}
}
