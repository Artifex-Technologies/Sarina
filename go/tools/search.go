package tools

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

const (
	maxGlobMatches  = 20000
	maxGrepFileSize = 20 << 20
	maxGrepLine     = 500
)

// ---- glob_search ------------------------------------------------------------

func splitGlob(pattern string) ([]string, error) {
	p := filepath.ToSlash(pattern)
	if strings.HasPrefix(p, "/") || strings.HasPrefix(p, "~") || (len(p) >= 2 && p[1] == ':') {
		return nil, errors.New("the pattern must be relative to the workspace, not an absolute path")
	}
	var segs []string
	for _, s := range strings.Split(p, "/") {
		switch s {
		case "", ".":
			continue
		case "..":
			return nil, errors.New("the pattern must not contain '..'")
		}
		segs = append(segs, s)
	}
	if len(segs) == 0 {
		return nil, errors.New("the pattern is empty")
	}
	return segs, nil
}

// globMatch matches name segments against pattern segments; "**" matches zero
// or more whole segments.
func globMatch(pat, name []string) bool {
	if len(pat) == 0 {
		return len(name) == 0
	}
	if pat[0] == "**" {
		for i := 0; i <= len(name); i++ {
			if globMatch(pat[1:], name[i:]) {
				return true
			}
		}
		return false
	}
	if len(name) == 0 {
		return false
	}
	if ok, err := path.Match(pat[0], name[0]); err != nil || !ok {
		return false
	}
	return globMatch(pat[1:], name[1:])
}

// couldMatchBelow reports whether anything under the directory name could still
// match, so the walk can skip whole subtrees.
func couldMatchBelow(pat, name []string) bool {
	if len(name) == 0 || len(pat) == 0 {
		return len(pat) > 0 && len(name) == 0
	}
	if pat[0] == "**" {
		return true
	}
	if ok, err := path.Match(pat[0], name[0]); err != nil || !ok {
		return false
	}
	if len(name) == 1 {
		return len(pat) > 1
	}
	return couldMatchBelow(pat[1:], name[1:])
}

// GlobSearch finds paths by glob pattern.
func GlobSearch() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "glob_search",
			Description: "Find files by glob pattern (supports **).",
			Parameters: obj(map[string]any{
				"pattern": prop("string", "", "minLength", 1),
			}, "pattern"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			pat, err := splitGlob(argStr(args, "pattern"))
			if err != nil {
				return "", nil, err
			}
			root := realRoot(tc.Root)
			hasStar := false
			for _, s := range pat {
				if s == "**" {
					hasStar = true
				}
			}
			var matches []string
			capped := false
			walkErr := filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
				if ctx.Err() != nil {
					return ctx.Err()
				}
				if err != nil {
					if d != nil && d.IsDir() {
						return filepath.SkipDir
					}
					return nil
				}
				if p == root {
					return nil
				}
				rel, _ := filepath.Rel(root, p)
				segs := strings.Split(filepath.ToSlash(rel), "/")
				if globMatch(pat, segs) {
					if d.Type()&os.ModeSymlink != 0 {
						real, err := filepath.EvalSymlinks(p)
						if err != nil || !within(root, real) {
							return nil
						}
					}
					name := filepath.ToSlash(rel)
					if d.IsDir() {
						name += "/"
					}
					matches = append(matches, name)
					if len(matches) >= maxGlobMatches {
						capped = true
						return fs.SkipAll
					}
				}
				if d.IsDir() {
					if !hasStar && len(segs) >= len(pat) {
						return filepath.SkipDir
					}
					if !couldMatchBelow(pat, segs) {
						return filepath.SkipDir
					}
				}
				return nil
			})
			if walkErr != nil && !errors.Is(walkErr, fs.SkipAll) {
				return "", nil, walkErr
			}
			if len(matches) == 0 {
				return "(no matches)", nil, nil
			}
			sort.Strings(matches)
			out := strings.Join(matches, "\n")
			if capped {
				out += fmt.Sprintf("\n[stopped at %d matches; use a narrower pattern]", maxGlobMatches)
			}
			return TruncateOutput(out, tc.MaxOutputChars), nil, nil
		},
	}
}

