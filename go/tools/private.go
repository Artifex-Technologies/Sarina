package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

// Folders the owner keeps from her (SARINA_PRIVATE_DIRS, separated by ":" , "~" allowed): no file tool reads, lists, searches or writes
// inside them, and searches skip them, whatever folder she was started in. For a spoken question she starts in the home folder, so this is
// what keeps, for example, a personal writing folder out of reach.
// credentialStores are always private, whatever SARINA_PRIVATE_DIRS says: the places in a home folder where logins, keys,
// tokens and browser sessions live. A prompt-injected turn must not be able to read them into the conversation (2026-10-27
// audit). Paths are relative to the home folder; a "*" is matched once per home folder.
var credentialStores = []string{
	".ssh", ".gnupg", ".password-store", ".local/share/keyrings", ".pki", ".aws", ".azure", ".kube", ".docker/config.json",
	".claude", ".claude.json", ".config/gh", ".config/hub", ".config/rclone", ".config/gcloud", ".npmrc", ".pypirc",
	".cargo/credentials", ".cargo/credentials.toml", ".config/labyrinth/sarina.env", ".sarina/sarina.env",
	".config/Proton*", ".config/proton*", ".config/me.proton.*", ".local/share/Proton*",
	".mozilla", ".thunderbird", ".config/vivaldi*", ".config/chromium", ".config/google-chrome*", ".config/BraveSoftware",
	".config/Signal", ".config/discord", ".config/Bitwarden", ".config/KeePassXC", ".var/app",
}

var (
	storesMu   sync.Mutex
	storesHome string
	stores     []string
)

func credentialStoreDirs(home string) []string {
	storesMu.Lock()
	defer storesMu.Unlock()
	if home != storesHome {
		storesHome, stores = home, nil
		for _, rel := range credentialStores {
			p := filepath.Join(home, rel)
			matches := []string{p}
			if strings.Contains(rel, "*") {
				matches, _ = filepath.Glob(p)
			}
			for _, m := range matches {
				if r, err := filepath.EvalSymlinks(m); err == nil {
					m = r
				}
				stores = append(stores, m)
			}
		}
	}
	return stores
}

func privateDirs() []string {
	home, _ := os.UserHomeDir()
	var out []string
	if home != "" {
		out = append(out, credentialStoreDirs(home)...)
	}
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
		return fmt.Errorf("refusing to %s %s: the owner keeps that folder private from me (credential stores, browser "+
			"profiles and the folders in SARINA_PRIVATE_DIRS are never read)", verb, given)
	}
	return nil
}
