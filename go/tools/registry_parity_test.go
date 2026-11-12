package tools

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// parityTools are made-up tools of every permission and class; what a call does depends on its "mode" argument.
func parityTools() []Tool {
	schema := map[string]any{"type": "object", "properties": map[string]any{"n": map[string]any{"type": "integer"},
		"mode": map[string]any{"type": "string"}}}
	handler := func(_ context.Context, args map[string]any, _ *Context) (string, map[string]any, error) {
		switch args["mode"] {
		case "err":
			return "", map[string]any{"k": "v"}, errors.New("it broke <here>")
		case "denied":
			return "", nil, Denied("not for %s", "you")
		case "wrapped":
			return "", nil, &wrapErr{Denied("deep")}
		case "panic":
			panic("boom")
		case "mderr":
			return "lost", map[string]any{"error_kind": "mine", "y": true}, errors.New("failed after all")
		case "md":
			return "with metadata", map[string]any{"error_kind": "mine", "z": 1.5, "streamed": false}, nil
		}
		b, _ := json.Marshal(args)
		return "ran " + string(b), nil, nil
	}
	mk := func(name string, req Permission, class Class, params map[string]any) Tool {
		return Tool{Spec: llm.ToolSpec{Name: name, Description: "the " + name + " tool", Parameters: params}, Handler: handler, Requires: req, Class: class}
	}
	return []Tool{mk("r", PermNone, ClassReadOnly, schema), mk("w", PermWrite, ClassMutating, schema), mk("s", PermShell, ClassMutating, schema),
		mk("u", PermNone, ClassUnset, nil), mk("m", PermNone, ClassMutating, schema), mk(ToolEnterPlanMode, PermNone, ClassPlanControl, nil),
		mk(ToolExitPlanMode, PermNone, ClassPlanControl, nil), mk("ws", PermWrite, ClassReadOnly, schema)}
}

type wrapErr struct{ inner error }

func (e *wrapErr) Error() string { return "wrapped: " + e.inner.Error() }
func (e *wrapErr) Unwrap() error { return e.inner }

func resultOut(r Result) []any {
	md, _ := json.Marshal(r.Metadata)
	return []any{r.Name, r.OK, r.Content, string(md)}
}

// TestParityRegistry records the permission gate and the registry (testdata/parity/registry.json): which tools a
// Builder offers for every allowlist, narrowing, plan mode, permission set and deny rule; and what Execute answers for
// every tool, permission set, deny rule, approver and argument, in Go's order (unknown tool, policy, permission, schema,
// approver, handler).
func TestParityRegistry(t *testing.T) {
	var cases []parity.Case
	permSets := []config.Permissions{{}, {AllowFileWrite: true}, {AllowShellCommands: true}, {AllowFileWrite: true, AllowShellCommands: true}}
	permName := func(p config.Permissions) string {
		return map[bool]string{true: "w", false: "-"}[p.AllowFileWrite] + map[bool]string{true: "s", false: "-"}[p.AllowShellCommands]
	}
	denyW := func(name string) string {
		if strings.HasPrefix(name, "w") {
			return "the operator's policy forbids " + name
		}
		return ""
	}
	lists := [][]string{nil, {}, {"r", "w", "zz"}, {"s", "exit_plan_mode", "enter_plan_mode", "u", "m"}}
	for _, allow := range lists {
		for _, narrow := range lists {
			for _, plan := range []bool{false, true} {
				for pi, perms := range append([]*config.Permissions{nil}, &permSets[0], &permSets[1], &permSets[3]) {
					for _, deny := range []bool{false, true} {
						b := Builder{Allowlist: allow, Narrow: narrow, PlanMode: plan, Perms: perms, Extra: parityTools(), NoDefaults: true}
						if deny {
							b.Deny = denyW
						}
						reg := b.Build()
						var specs []string
						for _, s := range reg.Specs() {
							specs = append(specs, s.Name)
						}
						_, hasW := reg.Get("w")
						cases = append(cases, parity.Case{Fn: "Build", In: []any{allow, narrow, plan, pi, deny},
							Out: []any{reg.Names(), reg.Len(), specs, hasW}})
					}
				}
			}
		}
	}
	argSets := []map[string]any{nil, {"mode": "ok", "n": 3.0}, {"mode": "err"}, {"mode": "denied"}, {"mode": "wrapped"}, {"mode": "panic"},
		{"mode": "md"}, {"mode": "mderr"}, {"n": "x"}, {"n": 2.5}, {"mode": 5.0}, {"extra": "kept", "n": "7"}}
	for _, name := range []string{"r", "w", "s", "u", "m", "enter_plan_mode", "exit_plan_mode", "ws", "zz"} {
		for _, perms := range permSets {
			for _, deny := range []bool{false, true} {
				for ai, approver := range []string{"none", "allow", "deny", "deny-why"} {
					for _, args := range argSets {
						reg := NewRegistry(parityTools()...)
						if deny {
							reg.deny = denyW
						}
						tc := &Context{Permissions: perms}
						var asked any
						if approver != "none" {
							tc.Approver = func(_ context.Context, req ApprovalRequest) (bool, string) {
								a, _ := json.Marshal(req.Arguments)
								asked = []any{req.Tool, string(a), req.Needs.String()}
								switch approver {
								case "deny":
									return false, ""
								case "deny-why":
									return false, "not now"
								}
								return true, ""
							}
						}
						res := reg.Execute(context.Background(), name, args, tc)
						cases = append(cases, parity.Case{Fn: "Execute", In: []any{name, permName(perms), deny, ai, parity.Typed(args)},
							Out: []any{resultOut(res), asked}})
					}
				}
			}
		}
	}
	// the gate's own words, and Permission's names
	for _, p := range []Permission{PermNone, PermWrite, PermShell, Permission(7)} {
		cases = append(cases, parity.Case{Fn: "Permission", In: int(p), Out: p.String()})
	}
	parity.Fixture(t, "registry", cases)
}
