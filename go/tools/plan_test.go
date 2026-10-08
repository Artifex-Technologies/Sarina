package tools

import (
	"context"
	"slices"
	"strings"
	"testing"
)

// Every tool must carry an explicit plan-mode decision. When this fails you
// added a tool: decide whether it is read-only and add it to the table.
func TestEveryToolHasAnExplicitPlanModeClass(t *testing.T) {
	want := map[string]Class{
		"read_file": ClassReadOnly, "list_dir": ClassReadOnly, "glob_search": ClassReadOnly, "grep_search": ClassReadOnly,
		"TaskOutput": ClassReadOnly, "task_list": ClassReadOnly,
		"plan_get":   ClassReadOnly,
		"write_file": ClassMutating, "edit_file": ClassMutating, "bash": ClassMutating, "TaskStop": ClassMutating,
		"task_create": ClassMutating, "task_update": ClassMutating,
		"todo_write": ClassMutating, "update_plan": ClassMutating,
		"enter_plan_mode": ClassPlanControl, "exit_plan_mode": ClassPlanControl,
	}
	for _, tool := range Builtins() {
		if tool.Class == ClassUnset {
			t.Errorf("tool %s has no plan-mode class: set Class in its definition", tool.Spec.Name)
		}
		got, known := want[tool.Spec.Name]
		if !known {
			t.Errorf("tool %s is missing from this table: decide read-only or not", tool.Spec.Name)
		} else if tool.Class != got {
			t.Errorf("%s class = %v, want %v", tool.Spec.Name, tool.Class, got)
		}
	}
	// A read-only tool must never need write or shell permission.
	for _, tool := range Builtins() {
		if tool.Class == ClassReadOnly && tool.Requires != PermNone {
			t.Errorf("%s is marked read-only but requires %v", tool.Spec.Name, tool.Requires)
		}
	}
}

func TestForModeIsRemovalOnlyAndFailsClosed(t *testing.T) {
	undecided := simple("mystery", nil)
	reg := NewRegistry(append(DefaultTools(), undecided)...)
	plan := reg.ForMode(true)
	if _, ok := plan.Get("mystery"); ok {
		t.Fatal("a tool with no decision must not be offered in plan mode")
	}
	for _, n := range plan.Names() {
		if _, ok := reg.Get(n); !ok {
			t.Fatalf("plan mode added %s", n)
		}
		if tl, _ := reg.Get(n); n != ToolExitPlanMode && tl.Class != ClassReadOnly {
			t.Fatalf("plan mode kept non-read-only %s", n)
		}
	}
	normal := reg.ForMode(false)
	if _, ok := normal.Get(ToolExitPlanMode); ok {
		t.Fatal("exit_plan_mode offered outside plan mode")
	}
	if _, ok := normal.Get("mystery"); !ok {
		t.Fatal("normal mode must keep other tools")
	}
}

func TestBuilderOrderAllowlistThenNarrowThenPlan(t *testing.T) {
	b := Builder{Allowlist: []string{"read_file", "write_file", "exit_plan_mode"}, PlanMode: true}
	if got := b.Build().Names(); !slices.Equal(got, []string{"read_file", "exit_plan_mode"}) {
		t.Fatalf("%v", got)
	}
	b = Builder{Allowlist: []string{}, PlanMode: true}
	if b.Build().Len() != 0 {
		t.Fatal("empty allowlist must stay empty in plan mode")
	}
	b = Builder{Allowlist: []string{"read_file"}, Narrow: []string{"read_file", "bash"}}
	if got := b.Build().Names(); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("narrow widened: %v", got)
	}
	if (Builder{Narrow: []string{}}).Build().Len() != 0 {
		t.Fatal("empty narrow must mean no tools")
	}
}

func TestPlanToolHandlers(t *testing.T) {
	reg := NewRegistry(PlanTools()...)
	active := false
	tc := &Context{Runtimes: map[string]any{RuntimePlanActive: func() bool { return active }}}
	run := func(name string, args map[string]any) Result {
		return reg.Execute(context.Background(), name, args, tc)
	}
	if r := run(ToolExitPlanMode, map[string]any{"plan": "p"}); r.OK {
		t.Fatal("exit outside plan mode must fail")
	}
	if r := run(ToolEnterPlanMode, nil); !r.OK || r.Metadata["plan_action"] != "enter" {
		t.Fatalf("%+v", r)
	}
	active = true
	if r := run(ToolEnterPlanMode, nil); r.OK {
		t.Fatal("entering twice must fail")
	}
	if r := run(ToolExitPlanMode, map[string]any{"plan": "p"}); !r.OK || r.Metadata["plan_action"] != "exit" {
		t.Fatalf("%+v", r)
	}
	if r := run(ToolExitPlanMode, map[string]any{}); r.OK {
		t.Fatal("plan is required")
	}
	if r := run(ToolExitPlanMode, map[string]any{"plan": strings.Repeat("x", MaxPlanChars+1)}); r.OK {
		t.Fatal("oversized plan must be refused")
	}
	if r := run(ToolExitPlanMode, map[string]any{"plan": strings.Repeat("x", MaxPlanChars)}); !r.OK {
		t.Fatal("plan at the limit is fine")
	}
}
