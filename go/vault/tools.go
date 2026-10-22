package vault

import (
	"context"
	"fmt"
	"os"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/guard"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// EnvVar names the vault folder. The tools exist only when it is set (and the folder is there).
const EnvVar = "SARINA_VAULT"

// RuntimeKey holds a *Vault (and ScannerKey a Scanner) in Context.Runtimes: tests use it instead of the environment.
const (
	RuntimeKey = "vault"
	ScannerKey = "vault_scanner"
)

func init() { tools.RegisterProvider(Tools) }

func vaultFor(tc *tools.Context) (*Vault, error) {
	if v, ok := tc.Runtimes[RuntimeKey].(*Vault); ok && v != nil {
		return v, nil
	}
	dir := os.Getenv(EnvVar)
	if tc.Environ != nil {
		for _, kv := range tc.Environ() {
			if strings.HasPrefix(kv, EnvVar+"=") {
				dir = strings.TrimPrefix(kv, EnvVar+"=")
			}
		}
	}
	if dir == "" {
		return nil, fmt.Errorf("no vault is set up (%s)", EnvVar)
	}
	return Open(dir)
}

func gateFor(tc *tools.Context) Scanner {
	if s, ok := tc.Runtimes[ScannerKey].(Scanner); ok {
		return s
	}
	return guard.Memory()
}

func str(a map[string]any, k string) string { v, _ := a[k].(string); return v }

// Tools returns vault_search, vault_read (read-only) and vault_write (write-gated), or nothing when no vault is configured, so a
// machine without one does not pay for them in every request.
func Tools() []tools.Tool {
	if os.Getenv(EnvVar) == "" {
		return nil
	}
	return []tools.Tool{
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "vault_search",
				Description: "Search your linked notes (the vault) by words and by links. Read the best note with vault_read before answering.",
				Parameters: map[string]any{"type": "object", "required": []string{"query"}, "properties": map[string]any{
					"query":       map[string]any{"type": "string", "description": "Words to look for."},
					"max_results": map[string]any{"type": "integer", "minimum": 1, "maximum": 20, "description": "Default 8."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				v, err := vaultFor(tc)
				if err != nil {
					return "", nil, err
				}
				notes, err := v.Notes()
				if err != nil {
					return "", nil, err
				}
				n := 8
				if f, ok := a["max_results"].(float64); ok && f >= 1 {
					n = int(f)
				}
				hits := Search(notes, str(a, "query"), n)
				if len(hits) == 0 {
					return "No notes matched.", map[string]any{"action": "vault_search", "hits": 0}, nil
				}
				var b strings.Builder
				for _, h := range hits {
					fmt.Fprintf(&b, "%s  (%s)  [%s]\n  %s\n", h.Path, h.Title, strings.Join(h.Why, ", "), h.Snippet)
				}
				return strings.TrimRight(b.String(), "\n"), map[string]any{"action": "vault_search", "hits": len(hits)}, nil
			},
		},
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "vault_read",
				Description: "Read one note, with what it links to and what links to it.",
				Parameters: map[string]any{"type": "object", "required": []string{"path"}, "properties": map[string]any{
					"path": map[string]any{"type": "string", "description": "Note path or name."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				v, err := vaultFor(tc)
				if err != nil {
					return "", nil, err
				}
				notes, err := v.Notes()
				if err != nil {
					return "", nil, err
				}
				p := Resolve(notes, str(a, "path"))
				if p == "" {
					return "", nil, fmt.Errorf("no note called %q (use vault_search)", str(a, "path"))
				}
				var note Note
				for _, n := range notes {
					if n.Path == p {
						note = n
					}
				}
				body := note.Body
				if r := []rune(body); len(r) > 6000 {
					body = string(r[:6000]) + "\n[cut: the note is longer]"
				}
				var b strings.Builder
				fmt.Fprintf(&b, "%s\n\n%s\n", p, body)
				var out []string
				for _, l := range note.Links {
					if t := Resolve(notes, l); t != "" {
						out = append(out, t)
					} else {
						out = append(out, l+" (no such note yet)")
					}
				}
				if len(out) > 0 {
					fmt.Fprintf(&b, "\nLinks to: %s\n", strings.Join(out, ", "))
				}
				if back := Backlinks(notes, p); len(back) > 0 {
					fmt.Fprintf(&b, "Linked from: %s\n", strings.Join(back, ", "))
				}
				return strings.TrimRight(b.String(), "\n"), map[string]any{"action": "vault_read", "path": p}, nil
			},
		},
		{
			Requires: tools.PermWrite, Class: tools.ClassMutating,
			Spec: llm.ToolSpec{
				Name:        "vault_write",
				Description: "Write a note in the vault. Facts only, no secrets. Link related notes with [[name]]. Search before creating, to extend a note instead of repeating one.",
				Parameters: map[string]any{"type": "object", "required": []string{"path", "content"}, "properties": map[string]any{
					"path":    map[string]any{"type": "string", "description": "A name inside the vault, never a full path: Lessons/Verify after writing, or Inbox/Idea."},
					"content": map[string]any{"type": "string", "description": "Markdown, one idea; at most 12000 characters."},
					"mode":    map[string]any{"type": "string", "enum": []string{"create", "append", "replace"}, "description": "Default create."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				v, err := vaultFor(tc)
				if err != nil {
					return "", nil, err
				}
				mode := Mode(str(a, "mode"))
				// a link that misses an existing note only by wording is pointed at that note before saving (and said so), so the note and
				// what she tells the user about it agree
				content, relinked := relink(v, str(a, "content"))
				res, err := v.Write(gateFor(tc), str(a, "path"), content, mode)
				if err != nil {
					return "", nil, err
				}
				v.Commit("note " + res.Path)
				msg := fmt.Sprintf("Saved %s.", res.Path)
				if !res.Created {
					msg = fmt.Sprintf("Updated %s.", res.Path)
				}
				if len(relinked) > 0 {
					msg += " Linked " + strings.Join(relinked, ", ") + " (the notes that exist)."
				}
				if len(res.BrokenLinks) > 0 {
					notes, _ := v.Notes()
					var parts []string
					for _, l := range res.BrokenLinks {
						if sug := Suggest(notes, l); sug != "" {
							parts = append(parts, fmt.Sprintf("[[%s]] (did you mean [[%s]]? fix it with mode replace)", l, sug))
						} else {
							parts = append(parts, "[["+l+"]]")
						}
					}
					msg += " Links to notes that do not exist yet: " + strings.Join(parts, ", ") + "."
				}
				return msg, map[string]any{"action": "vault_write", "path": res.Path, "created": res.Created}, nil
			},
		},
	}
}

var wikiLinkRE = regexp.MustCompile(`\[\[([^\[\]]+)\]\]`)

// relink points [[links]] that name no note, but closely match one, at that note. It returns the new content and what it changed.
func relink(v *Vault, content string) (string, []string) {
	notes, err := v.Notes()
	if err != nil || len(notes) == 0 {
		return content, nil
	}
	var changed []string
	out := wikiLinkRE.ReplaceAllStringFunc(content, func(m string) string {
		inner := m[2 : len(m)-2]
		target, alias := inner, ""
		if i := strings.IndexAny(inner, "|#"); i >= 0 {
			target, alias = inner[:i], inner[i:]
		}
		if Resolve(notes, target) != "" {
			return m
		}
		sug := Suggest(notes, target)
		if sug == "" {
			return m
		}
		name := sug[strings.LastIndex(sug, "/")+1:]
		changed = append(changed, "[["+target+"]] as [["+name+"]]")
		return "[[" + name + alias + "]]"
	})
	return out, changed
}
