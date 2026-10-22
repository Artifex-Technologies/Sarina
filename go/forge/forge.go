// Package forge lets Sarina write small programs ("agents") in whatever language a job needs and run them, only ever inside Limbo's
// disposable guests. See docs/AGENT_FORGE.md for the design and the rules this code enforces:
//
//   - an agent is a folder of source files plus a manifest; nothing is executed on the host, ever;
//   - the folder is hashed; an approval belongs to one exact hash and is void after any edit (by anyone, including her);
//   - the approvals file lives outside the agents folder and no tool offered to the model can write it;
//   - what is sent to Limbo is a small deterministic tar; what comes back is untrusted evidence;
//   - every create, run, approval and refusal is a line in an audit log (code by hash and a short preview, never in full).
package forge

import (
	"archive/tar"
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
)

// Limits. Kept in step with Limbo's own (it refuses a bigger bundle regardless).
const (
	MaxFiles      = 40
	MaxBundle     = 40 << 10 // bytes of tar, headers included
	MaxAgents     = 30
	MaxCommand    = 400
	MaxPerHour    = 20 // runs
	manifestName  = "manifest.json"
	maxPurposeLen = 300
)

var (
	nameRE = regexp.MustCompile(`^[a-z][a-z0-9-]{0,31}$`)
	fileRE = regexp.MustCompile(`^[A-Za-z0-9._][A-Za-z0-9._/-]{0,119}$`)
	langRE = regexp.MustCompile(`^[a-z0-9+#.-]{1,20}$`)
)

// Manifest says what an agent is for and how to start it inside the guest.
type Manifest struct {
	Name     string `json:"name"`
	Purpose  string `json:"purpose"`
	Language string `json:"language"`
	// Command starts the agent from inside its folder in the guest, for example "python3 main.py".
	Command string `json:"command"`
	// TimeoutSeconds is how long the command may run (Limbo clamps it to the profile's limit).
	TimeoutSeconds int `json:"timeout_seconds,omitempty"`
}

// Store keeps agents in Dir and approvals and the audit log in StateDir (a different folder: an agent cannot reach its own approval).
type Store struct {
	Dir      string
	StateDir string
	Now      func() time.Time

	mu sync.Mutex
	// results of this process's runs, by agent version and input: the same run is not repeated within a task
	done map[string]string
}

func (s *Store) now() time.Time {
	if s.Now != nil {
		return s.Now()
	}
	return time.Now()
}

func (s *Store) agentDir(name string) string { return filepath.Join(s.Dir, name) }

func cleanText(x string, max int) string {
	var b strings.Builder
	for _, r := range x {
		if r == '\n' || r == '\t' || r >= 0x20 && r != 0x7f && !(r >= 0x202a && r <= 0x202e) && !(r >= 0x2066 && r <= 0x2069) {
			b.WriteRune(r)
		}
	}
	out := b.String()
	if rr := []rune(out); len(rr) > max {
		out = string(rr[:max])
	}
	return out
}

// Validate checks a manifest and a set of files (path -> content) against the rules, without touching disk.
func Validate(m Manifest, files map[string]string) error {
	if !nameRE.MatchString(m.Name) {
		return errors.New("the agent's name must be lowercase letters, digits and hyphens, starting with a letter")
	}
	if strings.TrimSpace(m.Purpose) == "" || len(m.Purpose) > maxPurposeLen {
		return errors.New("say in one or two sentences what the agent is for (up to 300 characters)")
	}
	if !langRE.MatchString(m.Language) {
		return errors.New("the language must be a short lowercase name such as python, go, rust, bash or node")
	}
	if c := m.Command; strings.TrimSpace(c) == "" || len(c) > MaxCommand || strings.ContainsAny(c, "\x00\n\r") {
		return errors.New("the command must be one line that starts the agent from its folder")
	}
	if m.TimeoutSeconds < 0 || m.TimeoutSeconds > 900 {
		return errors.New("the timeout must be between 0 and 900 seconds")
	}
	if len(files) == 0 || len(files) > MaxFiles {
		return fmt.Errorf("an agent has between 1 and %d files", MaxFiles)
	}
	for p, body := range files {
		if p == manifestName {
			return errors.New("manifest.json is written by the forge, not by the agent")
		}
		if !fileRE.MatchString(p) || path.Clean(p) != p || strings.HasPrefix(p, "/") || strings.Contains(p, "..") {
			return fmt.Errorf("%q is not an allowed file name (relative, no \"..\", letters, digits and . _ - /)", p)
		}
		if strings.ContainsRune(body, 0) {
			return fmt.Errorf("%s has a NUL byte: only source text is allowed", p)
		}
	}
	// the size that counts is the tar that would be sent
	if _, err := Bundle(files); err != nil {
		return fmt.Errorf("the files are too large: send source only (%v)", err)
	}
	return nil
}

