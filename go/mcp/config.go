// Package mcp is a Model Context Protocol client for stdio servers.
//
// Trust gate: a server definition is a command to run, so where it comes from
// decides everything. Definitions are read only from operator-controlled files
// (~/.sarina/mcp.json and the files named by SARINA_MCP_FILE). A definition
// file in the workspace is ignored, with a visible notice, unless the operator
// passed --trust-workspace-mcp. Parent directories are never consulted.
package mcp

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// EnvFile lists extra operator MCP definition files (separated like PATH).
const EnvFile = "SARINA_MCP_FILE"

// WorkspaceFiles are the names recognised inside the workspace directory, and
// only there, only with the trust flag.
var WorkspaceFiles = []string{".sarina-mcp.json", ".mcp.json", "mcp.json"}

const maxConfigBytes = 256 << 10

// Source labels.
const (
	SourceOperator  = "operator"
	SourceWorkspace = "workspace"
)

// ServerDef is one configured stdio server.
type ServerDef struct {
	Name           string
	Command        string
	Args           []string
	Env            map[string]string // operator-chosen overlay, applied after the filtered parent env
	Cwd            string            // resolved; "" = the process's directory
	Description    string
	ReadOnly       bool            // every tool of this server is read-only
	ReadOnlyTools  map[string]bool // individual tools marked read-only
	TimeoutSeconds float64         // per tool call; 0 = default
	Source         string          // SourceOperator or SourceWorkspace
	File           string          // the definition file
}

// LoadOptions says where definitions may come from.
type LoadOptions struct {
	Home           string              // for ~/.sarina/mcp.json
	Getenv         func(string) string // default os.Getenv
	Workspace      string              // the workspace directory
	TrustWorkspace bool                // --trust-workspace-mcp
}

// Config is the discovery result. Problems never abort loading.
type Config struct {
	Servers []ServerDef
	// Notices are lines the operator must see (workspace file ignored or in use).
	Notices  []string
	Problems []string
	// Files lists every file read.
	Files []string
}

// Load discovers server definitions under the trust gate.
func Load(o LoadOptions) *Config {
	getenv := o.Getenv
	if getenv == nil {
		getenv = os.Getenv
	}
	c := &Config{}
	seenFile := map[string]bool{}
	byName := map[string]string{}
	read := func(path, source string) {
		abs, err := filepath.Abs(path)
		if err != nil {
			abs = path
		}
		if seenFile[abs] {
			return
		}
		seenFile[abs] = true
		c.loadFile(abs, source, byName)
	}
	if o.Home != "" {
		read(filepath.Join(o.Home, ".sarina", "mcp.json"), SourceOperator)
	}
	for _, f := range filepath.SplitList(getenv(EnvFile)) {
		if f != "" {
			read(f, SourceOperator)
		}
	}
	// Workspace: this directory only, never an ancestor.
	if o.Workspace != "" {
		for _, name := range WorkspaceFiles {
			p := filepath.Join(o.Workspace, name)
			st, err := os.Lstat(p)
			if err != nil || st.IsDir() {
				continue
			}
			if !o.TrustWorkspace {
				c.Notices = append(c.Notices, fmt.Sprintf(
					"ignored MCP definition file %s: a workspace file can start programs, so it is only used with --trust-workspace-mcp", p))
				continue
			}
			c.Notices = append(c.Notices, fmt.Sprintf(
				"workspace MCP definitions are enabled (--trust-workspace-mcp): %s may start programs", p))
			read(p, SourceWorkspace)
		}
	}
	return c
}

