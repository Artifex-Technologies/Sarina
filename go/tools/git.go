package tools

import (
	"bytes"
	"context"
	"fmt"
	"os/exec"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
)

var gitRefRE = regexp.MustCompile(`^[\w./~^@{}-]{1,100}$`)

// gitSafe are settings that keep a repository's own config from running programs when git only reads: an fsmonitor hook runs on
// status, an external diff or textconv driver on diff, a pager on anything. Hooks are pointed nowhere as well.
var gitSafe = []string{"-c", "core.fsmonitor=false", "-c", "core.pager=cat", "-c", "diff.external=", "-c", "core.hooksPath=/dev/null",
	"-c", "color.ui=false", "--no-pager"}

// GitRead shows what git knows about the workspace, without changing anything: a question about "the uncommitted change" could
// not be answered at all without the shell (eval code-explain-diff, 2026-10-26).
func GitRead() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "git_read",
			Description: "Read-only git: status, diff (uncommitted changes), log, show (a commit).",
			Parameters: obj(map[string]any{
				"command": prop("string", "", "enum", []string{"status", "diff", "log", "show"}),
				"path":    prop("string", ""),
				"ref":     prop("string", "Commit to show or diff against."),
			}, "command"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			command, ref := argStr(args, "command"), argStr(args, "ref")
			if ref != "" && (strings.HasPrefix(ref, "-") || !gitRefRE.MatchString(ref)) {
				return "", nil, fmt.Errorf("ref %q is not a plain commit name", ref)
			}
			var gitArgs []string
			switch command {
			case "status":
				gitArgs = []string{"status", "--short", "--branch"}
			case "diff":
				// against HEAD: staged and unstaged together, which is what "the uncommitted change" means (a new repository has no HEAD)
				gitArgs = []string{"diff", "--no-ext-diff", "--no-textconv", "--stat", "--patch"}
				if ref == "" && exec.CommandContext(ctx, "git", append(append([]string{}, gitSafe...), "-C", tc.Root, "rev-parse", "-q", "--verify", "HEAD")...).Run() == nil {
					ref = "HEAD"
				}
				if ref != "" {
					gitArgs = append(gitArgs, ref)
				}
			case "log":
				gitArgs = []string{"log", "-n", "15", "--date=short", "--format=%h %ad %an: %s"}
			case "show":
				if ref == "" {
					ref = "HEAD"
				}
				gitArgs = []string{"show", "--no-ext-diff", "--no-textconv", "--stat", "--patch", "--format=%h %ad %an%n%n%B", "--date=short", ref}
			default:
				return "", nil, fmt.Errorf("unknown command %q: status, diff, log or show", command)
			}
			if given := argStr(args, "path"); given != "" {
				p, err := ResolveInRoot(tc.Root, given)
				if err != nil {
					return "", nil, err
				}
				if err := privateGuard(given, p, "read"); err != nil {
					return "", nil, err
				}
				gitArgs = append(gitArgs, "--", p)
			}
			ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
			defer cancel()
			cmd := exec.CommandContext(ctx, "git", append(append(append([]string{}, gitSafe...), "-C", tc.Root), gitArgs...)...)
			cmd.Env = append(childEnv(tc), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0", "GIT_PAGER=cat", "PAGER=cat")
			var out bytes.Buffer
			cmd.Stdout, cmd.Stderr = &out, &out
			err := cmd.Run()
			text := strings.TrimSpace(strings.ToValidUTF8(out.String(), "�"))
			if err != nil {
				if text == "" {
					text = err.Error()
				}
				return "", nil, fmt.Errorf("git %s failed: %s", command, clipText(text, 1000))
			}
			if text == "" {
				text = map[string]string{"diff": "No changes: the working tree matches " + map[bool]string{true: ref, false: "the index"}[ref != ""] + " (new files git does not track yet are listed by status).",
					"log": "No commits yet.", "status": "Clean."}[command]
			}
			return clipText(text, 12000), map[string]any{"action": "git_read", "command": command}, nil
		},
	}
}
