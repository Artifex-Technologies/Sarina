package tools

import (
	"context"
	"encoding/json"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityPlanTools records enter_plan_mode and exit_plan_mode (testdata/parity/plantools.json) with plan mode unknown,
// on and off, and plans at, under and over the limit.
func TestParityPlanTools(t *testing.T) {
	reg := NewRegistry(PlanTools()...)
	plans := []any{"", "a plan", strings.Repeat("é", MaxPlanChars), strings.Repeat("x", MaxPlanChars+1), 5, nil}
	var cases []parity.Case
	for _, mode := range []string{"unknown", "on", "off"} {
		tc := &Context{}
		if mode != "unknown" {
			on := mode == "on"
			tc.Runtimes = map[string]any{RuntimePlanActive: func() bool { return on }}
		}
		var out []any
		r := reg.Execute(context.Background(), ToolEnterPlanMode, map[string]any{"x": 1}, tc)
		md, _ := json.Marshal(r.Metadata)
		out = append(out, []any{r.OK, r.Content, string(md)})
		for _, p := range plans {
			args := map[string]any{"plan": p}
			if p == nil {
				args = map[string]any{}
			}
			r := reg.Execute(context.Background(), ToolExitPlanMode, args, tc)
			md, _ := json.Marshal(r.Metadata)
			out = append(out, []any{r.OK, r.Content, string(md)})
		}
		cases = append(cases, parity.Case{Fn: "mode", In: mode, Out: out})
	}
	cases = append(cases, parity.Case{Fn: "plans", In: plans})
	parity.Fixture(t, "plantools", cases)
}
