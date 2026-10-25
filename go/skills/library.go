package skills

// Long skills. A skill's prompt has to fit the model's context with room left for the question, the tools and the answer, so at most
// maxSkillBytes of a skill go into the prompt. A longer skill (a whole course, a reference) is not refused: it is split into sections
// at its "##" headings (very long sections into parts), the prompt gets its introduction and a table of contents, and the sections are
// read when they are needed (skill_section) or found by meaning (skill_search). A skill folder may also keep its sections as files of
// their own: <skill>/sections/*.md, one section each, the first heading being its title.

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

const (
	// maxFileBytes is the most read of one skill or section file.
	maxFileBytes = 4 << 20
	// maxSectionBytes is the most of a section returned at once; a longer one is split into parts.
	maxSectionBytes = 12 << 10
	// introBytes is how much of a long skill's introduction stays in the prompt.
	introBytes = 6 << 10
	// tocBytes is the most the table of contents takes in the prompt.
	tocBytes = 8 << 10
	// maxSections caps the sections one skill may have.
	maxSections = 2000
)

// Section is one part of a long skill.
type Section struct {
	Title string
	Text  string
}

var headingRE = regexp.MustCompile(`(?m)^(#{1,3})[ \t]+(.+?)[ \t]*#*[ \t]*$`)

// splitSections cuts a long body at its headings: "##" when it has any, else "#", else "###"; text before the first heading is the
// introduction. Sections longer than maxSectionBytes become parts. Without headings the body is cut into parts at paragraph breaks.
func splitSections(body string) (intro string, out []Section) {
	level := ""
	for _, l := range []string{"##", "#", "###"} {
		n := 0
		for _, m := range headingRE.FindAllStringSubmatch(body, -1) {
			if m[1] == l {
				n++
			}
		}
		if n >= 2 {
			level = l
			break
		}
	}
	var raw []Section
	if level == "" {
		raw = []Section{{Title: "Text", Text: body}}
	} else {
		locs := headingRE.FindAllStringSubmatchIndex(body, -1)
		var starts []int
		var titles []string
		for _, loc := range locs {
			if body[loc[2]:loc[3]] == level {
				starts = append(starts, loc[0])
				titles = append(titles, strings.TrimSpace(body[loc[4]:loc[5]]))
			}
		}
		intro = strings.TrimSpace(body[:starts[0]])
		for i, st := range starts {
			end := len(body)
			if i+1 < len(starts) {
				end = starts[i+1]
			}
			raw = append(raw, Section{Title: titles[i], Text: strings.TrimSpace(body[st:end])})
		}
	}
	for _, s := range raw {
		parts := cutParts(s.Text, maxSectionBytes)
		for i, p := range parts {
			t := s.Title
			if len(parts) > 1 {
				t = fmt.Sprintf("%s (part %d of %d)", s.Title, i+1, len(parts))
			}
			out = append(out, Section{Title: t, Text: p})
		}
	}
	if len(out) > maxSections {
		out = out[:maxSections]
	}
	return intro, out
}

// cutParts splits text into pieces of at most max bytes, at paragraph breaks where it can, else at line breaks, else anywhere (on a
// UTF-8 boundary).
func cutParts(text string, max int) []string {
	var out []string
	for len(text) > max {
		cut := strings.LastIndex(text[:max], "\n\n")
		if cut < max/2 {
			cut = strings.LastIndex(text[:max], "\n")
		}
		if cut < max/2 {
			cut = max
			for cut > 0 && text[cut]&0xC0 == 0x80 {
				cut--
			}
		}
		out = append(out, strings.TrimSpace(text[:cut]))
		text = strings.TrimSpace(text[cut:])
	}
	if strings.TrimSpace(text) != "" {
		out = append(out, strings.TrimSpace(text))
	}
	return out
}

