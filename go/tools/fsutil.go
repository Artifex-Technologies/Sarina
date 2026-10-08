package tools

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"unicode/utf8"
)

// TruncateOutput keeps the head and tail of s when it exceeds max characters.
func TruncateOutput(s string, max int) string {
	if max <= 0 || utf8.RuneCountInString(s) <= max {
		return s
	}
	r := []rune(s)
	head := max * 2 / 3
	tail := max - head
	omitted := len(r) - head - tail
	return string(r[:head]) + fmt.Sprintf("\n[... %d characters omitted ...]\n", omitted) + string(r[len(r)-tail:])
}

func expandHome(p string) string {
	if p == "~" || strings.HasPrefix(p, "~/") || strings.HasPrefix(p, `~\`) {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, p[1:])
		}
	}
	return p
}

func realRoot(root string) string {
	if r, err := filepath.EvalSymlinks(root); err == nil {
		return r
	}
	return root
}

func within(root, p string) bool {
	rel, err := filepath.Rel(root, p)
	if err != nil {
		return false
	}
	return rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}

// ResolveInRoot resolves p against root for a path that must exist, following
// symlinks, and refuses anything that escapes the workspace.
func ResolveInRoot(root, p string) (string, error) {
	if p == "" {
		return "", errors.New("path is required")
	}
	full := expandHome(p)
	if !filepath.IsAbs(full) {
		full = filepath.Join(root, full)
	}
	full = filepath.Clean(full)
	resolved, err := filepath.EvalSymlinks(full)
	if err != nil {
		return "", fmt.Errorf("cannot access %s: %v", p, err)
	}
	if !within(realRoot(root), resolved) {
		return "", fmt.Errorf("%s is outside the workspace", p)
	}
	return resolved, nil
}

// ResolveForWrite is ResolveInRoot for a path that may not exist yet. The
// deepest existing ancestor is resolved through symlinks, so a write cannot
// leave the workspace through a link, and a dangling symlink is refused rather
// than followed.
func ResolveForWrite(root, p string) (string, error) {
	if p == "" {
		return "", errors.New("path is required")
	}
	full := expandHome(p)
	if !filepath.IsAbs(full) {
		full = filepath.Join(root, full)
	}
	full = filepath.Clean(full)
	cur := full
	var tail []string
	for {
		if _, err := os.Lstat(cur); err == nil {
			break
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			return "", fmt.Errorf("cannot access %s", p)
		}
		tail = append([]string{filepath.Base(cur)}, tail...)
		cur = parent
	}
	resolved, err := filepath.EvalSymlinks(cur)
	if err != nil {
		return "", fmt.Errorf("cannot write %s: %v (a dangling symlink is not followed)", p, err)
	}
	resolved = filepath.Join(append([]string{resolved}, tail...)...)
	if !within(realRoot(root), resolved) {
		return "", fmt.Errorf("%s is outside the workspace", p)
	}
	return resolved, nil
}

// relPath is p relative to the workspace root, with forward slashes.
func relPath(root, p string) string {
	for _, base := range []string{root, realRoot(root)} {
		if rel, err := filepath.Rel(base, p); err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return filepath.ToSlash(rel)
		}
	}
	return filepath.ToSlash(p)
}

func sha256Hex(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:])
}

// preview collapses whitespace and cuts to n characters.
func preview(s string, n int) string {
	s = strings.Join(strings.Fields(s), " ")
	r := []rune(s)
	if len(r) > n {
		return string(r[:n]) + "..."
	}
	return s
}

// ---- per-context state ----------------------------------------------------

// Runtimes keys used by this package.
const (
	runtimeFileState = "sarina.fs_state"
	// RuntimeProcs holds the *ProcManager background processes live in.
	RuntimeProcs = "sarina.procs"
)

var runtimesMu sync.Mutex

// fileState is the per-context memory behind the edit-loop, summary-churn and
// self-authored-file guards. It lives in Context.Runtimes, so it is empty again
// whenever the agent rebuilds the context (a workspace change) and is never
// persisted.
type fileState struct {
	mu            sync.Mutex
	edits         map[string]int
	summaryWrites int
	authored      map[string]bool
}

func stateOf(tc *Context) *fileState {
	runtimesMu.Lock()
	defer runtimesMu.Unlock()
	if tc.Runtimes == nil {
		tc.Runtimes = map[string]any{}
	}
	if s, ok := tc.Runtimes[runtimeFileState].(*fileState); ok {
		return s
	}
	s := &fileState{edits: map[string]int{}, authored: map[string]bool{}}
	tc.Runtimes[runtimeFileState] = s
	return s
}
