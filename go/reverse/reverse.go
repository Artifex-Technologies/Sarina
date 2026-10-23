// Package reverse gives Sarina a reverse-engineering tool: REA (https://github.com/morluto/rea, MIT) with Ghidra, run by labyrinth-rea inside a sandbox
// (bubblewrap: no network, no home, read-only system, files read-only). She can read what a program or driver does; she cannot run it.
//
// Only static, read-only REA commands are offered. Files must be in the analysis folder (~/Documents/Samples) or the malware scan's quarantine, where
// the sandbox sees them read-only: a path anywhere else is refused, so she cannot be talked into reading your other files through it.
package reverse

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Commands are the REA commands she may use (all static): what each one answers.
var Commands = map[string]string{
	"functions":                "list function names (from the symbol table; a stripped program has none)",
	"analyze":                  "overview: format, architecture, entry points, imports, notable strings",
	"decompile":                "pseudocode of a function (give its name or address in target)",
	"function":                 "details of one function (target)",
	"xrefs":                    "who calls or references an address or name (target)",
	"search":                   "search strings and symbols (target = the text)",
	"search-bytes":             "search a byte pattern (target = hex)",
	"inspect-binary-layout":    "sections, segments, headers",
	"inspect-with-objdump":     "objdump disassembly of the file (or a target range)",
	"inspect-signature":        "code signature and certificates",
	"inspect-go-binary":        "Go build info and package list",
	"inspect-rust-binary":      "Rust crates and metadata",
	"inspect-pe-resources":     "Windows resources (manifest, version, icons)",
	"inspect-firmware-regions": "regions of a firmware image",
	"network-map":              "web addresses, host names and IP addresses inside the program, with where each IP belongs (offline)",
}

// SamplesDir is where files to analyse are put (the sandbox sees it as /samples).
func SamplesDir() string {
	if d := os.Getenv("LABYRINTH_REA_SAMPLES"); d != "" {
		return d
	}
	h, _ := os.UserHomeDir()
	return filepath.Join(h, "Documents", "Samples")
}

// QuarantineDir is the malware scan's quarantine (the sandbox sees it as /quarantine).
func QuarantineDir() string {
	h, _ := os.UserHomeDir()
	return filepath.Join(h, ".local", "share", "labyrinth", "quarantine")
}

// SandboxPath maps a path on the computer to the path the sandbox sees, or says why it is refused.
func SandboxPath(p string) (string, error) {
	abs, err := filepath.Abs(p)
	if err != nil {
		return "", err
	}
	if r, err := filepath.EvalSymlinks(abs); err == nil {
		abs = r
	}
	for _, m := range []struct{ host, inside string }{{SamplesDir(), "/samples"}, {QuarantineDir(), "/quarantine"}} {
		host := m.host
		if r, err := filepath.EvalSymlinks(host); err == nil {
			host = r
		}
		if rel, err := filepath.Rel(host, abs); err == nil && rel != ".." && !strings.HasPrefix(rel, "../") {
			return filepath.ToSlash(filepath.Join(m.inside, rel)), nil
		}
	}
	return "", fmt.Errorf("%s is not in the analysis folder: only files in %s (or the quarantine) can be examined; ask the owner to put a copy there", p, SamplesDir())
}

var targetRE = regexp.MustCompile(`^[A-Za-z0-9_.:+$@/\- ]{1,200}$`)

// Args builds the labyrinth-rea arguments for one call, or says what is wrong.
func Args(command, path, target string, maxTokens int) ([]string, error) {
	if _, ok := Commands[command]; !ok {
		return nil, fmt.Errorf("unknown command %q (offered: %s)", command, strings.Join(names(), ", "))
	}
	inside, err := SandboxPath(path)
	if err != nil {
		return nil, err
	}
	if command == "functions" {
		return []string{"--exec", "nm", "-C", "--defined-only", "-n", inside}, nil // not a REA command: the symbol table, read inside the sandbox
	}
	if command == "network-map" { // not a REA command: the program's strings (8-bit and UTF-16), read inside the sandbox
		return []string{"--exec", "sh", "-c", `strings -a -n 6 "$1"; strings -a -n 6 -e l "$1"`, "strings", inside}, nil
	}
	args := []string{command, inside}
	if target != "" {
		if strings.HasPrefix(target, "-") || !targetRE.MatchString(target) {
			return nil, fmt.Errorf("target %q is not a plain function name, address or text", target)
		}
		args = append(args, target)
	}
	if maxTokens <= 0 || maxTokens > 6000 {
		maxTokens = 3000
	}
	return append(args, "--format", "md", "--token-limit", fmt.Sprint(maxTokens)), nil
}

func names() []string {
	var out []string
	for k := range Commands {
		out = append(out, k)
	}
	sortStrings(out)
	return out
}

func sortStrings(s []string) {
	for i := 1; i < len(s); i++ {
		for j := i; j > 0 && s[j] < s[j-1]; j-- {
			s[j], s[j-1] = s[j-1], s[j]
		}
	}
}

func init() { tools.RegisterProvider(Tools) }

