//go:build windows

package tiers

import (
	"os/exec"
	"strconv"
)

// prepareTreeKill makes cancellation/timeout kill the whole process tree with
// taskkill /T, not just the direct child.
func prepareTreeKill(cmd *exec.Cmd) {
	cmd.Cancel = func() error {
		if cmd.Process == nil {
			return nil
		}
		if err := exec.Command("taskkill", "/T", "/F", "/PID", strconv.Itoa(cmd.Process.Pid)).Run(); err != nil {
			return cmd.Process.Kill()
		}
		return nil
	}
}
