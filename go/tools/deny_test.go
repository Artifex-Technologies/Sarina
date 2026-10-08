package tools

import (
	"context"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

func TestBuilderDenyHidesAndRefusesTools(t *testing.T) {
	ran := false
	mk := func(name string) Tool {
		return Tool{Class: ClassReadOnly, Spec: llm.ToolSpec{Name: name, Description: name, Parameters: map[string]any{"type": "object"}},
			Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
				ran = true
				return "ran " + name, nil, nil
			}}
	}
	deny := func(n string) string {
		if n == "b" {
			return "tool b is denied by the operator policy (test)"
		}
		return ""
	}
	reg := Builder{NoDefaults: true, Extra: []Tool{mk("a"), mk("b")}, Deny: deny}.Build()
	if got := reg.Names(); len(got) != 1 || got[0] != "a" || reg.Len() != 1 || len(reg.Specs()) != 1 {
		t.Fatalf("names=%v len=%d specs=%d", got, reg.Len(), len(reg.Specs()))
	}
	r := reg.Execute(t.Context(), "b", nil, &Context{})
	if ran || r.OK || r.Metadata["error_kind"] != "permission_denied" || !strings.Contains(r.Content, "denied by the operator policy") {
		t.Fatalf("ran=%v %+v", ran, r)
	}
	if r := reg.Execute(t.Context(), "a", nil, &Context{}); !r.OK {
		t.Fatalf("%+v", r)
	}
	// plan mode and an allowlist only remove tools; the denial survives both
	plan := Builder{NoDefaults: true, Extra: []Tool{mk("a"), mk("b")}, Deny: deny, PlanMode: true, Allowlist: []string{"a", "b"}}.Build()
	if got := plan.Names(); len(got) != 1 || got[0] != "a" {
		t.Fatalf("names = %v", got)
	}
}
