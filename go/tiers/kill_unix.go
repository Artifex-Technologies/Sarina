//go:build !windows

package tiers

import (
	"os/exec"
	"syscall"
)

// prepareTreeKill puts the child in its own process group and makes
// cancellation/timeout kill the whole group, not just the direct child.
func prepareTreeKill(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Cancel = func() error {
		if cmd.Process == nil {
			return nil
		}
		return syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
}
