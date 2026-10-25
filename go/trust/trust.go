// Package trust checks Sarina's behaviour files (her rules, tool connections, skills, agents) against Heimdall's signed list of
// approved files (Heimdall's docs/TRUST.md). Only in "enforce" mode does it refuse anything: a file whose content is not exactly
// what the owner signed is not used, so a program running as the user cannot change how she behaves by editing those files. In
// "watch" and "off" nothing is refused here (in watch, Heimdall raises the alerts).
//
// The list and its public key live in Heimdall's root-owned folder (/etc/heimdall/trust): Sarina can check signatures but never
// make one. Files under /usr belong to installed packages (only root can change them) and are not checked.
package trust

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

// DefaultDir is Heimdall's trust folder.
const DefaultDir = "/etc/heimdall/trust"

// EnvDir overrides the folder (tests).
const EnvDir = "SARINA_TRUST_DIR"

// Checker reads the mode and the signed list once.
type Checker struct {
	dir     string
	once    sync.Once
	mode    string
	files   map[string]string
	loadErr string
}

var (
	defMu  sync.Mutex
	defOne *Checker
)

// Default is the checker for this process (Heimdall's folder, or SARINA_TRUST_DIR).
func Default() *Checker {
	defMu.Lock()
	defer defMu.Unlock()
	if defOne == nil {
		dir := os.Getenv(EnvDir)
		if dir == "" {
			dir = DefaultDir
		}
		defOne = New(dir)
	}
	return defOne
}

// New checks against one folder.
func New(dir string) *Checker { return &Checker{dir: dir} }

func (c *Checker) load() {
	c.once.Do(func() {
		b, err := os.ReadFile(filepath.Join(c.dir, "mode"))
		if err != nil {
			c.mode = "off"
			return
		}
		c.mode = strings.TrimSpace(string(b))
		if c.mode != "enforce" {
			return
		}
		pubText, err1 := os.ReadFile(filepath.Join(c.dir, "owner.pub"))
		body, err2 := os.ReadFile(filepath.Join(c.dir, "manifest.json"))
		sigText, err3 := os.ReadFile(filepath.Join(c.dir, "manifest.sig"))
		if err1 != nil || err2 != nil || err3 != nil {
			c.loadErr = "the signed list of approved files cannot be read"
			return
		}
		pub, err1 := base64.StdEncoding.DecodeString(strings.TrimSpace(string(pubText)))
		sig, err2 := base64.StdEncoding.DecodeString(strings.TrimSpace(string(sigText)))
		if err1 != nil || err2 != nil || len(pub) != ed25519.PublicKeySize || !ed25519.Verify(ed25519.PublicKey(pub), body, sig) {
			c.loadErr = "the signed list of approved files does not match its signature"
			return
		}
		var m struct {
			Files map[string]string `json:"files"`
		}
		if err := json.Unmarshal(body, &m); err != nil {
			c.loadErr = "the signed list of approved files cannot be read"
			return
		}
		c.files = m.Files
	})
}

// Enforcing reports whether unsigned files are refused.
func (c *Checker) Enforcing() bool {
	if c == nil {
		return false
	}
	c.load()
	return c.mode == "enforce"
}

// Refuse says why a behaviour file must not be used, or "" when it may (always "" unless enforcing).
func (c *Checker) Refuse(path string) string {
	if !c.Enforcing() {
		return ""
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		abs = path
	}
	if strings.HasPrefix(abs, "/usr/") {
		return ""
	}
	if c.loadErr != "" {
		return c.loadErr + " (heimdalld trust status)"
	}
	want, listed := c.files[abs]
	if !listed {
		return "not approved: it is not on the signed list (the owner approves it with: sudo heimdalld trust sign " + abs + ")"
	}
	f, err := os.Open(abs)
	if err != nil {
		return "cannot be checked: " + err.Error()
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "cannot be checked: " + err.Error()
	}
	if hex.EncodeToString(h.Sum(nil)) != want {
		return "not approved: it changed since it was signed (sudo heimdalld trust restore puts back the approved version)"
	}
	return ""
}
