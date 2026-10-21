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
		{"Recent crashes (core dumps)", []string{"coredumpctl", "list", "--no-pager", "-n", "12"}, 0},
		{"Crash and out-of-memory lines this boot", []string{"journalctl", "-b", "--no-pager", "-n", "25", "-g", "segfault|core dumped|dumped core|out of memory|oom-kill|Killed process|general protection", "-o", "short"}, 0},
	},
	"boot": {
		{"Time to start", []string{"systemd-analyze", "--no-pager"}, 0},
		{"Slowest services at start", []string{"systemd-analyze", "blame", "--no-pager"}, 12},
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
