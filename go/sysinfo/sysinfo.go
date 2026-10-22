// Package sysinfo gives Sarina one read-only tool, system_status, so a question like "why is the computer slow" or "is it the Wi-Fi" is answered
// from what the computer says, not guessed. Each topic runs a fixed, harmless command (no shell, no arguments from the model, a short timeout);
// nothing is changed and no file contents are read.
package sysinfo

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

type step struct {
	title string
	argv  []string
	lines int // keep this many lines (0: all)
}

// Topics are what she may ask about: each one a few fixed commands.
var Topics = map[string][]step{
	"performance": {
		{"Uptime and load", []string{"uptime"}, 0},
		{"Memory", []string{"free", "-h"}, 0},
		{"Busiest programs by processor", []string{"ps", "-eo", "pid,comm,pcpu,pmem", "--sort=-pcpu"}, 9},
		{"Biggest programs by memory", []string{"ps", "-eo", "pid,comm,pcpu,pmem", "--sort=-pmem"}, 9},
		{"Pressure (share of time programs waited)", []string{"cat", "/proc/pressure/cpu", "/proc/pressure/memory", "/proc/pressure/io"}, 0},
	},
	"network": {
		{"Connection state", []string{"nmcli", "-t", "-f", "STATE,CONNECTIVITY", "general", "status"}, 0},
		{"Devices", []string{"nmcli", "-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device", "status"}, 0},
		{"Addresses", []string{"ip", "-brief", "address"}, 0},
		{"Routes", []string{"ip", "route"}, 6},
		{"Name server answers", []string{"getent", "hosts", "archlinux.org"}, 2},
	},
	"disk": {
		{"Space", []string{"df", "-h", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs"}, 0},
		{"Drives", []string{"lsblk", "-o", "NAME,SIZE,TYPE,MOUNTPOINTS"}, 0},
	},
	"logs": {
		{"Warnings and errors since boot (newest last)", []string{"journalctl", "-b", "-p", "warning", "-n", "40", "--no-pager", "-o", "short"}, 0},
	},
	"services": {
		{"Services that failed", []string{"systemctl", "--failed", "--no-pager", "--plain"}, 0},
		{"User services that failed", []string{"systemctl", "--user", "--failed", "--no-pager", "--plain"}, 0},
	},
	"crashes": {
		{"Crashes in the last 7 days, newest first (core dumps)", []string{"coredumpctl", "list", "--no-pager", "--reverse", "--since=-7d", "-n", "12"}, 0},
		{"Crash and out-of-memory lines this boot", []string{"journalctl", "-b", "--no-pager", "-n", "25", "-g", "segfault|core dumped|dumped core|out of memory|oom-kill|Killed process|general protection", "-o", "short"}, 0},
	},
	"boot": {
		{"Time to start", []string{"systemd-analyze", "--no-pager"}, 0},
		{"When the login screen appeared (what it waited for)", []string{"systemd-analyze", "critical-chain", "display-manager.service", "--no-pager"}, 12},
		{"When everything was started (the graphical target; background services can make this later than the login screen)", []string{"systemd-analyze", "critical-chain", "--no-pager"}, 14},
		{"Slowest units overall (includes background jobs that do not delay the start)", []string{"systemd-analyze", "blame", "--no-pager"}, 10},
	},
	"audio": {
		{"Sound devices and apps", []string{"wpctl", "status"}, 28},
	},
	"kernel": {
		{"Kernel warnings and errors this boot", []string{"journalctl", "-k", "-b", "-p", "warning", "-n", "30", "--no-pager", "-o", "short"}, 0},
	},
	"hardware": {
		{"Temperatures and fans", []string{"sensors"}, 30},
		{"Graphics card", []string{"nvidia-smi", "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu", "--format=csv,noheader"}, 0},
		{"Battery and power", []string{"cat", "/sys/class/power_supply/BAT0/capacity"}, 0},
	},
}

func names() []string {
	var out []string
	for k := range Topics {
		out = append(out, k)
	}
	for i := 1; i < len(out); i++ {
		for j := i; j > 0 && out[j] < out[j-1]; j-- {
			out[j], out[j-1] = out[j-1], out[j]
		}
	}
	return out
}

func run(ctx context.Context, s step) string {
	ctx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, s.argv[0], s.argv[1:]...)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	err := cmd.Run()
	text := strings.TrimSpace(out.String())
	if text == "" {
		if err != nil {
			return "(not available here)"
		}
		return "(nothing to report)"
	}
	lines := strings.Split(text, "\n")
	if s.lines > 0 && len(lines) > s.lines {
		lines = lines[:s.lines]
	}
	text = strings.Join(lines, "\n")
	if r := []rune(text); len(r) > 3500 {
		text = string(r[:3500]) + "\n[cut]"
	}
	return text
}

// Report runs the commands of one topic and formats what they printed.
func Report(ctx context.Context, topic string) (string, error) {
	steps, ok := Topics[strings.ToLower(strings.TrimSpace(topic))]
	if !ok {
		return "", fmt.Errorf("no such topic %q: use one of %s", topic, strings.Join(names(), ", "))
	}
	var b strings.Builder
	// the time the report was made, so "today" and "recently" are judged against the dates in it, not guessed
	fmt.Fprintf(&b, "Now: %s\n\n", time.Now().Format("Mon 2006-01-02 15:04"))
	if strings.EqualFold(strings.TrimSpace(topic), "boot") {
		if sum := bootSummary(ctx); sum != "" {
			b.WriteString("## Summary\n" + sum + "\n\n")
		}
	}
	for _, s := range steps {
		fmt.Fprintf(&b, "## %s\n%s\n\n", s.title, run(ctx, s))
	}
	b.WriteString("[Evidence only: these are the numbers the computer gave just now. Say what they show, in plain words; if something looks fine, say it looks fine; if a part is not available, say you could not see it.]")
	return b.String(), nil
}

func init() { tools.RegisterProvider(Tools) }

// Tools returns system_status (read-only) when SARINA_SYSTEM=on (off by default: the tool's description costs every request some of the model's context).
func Tools() []tools.Tool {
	if os.Getenv("SARINA_SYSTEM") != "on" {
		return nil
	}
	return []tools.Tool{crashTool(), {
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "system_status",
			Description: "How the computer is doing now (read-only): performance, network, disk, logs, services, hardware, crashes, boot, audio, kernel. Call it before answering about slowness, internet, errors, crashes or health.",
			Parameters: map[string]any{"type": "object", "required": []string{"topic"}, "properties": map[string]any{
				"topic": map[string]any{"type": "string", "enum": names()},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			topic, _ := a["topic"].(string)
			text, err := Report(ctx, topic)
			if err != nil {
				return "", nil, err
			}
			return text, map[string]any{"action": "system_status", "topic": topic}, nil
		},
	}}
}

var atRE = regexp.MustCompile(`^(\S+)\s+@([0-9.]+(?:min [0-9.]+)?m?s)`)

// bootSummary says when the login screen appeared, when everything was started, and which of the slowest units are not in the login
// screen's chain (they ran in the background and did not delay it): a model reads the top of a report and skims the rest.
func bootSummary(ctx context.Context) string {
	dm := run(ctx, step{argv: []string{"systemd-analyze", "critical-chain", "display-manager.service", "--no-pager"}})
	all := run(ctx, step{argv: []string{"systemd-analyze", "critical-chain", "--no-pager"}})
	first := func(chain string) (string, string) {
		for _, l := range strings.Split(chain, "\n") {
			if m := atRE.FindStringSubmatch(strings.TrimSpace(l)); m != nil {
				return m[1], m[2]
			}
		}
		return "", ""
	}
	dmUnit, dmAt := first(dm)
	_, allAt := first(all)
	if dmAt == "" && allAt == "" {
		return ""
	}
	var parts []string
	if dmAt != "" {
		parts = append(parts, fmt.Sprintf("The login screen (%s) appeared %s after the start.", dmUnit, dmAt))
	}
	if allAt != "" {
		parts = append(parts, fmt.Sprintf("Everything was started after %s.", allAt))
	}
	var background []string
	for _, l := range strings.Split(run(ctx, step{argv: []string{"systemd-analyze", "blame", "--no-pager"}, lines: 6}), "\n") {
		f := strings.Fields(l)
		if len(f) < 2 {
			continue
		}
		unit := f[len(f)-1]
		if !strings.Contains(dm, unit) {
			background = append(background, unit)
		}
	}
	if len(background) > 0 {
		parts = append(parts, "These slow units are not in the login screen's chain, so they did not delay it (they run in the background or later): "+strings.Join(background, ", ")+".")
	}
	return strings.Join(parts, " ")
}