func (c *Config) loadFile(path, source string, byName map[string]string) {
	f, err := os.Open(path)
	if err != nil {
		if !os.IsNotExist(err) {
			c.Problems = append(c.Problems, fmt.Sprintf("%s: %v", path, err))
		}
		return
	}
	defer f.Close()
	raw, err := io.ReadAll(io.LimitReader(f, maxConfigBytes+1))
	if err != nil || len(raw) > maxConfigBytes {
		c.Problems = append(c.Problems, fmt.Sprintf("%s: unreadable or larger than %d bytes", path, maxConfigBytes))
		return
	}
	var top map[string]any
	if err := json.Unmarshal(raw, &top); err != nil {
		c.Problems = append(c.Problems, fmt.Sprintf("%s: not a JSON object: %v", path, err))
		return
	}
	c.Files = append(c.Files, path)
	dir := filepath.Dir(path)
	var raws []map[string]any
	if m, ok := top["mcpServers"].(map[string]any); ok {
		names := make([]string, 0, len(m))
		for n := range m {
			names = append(names, n)
		}
		sort.Strings(names)
		for _, n := range names {
			if sm, ok := m[n].(map[string]any); ok {
				sm["name"] = n
				raws = append(raws, sm)
			}
		}
	}
	if l, ok := top["servers"].([]any); ok {
		for _, it := range l {
			if sm, ok := it.(map[string]any); ok {
				raws = append(raws, sm)
			}
		}
	}
	for _, r := range raws {
		def, why := parseServer(r, dir)
		if why != "" {
			c.Problems = append(c.Problems, fmt.Sprintf("%s: %s", path, why))
			continue
		}
		def.Source, def.File = source, path
		key := strings.ToLower(def.Name)
		if first, dup := byName[key]; dup {
			c.Problems = append(c.Problems, fmt.Sprintf("%s: server %q is already defined in %s; this one is ignored", path, def.Name, first))
			continue
		}
		byName[key] = path
		c.Servers = append(c.Servers, def)
	}
}

func parseServer(m map[string]any, baseDir string) (ServerDef, string) {
	var d ServerDef
	d.Name, _ = m["name"].(string)
	d.Name = strings.TrimSpace(d.Name)
	if d.Name == "" {
		return d, "a server has no name"
	}
	if t, ok := m["transport"].(string); ok && t != "" && !strings.EqualFold(t, "stdio") {
		return d, fmt.Sprintf("server %q uses transport %q; only stdio is supported", d.Name, t)
	}
	d.Command, _ = m["command"].(string)
	if strings.TrimSpace(d.Command) == "" {
		return d, fmt.Sprintf("server %q has no command", d.Name)
	}
	if args, ok := m["args"].([]any); ok {
		for _, a := range args {
			if s, ok := a.(string); ok {
				d.Args = append(d.Args, s)
			}
		}
	}
	if env, ok := m["env"].(map[string]any); ok {
		d.Env = map[string]string{}
		for k, v := range env {
			if s, ok := v.(string); ok && k != "" {
				d.Env[k] = s
			}
		}
	}
	if cwd, ok := m["cwd"].(string); ok && cwd != "" {
		if strings.HasPrefix(cwd, "~") {
			if h, err := os.UserHomeDir(); err == nil {
				cwd = filepath.Join(h, strings.TrimPrefix(cwd, "~"))
			}
		}
		if !filepath.IsAbs(cwd) {
			cwd = filepath.Join(baseDir, cwd)
		}
		d.Cwd = cwd
	}
	d.Description, _ = m["description"].(string)
	if b, ok := m["read_only"].(bool); ok {
		d.ReadOnly = b
	}
	d.ReadOnlyTools = map[string]bool{}
	if l, ok := m["read_only_tools"].([]any); ok {
		for _, it := range l {
			if s, ok := it.(string); ok {
				d.ReadOnlyTools[s] = true
			}
		}
	}
	if tm, ok := m["tools"].(map[string]any); ok {
		for name, v := range tm {
			if o, ok := v.(map[string]any); ok {
				if b, ok := o["read_only"].(bool); ok && b {
					d.ReadOnlyTools[name] = true
				}
			}
		}
	}
	if n, ok := m["timeout_seconds"].(float64); ok && n > 0 {
		d.TimeoutSeconds = n
	}
	return d, ""
}
