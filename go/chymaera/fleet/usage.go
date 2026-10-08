package fleet

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// Sarina's usage record for Omarchy's agents panel. The record shape is
// Omarchy's own (its Codex collector's), so the keys here are the ones the panel
// reads. Attribution is approximate by design: a session file holds cumulative
// totals, so all of a session's tokens go to the local day the file was last
// written.

const (
	AgentID             = "sarina"
	AgentName           = "Sarina"
	RecordSchemaVersion = 1
	RecentDayCount      = 7

	// A session file larger than this is not one Sarina wrote; skip it.
	MaxSessionBytes = 64 * 1024 * 1024

	// ServiceSessionDir is where the service keeps sessions on an installed system.
	ServiceSessionDir = "/var/lib/sarina/sessions"

	SessionDirsEnv = "SARINA_SESSION_DIRS"
)

// SessionUsage is what one saved session contributes to the record.
type SessionUsage struct {
	SessionID        string
	Day              string
	Model            string
	Prompts          int
	InputTokens      int
	OutputTokens     int
	CacheReadTokens  int
	CacheWriteTokens int
}

// TotalTokens is input + output + both cache counters.
func (s SessionUsage) TotalTokens() int {
	return s.InputTokens + s.OutputTokens + s.CacheReadTokens + s.CacheWriteTokens
}

// nonNegInt reads a non-negative int from untrusted JSON; anything else is zero.
func nonNegInt(v any) int {
	switch n := v.(type) {
	case int:
		return max(n, 0)
	case int64:
		return int(max(n, 0))
	case float64:
		if math.IsNaN(n) || math.IsInf(n, 0) {
			return 0
		}
		if n >= 9.2e18 {
			return math.MaxInt64
		}
		return max(int(n), 0)
	}
	return 0
}

// UsageDirectory is where Omarchy's panel looks for usage records.
func UsageDirectory(getenv func(string) string, home string) string {
	if s := getenv("XDG_STATE_HOME"); s != "" {
		return filepath.Join(s, "omarchy", "agents", "usage")
	}
	return filepath.Join(home, ".local", "state", "omarchy", "agents", "usage")
}

