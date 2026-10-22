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
	info := run(ctx, step{argv: []string{"coredumpctl", "info", "--no-pager", "-1", exe}, lines: 80})
	where := resolveFrames(ctx, info)
	// the summary comes first: a small model reads the top of a long report and skims the rest
	b.Reset()
	b.WriteString("## Summary\n" + crashSummary(exe, info, where) + "\n\n## The crash record (selected lines)\n" + recordLines(info))
	if where != "" {
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

func field(info, name string) string {
	for _, l := range strings.Split(info, "\n") {
		t := strings.TrimSpace(l)
		if strings.HasPrefix(t, name+":") {
			return strings.TrimSpace(strings.TrimPrefix(t, name+":"))
		}
	}
	return ""
}

func recordLines(info string) string {
	var out []string
	for _, k := range []string{"Timestamp", "Signal", "Command Line", "Executable", "Package", "Storage"} {
		if v := field(info, k); v != "" {
			out = append(out, k+": "+v)
		}
	}
	if len(out) == 0 {
		return info
	}
	return strings.Join(out, "\n")
}

// crashSummary says in one or two sentences what crashed, how, when and where (when the frames could be named).
func crashSummary(exe, info, where string) string {
	sig, when := field(info, "Signal"), field(info, "Timestamp")
	if sig == "" {
		return "No crash of " + filepath.Base(exe) + " is recorded."
	}
	s := fmt.Sprintf("%s crashed with signal %s", filepath.Base(exe), sig)
	if when != "" {
		s += " at " + when
	}
	s += "."
	if where != "" {
		lines := strings.Split(where, "\n")
		first := strings.TrimPrefix(lines[0], "#0 ")
		s += " It crashed in " + first
		if len(lines) > 1 {
			s += ", called from " + strings.SplitN(lines[1], " ", 2)[1]
		}
		s += "."
	} else if named := namedFrames(info, 6); len(named) > 0 {
		s += " Its stack, innermost first: " + strings.Join(named, " < ") + "."
	} else {
		s += " The program has no symbols, so where in it the crash happened cannot be named."
	}
	if cmd := field(info, "Command Line"); len(strings.Fields(cmd)) > 1 {
		s += " It was running: " + cmd + "."
	}
	return s
}

var anyFrameRE = regexp.MustCompile(`#\d+\s+0x[0-9a-f]+\s+(\S+)\s+\(`)

// namedFrames lists the function names the record has (the libraries' exported ones), from the first stack, skipping the unnamed ones.
func namedFrames(info string, n int) []string {
	first := info
	if i := strings.Index(info, "Stack trace of thread"); i >= 0 {
		first = info[i:]
		if j := strings.Index(first[1:], "Stack trace of thread"); j >= 0 {
			first = first[:j+1]
		}
	}
	var out []string
	for _, m := range anyFrameRE.FindAllStringSubmatch(first, -1) {
		if m[1] != "n/a" {
			out = append(out, m[1])
		}
		if len(out) == n {
			break
		}
	}
	return out
}