// ---- grep_search ------------------------------------------------------------

// GrepSearch searches file contents with a regular expression (RE2 syntax).
func GrepSearch() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "grep_search",
			Description: "Search file contents with a regular expression (RE2).",
			Parameters: obj(map[string]any{
				"pattern":     prop("string", "", "minLength", 1),
				"path":        prop("string", "Default: workspace root."),
				"literal":     prop("boolean", "Plain text, not a regex."),
				"max_matches": prop("integer", "", "minimum", 1, "maximum", 500),
			}, "pattern"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			src := argStr(args, "pattern")
			if argBool(args, "literal", false) {
				src = regexp.QuoteMeta(src)
			}
			re, err := regexp.Compile(src)
			if err != nil {
				return "", nil, fmt.Errorf("invalid pattern: %v", err)
			}
			given := argStr(args, "path")
			if given == "" {
				given = "."
			}
			start, err := ResolveInRoot(tc.Root, given)
			if err != nil {
				return "", nil, err
			}
			info, err := os.Stat(start)
			if err != nil {
				return "", nil, fmt.Errorf("cannot access %s: %v", given, err)
			}
			if !info.IsDir() {
				if err := secretGuard(given, start, "search"); err != nil {
					return "", nil, err
				}
			}
			limit := argInt(args, "max_matches", 100)
			root := realRoot(tc.Root)
			var out []string
			truncated := false

			searchFile := func(p string) {
				rel := relPath(root, p)
				f, err := os.Open(p)
				if err != nil {
					return
				}
				defer f.Close()
				head := make([]byte, 8000)
				n, _ := io.ReadFull(f, head)
				if bytes.IndexByte(head[:n], 0) >= 0 {
					return
				}
				sc := bufio.NewScanner(io.MultiReader(bytes.NewReader(head[:n]), f))
				sc.Buffer(make([]byte, 0, 64*1024), 1<<20)
				for ln := 1; sc.Scan(); ln++ {
					line := sc.Text()
					if !re.MatchString(line) {
						continue
					}
					line = strings.ToValidUTF8(line, "�")
					if r := []rune(line); len(r) > maxGrepLine {
						line = string(r[:maxGrepLine]) + "..."
					}
					out = append(out, fmt.Sprintf("%s:%d: %s", rel, ln, line))
					if len(out) >= limit {
						truncated = true
						return
					}
				}
			}

			if !info.IsDir() {
				searchFile(start)
			} else {
				werr := filepath.WalkDir(start, func(p string, d fs.DirEntry, err error) error {
					if ctx.Err() != nil {
						return ctx.Err()
					}
					if truncated {
						return fs.SkipAll
					}
					if err != nil {
						if d != nil && d.IsDir() {
							return filepath.SkipDir
						}
						return nil
					}
					if d.IsDir() {
						switch d.Name() {
						case ".git", ".hg", ".svn":
							return filepath.SkipDir
						}
						return nil
					}
					// Only regular files: symlinks are never followed, so a link cannot
					// lead the search out of the workspace or to a secret file.
					if !d.Type().IsRegular() || secretName(d.Name()) {
						return nil
					}
					if fi, err := d.Info(); err != nil || fi.Size() > maxGrepFileSize || hardLinkedToSecret(p) {
						return nil
					}
					searchFile(p)
					if truncated {
						return fs.SkipAll
					}
					return nil
				})
				if werr != nil && !errors.Is(werr, fs.SkipAll) {
					return "", nil, werr
				}
			}
			if len(out) == 0 {
				return "(no matches)", nil, nil
			}
			text := strings.Join(out, "\n")
			if truncated {
				text += fmt.Sprintf("\n[stopped at %d matches; narrow the pattern or path]", limit)
			}
			return TruncateOutput(text, tc.MaxOutputChars), nil, nil
		},
	}
}
