package tools

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// ---- small argument helpers (the schema has already validated types) -------

func argStr(args map[string]any, key string) string {
	s, _ := args[key].(string)
	return s
}

func argInt(args map[string]any, key string, def int) int {
	if n, ok := config.AsInt(args[key]); ok {
		if _, isBool := args[key].(bool); !isBool {
			return n
		}
	}
	return def
}

func argBool(args map[string]any, key string, def bool) bool {
	if b, ok := args[key].(bool); ok {
		return b
	}
	return def
}

func argStrList(args map[string]any, key string) []string {
	var out []string
	if l, ok := args[key].([]any); ok {
		for _, x := range l {
			if s, ok := x.(string); ok {
				out = append(out, s)
			}
		}
	}
	return out
}

func obj(props map[string]any, required ...string) map[string]any {
	o := map[string]any{"type": "object", "properties": props}
	if len(required) > 0 {
		o["required"] = required
	}
	return o
}

func prop(typ, desc string, extra ...any) map[string]any {
	m := map[string]any{"type": typ}
	if desc != "" {
		m["description"] = desc
	}
	for i := 0; i+1 < len(extra); i += 2 {
		m[extra[i].(string)] = extra[i+1]
	}
	return m
}

const maxWholeFileRead = 32 << 20

// authoredBanner is prepended to what read_file and bash return when the
// content involves a file the agent wrote earlier in the session.
const authoredBanner = "[note: this involves a file you wrote or edited earlier in this session. " +
	"What it shows is not an independent measurement; treat results derived from your own files with skepticism.]\n"

// ReadFile reads a text file inside the workspace.
func ReadFile() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "read_file",
			Description: "Read a text file in the workspace, optionally a line range.",
			Parameters: obj(map[string]any{
				"path":       prop("string", "", "minLength", 1),
				"start_line": prop("integer", "", "minimum", 1),
				"end_line":   prop("integer", "", "minimum", 1),
			}, "path"),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			given := argStr(args, "path")
			path, err := ResolveInRoot(tc.Root, given)
			if err != nil {
				return "", nil, err
			}
			if err := secretGuard(given, path, "read"); err != nil {
				return "", nil, err
			}
			st, err := os.Stat(path)
			if err != nil {
				return "", nil, fmt.Errorf("cannot read %s: %v", given, err)
			}
			if !st.Mode().IsRegular() {
				return "", nil, fmt.Errorf("%s is not a regular file", given)
			}
			_, hasStart := args["start_line"]
			_, hasEnd := args["end_line"]
			var out string
			if !hasStart && !hasEnd {
				if st.Size() > maxWholeFileRead {
					return "", nil, fmt.Errorf("%s is %d bytes, too large to read whole; use start_line and end_line", given, st.Size())
				}
				data, err := os.ReadFile(path)
				if err != nil {
					return "", nil, fmt.Errorf("cannot read %s: %v", given, err)
				}
				out = strings.ToValidUTF8(string(data), "�")
			} else {
				start, end := argInt(args, "start_line", 1), argInt(args, "end_line", int(^uint(0)>>1))
				out, err = readLines(path, start, end)
				if err != nil {
					return "", nil, fmt.Errorf("cannot read %s: %v", given, err)
				}
				if out == "" {
					out = "(no lines in the requested range)"
				}
			}
			if stateOf(tc).isAuthored(path) {
				out = authoredBanner + out
			}
			return TruncateOutput(out, tc.MaxOutputChars), nil, nil
		},
	}
}

func readLines(path string, start, end int) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	br := bufio.NewReader(f)
	var b strings.Builder
	for n := 1; n <= end; n++ {
		line, err := br.ReadString('\n')
		if line != "" && n >= start {
			line = strings.TrimSuffix(strings.TrimSuffix(line, "\n"), "\r")
			fmt.Fprintf(&b, "%d: %s\n", n, strings.ToValidUTF8(line, "�"))
		}
		if err != nil {
			if errors.Is(err, io.EOF) {
				break
			}
			return "", err
		}
	}
	return strings.TrimSuffix(b.String(), "\n"), nil
}

// ListDir lists one directory.
func ListDir() Tool {
	return Tool{
		Class: ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "list_dir",
			Description: "List a directory in the workspace.",
			Parameters: obj(map[string]any{
				"path":        prop("string", "Default: workspace root."),
				"max_entries": prop("integer", "", "minimum", 1, "maximum", 5000),
			}),
		},
		Handler: func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error) {
			given := argStr(args, "path")
			if given == "" {
				given = "."
			}
			dir, err := ResolveInRoot(tc.Root, given)
			if err != nil {
				return "", nil, err
			}
			entries, err := os.ReadDir(dir)
			if err != nil {
				return "", nil, fmt.Errorf("cannot list %s: %v", given, err)
			}
			type ent struct {
				name string
				dir  bool
			}
			list := make([]ent, 0, len(entries))
			for _, e := range entries {
				isDir := e.IsDir()
				if e.Type()&os.ModeSymlink != 0 {
					if st, err := os.Stat(dir + string(os.PathSeparator) + e.Name()); err == nil {
						isDir = st.IsDir()
					}
				}
				list = append(list, ent{e.Name(), isDir})
			}
			sort.SliceStable(list, func(i, j int) bool {
				if list[i].dir != list[j].dir {
					return list[i].dir
				}
				return strings.ToLower(list[i].name) < strings.ToLower(list[j].name)
			})
			if len(list) == 0 {
				return "(empty directory)", nil, nil
			}
			limit := argInt(args, "max_entries", 200)
			var b strings.Builder
			for i, e := range list {
				if i >= limit {
					fmt.Fprintf(&b, "[listing truncated at %d entries]\n", limit)
					break
				}
				kind := "file"
				if e.dir {
					kind = "dir"
				}
				fmt.Fprintf(&b, "%s\t%s\n", kind, relPath(tc.Root, dir+string(os.PathSeparator)+e.name))
			}
			return TruncateOutput(strings.TrimRight(b.String(), "\n"), tc.MaxOutputChars), nil, nil
		},
	}
}
