// Package skills implements named, reusable prompts: their file format,
// operator-controlled discovery, and the model-facing skill tool.
package skills

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

// Skill is one parsed skill.
type Skill struct {
	Name           string
	Description    string
	WhenToUse      string
	AllowedTools   []string // meaningful only if HasAllowed
	HasAllowed     bool     // allowed_tools was present (an empty list then means no tools)
	UserInvocable  bool     // default true
	ModelInvocable bool     // default false
	Body           string
	Path           string
	Source         string // "operator" or "workspace"
}

// Expand returns the prompt: the body with every $ARGUMENTS replaced, by plain
// text substitution (the arguments are not themselves expanded again).
func (s *Skill) Expand(args string) string {
	return strings.ReplaceAll(s.Body, "$ARGUMENTS", args)
}

var nameRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$`)

// ValidName reports whether name is usable as a skill name.
func ValidName(name string) bool { return nameRE.MatchString(name) }

// Parse reads a skill file: a front-matter block between "---" lines, then the
// prompt body. The front matter is a deliberately small subset of YAML: flat
// "key: value" lines, '#' comments, optional single or double quotes, booleans
// (true/false/yes/no) and lists written inline ([a, b]), as "- item" lines, or
// as a comma-separated scalar. Unknown keys are ignored. fallbackName is used
// when "name" is absent.
func Parse(data []byte, fallbackName string) (*Skill, error) {
	text := strings.TrimPrefix(string(data), "\xef\xbb\xbf")
	text = strings.ReplaceAll(text, "\r\n", "\n")
	lines := strings.Split(text, "\n")
	if strings.TrimRight(lines[0], " \t") != "---" {
		return nil, fmt.Errorf("no front matter: the file must start with a '---' line")
	}
	end := -1
	for i := 1; i < len(lines); i++ {
		if strings.TrimRight(lines[i], " \t") == "---" {
			end = i
			break
		}
	}
	if end < 0 {
		return nil, fmt.Errorf("front matter is not closed by a '---' line")
	}
	fm, err := parseFrontMatter(lines[1:end])
	if err != nil {
		return nil, err
	}
	s := &Skill{UserInvocable: true, Body: strings.TrimSpace(strings.Join(lines[end+1:], "\n"))}
	if s.Name = fm.scalar("name"); s.Name == "" {
		s.Name = fallbackName
	}
	if !ValidName(s.Name) {
		return nil, fmt.Errorf("invalid skill name %q (letters, digits, . _ : - ; at most 64 characters)", s.Name)
	}
	s.Description, s.WhenToUse = fm.scalar("description"), fm.scalar("when_to_use")
	if v, ok := fm["allowed_tools"]; ok {
		s.HasAllowed = true
		s.AllowedTools = v.items()
	}
	for _, b := range []struct {
		key string
		dst *bool
	}{{"user_invocable", &s.UserInvocable}, {"model_invocable", &s.ModelInvocable}} {
		if v, ok := fm[b.key]; ok {
			switch strings.ToLower(v.text) {
			case "true", "yes":
				*b.dst = true
			case "false", "no":
				*b.dst = false
			default:
				return nil, fmt.Errorf("%s must be true or false, got %q", b.key, v.text)
			}
		}
	}
	if s.Body == "" {
		return nil, fmt.Errorf("the skill has no body")
	}
	return s, nil
}

type fmValue struct {
	text   string   // scalar form
	list   []string // list form
	isList bool
}

// items returns the value as a list: the list form, or a comma-separated scalar.
func (v fmValue) items() []string {
	out := []string{}
	if v.isList {
		return append(out, v.list...)
	}
	for _, it := range strings.Split(v.text, ",") {
		if it = strings.TrimSpace(it); it != "" {
			out = append(out, it)
		}
	}
	return out
}

type frontMatter map[string]fmValue

func (f frontMatter) scalar(k string) string { return f[k].text }

func parseFrontMatter(lines []string) (frontMatter, error) {
	fm := frontMatter{}
	open := "" // key whose "- item" lines are being collected
	for n, raw := range lines {
		line := strings.TrimRight(raw, " \t")
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		if trimmed == "-" || strings.HasPrefix(trimmed, "- ") {
			if open == "" {
				return nil, fmt.Errorf("front matter line %d: list item with no key", n+1)
			}
			v := fm[open]
			v.list = append(v.list, unquote(strings.TrimSpace(strings.TrimPrefix(trimmed, "-"))))
			fm[open] = v
			continue
		}
		if line[0] == ' ' || line[0] == '\t' {
			return nil, fmt.Errorf("front matter line %d: nested or multi-line values are not supported", n+1)
		}
		key, val, ok := strings.Cut(line, ":")
		if !ok || strings.TrimSpace(key) == "" {
			return nil, fmt.Errorf("front matter line %d: expected 'key: value'", n+1)
		}
		key, val = strings.TrimSpace(key), strings.TrimSpace(val)
		if _, dup := fm[key]; dup {
			return nil, fmt.Errorf("front matter line %d: duplicate key %q", n+1, key)
		}
		open = ""
		switch {
		case val == "":
			// Either an empty value or a block list that follows.
			fm[key] = fmValue{}
			open = key
		case strings.HasPrefix(val, "["):
			if !strings.HasSuffix(val, "]") {
				return nil, fmt.Errorf("front matter line %d: unterminated list", n+1)
			}
			var items []string
			if inner := strings.TrimSpace(val[1 : len(val)-1]); inner != "" {
				for _, it := range strings.Split(inner, ",") {
					if it = unquote(strings.TrimSpace(it)); it != "" {
						items = append(items, it)
					}
				}
			}
			fm[key] = fmValue{list: items, isList: true}
		case strings.ContainsRune("{|>&*!", rune(val[0])):
			return nil, fmt.Errorf("front matter line %d: unsupported YAML construct in %q", n+1, key)
		default:
			fm[key] = fmValue{text: unquote(val)}
		}
	}
	// "key:" followed by "- item" lines is a list; with no items it is empty.
	for k, v := range fm {
		if len(v.list) > 0 {
			v.isList = true
			fm[k] = v
		}
	}
	return fm, nil
}

func unquote(v string) string {
	if len(v) >= 2 && v[0] == '"' && v[len(v)-1] == '"' {
		if u, err := strconv.Unquote(v); err == nil {
			return u
		}
		return v[1 : len(v)-1]
	}
	if len(v) >= 2 && v[0] == '\'' && v[len(v)-1] == '\'' {
		return strings.ReplaceAll(v[1:len(v)-1], "''", "'")
	}
	return v
}