func expandUser(p, home string) string {
	if p == "~" {
		return home
	}
	if strings.HasPrefix(p, "~/") || strings.HasPrefix(p, `~\`) {
		return filepath.Join(home, p[2:])
	}
	return p
}

// DefaultSessionDirectories lists every place Sarina is known to save sessions.
// SARINA_SESSION_DIRS (path-list separated) replaces the defaults outright.
// Directories that do not exist are skipped by the reader.
func DefaultSessionDirectories(getenv func(string) string, cwd, home string) []string {
	if override := getenv(SessionDirsEnv); override != "" {
		var out []string
		for _, part := range filepath.SplitList(override) {
			if part != "" {
				out = append(out, expandUser(part, home))
			}
		}
		return out
	}
	var out []string
	for _, c := range []string{
		filepath.Join(cwd, ".port_sessions", "agent"),
		filepath.Join(home, ".port_sessions", "agent"),
		ServiceSessionDir,
	} {
		dup := false
		for _, s := range out {
			if s == c {
				dup = true
			}
		}
		if !dup {
			out = append(out, c)
		}
	}
	return out
}

func truthy(v any) bool {
	switch x := v.(type) {
	case nil:
		return false
	case string:
		return x != ""
	case bool:
		return x
	case float64:
		return x != 0
	case []any:
		return len(x) > 0
	case map[string]any:
		return len(x) > 0
	}
	return true
}

func str(v any) string {
	if s, ok := v.(string); ok {
		return s
	}
	return fmt.Sprint(v)
}

func readSession(path string, info os.FileInfo) *SessionUsage {
	if info.Size() > MaxSessionBytes {
		return nil
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var data any
	if err := json.Unmarshal(raw, &data); err != nil {
		return nil
	}
	obj, ok := data.(map[string]any)
	if !ok {
		return nil
	}
	usage, ok := obj["usage"].(map[string]any)
	if !ok {
		return nil
	}
	model := ""
	if mc, ok := obj["model_config"].(map[string]any); ok && truthy(mc["model"]) {
		model = str(mc["model"])
	}
	if model == "" {
		model = "unknown"
	}
	id := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	if truthy(obj["session_id"]) {
		id = str(obj["session_id"])
	}
	return &SessionUsage{
		SessionID:        id,
		Day:              info.ModTime().Local().Format("2006-01-02"),
		Model:            model,
		Prompts:          nonNegInt(obj["turns"]),
		InputTokens:      nonNegInt(usage["input_tokens"]),
		OutputTokens:     nonNegInt(usage["output_tokens"]),
		CacheReadTokens:  nonNegInt(usage["cache_read_input_tokens"]),
		CacheWriteTokens: nonNegInt(usage["cache_creation_input_tokens"]),
	}
}

// ReadSessions returns the sessions under dirs, one per session id. The same
// session can appear in more than one directory; the most recently written copy
// wins.
func ReadSessions(dirs []string) []SessionUsage {
	type held struct {
		mtime time.Time
		s     SessionUsage
	}
	newest := map[string]held{}
	var order []string
	for _, dir := range dirs {
		entries, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		names := make([]string, 0, len(entries))
		for _, e := range entries {
			if strings.HasSuffix(e.Name(), ".json") {
				names = append(names, e.Name())
			}
		}
		sort.Strings(names)
		for _, name := range names {
			path := filepath.Join(dir, name)
			info, err := os.Stat(path)
			if err != nil || info.IsDir() {
				continue
			}
			s := readSession(path, info)
			if s == nil {
				continue
			}
			known, ok := newest[s.SessionID]
			if !ok {
				order = append(order, s.SessionID)
			}
			if !ok || info.ModTime().After(known.mtime) {
				newest[s.SessionID] = held{info.ModTime(), *s}
			}
		}
	}
	out := make([]SessionUsage, 0, len(order))
	for _, id := range order {
		out = append(out, newest[id].s)
	}
	return out
}

// RecentDay is one entry of recentDays. Omarchy's panel calls the field
// messageCount but plots tokens.
type RecentDay struct {
	Date         string `json:"date"`
	MessageCount int    `json:"messageCount"`
}

// Record is the panel record; field order is the output key order.
type Record struct {
	SchemaVersion      int            `json:"schemaVersion"`
	ID                 string         `json:"id"`
	Name               string         `json:"name"`
	UpdatedAt          string         `json:"updatedAt"`
	Ready              bool           `json:"ready"`
	HasLocalStats      bool           `json:"hasLocalStats"`
	TodayPrompts       int            `json:"todayPrompts"`
	TodaySessions      int            `json:"todaySessions"`
	TodayTotalTokens   int            `json:"todayTotalTokens"`
	TodayTokensByModel *orderedInts   `json:"todayTokensByModel"`
	RecentDays         []RecentDay    `json:"recentDays"`
	TotalPrompts       int            `json:"totalPrompts"`
	TotalSessions      int            `json:"totalSessions"`
	ActiveDays         int            `json:"activeDays"`
	ActiveDates        []string       `json:"activeDates"`
	ModelUsage         *orderedModels `json:"modelUsage"`
}

// JSON renders the record as compact single-line JSON (what Omarchy's collector
// wrapper captures).
func (r Record) JSON() (string, error) { return compactJSON(r) }

func isoNow() string {
	return time.Now().Truncate(time.Microsecond).Format("2006-01-02T15:04:05.999999-07:00")
}

// BuildRecord builds the panel record. A zero now means the current time.
func BuildRecord(sessions []SessionUsage, now time.Time) Record {
	if now.IsZero() {
		now = time.Now()
	}
	today := now.Format("2006-01-02")
	recentDates := make([]string, 0, RecentDayCount)
	for offset := RecentDayCount - 1; offset >= 0; offset-- {
		recentDates = append(recentDates, now.AddDate(0, 0, -offset).Format("2006-01-02"))
	}
	recent := map[string]*RecentDay{}
	for _, d := range recentDates {
		recent[d] = &RecentDay{Date: d}
	}
	models := newOrderedModels()
	todayByModel := newOrderedInts()
	todaySessions := map[string]bool{}
	totalSessions := map[string]bool{}
	activeDays := map[string]bool{}
	var todayPrompts, todayTotal, totalPrompts int

	for _, s := range sessions {
		total := s.TotalTokens()
		totalPrompts += s.Prompts
		totalSessions[s.SessionID] = true
		activeDays[s.Day] = true

		b := models.bucket(s.Model)
		b.InputTokens += s.InputTokens
		b.OutputTokens += s.OutputTokens
		b.CacheReadInputTokens += s.CacheReadTokens
		b.CacheCreationInputTokens += s.CacheWriteTokens

		if d, ok := recent[s.Day]; ok {
			d.MessageCount += total
		}
		if s.Day == today {
			todayPrompts += s.Prompts
			todaySessions[s.SessionID] = true
			todayTotal += total
			todayByModel.add(s.Model, total)
		}
	}
	days := make([]RecentDay, 0, len(recentDates))
	for _, d := range recentDates {
		days = append(days, *recent[d])
	}
	dates := sortedKeys(activeDays)
	if dates == nil {
		dates = []string{}
	}
	return Record{
		SchemaVersion:      RecordSchemaVersion,
		ID:                 AgentID,
		Name:               AgentName,
		UpdatedAt:          isoNow(),
		Ready:              true,
		HasLocalStats:      true,
		TodayPrompts:       todayPrompts,
		TodaySessions:      len(todaySessions),
		TodayTotalTokens:   todayTotal,
		TodayTokensByModel: todayByModel,
		RecentDays:         days,
		TotalPrompts:       totalPrompts,
		TotalSessions:      len(totalSessions),
		ActiveDays:         len(activeDays),
		ActiveDates:        dates,
		ModelUsage:         models,
	}
}

// CollectRecord reads the sessions under dirs and builds the record.
func CollectRecord(dirs []string, now time.Time) Record {
	return BuildRecord(ReadSessions(dirs), now)
}

// WriteRecord writes sarina.json atomically (temp file + rename), so the panel
// never reads a torn file. Any value that marshals to JSON is accepted.
func WriteRecord(record any, dir string) (string, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	path := filepath.Join(dir, AgentID+".json")
	text, err := compactJSON(record)
	if err != nil {
		return "", err
	}
	tmp, err := os.CreateTemp(dir, "."+AgentID+".")
	if err != nil {
		return "", err
	}
	name := tmp.Name()
	cleanup := func() { os.Remove(name) }
	if _, err := tmp.WriteString(text + "\n"); err != nil {
		tmp.Close()
		cleanup()
		return "", err
	}
	if err := tmp.Close(); err != nil {
		cleanup()
		return "", err
	}
	// CreateTemp opens 0600; nothing in this record is sensitive.
	_ = os.Chmod(name, 0o644)
	if err := os.Rename(name, path); err != nil {
		cleanup()
		return "", err
	}
	return path, nil
}
