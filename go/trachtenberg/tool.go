package trachtenberg

import (
	"context"
	"fmt"
	"os"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

func init() {
	tools.RegisterProvider(Tools)
	cli.Register("trachtenberg", "work a multiplication by the Trachtenberg method, step by step: trachtenberg A B [ANSWER-TO-CHECK]", command)
}

// Tools offers trachtenberg when SARINA_MATH is on (auto turns it on for questions about it, in cli.Main): its description costs every
// request a little context.
func Tools() []tools.Tool {
	if os.Getenv("SARINA_MATH") != "on" {
		return nil
	}
	return []tools.Tool{{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "trachtenberg",
			Description: "Work a multiplication by the Trachtenberg method exactly, step by step (rules for 3-9, 11, 12; the two-finger method for any other). Give answer to check a learner's result. Teach only from its steps.",
			Parameters: map[string]any{"type": "object", "required": []string{"a", "b"}, "properties": map[string]any{
				"a":      map[string]any{"type": "string"},
				"b":      map[string]any{"type": "string"},
				"answer": map[string]any{"type": "string", "description": "A learner's answer to check."},
			}},
		},
		Handler: func(ctx context.Context, args map[string]any, tc *tools.Context) (string, map[string]any, error) {
			str := func(k string) string { v, _ := args[k].(string); return v }
			w, err := WorkOut(str("a"), str("b"))
			if err != nil {
				return "", nil, err
			}
			out := Report(w)
			if ans := strings.TrimSpace(str("answer")); ans != "" {
				out += "\n" + Grade(w, ans)
			}
			return out + "\n[Computed exactly. Teach from these steps; do not do the arithmetic yourself.]", map[string]any{"action": "trachtenberg"}, nil
		},
	}}
}

func command(env *cli.Env, args []string) int {
	if len(args) < 2 {
		fmt.Fprintln(env.Stderr, "usage: sarina trachtenberg A B [ANSWER-TO-CHECK]")
		return 2
	}
	w, err := WorkOut(args[0], args[1])
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina trachtenberg:", err)
		return 1
	}
	fmt.Fprintln(env.Stdout, Report(w))
	if len(args) > 2 {
		fmt.Fprintln(env.Stdout, Grade(w, args[2]))
	}
	return 0
}
