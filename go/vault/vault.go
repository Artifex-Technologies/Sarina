// Package vault is Sarina's linked notes: a folder of Markdown files (an Obsidian vault, but nothing here needs Obsidian) she can search,
// read and write. A note may link to others with [[wikilinks]], carry #tags and a small front matter, and the links are followed in both
// directions when she looks something up.
//
// What it borrows from Hindsight (vectorize-io/hindsight, MIT), without its Postgres server: recall mixes several strategies and merges them
// by reciprocal rank (keywords, the link graph, recency); what is remembered is typed (lessons, decisions, entities, reflections); and every
// write passes the same safety gate as her curated memory before it is kept, because what she writes is read back to her later.
package vault

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
)

// Folders a new vault starts with, and what each is for (also written into Home.md).
var Folders = []struct{ Name, Purpose string }{
	{"Lessons", "what went wrong or right, and what to do next time"},
	{"Decisions", "what was decided, by whom, and why"},
	{"Projects", "one note per project: goal, state, where things are"},
	{"Entities", "people, tools, machines and places, one note each"},
	{"Reflections", "notes that pull several others together (the vault's own summaries)"},
	{"Daily", "a short log of each day"},
	{"Inbox", "unsorted: things to file later"},
}

// Note is one Markdown file.
type Note struct {
	Path     string // relative to the vault, with forward slashes ("Lessons/Todo list in a file.md")
	Title    string
	Tags     []string
	Links    []string // targets of [[wikilinks]], as written (without #heading or |alias)
	Body     string   // everything after the front matter
	Front    map[string]string
	Modified time.Time
}

// Vault is a folder of notes.
type Vault struct {
	Root string
}

var (
	wikilinkRE = regexp.MustCompile(`\[\[([^\]\|#]+)(?:#[^\]\|]*)?(?:\|[^\]]*)?\]\]`)
	tagRE      = regexp.MustCompile(`(?:^|\s)#([A-Za-z][A-Za-z0-9_/-]*)`)
	headingRE  = regexp.MustCompile(`(?m)^#\s+(.+?)\s*$`)
)

// Open checks the folder exists and returns the vault.
func Open(root string) (*Vault, error) {
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	if resolved, err := filepath.EvalSymlinks(abs); err == nil {
		abs = resolved
	}
	if st, err := os.Stat(abs); err != nil || !st.IsDir() {
		return nil, fmt.Errorf("vault folder %s does not exist", root)
	}
	return &Vault{Root: abs}, nil
}

// ParseNote reads the structure of a note's text. rel is its path inside the vault.
func ParseNote(rel, text string, modified time.Time) Note {
	n := Note{Path: filepath.ToSlash(rel), Modified: modified, Front: map[string]string{}}
	body := text
	if strings.HasPrefix(text, "---\n") {
		if end := strings.Index(text[4:], "\n---"); end >= 0 {
			for _, line := range strings.Split(text[4:4+end], "\n") {
				if k, v, ok := strings.Cut(line, ":"); ok {
					n.Front[strings.ToLower(strings.TrimSpace(k))] = strings.TrimSpace(v)
				}
			}
			body = strings.TrimLeft(text[4+end+4:], "\n")
		}
	}
	n.Body = body
	stem := strings.TrimSuffix(filepath.Base(rel), ".md")
	n.Title = stem
	if m := headingRE.FindStringSubmatch(body); m != nil {
		n.Title = m[1]
	}
	for _, m := range wikilinkRE.FindAllStringSubmatch(body, -1) {
		n.Links = append(n.Links, strings.TrimSpace(m[1]))
	}
	seen := map[string]bool{}
	add := func(t string) {
		t = strings.ToLower(strings.Trim(t, " ,#\"'"))
		if t != "" && !seen[t] {
			seen[t] = true
			n.Tags = append(n.Tags, t)
		}
	}
	for _, m := range tagRE.FindAllStringSubmatch(body, -1) {
		add(m[1])
	}
	if t, ok := n.Front["tags"]; ok {
		for _, p := range strings.FieldsFunc(strings.Trim(t, "[]"), func(r rune) bool { return r == ',' || r == ' ' }) {
			add(p)
		}
	}
	return n
}

// Notes reads every Markdown file in the vault (not the hidden folders such as .obsidian or .git), newest first.
func (v *Vault) Notes() ([]Note, error) {
	var out []Note
	err := filepath.WalkDir(v.Root, func(p string, d os.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if d.IsDir() {
			if p != v.Root && strings.HasPrefix(d.Name(), ".") {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(d.Name(), ".md") || strings.HasPrefix(d.Name(), ".") {
			return nil
		}
		info, err := d.Info()
		if err != nil || info.Size() > 512*1024 {
			return nil // unreadable or not a note (an export, a dump)
		}
		b, err := os.ReadFile(p)
		if err != nil {
			return nil
		}
		rel, _ := filepath.Rel(v.Root, p)
		out = append(out, ParseNote(rel, string(b), info.ModTime()))
		return nil
	})
	sort.Slice(out, func(i, j int) bool { return out[i].Modified.After(out[j].Modified) })
	return out, err
}

// Resolve finds the note a [[link]] means: by path, or Obsidian-style by file name, ignoring case. "" when there is none.
func Resolve(notes []Note, target string) string {
	t := strings.ToLower(strings.TrimSuffix(filepath.ToSlash(strings.TrimSpace(target)), ".md"))
	for _, n := range notes {
		p := strings.ToLower(strings.TrimSuffix(n.Path, ".md"))
		if p == t || strings.HasSuffix(p, "/"+t) {
			return n.Path
		}
	}
	return ""
}

// ErrOutside is returned for a path that would leave the vault or touch a hidden file.
var ErrOutside = errors.New("that path is outside the vault or is a hidden file")

// SafePath turns a note name into a path inside the vault: ".md" is added, hidden files and folders, "..", absolute paths and
// anything that is not a plain note name are refused. Returns the relative and the absolute path.
func (v *Vault) SafePath(name string) (string, string, error) {
	name = strings.TrimSpace(filepath.ToSlash(name))
	if name == "" || strings.HasPrefix(name, "/") || strings.ContainsAny(name, "\x00\\") || len(name) > 200 {
		return "", "", ErrOutside
	}
	if !strings.HasSuffix(name, ".md") {
		name += ".md"
	}
	clean := filepath.ToSlash(filepath.Clean(name))
	for _, part := range strings.Split(clean, "/") {
		if part == ".." || part == "." || part == "" || strings.HasPrefix(part, ".") {
			return "", "", ErrOutside
		}
	}
	abs := filepath.Join(v.Root, filepath.FromSlash(clean))
	if rel, err := filepath.Rel(v.Root, abs); err != nil || strings.HasPrefix(rel, "..") {
		return "", "", ErrOutside
	}
	// a symlinked folder inside the vault must not lead out of it
	if dir, err := filepath.EvalSymlinks(filepath.Dir(abs)); err == nil {
		if rel, err := filepath.Rel(v.Root, dir); err != nil || strings.HasPrefix(rel, "..") {
			return "", "", ErrOutside
		}
	}
	return clean, abs, nil
}
