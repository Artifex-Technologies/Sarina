package fleet

import (
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// fleet-status --json: the same sanitised view as the text, as one JSON object, for a desktop's top bar (Yggdrasil's
// Agents item reads it). Read-only like the rest of fleet-status: it reads the records, two folders' times and the
// process list; nothing here starts, signals or writes anything.

type limitJSON struct {
	Label       string   `json:"label"`
	PercentUsed *float64 `json:"percentUsed"`
	ResetsAt    string   `json:"resetsAt"`
}

type agentJSON struct {
	ID            string      `json:"id"`
	Name          string      `json:"name"`
	Ready         bool        `json:"ready"`
	Stale         bool        `json:"stale"`
	UpdatedAt     string      `json:"updatedAt"`
	TodayTokens   int         `json:"todayTokens"`
	WeekTokens    int         `json:"weekTokens"`
	TotalSessions int         `json:"totalSessions"`
	Limits        []limitJSON `json:"limits"`
	StatusText    string      `json:"statusText"`
}

// SelfStatus is Sarina herself, from what is cheap to look at: when a session was last saved (a session folder's own
// time: a save renames the finished file into it, so no session file is opened) and whether her service is running.
type SelfStatus struct {
	ID         string `json:"id"`
	Name       string `json:"name"`
	Running    bool   `json:"running"`
	LastActive string `json:"lastActive"`
}

type statusDoc struct {
	Agents       []agentJSON `json:"agents"`
	Self         SelfStatus  `json:"self"`
	MostHeadroom *string     `json:"mostHeadroom,omitempty"`
}

// serviceName is argv[0]'s last part of Sarina's service (packaging/arch/sarina-service.service).
const serviceName = "sarina-service"

// ReadSelf looks at Sarina's session folders' times and at procRoot (normally /proc) for her service.
func ReadSelf(sessionDirs []string, procRoot string) SelfStatus {
	self := SelfStatus{ID: AgentID, Name: "Sarina"}
	var last time.Time
	for _, d := range sessionDirs {
		if st, err := os.Stat(d); err == nil && st.IsDir() && st.ModTime().After(last) {
			last = st.ModTime()
		}
	}
	if !last.IsZero() {
		self.LastActive = last.UTC().Format(time.RFC3339)
	}
	self.Running = serviceRunning(procRoot)
	return self
}

// serviceRunning: a process whose first argument ends in serviceName. Only regular files are read (procfs's are), and
// only their first 4 KiB.
func serviceRunning(procRoot string) bool {
	entries, err := os.ReadDir(procRoot)
	if err != nil {
		return false
	}
	for _, e := range entries {
		if _, err := strconv.Atoi(e.Name()); err != nil {
			continue
		}
		p := filepath.Join(procRoot, e.Name(), "cmdline")
		if st, err := os.Lstat(p); err != nil || !st.Mode().IsRegular() {
			continue
		}
		f, err := os.Open(p)
		if err != nil {
			continue
		}
		raw, _ := io.ReadAll(io.LimitReader(f, 4096))
		f.Close()
		argv0, _, _ := strings.Cut(string(raw), "\x00")
		if argv0 != "" && filepath.Base(argv0) == serviceName {
			return true
		}
	}
	return false
}

// StatusJSON is fleet-status --json: {"agents": [...], "self": {...}} and, with suggest, "mostHeadroom" (an agent's id,
// or "" when none has a current, measured limit).
func StatusJSON(statuses []AgentStatus, self SelfStatus, suggest bool) (string, error) {
	doc := statusDoc{Agents: []agentJSON{}, Self: self}
	for _, s := range statuses {
		a := agentJSON{
			ID: s.AgentID, Name: s.Name, Ready: s.Ready, Stale: s.Stale, UpdatedAt: s.UpdatedAt,
			TodayTokens: s.TodayTokens, WeekTokens: s.WeekTokens, TotalSessions: s.TotalSessions,
			Limits: []limitJSON{}, StatusText: s.StatusText,
		}
		for _, l := range s.Limits {
			a.Limits = append(a.Limits, limitJSON{Label: l.Label, PercentUsed: l.PercentUsed, ResetsAt: l.ResetsAt})
		}
		doc.Agents = append(doc.Agents, a)
	}
	if suggest {
		id := ""
		if pick := SuggestAgent(statuses, nil); pick != nil {
			id = pick.AgentID
		}
		doc.MostHeadroom = &id
	}
	return compactJSON(doc)
}
