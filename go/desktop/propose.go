package desktop

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// CategoryProposal is a single proposed menu change.
type CategoryProposal struct {
	Action      string // "repair" or "create"
	Target      string
	Name        string
	Categories  []string
	Source      string // group | description | semantic
	Confidence  float64
	ExecCommand string
	Comment     string
	Terminal    bool
	Icon        string
	// SourcePath is, for a repair, the existing file whose Categories line is rewritten.
	SourcePath string
}

// AsLine is the one-line report form.
func (p CategoryProposal) AsLine() string {
	return fmt.Sprintf("%-7s %-28s -> %-28s [%s, %.2f]",
		p.Action, p.Target, strings.Join(p.Categories, ";"), p.Source, p.Confidence)
}

// Render returns the .desktop body this proposal would write. A repair rewrites
// one line of a copy of the original so nothing else is lost; a creation (or a
// repair whose original cannot be read) renders a fresh entry.
func (p CategoryProposal) Render() string {
	if p.Action == "repair" && p.SourcePath != "" {
		if original, err := readText(p.SourcePath); err == nil && original != "" {
			return RewriteCategories(original, p.Categories, "sarina")
		}
	}
	return RenderDesktopEntry(RenderOptions{
		Name: p.Name, ExecCommand: p.ExecCommand, Categories: p.Categories,
		Comment: p.Comment, Icon: p.Icon, Terminal: p.Terminal,
	})
}

// MenuAudit is what the menu looks like now, and what would change.
type MenuAudit struct {
	TotalEntries     int
	VisibleEntries   int
	HiddenEntries    int
	WellPlaced       int
	Stranded         []string
	UncoveredPackage []string
	Proposals        []CategoryProposal
	Undetermined     []string
	// SemanticUnavailableReason is set when no classifier was used.
	SemanticUnavailableReason string
}

// AsText renders the human-readable report (no trailing newline).
func (a MenuAudit) AsText() string {
	lines := []string{
		"# Desktop menu audit",
		"",
		fmt.Sprintf("desktop entries found:   %d", a.TotalEntries),
		fmt.Sprintf("  menu-visible:          %d", a.VisibleEntries),
		fmt.Sprintf("  hidden (NoDisplay):    %d  (left alone)", a.HiddenEntries),
		fmt.Sprintf("  correctly placed:      %d", a.WellPlaced),
		fmt.Sprintf("  stranded (no menu):    %d", len(a.Stranded)),
		fmt.Sprintf("packages with no entry:  %d", len(a.UncoveredPackage)),
		"",
	}
	var repairs, creations []CategoryProposal
	for _, p := range a.Proposals {
		if p.Action == "repair" {
			repairs = append(repairs, p)
		} else if p.Action == "create" {
			creations = append(creations, p)
		}
	}
	if len(repairs) > 0 {
		lines = append(lines, "## Repair -- entry exists but has no menu location")
		for _, p := range repairs {
			lines = append(lines, "- "+p.AsLine())
		}
		lines = append(lines, "")
	}
	if len(creations) > 0 {
		lines = append(lines, "## Create -- installed tool with no desktop entry")
		for _, p := range creations {
			lines = append(lines, "- "+p.AsLine())
		}
		lines = append(lines, "")
	}
	if len(a.Undetermined) > 0 {
		lines = append(lines, "## Undetermined -- no category signal, left alone",
			strings.Join(a.Undetermined, ", "), "")
	}
	if len(a.Proposals) == 0 {
		lines = append(lines, "No changes proposed.", "")
	}
	if a.SemanticUnavailableReason != "" {
		lines = append(lines, "Semantic inference unavailable: "+a.SemanticUnavailableReason)
	}
	return strings.TrimRight(strings.Join(lines, "\n"), " \t\r\n")
}

var fieldCodes = map[string]bool{}

func init() {
	for _, c := range strings.Fields("%f %F %u %U %d %D %n %N %i %c %k %v %m") {
		fieldCodes[c] = true
	}
}

func lastSegment(s string) string {
	if i := strings.LastIndex(s, "/"); i >= 0 {
		return s[i+1:]
	}
	return s
}

// BinaryFromExec is the bare command name an Exec line runs: field codes, env
// wrappers (with VAR=value assignments) and directory parts are stripped.
func BinaryFromExec(exec string) string {
	var tokens []string
	for _, t := range strings.Fields(exec) {
		if !fieldCodes[t] && !strings.HasPrefix(t, "%") {
			tokens = append(tokens, t)
		}
	}
	if len(tokens) == 0 {
		return ""
	}
	i := 0
	if lastSegment(tokens[0]) == "env" {
		i++
		for i < len(tokens) && strings.Contains(tokens[i], "=") {
			i++
		}
	}
	if i >= len(tokens) {
		return ""
	}
	return lastSegment(tokens[i])
}

func deterministicCategories(groups, tokens []string, description string) ([]string, string) {
	for _, g := range append(append([]string{}, groups...), tokens...) {
		if m := CategoriesFromGroup(g); m != nil {
			return m, "group"
		}
	}
	if m := CategoriesFromDescription(description); m != nil {
		return m, "description"
	}
	return nil, ""
}

