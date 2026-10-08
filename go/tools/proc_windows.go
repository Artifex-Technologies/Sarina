//go:build windows

package tools

import (
	"os/exec"
	"strconv"
	"syscall"
)

func setProcAttrs(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: syscall.CREATE_NEW_PROCESS_GROUP}
}

// hardKill force-kills a process and its children.
func hardKill(pid int) {
	if pid > 0 {
		_ = exec.Command("taskkill", "/T", "/F", "/PID", strconv.Itoa(pid)).Run()
	}
}

// Windows has no gentle signal for a console tree; stopping is killing.
func softStop(pid int) { hardKill(pid) }

// reapGroup does nothing on Windows: a finished process's id may already be
// reused, and killing by id could hit a stranger.
func reapGroup(pid int) {}
