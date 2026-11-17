package daily

import (
	"fmt"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/testguard"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

// Note is the day's note taken apart: the check-up's own lines (and anything else above the status), the status section, the
// numbers lines.
type Note struct {
	Day      string // YYYY-MM-DD
	Head     []string
	StatusAt string // HH:MM of the status section, "" when there is none
	Status   []string
	Numbers  []string
}

var statusHeadRx = regexp.MustCompile(`^## Status \((\d{2}:\d{2})\)\s*$`)

// Parse takes the note's text apart. Check-up bullets that landed below the status (an older check-up appends to the file) are
// moved back up with the head.
func Parse(day, text string) Note {
	n := Note{Day: day}
	section := "head"
	for _, l := range strings.Split(text, "\n") {
		t := strings.TrimSpace(l)
		if m := statusHeadRx.FindStringSubmatch(t); m != nil {
			section, n.StatusAt = "status", m[1]
			continue
		}
		if t == "## Numbers" {
			section = "numbers"
			continue
		}
		if strings.HasPrefix(t, "## ") {
			section = "head"
		}
		switch section {
		case "status":
			if t != "" {
				n.Status = append(n.Status, l)
			}
		case "numbers":
			if _, ok := ParseNumbers(t); ok {
				n.Numbers = append(n.Numbers, t)
			} else if t != "" {
				n.Head = append(n.Head, l)
			}
		default:
			n.Head = append(n.Head, l)
		}
	}
	// the head keeps its front matter and title; stray blank lines at its end go
	for len(n.Head) > 0 && strings.TrimSpace(n.Head[len(n.Head)-1]) == "" {
		n.Head = n.Head[:len(n.Head)-1]
	}
	if len(n.Head) == 0 {
		n.Head = header(day)
	}
	return n
}

func header(day string) []string {
	return strings.Split(fmt.Sprintf("---\ncreated: %s\nsource: labyrinth-sarina-checkup\ntags: [daily]\n---\n\n# %s\n", day, day), "\n")
}

// Render writes the note back: head, status, numbers.
func (n Note) Render() string {
	var b strings.Builder
	b.WriteString(strings.Join(n.Head, "\n"))
	b.WriteString("\n")
	if len(n.Status) > 0 {
		fmt.Fprintf(&b, "\n## Status (%s)\n\n%s\n", n.StatusAt, strings.Join(n.Status, "\n"))
	}
	if len(n.Numbers) > 0 {
		b.WriteString("\n## Numbers\n\n" + strings.Join(n.Numbers, "\n") + "\n")
	}
	return b.String()
}

// Fresh says whether the status was taken within maxAge of `now` (today's note only).
func (n Note) Fresh(now time.Time, maxAge time.Duration) bool {
	if n.StatusAt == "" || maxAge <= 0 || n.Day != now.Format("2006-01-02") {
		return false
	}
	at, err := time.ParseInLocation("2006-01-02 15:04", n.Day+" "+n.StatusAt, now.Location())
	if err != nil {
		return false
	}
	return !at.After(now) && now.Sub(at) <= maxAge
}

// SetStatus replaces the status section with a probe's rendering and appends its numbers line.
func (n *Note) SetStatus(s Status) {
	section, numbers := s.Render()
	lines := strings.Split(strings.TrimSpace(section), "\n")
	n.StatusAt = s.At.Format("15:04")
	n.Status = nil
	for _, l := range lines[1:] { // the heading is written by Render
		if strings.TrimSpace(l) != "" {
			n.Status = append(n.Status, l)
		}
	}
	n.Numbers = append(n.Numbers, numbers.Line())
	if len(n.Numbers) > 48 {
		n.Numbers = n.Numbers[len(n.Numbers)-48:]
	}
}

// Path of the day's note in the vault.
func Path(vault string, day time.Time) string {
	return filepath.Join(vault, "Daily", day.Format("2006-01-02")+".md")
}

// Load reads the day's note (an empty one when there is none).
func Load(vault string, day time.Time) Note {
	// a bounded regular file: a named pipe planted under the day's name is not waited on
	b, _ := session.ReadRegular(Path(vault, day), 1<<20)
	return Parse(day.Format("2006-01-02"), string(b))
}

// Save writes the note.
func Save(vault string, n Note) error {
	p := filepath.Join(vault, "Daily", n.Day+".md")
	if err := testguard.Allow(p); err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	// the copy is made anew: whatever was left under its name (a link too) is removed, not written through
	tmp := p + ".tmp"
	_ = os.Remove(tmp)
	f, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
	if err != nil {
		return err
	}
	_, err = f.Write([]byte(n.Render()))
	if err1 := f.Close(); err == nil {
		err = err1
	}
	if err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return os.Rename(tmp, p)
}
