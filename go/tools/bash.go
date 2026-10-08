package tools

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Bash runs a shell command in the workspace.
//
// Gates, in order: the shell permission (central, on every path including
// streaming), then the destructive-command check unless --unsafe. The child
// environment is filtered of secret-looking variables. bash is deliberately
// NOT confined by the secret-file guard: running `cat .env` through it is an
// explicit act the operator allowed by enabling the shell.
func Bash() Tool {
	run := func(ctx context.Context, args map[string]any, tc *Context, emit Emit) (string, map[string]any, error) {
		command := argStr(args, "command")
		if strings.TrimSpace(command) == "" {
			return "", nil, fmt.Errorf("command is empty")
		}
		if !tc.Permissions.AllowDestructiveShellCommands {
			if v := CheckDestructive(command, tc.Root); v.Blocked {
				return "", nil, Denied("refused: %s. Destructive commands are blocked unless the operator re-runs with --unsafe (allow destructive shell commands). "+
					"If this is a mistake, rephrase the command without it; otherwise tell the user what you wanted to do and let them decide.", v.Reason)
			}
		}
		if argBool(args, "run_in_background", false) {
			p, err := procManager(tc).Start(command, tc)
			if err != nil {
				return "", nil, err
			}
			return fmt.Sprintf("Started background process %s (pid %d). Read its output with TaskOutput and stop it with TaskStop, using task_id %q.", p.ID, p.PID, p.ID),
				map[string]any{"action": "bash", "command": command, "background": true, "task_id": p.ID, "pid": p.PID}, nil
		}
		timeout := tc.CommandTimeout
		if n := argInt(args, "timeout_seconds", 0); n > 0 {
			timeout = time.Duration(n) * time.Second
		}
		res, err := runForeground(ctx, command, tc, timeout, emit)
		if err != nil {
			return "", nil, err
		}
		md := map[string]any{
			"action": "bash", "command": command, "exit_code": res.exitCode,
			"stdout_preview": preview(res.stdout, 240), "stderr_preview": preview(res.stderr, 240),
		}
		switch {
		case res.timedOut:
			md["timed_out"] = true
			return "", md, fmt.Errorf("command timed out after %s and was killed (with everything it started): %s. "+
				"For something long-running use run_in_background, or raise timeout_seconds", timeout, preview(command, 120))
		case res.interrupted:
			md["interrupted"] = true
			return "", md, fmt.Errorf("command was interrupted and killed: %s", preview(command, 120))
		}
		var b strings.Builder
		fmt.Fprintf(&b, "exit_code=%d\n[stdout]\n%s\n[stderr]\n%s", res.exitCode,
			strings.TrimRight(res.stdout, " \t\r\n"), strings.TrimRight(res.stderr, " \t\r\n"))
		text := strings.TrimSpace(b.String())
		md["output_preview"] = preview(text, 240)
		if names := stateOf(tc).authoredIn(command); len(names) > 0 {
			text = fmt.Sprintf("[note: this command involves files you wrote earlier in this session (%s). "+
				"Its output is not an independent measurement of anything; treat it with skepticism.]\n", strings.Join(names, ", ")) + text
		}
		return TruncateOutput(text, tc.MaxOutputChars), md, nil
	}
	return Tool{
		Requires: PermShell, Class: ClassMutating,
		Spec: llm.ToolSpec{
			Name: "bash",
			Description: "Run a bash command in the workspace. Destructive commands are refused. " +
				"run_in_background returns a task_id.",
			Parameters: obj(map[string]any{
				"command":           prop("string", "", "minLength", 1),
				"run_in_background": prop("boolean", ""),
				"timeout_seconds":   prop("integer", "Seconds.", "minimum", 1, "maximum", 600),
			}, "command"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			return run(ctx, args, tc, nil)
		},
		Stream: run,
	}
}
