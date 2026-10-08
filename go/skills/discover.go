package skills

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

const (
	// SourceOperator marks skills from operator-controlled locations.
	SourceOperator = "operator"
	// SourceWorkspace marks skills found inside the workspace (opt-in only).
	SourceWorkspace = "workspace"
	// EnvDir lists extra skill directories (separated like PATH).
	EnvDir = "SARINA_SKILLS_DIR"
	// DefaultSystemDir is the root-owned directory the distro package installs
	// skills into. It is operator-controlled by definition (only root can write
	// it) and is searched on Linux only.
	DefaultSystemDir = "/usr/share/sarina/skills"

	maxSkillBytes = 64 << 10
)

// Options says where to look. Nothing inside the workspace is read unless
// TrustWorkspace is set: a repository must not be able to inject prompts.
type Options struct {
	Home           string              // home directory (for ~/.sarina)
	Getenv         func(string) string // default os.Getenv
	Workspace      string              // the workspace directory
	TrustWorkspace bool                // --trust-workspace-skills
	Scanner        Scanner             // default BasicScanner
	// SystemDir overrides the system skills directory (tests). Empty means
	// DefaultSystemDir on Linux and none elsewhere.
	SystemDir string
}

// Root is one directory searched for skills.
type Root struct{ Dir, Source string }

// Problem is a skill file that could not be used, with the reason.
type Problem struct{ Path, Message string }

// Collision records two skills with the same name; the earlier one is kept.
type Collision struct{ Name, Kept, Shadowed string }

// Catalog is the result of discovery.
type Catalog struct {
	Skills     []*Skill
	Problems   []Problem
	Collisions []Collision
	// Notices are lines the operator must see (e.g. workspace skills in use).
	Notices []string
}

// Roots returns the search path in priority order: ~/.sarina/skills, the
// directories in SARINA_SKILLS_DIR, the "paths" of ~/.sarina/skills.json, the
// root-owned system directory (/usr/share/sarina/skills, Linux), and only with
// the opt-in, <workspace>/.sarina/skills last (so a workspace skill
// can never shadow an operator one).
func Roots(o Options) []Root {
	getenv := o.Getenv
	if getenv == nil {
		getenv = os.Getenv
	}
	var roots []Root
	seen := map[string]bool{}
	add := func(dir, source string) {
		if dir == "" {
			return
		}
		if abs, err := filepath.Abs(dir); err == nil {
			dir = abs
		}
		if key := filepath.Clean(dir); !seen[key] {
			seen[key] = true
			roots = append(roots, Root{key, source})
		}
	}
	if o.Home != "" {
		add(filepath.Join(o.Home, ".sarina", "skills"), SourceOperator)
	}
	for _, d := range filepath.SplitList(getenv(EnvDir)) {
		add(d, SourceOperator)
	}
	if o.Home != "" {
		if raw, err := os.ReadFile(filepath.Join(o.Home, ".sarina", "skills.json")); err == nil {
			var cfg struct {
				Paths []string `json:"paths"`
			}
			if json.Unmarshal(raw, &cfg) == nil {
				for _, d := range cfg.Paths {
					add(d, SourceOperator)
				}
			}
		}
	}
	sys := o.SystemDir
	if sys == "" && runtime.GOOS == "linux" {
		sys = DefaultSystemDir
	}
	add(sys, SourceOperator)
	if o.TrustWorkspace && o.Workspace != "" {
		add(filepath.Join(o.Workspace, ".sarina", "skills"), SourceWorkspace)
	}
	return roots
}

// Discover loads every usable skill. Failures are recorded, never fatal.
func Discover(o Options) *Catalog {
	sc := o.Scanner
	if sc == nil {
		sc = BasicScanner{}
	}
	c := &Catalog{}
	byName := map[string]*Skill{}
	for _, root := range Roots(o) {
		if root.Source == SourceWorkspace {
			c.Notices = append(c.Notices, "workspace skills are enabled (--trust-workspace-skills): loading skills from "+root.Dir)
		}
		for _, cand := range candidates(root.Dir, c) {
			s, err := load(cand.path, cand.fallback, sc)
			if err != nil {
				c.Problems = append(c.Problems, Problem{cand.path, err.Error()})
				continue
			}
			s.Source = root.Source
			key := strings.ToLower(s.Name)
			if kept, dup := byName[key]; dup {
				c.Collisions = append(c.Collisions, Collision{s.Name, kept.Path, s.Path})
				continue
			}
			byName[key] = s
			c.Skills = append(c.Skills, s)
		}
	}
	return c
}

type candidate struct{ path, fallback string }

// candidates lists skill files in one root: name.md files and name/SKILL.md.
// Symlinks are never followed, so a link cannot pull in a file from elsewhere.
func candidates(dir string, c *Catalog) []candidate {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Name() < entries[j].Name() })
	var out []candidate
	for _, e := range entries {
		name := e.Name()
		full := filepath.Join(dir, name)
		if strings.HasPrefix(name, ".") {
			continue
		}
		if e.Type()&os.ModeSymlink != 0 {
			c.Problems = append(c.Problems, Problem{full, "symbolic links are not followed"})
			continue
		}
		switch {
		case e.IsDir():
			p := filepath.Join(full, "SKILL.md")
			if st, err := os.Lstat(p); err == nil && st.Mode().IsRegular() {
				out = append(out, candidate{p, name})
			}
		case e.Type().IsRegular() && strings.EqualFold(filepath.Ext(name), ".md") && !strings.EqualFold(name, "README.md"):
			out = append(out, candidate{full, strings.TrimSuffix(name, filepath.Ext(name))})
		}
	}
	return out
}

func load(path, fallback string, sc Scanner) (*Skill, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, maxSkillBytes+1))
	if err != nil {
		return nil, err
	}
	if len(data) > maxSkillBytes {
		return nil, fmt.Errorf("file is larger than %d bytes", maxSkillBytes)
	}
	if v := sc.Scan(string(data)); v.Blocked {
		return nil, fmt.Errorf("blocked by the security scan: %s", v.Reason)
	}
	s, err := Parse(data, fallback)
	if err != nil {
		return nil, err
	}
	s.Path = path
	return s, nil
}

// Get finds a skill by name, ignoring case.
func (c *Catalog) Get(name string) (*Skill, bool) {
	if c == nil {
		return nil, false
	}
	for _, s := range c.Skills {
		if strings.EqualFold(s.Name, name) {
			return s, true
		}
	}
	return nil, false
}

// ModelInvocable lists the skills the model may call.
func (c *Catalog) ModelInvocable() []*Skill {
	var out []*Skill
	if c != nil {
		for _, s := range c.Skills {
			if s.ModelInvocable {
				out = append(out, s)
			}
		}
	}
	return out
}
