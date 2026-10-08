//go:build windows

package tools

import (
	"os"
	"os/exec"
	"strconv"
	"strings"
)

// processAliveOS asks tasklist whether the pid still exists.
func processAliveOS(p *os.Process) bool {
	out, err := exec.Command("tasklist", "/FI", "PID eq "+strconv.Itoa(p.Pid), "/NH").Output()
	return err == nil && strings.Contains(string(out), " "+strconv.Itoa(p.Pid)+" ")
}
