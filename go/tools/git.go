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
	"-c", "color.ui=false", "-c", "log.showSignature=false", "-c", "gpg.program=/bin/false", "-c", "gpg.ssh.program=/bin/false",
	"-c", "gpg.x509.program=/bin/false", "-c", "core.sshCommand=/bin/false", "-c", "credential.helper=", "--no-pager"}

// maxGitOutput bounds what is kept of git's output (the answer is cut to 12,000 characters after): a huge diff was read
// whole, and so was a repository's config listing (found by the Rust port, 2026-11-12). The rest is read and dropped, so
// git is not left blocked on a full pipe.
const maxGitOutput = 1 << 20

// boundedBuffer keeps the first max bytes written and notes that more came.
type boundedBuffer struct {
	buf  bytes.Buffer
	max  int
	over bool
}

func (b *boundedBuffer) Write(p []byte) (int, error) {
	room := b.max - b.buf.Len()
	if len(p) > room {
		b.over = true
		if room > 0 {
			b.buf.Write(p[:room])
		}
		return len(p), nil
	}
	b.buf.Write(p)
	return len(p), nil
}

// GitSafeArgs are gitSafe, for the session's git snapshot.
func GitSafeArgs() []string { return append([]string{}, gitSafe...) }

// gitRiskyKey are settings in a repository's own .git/config that make git run a program and that -c cannot switch off,
// because their names are not known in advance: a filter driver runs on status and diff for every file its attributes name,
// and an include pulls in more settings (filters among them). Everything else that runs a program on a read (fsmonitor,
// pager, external diff, signature checks, ssh, credential helper, hooks) is switched off by gitSafe.
var gitRiskyKey = regexp.MustCompile(`^(filter\..+\.(clean|smudge|process)|include\..*|includeif\..*)$`)

// GitConfigRisk names the settings in root's own git config that would make git run a program, or "" when there are none.
// The config is only listed (--local does not follow includes, so an include shows up as itself).
func GitConfigRisk(ctx context.Context, root string) string {
	cmd := exec.CommandContext(ctx, "git", append(GitSafeArgs(), "-C", root, "config", "--local", "--name-only", "--list")...)
	cmd.Env = append(childEnv(nil), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
	out := &boundedBuffer{max: maxGitOutput}
	cmd.Stdout = out
	if err := cmd.Run(); err != nil {
		return ""
	}
	if out.over {
		// a listing past the bound is not read whole, so a filter or include in the rest would go unseen
		return fmt.Sprintf("more than %d bytes of settings, not read whole", maxGitOutput)
	}
	var risky []string
	for _, k := range strings.Split(strings.TrimSpace(out.buf.String()), "\n") {
		if k = strings.ToLower(strings.TrimSpace(k)); gitRiskyKey.MatchString(k) {
			risky = append(risky, k)
		}
	}
	return strings.Join(risky, ", ")
}

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
			diffHead := false
			switch command {
			case "status":
				gitArgs = []string{"status", "--short", "--branch"}
			case "diff":
				// against HEAD: staged and unstaged together, which is what "the uncommitted change" means (a new repository has no HEAD)
				gitArgs = []string{"diff", "--no-ext-diff", "--no-textconv", "--stat", "--patch"}
				if ref != "" {
					gitArgs = append(gitArgs, ref)
				}
				// whether there is a HEAD is asked only after the repository's settings are checked (it ran git before, with
				// the whole environment and no time limit; found by the Rust port, 2026-11-12)
				diffHead = ref == ""
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
			var pathArgs []string
			if given := argStr(args, "path"); given != "" {
				p, err := ResolveInRoot(tc.Root, given)
				if err != nil {
					return "", nil, err
				}
				if err := privateGuard(given, p, "read"); err != nil {
					return "", nil, err
				}
				pathArgs = []string{"--", p}
			}
			ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
			defer cancel()
			if risky := GitConfigRisk(ctx, tc.Root); risky != "" {
				return "", nil, fmt.Errorf("this repository's own git settings name programs for git to run (%s): git_read does not run git here; ask the owner to look at .git/config", risky)
			}
			if diffHead {
				rev := exec.CommandContext(ctx, "git", append(append([]string{}, gitSafe...), "-C", tc.Root, "rev-parse", "-q", "--verify", "HEAD")...)
				rev.Env = append(childEnv(tc), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0")
				if rev.Run() == nil {
					ref = "HEAD"
					gitArgs = append(gitArgs, ref)
				}
			}
			gitArgs = append(gitArgs, pathArgs...)
			cmd := exec.CommandContext(ctx, "git", append(append(append([]string{}, gitSafe...), "-C", tc.Root), gitArgs...)...)
			cmd.Env = append(childEnv(tc), "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0", "GIT_PAGER=cat", "PAGER=cat")
			out := &boundedBuffer{max: maxGitOutput}
			cmd.Stdout, cmd.Stderr = out, out
			err := cmd.Run()
			text := strings.TrimSpace(strings.ToValidUTF8(out.buf.String(), "�"))
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