// Create writes (or replaces) an agent. Replacing changes its hash, which voids any approval.
func (s *Store) Create(m Manifest, files map[string]string) (string, error) {
	if err := Validate(m, files); err != nil {
		s.audit("create", m.Name, "", "refused", err.Error())
		return "", err
	}
	if err := s.checkPaths(); err != nil {
		return "", err
	}
	if _, err := os.Stat(s.agentDir(m.Name)); err != nil {
		if n, _ := s.count(); n >= MaxAgents {
			return "", fmt.Errorf("there are already %d agents: remove one first", MaxAgents)
		}
	}
	tmp, err := os.MkdirTemp(s.Dir, ".new-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(tmp)
	for p, body := range files {
		full := filepath.Join(tmp, filepath.FromSlash(p))
		if err := os.MkdirAll(filepath.Dir(full), 0o700); err != nil {
			return "", err
		}
		if err := os.WriteFile(full, []byte(body), 0o600); err != nil {
			return "", err
		}
	}
	mb, _ := json.MarshalIndent(m, "", "  ")
	if err := os.WriteFile(filepath.Join(tmp, manifestName), append(mb, '\n'), 0o600); err != nil {
		return "", err
	}
	dst := s.agentDir(m.Name)
	if err := os.RemoveAll(dst); err != nil {
		return "", err
	}
	if err := os.Rename(tmp, dst); err != nil {
		return "", err
	}
	h, err := s.Hash(m.Name)
	if err != nil {
		return "", err
	}
	s.audit("create", m.Name, h, "ok", fmt.Sprintf("%d files, %s", len(files), cleanText(m.Purpose, 60)))
	return h, nil
}

func (s *Store) checkPaths() error {
	if s.Dir == "" || s.StateDir == "" {
		return errors.New("the forge is not configured")
	}
	if a, b := filepath.Clean(s.Dir), filepath.Clean(s.StateDir); a == b || strings.HasPrefix(b+"/", a+"/") || strings.HasPrefix(a+"/", b+"/") {
		return errors.New("the agents folder and the state folder must be separate, so an agent cannot reach its own approval")
	}
	for _, d := range []string{s.Dir, s.StateDir} {
		if err := os.MkdirAll(d, 0o700); err != nil {
			return err
		}
	}
	return nil
}

func (s *Store) count() (int, error) {
	es, err := os.ReadDir(s.Dir)
	if err != nil {
		return 0, err
	}
	n := 0
	for _, e := range es {
		if e.IsDir() && nameRE.MatchString(e.Name()) {
			n++
		}
	}
	return n, nil
}

// List returns the names of the agents.
func (s *Store) List() ([]string, error) {
	es, err := os.ReadDir(s.Dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	var out []string
	for _, e := range es {
		if e.IsDir() && nameRE.MatchString(e.Name()) {
			out = append(out, e.Name())
		}
	}
	sort.Strings(out)
	return out, nil
}

// Load reads an agent's manifest and files. Anything that is not a plain file in the folder (a link, a device, a folder swapped in
// by someone else) is an error: what is hashed and what is sent are only ever regular files this code can account for.
func (s *Store) Load(name string) (Manifest, map[string]string, error) {
	var m Manifest
	if !nameRE.MatchString(name) {
		return m, nil, errors.New("no such agent")
	}
	root := s.agentDir(name)
	files := map[string]string{}
	err := filepath.Walk(root, func(p string, fi os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		if fi.IsDir() {
			return nil
		}
		lfi, lerr := os.Lstat(p)
		if lerr != nil || !lfi.Mode().IsRegular() {
			return fmt.Errorf("%s is not a plain file", p)
		}
		rel, _ := filepath.Rel(root, p)
		b, err := os.ReadFile(p)
		if err != nil {
			return err
		}
		files[filepath.ToSlash(rel)] = string(b)
		return nil
	})
	if err != nil {
		return m, nil, err
	}
	raw, ok := files[manifestName]
	if !ok {
		return m, nil, errors.New("the agent has no manifest")
	}
	delete(files, manifestName)
	if err := json.Unmarshal([]byte(raw), &m); err != nil {
		return m, nil, errors.New("the manifest is not valid")
	}
	if m.Name != name {
		return m, nil, errors.New("the manifest names a different agent")
	}
	if err := Validate(m, files); err != nil {
		return m, nil, err
	}
	return m, files, nil
}

// Hash is the sha256 of the manifest and every file, in a fixed order.
func (s *Store) Hash(name string) (string, error) {
	m, files, err := s.Load(name)
	if err != nil {
		return "", err
	}
	return hashOf(m, files), nil
}

func hashOf(m Manifest, files map[string]string) string {
	h := sha256.New()
	mb, _ := json.Marshal(m)
	fmt.Fprintf(h, "manifest %d\n", len(mb))
	h.Write(mb)
	for _, p := range sortedKeys(files) {
		fmt.Fprintf(h, "\nfile %s %d\n", p, len(files[p]))
		h.Write([]byte(files[p]))
	}
	return hex.EncodeToString(h.Sum(nil))
}

func sortedKeys(m map[string]string) []string {
	ks := make([]string, 0, len(m))
	for k := range m {
		ks = append(ks, k)
	}
	sort.Strings(ks)
	return ks
}

// Bundle is the deterministic tar Limbo carries into the guest: the files only, fixed owner, date and mode, no manifest.
func Bundle(files map[string]string) ([]byte, error) {
	var buf bytes.Buffer
	tw := tar.NewWriter(&buf)
	for _, p := range sortedKeys(files) {
		body := files[p]
		hdr := &tar.Header{Name: p, Mode: 0o644, Size: int64(len(body)), ModTime: time.Unix(0, 0), Typeflag: tar.TypeReg, Format: tar.FormatPAX}
		if err := tw.WriteHeader(hdr); err != nil {
			return nil, err
		}
		if _, err := tw.Write([]byte(body)); err != nil {
			return nil, err
		}
	}
	if err := tw.Close(); err != nil {
		return nil, err
	}
	if buf.Len() > MaxBundle {
		return nil, fmt.Errorf("the bundle is %d bytes: the limit is %d", buf.Len(), MaxBundle)
	}
	return buf.Bytes(), nil
}

// Remove deletes an agent (its approval goes with it).
func (s *Store) Remove(name string) error {
	if !nameRE.MatchString(name) {
		return errors.New("no such agent")
	}
	if err := os.RemoveAll(s.agentDir(name)); err != nil {
		return err
	}
	s.audit("remove", name, "", "ok", "")
	return s.dropApproval(name)
}
