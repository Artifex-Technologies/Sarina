package fleet

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
)

// Reading other agents' usage records from Omarchy's directory. READ-ONLY, and
// every record is untrusted: strings lose control/bidi characters and are
// truncated, ids must match a strict pattern, numbers are clamped, big files
// are skipped.

const (
	// MaxRecordBytes: a bigger record is not one of Omarchy's collectors' output.
	MaxRecordBytes = 1024 * 1024
	MaxTextLength  = 80
	maxLimits      = 16
	// DefaultStaleAfter: Omarchy refreshes every 15 minutes; an hour is four misses.
	DefaultStaleAfter = time.Hour
)

var (
	agentIDPattern = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,31}$`)
	controlChars   = regexp.MustCompile("[\\x00-\\x1f\\x{7f}-\\x{9f}\\x{200b}-\\x{200f}\\x{202a}-\\x{202e}\\x{2066}-\\x{2069}]")
)

// LimitReading is one rate-limit window of another agent.
type LimitReading struct {
	Label       string
	PercentUsed *float64 // nil = unknown
	ResetsAt    string
}

// AgentStatus is the sanitised view of one record.
type AgentStatus struct {
	AgentID       string
	Name          string
	Ready         bool
	Stale         bool
	UpdatedAt     string
	TodayTokens   int
	WeekTokens    int
	TotalSessions int
	Limits        []LimitReading
	StatusText    string
}

// HeadroomPercent is the room left in the tightest window, or nil when no limit
// is known. Prepaid and local agents report no windows; that is "unknown", not
// "unlimited", so it never ranks above an agent with a measured limit.
func (s AgentStatus) HeadroomPercent() *float64 {
	var maxUsed *float64
	for _, l := range s.Limits {
		if l.PercentUsed != nil && (maxUsed == nil || *l.PercentUsed > *maxUsed) {
			v := *l.PercentUsed
			maxUsed = &v
		}
	}
	if maxUsed == nil {
		return nil
	}
	h := math.Max(0, 100-*maxUsed)
	return &h
}

func clean(v any) string {
	s, ok := v.(string)
	if !ok {
		return ""
	}
	s = strings.TrimSpace(controlChars.ReplaceAllString(s, ""))
	if r := []rune(s); len(r) > MaxTextLength {
		s = string(r[:MaxTextLength])
	}
	return s
}

func number(v any) (float64, bool) {
	switch n := v.(type) {
	case float64:
		return n, true
	case float32:
		return float64(n), true
	case int:
		return float64(n), true
	case int64:
		return float64(n), true
	}
	return 0, false // includes bool
}

func count(v any) int {
	f, ok := number(v)
	if !ok || math.IsNaN(f) || math.IsInf(f, 0) {
		return 0
	}
	if f >= 9.2e18 {
		return math.MaxInt64
	}
	return max(int(f), 0)
}

func percent(v any) *float64 {
	f, ok := number(v)
	if !ok || math.IsNaN(f) || math.IsInf(f, 0) {
		return nil
	}
	f = math.Min(math.Max(f, 0), 100)
	return &f
}

var timeLayouts = []string{
	time.RFC3339Nano,
	"2006-01-02T15:04:05.999999999Z0700",
	"2006-01-02 15:04:05.999999999Z07:00",
	"2006-01-02 15:04:05.999999999Z0700",
	"2006-01-02T15:04:05.999999999", // naive: UTC
	"2006-01-02 15:04:05.999999999",
	"2006-01-02",
}

func parseTime(v any) (time.Time, bool) {
	s, ok := v.(string)
	if !ok {
		return time.Time{}, false
	}
	for _, layout := range timeLayouts {
		if t, err := time.Parse(layout, s); err == nil {
			return t, true
		}
	}
	return time.Time{}, false
}

// ParseRecord returns the sanitised status of one decoded record, or nil if it
// is not usable. A zero now means the current time; a zero staleAfter means the
// default.
func ParseRecord(data any, now time.Time, staleAfter time.Duration) *AgentStatus {
	obj, ok := data.(map[string]any)
	if !ok {
		return nil
	}
	id, ok := obj["id"].(string)
	if !ok || !agentIDPattern.MatchString(id) {
		return nil
	}
	if now.IsZero() {
		now = time.Now()
	}
	if staleAfter == 0 {
		staleAfter = DefaultStaleAfter
	}
	updated, ok := parseTime(obj["updatedAt"])
	stale := !ok || now.Sub(updated) > staleAfter

	week := 0
	if days, ok := obj["recentDays"].([]any); ok {
		for _, d := range days {
			if dm, ok := d.(map[string]any); ok {
				// Omarchy's panel names this messageCount but it carries tokens.
				week += count(dm["messageCount"])
			}
		}
	}

	var limits []LimitReading
	if raw, ok := obj["limits"].([]any); ok {
		if len(raw) > maxLimits {
			raw = raw[:maxLimits]
		}
		for _, e := range raw {
			if em, ok := e.(map[string]any); ok {
				label := clean(em["label"])
				if label == "" {
					label = "limit"
				}
				limits = append(limits, LimitReading{
					Label:       label,
					PercentUsed: percent(em["percent"]),
					ResetsAt:    clean(em["resetsAt"]),
				})
			}
		}
	}
	name := clean(obj["name"])
	if name == "" {
		name = id
	}
	ready, _ := obj["ready"].(bool)
	return &AgentStatus{
		AgentID:       id,
		Name:          name,
		Ready:         ready,
		Stale:         stale,
		UpdatedAt:     clean(obj["updatedAt"]),
		TodayTokens:   count(obj["todayTotalTokens"]),
		WeekTokens:    week,
		TotalSessions: count(obj["totalSessions"]),
		Limits:        limits,
		StatusText:    clean(obj["usageStatusText"]),
	}
}

// ReadAgentStatus returns every readable record in dir, sorted by agent id.
func ReadAgentStatus(dir string, now time.Time, staleAfter time.Duration) []AgentStatus {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []AgentStatus
	for _, e := range entries {
		if !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		info, err := os.Stat(path)
		if err != nil || info.IsDir() || info.Size() > MaxRecordBytes {
			continue
		}
		raw, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var data any
		if json.Unmarshal(raw, &data) != nil {
			continue
		}
		if s := ParseRecord(data, now, staleAfter); s != nil {
			out = append(out, *s)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].AgentID < out[j].AgentID })
	return out
}

// SuggestAgent names the agent with the most measured headroom, or nil.
// Advisory only. A candidate must be current, ready, and report at least one
// limit window; ties go to whoever used fewer tokens today. A nil exclude means
// "exclude Sarina herself".
func SuggestAgent(statuses []AgentStatus, exclude []string) *AgentStatus {
	if exclude == nil {
		exclude = []string{AgentID}
	}
	var best *AgentStatus
	var bestHead float64
	for i := range statuses {
		s := &statuses[i]
		skip := false
		for _, x := range exclude {
			if s.AgentID == x {
				skip = true
			}
		}
		h := s.HeadroomPercent()
		if skip || !s.Ready || s.Stale || h == nil {
			continue
		}
		if best == nil || *h > bestHead || (*h == bestHead && s.TodayTokens < best.TodayTokens) {
			best, bestHead = s, *h
		}
	}
	return best
}

func tokens(n int) string {
	switch {
	case n >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(n)/1_000_000)
	case n >= 1_000:
		return fmt.Sprintf("%.1fk", float64(n)/1_000)
	}
	return fmt.Sprint(n)
}

// RenderStatus is the human-readable view.
func RenderStatus(statuses []AgentStatus) string {
	if len(statuses) == 0 {
		return "No agent usage records found."
	}
	lines := []string{"Agents", ""}
	for _, s := range statuses {
		var flags []string
		if s.Stale {
			flags = append(flags, "stale")
		}
		if !s.Ready {
			flags = append(flags, "not ready")
		}
		suffix := ""
		if len(flags) > 0 {
			suffix = "  [" + strings.Join(flags, ", ") + "]"
		}
		lines = append(lines, fmt.Sprintf("%s (%s): %s today, %s this week, %d sessions%s",
			s.Name, s.AgentID, tokens(s.TodayTokens), tokens(s.WeekTokens), s.TotalSessions, suffix))
		for _, l := range s.Limits {
			used := "unknown"
			if l.PercentUsed != nil {
				used = fmt.Sprintf("%.0f%% used", *l.PercentUsed)
			}
			reset := ""
			if l.ResetsAt != "" {
				reset = ", resets " + l.ResetsAt
			}
			lines = append(lines, fmt.Sprintf("    %s: %s%s", l.Label, used, reset))
		}
		if len(s.Limits) == 0 {
			text := s.StatusText
			if text == "" {
				text = "no limit data"
			}
			lines = append(lines, "    "+text)
		}
	}
	return strings.Join(lines, "\n")
}
