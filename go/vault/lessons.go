package vault

import (
	"fmt"
	"sort"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// Lessons in the vault: one note per lesson under Lessons/, linked from Reflections/Lessons index.md and from the homework that
// taught it (Homework/), so the owner sees how they connect in Obsidian. Two front-matter keys make a note a lesson she always
// sees: `rule:` (one sentence) and `status:` (proposed, active or retired). Only active rules go into the digest, the short list
// installed for every session (agent.DefaultLessonsFile). A proposed lesson waits for the owner: changing its status to active
// in Obsidian is the approval.

func init() {
	cli.Register("lessons", "her lessons from the vault: lessons [--path DIR] list | digest  (digest prints the active rules, for /usr/local/share/sarina/lessons.md)", lessonsCommand)
}

// Lesson is a lesson note's rule and state.
type Lesson struct {
	Note   string
	Rule   string
	Status string
}

// Lessons returns every note under Lessons/ that has a rule, oldest first by path.
func (v *Vault) Lessons() ([]Lesson, error) {
	notes, err := v.Notes()
	if err != nil {
		return nil, err
	}
	var out []Lesson
	for _, n := range notes {
		if !strings.HasPrefix(n.Path, "Lessons/") {
			continue
		}
		rule := strings.Trim(n.Front["rule"], `"' `)
		if rule == "" {
			continue
		}
		st := strings.ToLower(strings.Trim(n.Front["status"], `"' `))
		if st == "" {
			st = "proposed"
		}
		out = append(out, Lesson{Note: n.Path, Rule: oneLine(rule), Status: st})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Note < out[j].Note })
	return out, nil
}

func oneLine(s string) string { return strings.Join(strings.Fields(s), " ") }

// Digest is the file installed for every session: the active rules, one "- " line each.
func Digest(ls []Lesson) string {
	var b strings.Builder
	b.WriteString("# Sarina's lessons (made by `sarina lessons digest` from the vault's active lessons; edit the notes, not this file)\n\n")
	for _, l := range ls {
		if l.Status == "active" {
			b.WriteString("- " + l.Rule + "\n")
		}
	}
	return b.String()
}

func lessonsCommand(env *cli.Env, args []string) int {
	v, rest, rc := open(env, "lessons", args)
	if rc >= 0 {
		return rc
	}
	ls, err := v.Lessons()
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina lessons:", err)
		return 1
	}
	what := "list"
	if len(rest) > 0 {
		what = rest[0]
	}
	switch what {
	case "digest":
		fmt.Fprint(env.Stdout, Digest(ls))
	case "list":
		for _, l := range ls {
			fmt.Fprintf(env.Stdout, "%-9s %s\n          %s\n", l.Status, strings.TrimSuffix(strings.TrimPrefix(l.Note, "Lessons/"), ".md"), l.Rule)
		}
	default:
		fmt.Fprintln(env.Stderr, "sarina lessons: list or digest")
		return 2
	}
	return 0
}
