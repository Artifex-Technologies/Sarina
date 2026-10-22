package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Folders the owner keeps from her (SARINA_PRIVATE_DIRS, separated by ":" , "~" allowed): no file tool reads, lists, searches or writes
// inside them, and searches skip them, whatever folder she was started in. For a spoken question she starts in the home folder, so this is
// what keeps, for example, a personal writing folder out of reach.
func privateDirs() []string {
	var out []string
	home, _ := os.UserHomeDir()
	for _, d := range strings.Split(os.Getenv("SARINA_PRIVATE_DIRS"), ":") {
		d = strings.TrimSpace(d)
		if d == "" {
			continue
		}
		if strings.HasPrefix(d, "~/") {
			d = filepath.Join(home, d[2:])
		}
		if abs, err := filepath.Abs(d); err == nil {
			if r, err := filepath.EvalSymlinks(abs); err == nil {
				abs = r
			}
			out = append(out, abs)
		}
	}
	return out
}

// IsPrivate reports whether path is inside one of the private folders.
func IsPrivate(path string) bool {
	dirs := privateDirs()
	if len(dirs) == 0 {
		return false
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		return true
	}
	if r, err := filepath.EvalSymlinks(abs); err == nil {
		abs = r
	}
	for _, d := range dirs {
		if abs == d || strings.HasPrefix(abs, d+string(filepath.Separator)) {
			return true
		}
	}
	return false
}

func privateGuard(given, resolved, verb string) error {
	if IsPrivate(resolved) {
		return fmt.Errorf("refusing to %s %s: the owner keeps that folder private from me", verb, given)
	}
	return nil
}
