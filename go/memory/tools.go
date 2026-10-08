package memory

import (
	"context"
	"fmt"
	"path/filepath"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/sessionsearch"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Context.Runtimes keys.
const (
	// RuntimeKey holds a *Runtime that overrides the one built from the workspace.
	RuntimeKey = "memory"
	// HomeKey holds a home-directory string (tests) for the user-scope file.
	HomeKey = "memory_home"
)

func init() { tools.RegisterProvider(Tools) }

func runtimeFor(tc *tools.Context) *Runtime {
	if rt, ok := tc.Runtimes[RuntimeKey].(*Runtime); ok && rt != nil {
		return rt
	}
	home, _ := tc.Runtimes[HomeKey].(string)
	return FromWorkspace(tc.Root, home)
}

func s(args map[string]any, k string) string { v, _ := args[k].(string); return v }

func result(verb string, m Mutation) (string, map[string]any, error) {
	return fmt.Sprintf("%s %s memory entry (%d total)", verb, m.Scope, m.AfterCount), map[string]any{
		"action": "memory_" + m.Action, "scope": m.Scope, "path": m.Path,
		"before_count": m.BeforeCount, "after_count": m.AfterCount,
	}, nil
}

var scopeProp = map[string]any{
	"type": "string", "enum": []string{"agent", "user"},
	"description": "agent = this project; user = operator.",
}

// scopeEnum is scopeProp without the explanation, for the tools after memory_add.
var scopeEnum = map[string]any{"type": "string", "enum": []string{"agent", "user"}}

const factRule = " Facts only, no secrets."

// Tools returns memory_add, memory_replace, memory_remove (write-gated) and
// session_search (read-only). There is deliberately no memory_read: the
// entries are already in your context at session start.
func Tools() []tools.Tool {
	return []tools.Tool{
		{
			Requires: tools.PermWrite, Class: tools.ClassMutating,
			Spec: llm.ToolSpec{
				Name:        "memory_add",
				Description: "Save a short entry to curated memory." + factRule,
				Parameters: map[string]any{"type": "object", "required": []string{"scope", "content"}, "properties": map[string]any{
					"scope":   scopeProp,
					"content": map[string]any{"type": "string", "description": "At most 1500 characters."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				m, err := runtimeFor(tc).Add(s(a, "scope"), s(a, "content"))
				if err != nil {
					return "", nil, err
				}
				return result("saved", m)
			},
		},
		{
			Requires: tools.PermWrite, Class: tools.ClassMutating,
			Spec: llm.ToolSpec{
				Name:        "memory_replace",
				Description: "Replace one memory entry." + factRule,
				Parameters: map[string]any{"type": "object", "required": []string{"scope", "old_text", "content"}, "properties": map[string]any{
					"scope":    scopeEnum,
					"old_text": map[string]any{"type": "string", "description": "Matches exactly one entry."},
					"content":  map[string]any{"type": "string", "description": "The new entry."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				m, err := runtimeFor(tc).Replace(s(a, "scope"), s(a, "old_text"), s(a, "content"))
				if err != nil {
					return "", nil, err
				}
				return result("replaced", m)
			},
		},
		{
			Requires: tools.PermWrite, Class: tools.ClassMutating,
			Spec: llm.ToolSpec{
				Name:        "memory_remove",
				Description: "Remove one memory entry.",
				Parameters: map[string]any{"type": "object", "required": []string{"scope", "old_text"}, "properties": map[string]any{
					"scope":    scopeEnum,
					"old_text": map[string]any{"type": "string", "description": "Matches exactly one entry."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				m, err := runtimeFor(tc).Remove(s(a, "scope"), s(a, "old_text"))
				if err != nil {
					return "", nil, err
				}
				return result("removed", m)
			},
		},
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "session_search",
				Description: "Search earlier saved sessions for a phrase.",
				Parameters: map[string]any{"type": "object", "required": []string{"query"}, "properties": map[string]any{
					"query":       map[string]any{"type": "string", "description": "Phrase to find."},
					"max_results": map[string]any{"type": "integer", "minimum": 1, "maximum": 50, "description": "Default 10."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				return SearchSessions(s(a, "query"), SessionDir(tc), intArg(a, "max_results", 10)), nil, nil
			},
		},
	}
}

func intArg(a map[string]any, k string, def int) int {
	switch v := a[k].(type) {
	case int:
		return v
	case int64:
		return int(v)
	case float64:
		return int(v)
	}
	return def
}

// SessionDir is where saved sessions live for a tool context.
func SessionDir(tc *tools.Context) string {
	if tc.SessionDirectory != "" {
		return tc.SessionDirectory
	}
	return filepath.Join(tc.Root, ".port_sessions", "agent")
}

// SearchSessions renders session search results the way the tool returns them.
func SearchSessions(query, dir string, max int) string {
	hits := sessionsearch.Search(query, dir, max)
	if len(hits) == 0 {
		return "No matching messages were found in the saved sessions."
	}
	out := fmt.Sprintf("Found %d matching message(s) in saved sessions:", len(hits))
	for _, h := range hits {
		out += fmt.Sprintf("\nsession=%s turn=%d role=%s\n  %s", h.SessionID, h.TurnIndex, h.Role, h.Snippet)
	}
	return out
}
