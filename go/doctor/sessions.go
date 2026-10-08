package doctor

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

const maxSessionFileBytes = 64 << 20

type sessionScan struct {
	dir        string
	statErr    error
	total      int
	parsed     int
	unreadable []string // file names that cannot be parsed as a session
	retired    []string // files that still carry retired tool names
	badShape   []string // valid JSON, but not the shape Sentry reads
	leaks      []string // files holding a non-empty api_key
}

func (r *runner) sessionDir() string {
	if r.o.SessionDir != "" {
		return r.o.SessionDir
	}
	return config.DefaultRuntime(r.o.Cwd).SessionDirectory
}

func scanSessions(dir string) *sessionScan {
	s := &sessionScan{dir: dir}
	st, err := os.Stat(dir)
	if err == nil && !st.IsDir() {
		err = fmt.Errorf("not a directory")
	}
	if err != nil {
		s.statErr = err
		return s
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		s.statErr = err
		return s
	}
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		s.total++
		path := filepath.Join(dir, e.Name())
		info, err := os.Stat(path)
		if err != nil || info.Size() > maxSessionFileBytes {
			s.unreadable = append(s.unreadable, e.Name())
			continue
		}
		raw, err := os.ReadFile(path)
		if err != nil {
			s.unreadable = append(s.unreadable, e.Name())
			continue
		}
		var generic map[string]any
		if json.Unmarshal(raw, &generic) != nil || generic == nil {
			s.unreadable = append(s.unreadable, e.Name())
			continue
		}
		if leaksKey(generic) {
			s.leaks = append(s.leaks, e.Name())
		}
		if !sentryShape(generic) {
			s.badShape = append(s.badShape, e.Name())
		}
		if _, err := session.Parse(raw); err != nil {
			s.unreadable = append(s.unreadable, e.Name())
			continue
		}
		s.parsed++
		if hasRetiredTool(generic) { // read from the raw file: Parse already rewrites them in memory
			s.retired = append(s.retired, e.Name())
		}
	}
	return s
}

// sentryShape: messages is an array of objects, each with a string role.
func sentryShape(m map[string]any) bool {
	msgs, ok := m["messages"].([]any)
	if !ok {
		return false
	}
	for _, x := range msgs {
		o, ok := x.(map[string]any)
		if !ok {
			return false
		}
		if role, ok := o["role"].(string); !ok || role == "" {
			return false
		}
	}
	return true
}

// hasRetiredTool looks for a retired tool name in a call or a result message.
func hasRetiredTool(m map[string]any) bool {
	msgs, _ := m["messages"].([]any)
	for _, x := range msgs {
		o, _ := x.(map[string]any)
		if name, _ := o["name"].(string); session.IsRetiredTool(name) {
			return true
		}
		calls, _ := o["tool_calls"].([]any)
		for _, c := range calls {
			fn, _ := c.(map[string]any)["function"].(map[string]any)
			if name, _ := fn["name"].(string); session.IsRetiredTool(name) {
				return true
			}
		}
	}
	return false
}

// leaksKey finds a non-empty value under a key named api_key / apiKey / api-key.
func leaksKey(v any) bool {
	switch t := v.(type) {
	case map[string]any:
		for k, val := range t {
			n := strings.NewReplacer("_", "", "-", "").Replace(strings.ToLower(k))
			if s, ok := val.(string); ok && n == "apikey" && strings.TrimSpace(s) != "" {
				return true
			}
			if leaksKey(val) {
				return true
			}
		}
	case []any:
		for _, x := range t {
			if leaksKey(x) {
				return true
			}
		}
	}
	return false
}

func nameList(names []string) string {
	sort.Strings(names)
	const show = 5
	if len(names) <= show {
		return strings.Join(names, ", ")
	}
	return fmt.Sprintf("%s and %d more", strings.Join(names[:show], ", "), len(names)-show)
}

func (r *runner) sessions() {
	s := scanSessions(r.sessionDir())
	if s.statErr != nil {
		if os.IsNotExist(s.statErr) {
			r.add("sessions", Warn, "session directory "+s.dir+" does not exist yet",
				"it is created by the first chat; use --session-dir or --cwd if sessions are kept elsewhere")
		} else {
			r.add("sessions", Fail, fmt.Sprintf("session directory %s cannot be read (%v)", s.dir, s.statErr),
				"fix the directory's permissions or pass --session-dir")
		}
		r.add("session_retired_tools", Skip, "no session directory to scan", "")
		r.add("session_contract", Skip, "no session directory to scan", "")
		return
	}
	write := "writable"
	st, remedy := Pass, ""
	if !writableDir(s.dir) {
		write, st, remedy = "NOT writable", Fail, "give this user write access to "+s.dir+"; sessions cannot be saved without it"
	}
	finding := fmt.Sprintf("%s is %s; %d session files, %d parse", s.dir, write, s.total, s.parsed)
	if len(s.unreadable) > 0 {
		finding += fmt.Sprintf(", %d unreadable: %s", len(s.unreadable), nameList(s.unreadable))
		if st == Pass {
			st, remedy = Warn, "inspect or delete the named files (a damaged file is skipped; Sarina rewrites a session on its next turn)"
		}
	}
	r.add("sessions", st, finding, remedy)

	if len(s.retired) > 0 {
		r.add("session_retired_tools", Warn, fmt.Sprintf("%d session file(s) still contain retired tool names: %s", len(s.retired), nameList(s.retired)),
			"run `sarina session-migrate` (use --dry-run first to preview)")
	} else {
		r.add("session_retired_tools", Pass, "no retired tool names in saved sessions", "")
	}

	switch {
	case len(s.leaks) > 0:
		r.add("session_contract", Fail, fmt.Sprintf("an api_key value is stored in %d session file(s): %s (value not shown)", len(s.leaks), nameList(s.leaks)),
			"SECRET LEAK: rotate that key, delete or redact the named files, and report it; Sarina never writes the key, so something else did")
	case len(s.badShape) > 0:
		r.add("session_contract", Warn, fmt.Sprintf("%d session file(s) do not have the shape Chymaera Sentry reads (messages as objects with a role): %s", len(s.badShape), nameList(s.badShape)),
			"Sentry reports these as agent_session_unknown_shape (Chymaera_Sentry docs/INTEGRATION-FAILURES.md); compare with docs/spec/01-agent-core.md section 4.2")
	default:
		r.add("session_contract", Pass, fmt.Sprintf("%d session file(s) match what Sentry reads and hold no api_key", s.parsed), "")
	}
}
