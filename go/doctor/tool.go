package doctor

import (
	"context"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// ToolName is the model-facing name.
const ToolName = "sarina_doctor"

// Tools returns the read-only sarina_doctor tool.
func Tools() []tools.Tool {
	return []tools.Tool{{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name: ToolName,
			Description: "Read-only checkup of this Sarina setup (model backend, sessions, tiers, config files, Limbo, Sentry). " +
				"Returns a JSON array of {id,status,finding,remedy}; start here when something is broken.",
			Parameters: map[string]any{"type": "object", "properties": map[string]any{}},
		},
		Handler: func(ctx context.Context, _ map[string]any, tc *tools.Context) (string, map[string]any, error) {
			o := Options{}
			if tc != nil {
				o.Cwd, o.SessionDir = tc.Root, tc.SessionDirectory
			}
			cs := Run(ctx, o)
			return JSON(cs), map[string]any{"doctor_failed": HasFailure(cs)}, nil
		},
	}}
}
