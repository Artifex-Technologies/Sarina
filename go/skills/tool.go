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
func Tool(c *Catalog) (tools.Tool, bool) {
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
			return s.Expand(arguments), md, nil
		},
	}, true
}