// Tools returns the tool when SARINA_REVERSE=on and labyrinth-rea is installed (off by default: the tool's description costs every request some of the model's context).
func Tools() []tools.Tool {
	if os.Getenv("SARINA_REVERSE") != "on" {
		return nil
	}
	bin := os.Getenv("LABYRINTH_REA")
	if bin == "" {
		p, err := exec.LookPath("labyrinth-rea")
		if err != nil {
			return nil
		}
		bin = p
	}
	return []tools.Tool{stageTool(), {
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "reverse_engineer",
			Description: "Read what a program, driver or firmware does (static analysis in a sandbox; never run). File in ~/Documents/Samples. analyze is only an overview: decompile the function to explain logic. Commands: " + strings.Join(names(), ", ") + ".",
			Parameters: map[string]any{"type": "object", "required": []string{"command", "path"}, "properties": map[string]any{
				"command": map[string]any{"type": "string", "description": "analyze, then functions, then decompile."},
				"path":    map[string]any{"type": "string", "description": "The file."},
				"target":  map[string]any{"type": "string", "description": "Function name or address, or the text to search for."},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			str := func(k string) string { v, _ := a[k].(string); return v }
			args, err := Args(str("command"), str("path"), str("target"), 0)
			if err != nil {
				return "", nil, err
			}
			ctx, cancel := context.WithTimeout(ctx, 6*time.Minute)
			defer cancel()
			cmd := exec.CommandContext(ctx, bin, args...)
			var out, errb bytes.Buffer
			cmd.Stdout, cmd.Stderr = &out, &errb
			runErr := cmd.Run()
			text := strings.TrimSpace(out.String())
			if str("command") == "network-map" && (runErr == nil || text != "") {
				text = FindAddresses(text).Report(ctx, filepath.Base(str("path")))
			}
			if r := []rune(text); len(r) > 14000 {
				text = string(r[:14000]) + "\n[cut]"
			}
			if runErr != nil && text == "" {
				return "", nil, fmt.Errorf("the analysis did not run: %v: %s", runErr, strings.TrimSpace(errb.String()))
			}
			if str("command") == "decompile" {
				if calls := calledFunctions(text, str("target")); len(calls) > 0 {
					text += "\n\nThis code calls functions of the same program: " + strings.Join(calls, ", ") + ". If the question depends on what they do (a check, a rule), decompile them too before you answer; do not guess their logic."
				}
			}
			text += "\n\n[Evidence only: say nothing about the program that is not in the output above. An overview does not show the logic: to explain a check, decompile the function that does it and quote its code. If you have not seen it, say you have not verified it.]"
			return text, map[string]any{"action": "reverse_engineer", "command": str("command"), "path": str("path")}, nil
		},
	}}
}

func stageTool() tools.Tool {
	return tools.Tool{
		Requires: tools.PermWrite, Class: tools.ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "stage_binary",
			Description: "Copy an installed program or driver (under /usr, /opt, your program folders) into the analysis folder, read-only, so reverse_engineer can read it. The original is not touched or run. Returns the copy's path.",
			Parameters: map[string]any{"type": "object", "required": []string{"path"}, "properties": map[string]any{
				"path": map[string]any{"type": "string", "description": "Full path of the installed program."},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			p, _ := a["path"].(string)
			dst, err := Stage(p)
			if err != nil {
				return "", nil, err
			}
			return "Copied to " + dst + " (read-only). Now use reverse_engineer on that path.", map[string]any{"action": "stage_binary", "path": p, "copy": dst}, nil
		},
	}
}

// libcNames are calls that are not the program's own logic.
var libcNames = map[string]bool{"printf": true, "puts": true, "putchar": true, "fprintf": true, "sprintf": true, "snprintf": true, "strcmp": true, "strncmp": true,
	"strlen": true, "strcpy": true, "strncpy": true, "strcat": true, "memcpy": true, "memset": true, "memcmp": true, "malloc": true, "calloc": true, "realloc": true,
	"free": true, "exit": true, "abort": true, "atoi": true, "strtol": true, "fopen": true, "fclose": true, "fread": true, "fwrite": true, "fgets": true, "scanf": true,
	"sscanf": true, "getenv": true, "return": true, "if": true, "while": true, "for": true, "switch": true, "sizeof": true}

var callRE = regexp.MustCompile(`\b([A-Za-z_][A-Za-z0-9_]{2,})\s*\(`)

// calledFunctions lists the program's own functions a piece of decompiled code calls (best effort: names that are not C keywords or common libc).
func calledFunctions(code, self string) []string {
	seen := map[string]bool{}
	var out []string
	for _, m := range callRE.FindAllStringSubmatch(code, -1) {
		n := m[1]
		if libcNames[n] || n == self || seen[n] || strings.HasPrefix(n, "__") {
			continue
		}
		if strings.HasPrefix(n, "_") || strings.ToUpper(n) == n {
			continue
		}
		seen[n] = true
		out = append(out, n)
		if len(out) == 6 {
			break
		}
	}
	return out
}
