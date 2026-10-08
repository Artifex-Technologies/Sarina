package desktop

import (
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode/utf8"
)

// GeneratedMarker marks every file this package generates, so cleanup removes
// exactly those and nothing else.
const GeneratedMarker = "X-Sarina-Generated"

const desktopEntryGroup = "[Desktop Entry]"

var systemApplicationDirs = []string{"/usr/share/applications", "/usr/local/share/applications"}

// DesktopEntry is one [Desktop Entry] group reduced to what menu placement needs.
type DesktopEntry struct {
	Path        string
	EntryID     string
	Name        string
	EntryType   string
	ExecCommand string
	Icon        string
	Categories  []string
	NoDisplay   bool
	Hidden      bool
	Terminal    bool
	TryExec     string
	GeneratedBy string
}

// IsApplication reports Type=Application.
func (e DesktopEntry) IsApplication() bool { return e.EntryType == "Application" }

// IsMenuVisible: NoDisplay=true entries (plumbing) and Hidden=true (deleted)
// are never menu-visible and must never be given a category.
func (e DesktopEntry) IsMenuVisible() bool { return e.IsApplication() && !e.NoDisplay && !e.Hidden }

func unescape(value string) string {
	if !strings.Contains(value, `\`) {
		return value
	}
	var b strings.Builder
	rs := []rune(value)
	for i := 0; i < len(rs); i++ {
		if rs[i] != '\\' {
			b.WriteRune(rs[i])
			continue
		}
		if i+1 >= len(rs) {
			break
		}
		i++
		switch rs[i] {
		case 's':
			b.WriteRune(' ')
		case 'n':
			b.WriteRune('\n')
		case 't':
			b.WriteRune('\t')
		case 'r':
			b.WriteRune('\r')
		default:
			b.WriteRune(rs[i])
		}
	}
	return b.String()
}

func splitList(value string) []string {
	var out []string
	for _, p := range strings.Split(value, ";") {
		if p = strings.TrimSpace(p); p != "" {
			out = append(out, p)
		}
	}
	return out
}

// splitLines splits like Python's str.splitlines for the separators that occur
// in desktop files (\n, \r\n, \r); no trailing empty element.
func splitLines(text string) []string {
	text = strings.ReplaceAll(text, "\r\n", "\n")
	text = strings.ReplaceAll(text, "\r", "\n")
	if text == "" {
		return nil
	}
	text = strings.TrimSuffix(text, "\n")
	return strings.Split(text, "\n")
}

// ParseDesktopEntry parses the [Desktop Entry] group. It returns nil when the
// text has no such group. Locale-suffixed keys and later groups are ignored.
func ParseDesktopEntry(text, path string) *DesktopEntry {
	fields := map[string]string{}
	inEntry, seen := false, false
	for _, raw := range splitLines(text) {
		line := strings.TrimSpace(raw)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		if strings.HasPrefix(line, "[") {
			if inEntry {
				break
			}
			inEntry = line == desktopEntryGroup
			seen = seen || inEntry
			continue
		}
		if !inEntry || !strings.Contains(line, "=") {
			continue
		}
		i := strings.Index(line, "=")
		key, value := strings.TrimSpace(line[:i]), strings.TrimSpace(line[i+1:])
		if strings.HasSuffix(key, "]") {
			continue
		}
		if _, ok := fields[key]; !ok {
			fields[key] = value
		}
	}
	if !seen {
		return nil
	}
	typ, ok := fields["Type"]
	if !ok {
		typ = "Application"
	}
	flag := func(k string) bool { return strings.EqualFold(fields[k], "true") }
	base := filepath.Base(path)
	return &DesktopEntry{
		Path:        path,
		EntryID:     strings.TrimSuffix(base, filepath.Ext(base)),
		Name:        unescape(fields["Name"]),
		EntryType:   typ,
		ExecCommand: unescape(fields["Exec"]),
		Icon:        fields["Icon"],
		Categories:  splitList(fields["Categories"]),
		NoDisplay:   flag("NoDisplay"),
		Hidden:      flag("Hidden"),
		Terminal:    flag("Terminal"),
		TryExec:     fields["TryExec"],
		GeneratedBy: fields[GeneratedMarker],
	}
}

// UserApplicationDir is where generated entries belong (no root, survives upgrades).
func UserApplicationDir() string {
	if d := os.Getenv("XDG_DATA_HOME"); d != "" {
		return filepath.Join(d, "applications")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		home = "."
	}
	return filepath.Join(home, ".local", "share", "applications")
}

// ApplicationDirs returns existing XDG application directories, most general
// first, honouring XDG_DATA_DIRS and XDG_DATA_HOME.
func ApplicationDirs() []string {
	var candidates []string
	if dd := os.Getenv("XDG_DATA_DIRS"); dd != "" {
		for _, p := range filepath.SplitList(dd) {
			if p != "" {
				candidates = append(candidates, filepath.Join(p, "applications"))
			}
		}
	} else {
		candidates = append(candidates, systemApplicationDirs...)
	}
	candidates = append(candidates, UserApplicationDir())
	seen := map[string]bool{}
	var found []string
	for _, c := range candidates {
		if seen[c] {
			continue
		}
		if st, err := os.Stat(c); err != nil || !st.IsDir() {
			continue
		}
		seen[c] = true
		found = append(found, c)
	}
	return found
}

// readText reads a file, replacing invalid UTF-8 bytes with U+FFFD.
func readText(path string) (string, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	if utf8.Valid(raw) {
		return string(raw), nil
	}
	var b strings.Builder
	for len(raw) > 0 {
		r, size := utf8.DecodeRune(raw)
		b.WriteRune(r)
		raw = raw[size:]
	}
	return b.String(), nil
}

func globDesktop(dir string) []string {
	matches, _ := filepath.Glob(filepath.Join(dir, "*.desktop"))
	sort.Strings(matches)
	return matches
}

// ScanDesktopEntries parses every .desktop file in dirs; later directories
// shadow earlier ones by entry id (XDG), so the result has one entry per id.
func ScanDesktopEntries(dirs []string) []DesktopEntry {
	byID := map[string]DesktopEntry{}
	var order []string
	for _, dir := range dirs {
		for _, p := range globDesktop(dir) {
			text, err := readText(p)
			if err != nil {
				continue
			}
			e := ParseDesktopEntry(text, p)
			if e == nil {
				continue
			}
			if _, ok := byID[e.EntryID]; !ok {
				order = append(order, e.EntryID)
			}
			byID[e.EntryID] = *e
		}
	}
	out := make([]DesktopEntry, 0, len(order))
	for _, id := range order {
		out = append(out, byID[id])
	}
	return out
}

// RewriteCategories returns text with its [Desktop Entry] Categories key
// replaced (and the generated marker added), changing nothing else: a
// user-level file replaces the system one outright, so MimeType, actions and
// translated names must be carried forward.
func RewriteCategories(text string, categories []string, generatedBy string) string {
	replacement := "Categories=" + strings.Join(categories, ";") + ";"
	marker := GeneratedMarker + "=" + generatedBy
	var out []string
	inEntry, written, insertedMarker := false, false, false

	closeGroup := func() {
		var trailing []string
		for len(out) > 0 && strings.TrimSpace(out[len(out)-1]) == "" {
			trailing = append(trailing, out[len(out)-1])
			out = out[:len(out)-1]
		}
		if !written {
			out = append(out, replacement)
			written = true
		}
		if !insertedMarker {
			out = append(out, marker)
			insertedMarker = true
		}
		for i := len(trailing) - 1; i >= 0; i-- {
			out = append(out, trailing[i])
		}
	}

	for _, line := range splitLines(text) {
		stripped := strings.TrimSpace(line)
		if strings.HasPrefix(stripped, "[") {
			if inEntry {
				closeGroup()
			}
			inEntry = stripped == desktopEntryGroup
			out = append(out, line)
			continue
		}
		if inEntry {
			key := strings.TrimSpace(strings.SplitN(stripped, "=", 2)[0])
			if key == "Categories" {
				if !written {
					out = append(out, replacement)
					written = true
				}
				continue
			}
			if key == GeneratedMarker {
				continue
			}
		}
		out = append(out, line)
	}
	if inEntry {
		closeGroup()
	}
	return strings.Join(out, "\n") + "\n"
}

// RenderOptions are the fields of a freshly rendered entry.
type RenderOptions struct {
	Name, ExecCommand, Comment, Icon string
	Categories                       []string
	Terminal                         bool
	GeneratedBy                      string // default "sarina"
}

// RenderDesktopEntry renders a .desktop file body.
func RenderDesktopEntry(o RenderOptions) string {
	by := o.GeneratedBy
	if by == "" {
		by = "sarina"
	}
	lines := []string{desktopEntryGroup, "Type=Application", "Name=" + o.Name}
	if o.Comment != "" {
		lines = append(lines, "Comment="+o.Comment)
	}
	lines = append(lines, "Exec="+o.ExecCommand)
	if o.Icon != "" {
		lines = append(lines, "Icon="+o.Icon)
	}
	term := "false"
	if o.Terminal {
		term = "true"
	}
	lines = append(lines, "Terminal="+term,
		"Categories="+strings.Join(o.Categories, ";")+";",
		GeneratedMarker+"="+by)
	return strings.Join(lines, "\n") + "\n"
}