// AuditOptions tunes AuditMenu.
type AuditOptions struct {
	// Classifier is the optional semantic fallback; nil means deterministic only.
	Classifier          Classifier
	IncludeDependencies bool
	IncludeLibraries    bool
	// UnavailableReason replaces the default reason shown when Classifier is nil.
	UnavailableReason string
}

// AuditMenu audits the menu and proposes repairs and additions. packages may be
// empty (no pacman): the repair half needs only the entries. Only explicitly
// installed, application-shaped packages are considered for new entries.
func AuditMenu(entries []DesktopEntry, packages []InstalledPackage, opt AuditOptions) MenuAudit {
	var visible []DesktopEntry
	hidden := 0
	for _, e := range entries {
		if e.IsMenuVisible() {
			visible = append(visible, e)
		} else if e.IsApplication() {
			hidden++
		}
	}
	reason := ""
	if opt.Classifier == nil {
		reason = opt.UnavailableReason
		if reason == "" {
			reason = "no model directory given; deterministic signals only"
		}
	}
	descriptions := map[string]string{}
	for _, p := range packages {
		descriptions[p.Name] = p.Description
	}

	var proposals []CategoryProposal
	var stranded, undetermined []string

	for _, e := range visible {
		if HasMainCategory(e.Categories) {
			continue
		}
		stranded = append(stranded, e.EntryID)
		text := strings.TrimSpace(e.Name + " " + descriptions[e.EntryID])
		cs, source := deterministicCategories(nil, NonstandardTokens(e.Categories), text)
		conf := 1.0
		if cs == nil && opt.Classifier != nil {
			cs, conf = opt.Classifier.Classify(text)
			source = "semantic"
		}
		if cs == nil {
			undetermined = append(undetermined, e.EntryID)
			continue
		}
		name := e.Name
		if name == "" {
			name = e.EntryID
		}
		proposals = append(proposals, CategoryProposal{
			Action: "repair", Target: e.EntryID, Name: name, Categories: cs, Source: source,
			Confidence: conf, ExecCommand: e.ExecCommand, Terminal: e.Terminal, Icon: e.Icon,
			SourcePath: e.Path,
		})
	}

	covered := map[string]bool{}
	for _, e := range visible {
		if b := BinaryFromExec(e.ExecCommand); b != "" {
			covered[b] = true
		}
	}
	var uncovered []string
	for _, p := range packages {
		if !p.Explicit && !opt.IncludeDependencies {
			continue
		}
		if !opt.IncludeLibraries && !LooksLikeApplication(p) {
			continue
		}
		binary := p.PrimaryBinary()
		if binary == "" || covered[binary] {
			continue
		}
		uncovered = append(uncovered, p.Name)
		cs, source := deterministicCategories(p.Groups, nil, p.Description)
		conf := 1.0
		if cs == nil && opt.Classifier != nil {
			cs, conf = opt.Classifier.Classify(p.Description)
			source = "semantic"
		}
		if cs == nil {
			undetermined = append(undetermined, p.Name)
			continue
		}
		proposals = append(proposals, CategoryProposal{
			Action: "create", Target: p.Name, Name: p.Name, Categories: cs, Source: source,
			Confidence: conf, ExecCommand: binary, Comment: p.Description, Terminal: true,
		})
	}

	return MenuAudit{
		TotalEntries: len(entries), VisibleEntries: len(visible), HiddenEntries: hidden,
		WellPlaced: len(visible) - len(stranded), Stranded: stranded, UncoveredPackage: uncovered,
		Proposals: proposals, Undetermined: undetermined, SemanticUnavailableReason: reason,
	}
}

// ApplyProposals writes proposals as .desktop files into targetDir (default:
// the user application directory) and returns the paths written. Proposals
// below minConfidence are skipped. The caller must have asked the operator.
func ApplyProposals(proposals []CategoryProposal, targetDir string, minConfidence float64) ([]string, error) {
	if targetDir == "" {
		targetDir = UserApplicationDir()
	}
	if err := os.MkdirAll(targetDir, 0o755); err != nil {
		return nil, err
	}
	var written []string
	for _, p := range proposals {
		if p.Confidence < minConfidence {
			continue
		}
		stem := p.Target
		if p.Action != "repair" {
			stem = "sarina-" + p.Target
		}
		path := filepath.Join(targetDir, stem+".desktop")
		if err := os.WriteFile(path, []byte(p.Render()), 0o644); err != nil {
			return written, err
		}
		written = append(written, path)
	}
	return written, nil
}

// RemoveGenerated deletes only entries carrying the generated marker, reading
// each file rather than trusting its name, so hand-written files are left alone.
func RemoveGenerated(targetDir string) []string {
	if targetDir == "" {
		targetDir = UserApplicationDir()
	}
	if st, err := os.Stat(targetDir); err != nil || !st.IsDir() {
		return nil
	}
	var removed []string
	for _, p := range globDesktop(targetDir) {
		text, err := readText(p)
		if err != nil {
			continue
		}
		if e := ParseDesktopEntry(text, p); e != nil && e.GeneratedBy != "" {
			if os.Remove(p) == nil {
				removed = append(removed, p)
			}
		}
	}
	return removed
}
