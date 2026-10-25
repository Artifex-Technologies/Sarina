package skills

import (
	"context"
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// ToolName is the model-facing tool.
const ToolName = "skill"

// Tool builds the `skill` tool for the model. It returns false when no skill
// is model_invocable, so the tool is then not offered at all. Only
// model_invocable skills are listed or runnable. A skill with allowed_tools
// reports them in the result metadata (skill_allowed_tools); the agent narrows
// its registry with them, which can only remove tools.
func Tool(c *Catalog, lib *Library) (tools.Tool, bool) {
	list := c.ModelInvocable()
	if len(list) == 0 {
		return tools.Tool{}, false
	}
	var b strings.Builder
	b.WriteString("Run a named, reusable procedure. The result is the procedure's instructions; follow them. Available skills:\n")
	for _, s := range list {
		fmt.Fprintf(&b, "- %s: %s", s.Name, s.Description)
		if s.WhenToUse != "" {
			fmt.Fprintf(&b, " (use when: %s)", s.WhenToUse)
		}
		b.WriteString("\n")
	}
	return tools.Tool{
		Class: tools.ClassReadOnly, // it only returns text and can only narrow the registry
		Spec: llm.ToolSpec{
			Name:        ToolName,
			Description: strings.TrimRight(b.String(), "\n"),
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"name":      map[string]any{"type": "string", "description": "The skill's name.", "minLength": 1},
					"arguments": map[string]any{"type": "string", "description": "Text substituted for $ARGUMENTS in the skill."},
				},
				"required": []any{"name"},
			},
		},
		Handler: func(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
			name, _ := args["name"].(string)
			arguments, _ := args["arguments"].(string)
			s, ok := c.Get(name)
			if !ok || !s.ModelInvocable {
				var names []string
				for _, m := range list {
					names = append(names, m.Name)
				}
				return "", nil, fmt.Errorf("no skill named %q is available to you; available: %s", name, strings.Join(names, ", "))
			}
			md := map[string]any{"skill": s.Name}
			if s.HasAllowed {
				md["skill_allowed_tools"] = append([]string{}, s.AllowedTools...)
			}
			return s.ExpandWith(arguments, lib), md, nil
		},
	}, true
}

// Section and search tool names.
const (
	SectionToolName = "skill_section"
	SearchToolName  = "skill_search"
)

// LibraryTools are skill_section and skill_search, for reading long skills a part at a time. None when no skill is long.
func LibraryTools(c *Catalog, lib *Library) []tools.Tool {
	// offered when any skill is long (the operator may start one); the descriptions name only those the model may use itself, and the
	// handlers refuse a skill that is neither the model's to use nor started by the operator in this session
	var long []string
	anyLong := false
	if c != nil {
		for _, s := range c.Skills {
			if s.IsLibrary() {
				anyLong = true
				if s.ModelInvocable {
					long = append(long, s.Name)
				}
			}
		}
	}
	if !anyLong {
		return nil
	}
	names := strings.Join(long, ", ")
	if names == "" {
		names = "(only one the operator has started)"
	}
	section := tools.Tool{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        SectionToolName,
			Description: "Read one section of a long skill (its table of contents is in the skill's text). Long skills: " + names,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"skill":   map[string]any{"type": "string", "description": "The skill's name.", "minLength": 1},
					"section": map[string]any{"type": "string", "description": "The section's number from the table of contents, or (part of) its title.", "minLength": 1},
				},
				"required": []any{"skill", "section"},
			},
		},
		Handler: func(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
			name, _ := args["skill"].(string)
			which := fmt.Sprint(args["section"])
			s, ok := c.Get(name)
			if !ok || !s.IsLibrary() || !lib.Readable(s) {
				return "", nil, fmt.Errorf("no long skill named %q that you may read; long skills: %s", name, names)
			}
			n, sec, ok := s.Section(which)
			if !ok {
				return "", nil, fmt.Errorf("skill %s has no section %q (it has %d; use a number from its table of contents, or skill_search)", s.Name, which, len(s.Sections))
			}
			next := ""
			if n < len(s.Sections) {
				next = fmt.Sprintf("\n\n(next: section %d, %s)", n+1, s.Sections[n].Title)
			}
			return fmt.Sprintf("%s, section %d of %d: %s\n\n%s%s", s.Name, n, len(s.Sections), sec.Title, sec.Text, next), map[string]any{"skill": s.Name, "section": n}, nil
		},
	}
	search := tools.Tool{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        SearchToolName,
			Description: "Find the passages of long skills that are about something (by meaning when the embedding model is there, else by words). Long skills: " + names,
			Parameters: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"query": map[string]any{"type": "string", "description": "What to find, in words.", "minLength": 1},
					"skill": map[string]any{"type": "string", "description": "Only this skill (optional; all long skills without it)."},
					"limit": map[string]any{"type": "integer", "description": "How many passages (default 5, at most 10)."},
				},
				"required": []any{"query"},
			},
		},
		Handler: func(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
			query, _ := args["query"].(string)
			skill, _ := args["skill"].(string)
			limit := 5
			if f, ok := args["limit"].(float64); ok && f >= 1 {
				limit = min(int(f), 10)
			}
			found, how, err := lib.Search(skill, query, limit)
			if err != nil {
				return "", nil, err
			}
			if len(found) == 0 {
				return "Nothing in the long skills matches that (" + how + "). Try other words, or open a section from a table of contents.", nil, nil
			}
			var b strings.Builder
			fmt.Fprintf(&b, "%d passage(s), found %s:\n", len(found), how)
			for _, p := range found {
				fmt.Fprintf(&b, "\n--- %s, section %d: %s ---\n%s\n", p.Skill, p.Section, p.Title, p.Text)
			}
			return b.String(), map[string]any{"found": len(found)}, nil
		},
	}
	return []tools.Tool{section, search}
}
