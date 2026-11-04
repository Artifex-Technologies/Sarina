package agent

import (
	"os"
	"strings"
	"syscall"
	"unicode"
)

// Lessons: the owner-approved rules she learned from her mistakes (homework, docs/VOICE_AND_TRUST_PLAN.md and the vault's
// Lessons/ folder), one line each, in front of every session. The vault keeps each lesson as a linked note (the owner reads them
// in Obsidian); this file is only the short list made from the active ones (`sarina lessons digest`), the Hermes-style memory
// she always sees. Before this she had a lesson for exactly the mistake she made (decompile-explain, 2026-11-04) but only found
// lessons when she thought to search for them.
//
// The default file is root-owned (/usr/local/share/sarina/lessons.md): both of her accounts read it, neither can change what
// she was taught. A file anyone but its owner can write is ignored, and so is a default file root does not own.

// DefaultLessonsFile is where the installed list lives.
const DefaultLessonsFile = "/usr/local/share/sarina/lessons.md"

const (
	maxLessons     = 20
	maxLessonBytes = 300
)

// lessonsPart returns the system-prompt part with the active lessons, or "" when there are none or the file is not trusted.
func lessonsPart() string {
	path, explicit := os.Getenv("SARINA_LESSONS_FILE"), true
	if path == "" {
		path, explicit = DefaultLessonsFile, false
	}
	if path == "off" {
		return ""
	}
	rules := readLessons(path, explicit)
	if len(rules) == 0 {
		return ""
	}
	return "Lessons from your own mistakes (the owner approved each one; follow them):\n- " + strings.Join(rules, "\n- ")
}

func readLessons(path string, explicit bool) []string {
	st, err := os.Stat(path)
	if err != nil || !st.Mode().IsRegular() || st.Size() > 64<<10 || st.Mode().Perm()&0o022 != 0 {
		return nil
	}
	if sys, ok := st.Sys().(*syscall.Stat_t); ok && !explicit && sys.Uid != 0 {
		return nil
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var out []string
	for _, line := range strings.Split(string(b), "\n") {
		line = strings.TrimSpace(line)
		if !strings.HasPrefix(line, "- ") {
			continue
		}
		rule := strings.Map(func(r rune) rune {
			if unicode.IsControl(r) || unicode.Is(unicode.Cf, r) {
				return -1
			}
			return r
		}, strings.TrimSpace(line[2:]))
		if rule == "" {
			continue
		}
		if len(rule) > maxLessonBytes {
			rule = rule[:maxLessonBytes]
		}
		out = append(out, rule)
		if len(out) == maxLessons {
			break
		}
	}
	return out
}