// sectionFiles reads <dir>/sections/*.md, sorted by name; the first heading of each is its title (else the file name).
func sectionFiles(dir string, sc Scanner, tr Trust) ([]Section, []Problem) {
	entries, err := os.ReadDir(filepath.Join(dir, "sections"))
	if err != nil {
		return nil, nil
	}
	sort.Slice(entries, func(i, j int) bool { return entries[i].Name() < entries[j].Name() })
	var out []Section
	var probs []Problem
	for _, e := range entries {
		p := filepath.Join(dir, "sections", e.Name())
		if !e.Type().IsRegular() || !strings.EqualFold(filepath.Ext(e.Name()), ".md") {
			continue
		}
		if tr != nil {
			if why := tr.Refuse(p); why != "" {
				probs = append(probs, Problem{p, why})
				continue
			}
		}
		data, err := readCapped(p)
		if err != nil {
			probs = append(probs, Problem{p, err.Error()})
			continue
		}
		if v := sc.Scan(string(data)); v.Blocked {
			probs = append(probs, Problem{p, "blocked by the security scan: " + v.Reason})
			continue
		}
		text := strings.TrimSpace(string(data))
		title := strings.TrimSuffix(e.Name(), filepath.Ext(e.Name()))
		if m := headingRE.FindStringSubmatch(text); m != nil {
			title = strings.TrimSpace(m[2])
		}
		parts := cutParts(text, maxSectionBytes)
		for i, pt := range parts {
			t := title
			if len(parts) > 1 {
				t = fmt.Sprintf("%s (part %d of %d)", title, i+1, len(parts))
			}
			out = append(out, Section{Title: t, Text: pt})
		}
		if len(out) >= maxSections {
			break
		}
	}
	return out, probs
}

func readCapped(path string) ([]byte, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return nil, err
	}
	if st.Size() > maxFileBytes {
		return nil, fmt.Errorf("file is larger than %d bytes", maxFileBytes)
	}
	data := make([]byte, st.Size())
	n, err := f.Read(data)
	if err != nil && n == 0 && st.Size() > 0 {
		return nil, err
	}
	return data[:n], nil
}

// makeLibrary turns a skill into a long one: the body becomes its introduction and the table of contents of its sections. authored: the
// introduction is the skill's own SKILL.md text (kept whole); otherwise it was cut from an oversized file and is shortened.
func (s *Skill) makeLibrary(intro string, sections []Section, authored bool) {
	s.Sections = sections
	var b strings.Builder
	if intro != "" {
		if !authored && len(intro) > introBytes {
			intro = cutParts(intro, introBytes)[0] + "\n[…]"
		}
		b.WriteString(intro)
		b.WriteString("\n\n")
	}
	fmt.Fprintf(&b, "This skill is long: its %d sections are not in this message. Open one with the skill_section tool (skill %q and the section's number or title), or find passages by meaning with skill_search. Read only what the task needs.\n\nSections:\n", len(sections), s.Name)
	toc := 0
	for i, sec := range sections {
		line := fmt.Sprintf("%d. %s\n", i+1, sec.Title)
		if toc+len(line) > tocBytes {
			fmt.Fprintf(&b, "… and %d more (skill_search finds them)\n", len(sections)-i)
			break
		}
		b.WriteString(line)
		toc += len(line)
	}
	s.Body = strings.TrimSpace(b.String())
	if !strings.Contains(s.Body, "$ARGUMENTS") {
		s.Body += "\n\nThe task: $ARGUMENTS"
	}
}

// Section finds a section by its number (1 first) or its title (whole, else the first title that contains it, ignoring case).
func (s *Skill) Section(which string) (int, *Section, bool) {
	which = strings.TrimSpace(which)
	if n, err := strconv.Atoi(strings.TrimSuffix(which, ".")); err == nil {
		if n >= 1 && n <= len(s.Sections) {
			return n, &s.Sections[n-1], true
		}
		return 0, nil, false
	}
	low := strings.ToLower(which)
	for i := range s.Sections {
		if strings.ToLower(s.Sections[i].Title) == low {
			return i + 1, &s.Sections[i], true
		}
	}
	for i := range s.Sections {
		if low != "" && strings.Contains(strings.ToLower(s.Sections[i].Title), low) {
			return i + 1, &s.Sections[i], true
		}
	}
	return 0, nil, false
}

// IsLibrary reports whether the skill has sections to read on demand.
func (s *Skill) IsLibrary() bool { return len(s.Sections) > 0 }
