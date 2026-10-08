package tiers

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// WakeFunc powers a tier on. It is only ever called for a tier whose config has
// a wake entry, and only when that tier is unreachable.
type WakeFunc func(ctx context.Context, t Tier) error

// NewWake builds the wake runner under the existing command permission model.
// A wake entry that is an http(s) URL is requested with GET (the operator
// configured that exact URL). Anything else is a shell command, which runs only
// if shell commands are allowed and, unless destructive commands are allowed,
// it passes the same destructive-command check the bash tool applies. The
// child gets the parent environment minus secret-looking variables, so a wake
// script can never see an API key.
func NewWake(perms config.Permissions, root string, httpc *http.Client) WakeFunc {
	return func(ctx context.Context, t Tier) error {
		w := strings.TrimSpace(t.Wake)
		if w == "" {
			return fmt.Errorf("no wake entry configured for tier %q", t.Name)
		}
		cctx, cancel := context.WithTimeout(ctx, 20*time.Second)
		defer cancel()
		if strings.HasPrefix(w, "http://") || strings.HasPrefix(w, "https://") {
			req, err := http.NewRequestWithContext(cctx, http.MethodGet, w, nil)
			if err != nil {
				return fmt.Errorf("bad wake URL: %v", err)
			}
			if httpc == nil {
				httpc = http.DefaultClient
			}
			resp, err := httpc.Do(req)
			if err != nil {
				return fmt.Errorf("wake request failed")
			}
			resp.Body.Close()
			if resp.StatusCode >= 400 {
				return fmt.Errorf("wake request returned HTTP %d", resp.StatusCode)
			}
			return nil
		}
		if !perms.AllowShellCommands {
			return fmt.Errorf("wake command not run: shell commands are disabled (the operator can re-run with --allow-shell)")
		}
		if !perms.AllowDestructiveShellCommands {
			if v := tools.CheckDestructive(w, root); v.Blocked {
				return fmt.Errorf("wake command not run: %s", v.Reason)
			}
		}
		var cmd *exec.Cmd
		if sh, err := exec.LookPath("sh"); err == nil {
			cmd = exec.CommandContext(cctx, sh, "-c", w)
		} else if runtime.GOOS == "windows" {
			cmd = exec.CommandContext(cctx, "cmd", "/C", w)
		} else {
			return fmt.Errorf("wake command not run: no shell found")
		}
		cmd.Env = filteredEnv(os.Environ())
		if err := cmd.Run(); err != nil {
			return fmt.Errorf("wake command failed: %v", err)
		}
		return nil
	}
}

var secretNameParts = []string{"SECRET", "TOKEN", "PASSWORD", "PRIVATE_KEY", "API_KEY", "CREDENTIAL", "AUTH"}

func filteredEnv(env []string) []string {
	var out []string
	for _, kv := range env {
		name, _, _ := strings.Cut(kv, "=")
		up := strings.ToUpper(name)
		drop := false
		for _, p := range secretNameParts {
			if strings.Contains(up, p) {
				drop = true
				break
			}
		}
		if !drop {
			out = append(out, kv)
		}
	}
	return out
}
