//go:build !windows

package tools

import (
	"os/exec"
	"syscall"
)

// Every shell runs in its own process group so the whole tree can be stopped.
func setProcAttrs(cmd *exec.Cmd) { cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true} }

// softStop asks a process tree to exit (SIGTERM to the group).
func softStop(pid int) {
	if pid > 0 {
		_ = syscall.Kill(-pid, syscall.SIGTERM)
	}
}

// hardKill force-kills a process tree (SIGKILL to the group).
func hardKill(pid int) {
	if pid > 0 {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
	}
}

// reapGroup kills anything a finished foreground command left in its group.
func reapGroup(pid int) { hardKill(pid) }
