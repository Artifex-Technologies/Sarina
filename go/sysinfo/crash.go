package sysinfo

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

var exeRE = regexp.MustCompile(`^[A-Za-z0-9._+/-]{1,120}$`)

// CrashDetail describes the latest crash of a program: what coredumpctl recorded (signal, time, the stack the system could read), which package
// owns the program, and the log lines around the crash. Read-only; the name is checked, passed as one argument, and never goes through a shell.
func CrashDetail(ctx context.Context, exe string) (string, error) {
	exe = strings.TrimSpace(exe)
	if !exeRE.MatchString(exe) || strings.Contains(exe, "..") || strings.HasPrefix(exe, "-") {
		return "", fmt.Errorf("%q is not a program name or path", exe)
	}
	var b strings.Builder
	b.WriteString("## Latest crash\n")
	info := run(ctx, step{argv: []string{"coredumpctl", "info", "--no-pager", "-1", exe}, lines: 60})
	b.WriteString(info)
	if where := resolveFrames(ctx, info); where != "" {
		b.WriteString("\n\n## Where it crashed (frames named from the program file; it was not run)\n")
		b.WriteString(where)
	}
	if filepath.IsAbs(exe) {
		if _, err := os.Stat(exe); err == nil {
			b.WriteString("\n\n## Package that owns it\n")
			b.WriteString(run(ctx, step{argv: []string{"pacman", "-Qo", exe}}))
		}
	} else if p, err := exec.LookPath(exe); err == nil {
		b.WriteString("\n\n## Package that owns it\n")
		b.WriteString(run(ctx, step{argv: []string{"pacman", "-Qo", p}}))
	}
	b.WriteString("\n\n## Log lines mentioning it this boot\n")
	b.WriteString(run(ctx, step{argv: []string{"journalctl", "-b", "--no-pager", "-n", "30", "-g", regexp.QuoteMeta(filepath.Base(exe)), "-o", "short"}}))
	b.WriteString("\n\n[Evidence only: what the system recorded. A stack with no symbols means the package has no debug info; say so, and say what you cannot tell. Suggest a cause only as a guess and say how to check it.]")
	return b.String(), nil
}

func crashTool() tools.Tool {
	return tools.Tool{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "crash_detail",
			Description: "Details of the latest crash of one program (name or path): signal, time, stack, owning package, related log lines. Read-only.",
			Parameters: map[string]any{"type": "object", "required": []string{"program"}, "properties": map[string]any{
				"program": map[string]any{"type": "string"},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			p, _ := a["program"].(string)
			text, err := CrashDetail(ctx, p)
			if err != nil {
				return "", nil, err
			}
			return text, map[string]any{"action": "crash_detail", "program": p}, nil
		},
	}
}

var frameRE = regexp.MustCompile(`#(\d+)\s+0x[0-9a-f]+\s+(\S+)\s+\((/[^ )]+) \+ (0x[0-9a-f]+)\)`)

// resolveFrames names the stack frames the crash record left as "n/a", reading the program file with addr2line (function, and source file and
// line when the program has debug information). Only frames in files that exist are looked up, at most six.
func resolveFrames(ctx context.Context, info string) string {
	var out []string
	for _, m := range frameRE.FindAllStringSubmatch(info, -1) {
		if len(out) == 6 {
			break
		}
		num, name, path, off := m[1], m[2], m[3], m[4]
		if fi, err := os.Stat(path); err != nil || !fi.Mode().IsRegular() {
			continue
		}
		res := run(ctx, step{argv: []string{"addr2line", "-f", "-C", "-e", path, off}, lines: 2})
		lines := strings.Split(strings.TrimSpace(res), "\n")
		fn, loc := name, ""
		if len(lines) >= 1 && lines[0] != "??" && !strings.HasPrefix(lines[0], "(") {
			fn = lines[0]
		}
		if len(lines) >= 2 && !strings.HasPrefix(lines[1], "??") {
			loc = " at " + lines[1]
		}
		if fn == "n/a" {
			continue
		}
		out = append(out, fmt.Sprintf("#%s %s in %s%s", num, fn, filepath.Base(path), loc))
	}
	return strings.Join(out, "\n")
}
