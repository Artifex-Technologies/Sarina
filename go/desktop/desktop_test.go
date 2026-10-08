package desktop

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/analysiscache"
	"github.com/Artifex-Technologies/Sarina/go/cli"
)

func eq(t *testing.T, got, want any, msg string) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%s: got %#v want %#v", msg, got, want)
	}
}

func write(t *testing.T, dir, name, body string) string {
	t.Helper()
	p := filepath.Join(dir, name)
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

// ---- entries ----

const kdeEntry = `[Desktop Entry]
Type=Application
Name=Konsole
Name[de]=Konsole
GenericName=Terminal
Exec=konsole %U
Icon=utilities-terminal
Terminal=false
Categories=Qt;KDE;System;TerminalEmulator;
MimeType=application/x-terminal;

[Desktop Action new-window]
Name=New Window
Exec=konsole --new-tab
`

const serviceEntry = "[Desktop Entry]\nType=Application\nName=Thumbnail Service\nExec=thumbnailer %f\nNoDisplay=true\nCategories=Qt;KDE;\n"

const blackarchEntry = "[Desktop Entry]\nType=Application\nName=burpsuite\nExec=burpsuite\nTerminal=false\n" +
	"Categories=X-BlackArch-Fuzzer;X-BlackArch-Proxy;X-BlackArch-Scanner;X-BlackArch-Webapp;\n"

func TestParseCoreFieldsAndLocaleAndActionGroup(t *testing.T) {
	e := ParseDesktopEntry(kdeEntry, "konsole.desktop")
	eq(t, e.Name, "Konsole", "name")
	eq(t, e.EntryID, "konsole", "id")
	eq(t, e.ExecCommand, "konsole %U", "exec (action Exec must not leak)")
	eq(t, e.Icon, "utilities-terminal", "icon")
	if e.Terminal {
		t.Fatal("terminal false")
	}
	eq(t, e.Categories, []string{"Qt", "KDE", "System", "TerminalEmulator"}, "categories without empty trailing field")
}

func TestNoDisplayHiddenLink(t *testing.T) {
	s := ParseDesktopEntry(serviceEntry, "thumbnailer.desktop")
	if !s.IsApplication() || s.IsMenuVisible() {
		t.Fatal("NoDisplay entry is an application but not visible")
	}
	h := ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=X\nHidden=true\n", "x.desktop")
	if h.IsMenuVisible() {
		t.Fatal("hidden")
	}
	l := ParseDesktopEntry("[Desktop Entry]\nType=Link\nName=Site\nURL=https://example.invalid\n", "l.desktop")
	if l.IsApplication() || l.IsMenuVisible() {
		t.Fatal("link is not an application")
	}
}

func TestParseNoGroupAndCommentsAndEscapesAndMarker(t *testing.T) {
	if ParseDesktopEntry("[Desktop Action foo]\nName=X\n", "a.desktop") != nil || ParseDesktopEntry("", "e.desktop") != nil {
		t.Fatal("no entry group yields nil")
	}
	e := ParseDesktopEntry("# a comment\n\n[Desktop Entry]\nType=Application\nName=Tool\n", "t.desktop")
	eq(t, e.Name, "Tool", "comments ignored")
	e = ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=A\\sB\n", "ab.desktop")
	eq(t, e.Name, "A B", "escape")
	e = ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=X\n"+GeneratedMarker+"=sarina\n", "x.desktop")
	eq(t, e.GeneratedBy, "sarina", "marker")
}

func TestScanShadowingBadFileMissingDir(t *testing.T) {
	root := t.TempDir()
	sys, user := filepath.Join(root, "system"), filepath.Join(root, "user")
	os.Mkdir(sys, 0o755)
	os.Mkdir(user, 0o755)
	write(t, sys, "tool.desktop", "[Desktop Entry]\nType=Application\nName=System Tool\n")
	write(t, user, "tool.desktop", "[Desktop Entry]\nType=Application\nName=User Tool\n")
	es := ScanDesktopEntries([]string{sys, user})
	if len(es) != 1 || es[0].Name != "User Tool" {
		t.Fatalf("%v", es)
	}

	d := t.TempDir()
	write(t, d, "bad.desktop", "not a desktop file")
	write(t, d, "good.desktop", "[Desktop Entry]\nType=Application\nName=Good\n")
	es = ScanDesktopEntries([]string{d})
	if len(es) != 1 || es[0].Name != "Good" {
		t.Fatalf("%v", es)
	}
	if len(ScanDesktopEntries([]string{filepath.Join(root, "nonexistent-xyz")})) != 0 {
		t.Fatal("missing dir yields nothing")
	}
}

func TestRewriteReplacesInPlace(t *testing.T) {
	r := RewriteCategories(blackarchEntry, []string{"Network", "Security"}, "sarina")
	if !strings.Contains(r, "Categories=Network;Security;") || strings.Contains(r, "X-BlackArch-Fuzzer") {
		t.Fatal(r)
	}
}

func TestRewritePreservesMimeActionsLocale(t *testing.T) {
	r := RewriteCategories(kdeEntry, []string{"System"}, "sarina")
	for _, want := range []string{"MimeType=application/x-terminal;", "[Desktop Action new-window]", "Name[de]=Konsole"} {
		if !strings.Contains(r, want) {
			t.Fatalf("lost %q", want)
		}
	}
}

func TestRewriteLeavesActionCategoriesAndAddsMissingKey(t *testing.T) {
	text := "[Desktop Entry]\nType=Application\nName=X\nCategories=Qt;\n\n[Desktop Action alt]\nName=Alt\nCategories=ShouldStay;\n"
	r := RewriteCategories(text, []string{"System"}, "sarina")
	if !strings.Contains(r, "Categories=ShouldStay;") || !strings.Contains(r, "Categories=System;") {
		t.Fatal(r)
	}
	r = RewriteCategories("[Desktop Entry]\nType=Application\nName=X\nExec=x\n", []string{"Utility"}, "sarina")
	if !strings.Contains(r, "Categories=Utility;") {
		t.Fatal(r)
	}
}

func TestRewriteMarkerOnceAndTwice(t *testing.T) {
	once := RewriteCategories(blackarchEntry, []string{"Network"}, "sarina")
	if !strings.Contains(once, GeneratedMarker+"=sarina") {
		t.Fatal("marker")
	}
	twice := RewriteCategories(once, []string{"System"}, "sarina")
	if strings.Count(twice, GeneratedMarker) != 1 || strings.Count(twice, "Categories=") != 1 {
		t.Fatal(twice)
	}
}

func TestRewriteKeysStayAboveGroupSeparator(t *testing.T) {
	lines := strings.Split(RewriteCategories(kdeEntry, []string{"System"}, "sarina"), "\n")
	mi, ai := -1, -1
	for i, l := range lines {
		if strings.HasPrefix(l, GeneratedMarker) {
			mi = i
		}
		if l == "[Desktop Action new-window]" {
			ai = i
		}
	}
	if mi < 0 || mi >= ai || lines[ai-1] != "" {
		t.Fatalf("marker %d action %d", mi, ai)
	}
}

func TestRewriteResultReparses(t *testing.T) {
	e := ParseDesktopEntry(RewriteCategories(blackarchEntry, []string{"Network", "Security"}, "sarina"), "burpsuite.desktop")
	eq(t, e.Categories, []string{"Network", "Security"}, "cats")
	eq(t, e.Name, "burpsuite", "name")
}

func TestRenderReparsesAndOmitsEmptyIcon(t *testing.T) {
	text := RenderDesktopEntry(RenderOptions{Name: "nmap", ExecCommand: "nmap", Categories: []string{"Network", "Security"},
		Comment: "Utility for network discovery and security auditing", Terminal: true})
	e := ParseDesktopEntry(text, "sarina-nmap.desktop")
	eq(t, e.Name, "nmap", "name")
	eq(t, e.ExecCommand, "nmap", "exec")
	eq(t, e.Categories, []string{"Network", "Security"}, "cats")
	if !e.Terminal || !e.IsMenuVisible() || e.GeneratedBy != "sarina" {
		t.Fatalf("%+v", e)
	}
	if strings.Contains(RenderDesktopEntry(RenderOptions{Name: "x", ExecCommand: "x", Categories: []string{"Utility"}}), "Icon=") {
		t.Fatal("icon omitted when empty")
	}
}

func TestApplicationDirsHonoursXDG(t *testing.T) {
	root := t.TempDir()
	a, b := filepath.Join(root, "a"), filepath.Join(root, "b")
	os.MkdirAll(filepath.Join(a, "applications"), 0o755)
	os.MkdirAll(filepath.Join(b, "applications"), 0o755)
	t.Setenv("XDG_DATA_DIRS", strings.Join([]string{a, b, filepath.Join(root, "nope")}, string(os.PathListSeparator)))
	t.Setenv("XDG_DATA_HOME", filepath.Join(root, "home"))
	eq(t, ApplicationDirs(), []string{filepath.Join(a, "applications"), filepath.Join(b, "applications")}, "dirs")
	eq(t, UserApplicationDir(), filepath.Join(root, "home", "applications"), "user dir")
}

// ---- categories ----

func TestGroupMapping(t *testing.T) {
	eq(t, CategoriesFromGroup("blackarch-scanner"), []string{"Network", "Security"}, "group")
	eq(t, CategoriesFromGroup("X-BlackArch-Scanner"), []string{"Network", "Security"}, "token")
	eq(t, CategoriesFromGroup("blackarch-reocn"), []string{"Network", "Security"}, "typo group")
	if CategoriesFromGroup("blackarch") != nil || CategoriesFromGroup("base-devel") != nil {
		t.Fatal("catch-all and foreign groups are ignored")
	}
	if len(BlackArchGroupCategories) != 52 {
		t.Fatalf("52 groups, got %d", len(BlackArchGroupCategories))
	}
}

func TestEveryEmittedCategoryIsRegistered(t *testing.T) {
	check := func(c []string) {
		if !HasMainCategory(c) {
			t.Errorf("no main category in %v", c)
		}
		for _, tok := range c {
			if !MainCategories[tok] && !AdditionalCategories[tok] {
				t.Errorf("unregistered token %q", tok)
			}
		}
	}
	for _, c := range BlackArchGroupCategories {
		check(c)
	}
	for _, r := range DescriptionRules {
		check(r.Categories)
	}
	for _, p := range CategoryPrototypes {
		check(p.Categories)
	}
}

func TestNonstandardTokens(t *testing.T) {
	eq(t, NonstandardTokens([]string{"X-BlackArch-Fuzzer", "Qt", "Network", "Security"}), []string{"X-BlackArch-Fuzzer"}, "tokens")
	if HasMainCategory([]string{"Qt", "KDE"}) {
		t.Fatal("environment tokens are not a location")
	}
}

// ---- packages ----

const pacmanQi = `Name            : abseil-cpp
Version         : 20260817.0-2
Description     : Collection of C++ library code designed to augment the C++ standard library
Architecture    : x86_64
Groups          : None
Install Reason  : Installed as a dependency for another package
Validated By    : Signature

Name            : websockify
Version         : 0.13.0-1
Description     : WebSocket to TCP proxy/bridge.
Groups          : blackarch  blackarch-networking
Install Reason  : Explicitly installed

Name            : nmap
Version         : 7.98-1
Description     : Utility for network discovery and security auditing
Groups          : None
Install Reason  : Explicitly installed
`

const pacmanQl = `nmap /usr/bin/
nmap /usr/bin/nmap
nmap /usr/bin/ncat
nmap /usr/bin/nping
nmap /usr/share/nmap/scripts/http-title.nse
websockify /usr/bin/websockify
abseil-cpp /usr/lib/libabsl_base.so
john /usr/bin/john
john /usr/lib/john/helper.sh
`

func TestParsePacmanInfo(t *testing.T) {
	ps := ParsePacmanInfo(pacmanQi)
	if len(ps) != 3 || ps[0].Name != "abseil-cpp" || ps[1].Name != "websockify" || ps[2].Name != "nmap" {
		t.Fatalf("%+v", ps)
	}
	eq(t, ps[2].Description, "Utility for network discovery and security auditing", "desc")
	if ps[2].Groups != nil {
		t.Fatal("None group is empty")
	}
	eq(t, ps[1].Groups, []string{"blackarch", "blackarch-networking"}, "groups")
	if !ps[2].Explicit || !ps[1].Explicit || ps[0].Explicit {
		t.Fatal("explicit flags")
	}
	if len(ParsePacmanInfo("")) != 0 {
		t.Fatal("empty")
	}
}

func TestParsePacmanInfoContinuation(t *testing.T) {
	p := ParsePacmanInfo("Name            : x\nDescription     : a very long description that pacman\n                  wrapped onto a second line\nGroups          : None\n")[0]
	eq(t, p.Description, "a very long description that pacman wrapped onto a second line", "wrapped")
}

func TestParsePacmanFileList(t *testing.T) {
	b := ParsePacmanFileList(pacmanQl)
	eq(t, b["nmap"], []string{"ncat", "nmap", "nping"}, "nmap")
	eq(t, b["john"], []string{"john"}, "files below bin excluded")
	if _, ok := b["abseil-cpp"]; ok {
		t.Fatal("no binaries -> absent")
	}
}

func TestPrimaryBinary(t *testing.T) {
	eq(t, InstalledPackage{Name: "nmap", Binaries: []string{"ncat", "nmap", "nping"}}.PrimaryBinary(), "nmap", "exact")
	eq(t, InstalledPackage{Name: "john-the-ripper", Binaries: []string{"john", "unshadow"}}.PrimaryBinary(), "john", "shortest")
	eq(t, InstalledPackage{Name: "noto-fonts"}.PrimaryBinary(), "", "none")
}

func TestLooksLikeApplication(t *testing.T) {
	for _, n := range []string{"nmap", "sqlmap", "imagemagick", "git", "wget", "radare2"} {
		if !LooksLikeApplication(InstalledPackage{Name: n, Binaries: []string{n}}) {
			t.Errorf("%s should be an application", n)
		}
	}
	for _, n := range []string{"qt6-base", "qt6-tools", "python-httpx", "libfoo", "gcc-libs", "ttf-dejavu", "noto-fonts",
		"base", "base-devel", "archiso", "linux-headers", "foo-doc", "bar-mirrorlist"} {
		if LooksLikeApplication(InstalledPackage{Name: n, Binaries: []string{"something"}}) {
			t.Errorf("%s should be rejected", n)
		}
	}
	if LooksLikeApplication(InstalledPackage{Name: "sometool"}) {
		t.Fatal("no binaries")
	}
}

func TestQueryInstalledPackagesWithFakePacman(t *testing.T) {
	var calls [][]string
	run := func(name string, args ...string) (string, error) {
		calls = append(calls, append([]string{name}, args...))
		if args[0] == "-Qi" {
			return pacmanQi, nil
		}
		return pacmanQl, nil
	}
	ps := QueryInstalledPackages("pacman", run)
	if len(calls) != 2 {
		t.Fatalf("exactly two subprocess calls for the whole system, got %v", calls)
	}
	eq(t, ps[2].Binaries, []string{"ncat", "nmap", "nping"}, "binaries joined")
	if ps[0].Binaries != nil {
		t.Fatal("abseil has no binaries")
	}
	failing := func(string, ...string) (string, error) { return "", errors.New("boom") }
	if QueryInstalledPackages("pacman", failing) != nil {
		t.Fatal("failure yields nil, never an error")
	}
	if QueryInstalledPackages("definitely-not-a-real-pacman-xyz", nil) != nil {
		t.Fatal("missing pacman yields nil")
	}
}

// ---- propose ----

var realTools = []struct {
	name, desc string
	want       []string
}{
	{"nmap", "Utility for network discovery and security auditing", []string{"Network", "Security"}},
	{"sqlmap", "Automatic SQL injection and database takeover tool", []string{"Network", "Security"}},
	{"nikto", "A web server scanner which performs comprehensive tests against web servers for multiple items", []string{"Network", "Security"}},
	{"hydra", "Very fast network logon cracker which support many different services", []string{"System", "Security"}},
	{"bettercap", "Swiss army knife for network attacks and monitoring", []string{"Network", "Security"}},
	{"radare2", "Open-source tools to disasm, debug, analyze and manipulate binary files", []string{"Development", "Security"}},
	{"john", "John the Ripper password cracker", []string{"System", "Security"}},
	{"hashcat", "Multithreaded advanced password recovery utility", []string{"System", "Security"}},
	{"aircrack-ng", "Key cracker for the 802.11 WEP and WPA-PSK protocols", []string{"Network", "Security"}},
	{"binwalk", "Tool for searching a given binary image for embedded files", []string{"Development", "Security"}},
	{"metasploit", "Advanced open-source platform for developing, testing, and using exploit code", []string{"System", "Security"}},
}

func toolPkg(name, desc string, groups ...string) InstalledPackage {
	return InstalledPackage{Name: name, Description: desc, Groups: groups, Binaries: []string{name}, Explicit: true}
}

func realPackages() []InstalledPackage {
	var ps []InstalledPackage
	for _, r := range realTools {
		ps = append(ps, toolPkg(r.name, r.desc))
	}
	return ps
}

func TestRealToolsLandWithNoModel(t *testing.T) {
	a := AuditMenu(nil, realPackages(), AuditOptions{})
	got := map[string][]string{}
	for _, p := range a.Proposals {
		got[p.Target] = p.Categories
		if p.Confidence != 1.0 || p.Source != "description" {
			t.Errorf("%s: %v %v", p.Target, p.Confidence, p.Source)
		}
	}
	for _, r := range realTools {
		eq(t, got[r.name], r.want, r.name)
	}
	if len(a.Undetermined) != 0 {
		t.Fatalf("undetermined %v", a.Undetermined)
	}
}

func TestWirelessBeatsCrackerForAircrack(t *testing.T) {
	a := AuditMenu(nil, []InstalledPackage{toolPkg("aircrack-ng", realTools[8].desc)}, AuditOptions{})
	eq(t, a.Proposals[0].Categories, []string{"Network", "Security"}, "aircrack")
}

func TestGroupOutranksDescription(t *testing.T) {
	a := AuditMenu(nil, []InstalledPackage{toolPkg("websockify", "WebSocket to TCP proxy/bridge.", "blackarch", "blackarch-networking")}, AuditOptions{})
	eq(t, a.Proposals[0].Categories, []string{"Network", "Security"}, "cats")
	eq(t, a.Proposals[0].Source, "group", "source")
}

func TestRepairStrandedEntry(t *testing.T) {
	e := ParseDesktopEntry(blackarchEntry, "burpsuite.desktop")
	a := AuditMenu([]DesktopEntry{*e}, nil, AuditOptions{})
	eq(t, a.Stranded, []string{"burpsuite"}, "stranded")
	if len(a.Proposals) != 1 {
		t.Fatalf("%v", a.Proposals)
	}
	p := a.Proposals[0]
	eq(t, p.Action, "repair", "action")
	eq(t, p.Categories, []string{"Development", "Security"}, "cats")
	eq(t, p.Source, "group", "source")
}

func TestPlacedEntryLeftAloneAndEnvOnlyIsStranded(t *testing.T) {
	k := ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=Konsole\nCategories=Qt;KDE;System;\n", "konsole.desktop")
	a := AuditMenu([]DesktopEntry{*k}, nil, AuditOptions{})
	if len(a.Stranded) != 0 || len(a.Proposals) != 0 || a.WellPlaced != 1 {
		t.Fatalf("%+v", a)
	}
	env := ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=Some Tool\nCategories=Qt;KDE;\n", "tool.desktop")
	a = AuditMenu([]DesktopEntry{*env}, nil, AuditOptions{})
	eq(t, a.Stranded, []string{"tool"}, "stranded")
}

func TestHiddenEntriesNeverProposed(t *testing.T) {
	h := ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=Svc\nNoDisplay=true\nCategories=Qt;\n", "svc.desktop")
	a := AuditMenu([]DesktopEntry{*h}, nil, AuditOptions{})
	if len(a.Proposals) != 0 || len(a.Stranded) != 0 || a.HiddenEntries != 1 || a.VisibleEntries != 0 {
		t.Fatalf("%+v", a)
	}
}

func TestCoverageFilters(t *testing.T) {
	e := ParseDesktopEntry("[Desktop Entry]\nType=Application\nName=Nmap\nExec=nmap %U\nCategories=Network;\n", "nmap.desktop")
	a := AuditMenu([]DesktopEntry{*e}, []InstalledPackage{toolPkg("nmap", "network scanner")}, AuditOptions{})
	if len(a.UncoveredPackage) != 0 || len(a.Proposals) != 0 {
		t.Fatal("already launched by an entry")
	}

	dep := InstalledPackage{Name: "sometool", Description: "network discovery", Binaries: []string{"sometool"}}
	if len(AuditMenu(nil, []InstalledPackage{dep}, AuditOptions{}).Proposals) != 0 {
		t.Fatal("dependencies skipped by default")
	}
	if len(AuditMenu(nil, []InstalledPackage{dep}, AuditOptions{IncludeDependencies: true}).Proposals) != 1 {
		t.Fatal("include-dependencies")
	}

	lib := InstalledPackage{Name: "qt6-base", Description: "a cross-platform compiler and build system", Binaries: []string{"qmake6"}, Explicit: true}
	if len(AuditMenu(nil, []InstalledPackage{lib}, AuditOptions{}).Proposals) != 0 {
		t.Fatal("libraries skipped by default")
	}
	if len(AuditMenu(nil, []InstalledPackage{lib}, AuditOptions{IncludeLibraries: true}).Proposals) != 1 {
		t.Fatal("include-libraries")
	}

	nobin := InstalledPackage{Name: "sometool", Description: "network discovery", Explicit: true}
	if len(AuditMenu(nil, []InstalledPackage{nobin}, AuditOptions{}).Proposals) != 0 {
		t.Fatal("no binary skipped")
	}
}

func TestBinaryFromExec(t *testing.T) {
	eq(t, BinaryFromExec("/usr/bin/gimp-2.10 %U"), "gimp-2.10", "path and codes")
	eq(t, BinaryFromExec("env GDK_BACKEND=x11 inkscape %f"), "inkscape", "env")
	eq(t, BinaryFromExec("nmap"), "nmap", "bare")
	eq(t, BinaryFromExec(""), "", "empty")
	eq(t, BinaryFromExec("%U"), "", "only code")
	eq(t, BinaryFromExec("env FOO=1"), "", "env with nothing")
}

// ---- semantic ----

// bagModel is a bag-of-words Embedder over a fixed vocabulary; it counts every
// text it embeds (including the prototype set at construction).
type bagModel struct {
	vocab []string
	count int
}

func (m *bagModel) Embed(texts []string) ([][]float64, error) {
	out := make([][]float64, len(texts))
	for i, tx := range texts {
		m.count++
		v := make([]float64, len(m.vocab))
		low := strings.ToLower(tx)
		for j, w := range m.vocab {
			if strings.Contains(low, w) {
				v[j] = 1
			}
		}
		out[i] = v
	}
	return out, nil
}

func classifier(t *testing.T, m Embedder, threshold float64, cache *analysiscache.Cache) *SemanticClassifier {
	t.Helper()
	s, err := NewEmbeddingScorer(m)
	if err != nil {
		t.Fatal(err)
	}
	return &SemanticClassifier{Scorer: s, Threshold: threshold, Cache: cache}
}

func TestSemanticUsedOnlyWhenNoRuleMatches(t *testing.T) {
	m := &bagModel{vocab: []string{"image", "drawing", "network", "compiler", "audio", "forensics"}}
	a := AuditMenu(nil, []InstalledPackage{toolPkg("somedraw", "a program for drawing")}, AuditOptions{Classifier: classifier(t, m, 0.1, nil)})
	if len(a.Proposals) != 1 || a.Proposals[0].Source != "semantic" {
		t.Fatalf("%+v", a)
	}
	found := false
	for _, c := range a.Proposals[0].Categories {
		found = found || c == "Graphics"
	}
	if !found {
		t.Fatal("expected Graphics")
	}
}

func TestDeterministicNotOverriddenBySemantic(t *testing.T) {
	m := &bagModel{vocab: []string{"image", "network"}}
	a := AuditMenu(nil, []InstalledPackage{toolPkg("nmap", realTools[0].desc)}, AuditOptions{Classifier: classifier(t, m, 0.35, nil)})
	if a.Proposals[0].Source != "description" || a.Proposals[0].Confidence != 1.0 {
		t.Fatalf("%+v", a.Proposals[0])
	}
}

func TestBelowThresholdIsUndetermined(t *testing.T) {
	m := &bagModel{vocab: []string{"image", "drawing"}}
	a := AuditMenu(nil, []InstalledPackage{toolPkg("mystery", "zzzz qqqq")}, AuditOptions{Classifier: classifier(t, m, 0.9, nil)})
	if len(a.Proposals) != 0 {
		t.Fatal("no proposals")
	}
	eq(t, a.Undetermined, []string{"mystery"}, "undetermined")
}

func TestNoModelReportsWhy(t *testing.T) {
	if AuditMenu(nil, nil, AuditOptions{}).SemanticUnavailableReason == "" {
		t.Fatal("reason expected")
	}
	if AuditMenu(nil, nil, AuditOptions{Classifier: classifier(t, &bagModel{vocab: []string{"x"}}, 0.5, nil)}).SemanticUnavailableReason != "" {
		t.Fatal("no reason when a classifier is used")
	}
}

// countingScorer proves cache hits skip re-classification.
type countingScorer struct {
	calls int
	label string
	score float64
}

func (c *countingScorer) Score(string) (string, float64, error) {
	c.calls++
	return c.label, c.score, nil
}

func TestCacheHitSkipsReclassification(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cache.json")
	sc := &countingScorer{label: "Graphics", score: 0.8}
	c1 := &SemanticClassifier{Scorer: sc, Threshold: 0.35, Cache: DefaultClassificationCache(path)}
	cats1, s1 := c1.Classify("a program for drawing")
	eq(t, cats1, []string{"Graphics"}, "first")
	eq(t, sc.calls, 1, "first call classifies")

	// A fresh cache instance from the same file simulates a second run.
	c2 := &SemanticClassifier{Scorer: sc, Threshold: 0.35, Cache: DefaultClassificationCache(path)}
	cats2, s2 := c2.Classify("a program for drawing")
	eq(t, sc.calls, 1, "second run served from cache")
	eq(t, cats2, cats1, "same categories")
	eq(t, s2, s1, "same score")

	c2.Classify("a different description")
	eq(t, sc.calls, 2, "new text is classified")
}

func TestThresholdAppliedFreshAfterCacheLookup(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cache.json")
	sc := &countingScorer{label: "Graphics", score: 0.5}
	high := &SemanticClassifier{Scorer: sc, Threshold: 0.95, Cache: DefaultClassificationCache(path)}
	if cats, _ := high.Classify("drawing"); cats != nil {
		t.Fatal("below high threshold")
	}
	low := &SemanticClassifier{Scorer: sc, Threshold: 0.1, Cache: DefaultClassificationCache(path)}
	cats, _ := low.Classify("drawing")
	eq(t, cats, []string{"Graphics"}, "lower threshold changes outcome for cached text")
	eq(t, sc.calls, 1, "and still no re-classification")
}

func TestChangedPrototypeSetInvalidatesCache(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cache.json")
	analysiscache.New(path, "some-older-prototype-set").Set("a program for drawing", map[string]any{"label": "stale", "score": 1.0})
	sc := &countingScorer{label: "Graphics", score: 0.8}
	c := &SemanticClassifier{Scorer: sc, Threshold: 0.1, Cache: DefaultClassificationCache(path)}
	cats, _ := c.Classify("a program for drawing")
	eq(t, sc.calls, 1, "stale entry from another namespace not served")
	eq(t, cats, []string{"Graphics"}, "result")
}

func TestEmbeddingScorerCacheSkipsReEmbedding(t *testing.T) {
	path := filepath.Join(t.TempDir(), "cache.json")
	m := &bagModel{vocab: []string{"image", "drawing", "network", "compiler"}}
	pkg := toolPkg("somedraw", "a program for drawing")
	AuditMenu(nil, []InstalledPackage{pkg}, AuditOptions{Classifier: classifier(t, m, 0.1, DefaultClassificationCache(path))})
	after := m.count
	AuditMenu(nil, []InstalledPackage{pkg}, AuditOptions{Classifier: classifier(t, m, 0.1, DefaultClassificationCache(path))})
	// Only the unavoidable per-run prototype embedding remains.
	eq(t, m.count-after, len(CategoryPrototypes), "description not re-embedded")
}

func TestBlankTextAndScorerErrorAreNoSignal(t *testing.T) {
	c := &SemanticClassifier{Scorer: &countingScorer{label: "Graphics", score: 1}, Threshold: 0}
	if cats, s := c.Classify("   "); cats != nil || s != 0 {
		t.Fatal("blank")
	}
	bad := &SemanticClassifier{Scorer: errScorer{}, Threshold: 0}
	if cats, _ := bad.Classify("x"); cats != nil {
		t.Fatal("error")
	}
}

type errScorer struct{}

func (errScorer) Score(string) (string, float64, error) { return "", 0, errors.New("no model") }

func TestFingerprintStable(t *testing.T) {
	if PrototypesFingerprint() != PrototypesFingerprint() || len(PrototypesFingerprint()) != 64 {
		t.Fatal("fingerprint")
	}
}

// ---- apply / remove ----

func TestApplyWritesParseableEntryAndRemoveUndoes(t *testing.T) {
	a := AuditMenu(nil, []InstalledPackage{toolPkg("nmap", realTools[0].desc)}, AuditOptions{})
	target := t.TempDir()
	written, err := ApplyProposals(a.Proposals, target, 0)
	if err != nil || len(written) != 1 {
		t.Fatalf("%v %v", written, err)
	}
	eq(t, filepath.Base(written[0]), "sarina-nmap.desktop", "create name")
	text, _ := os.ReadFile(written[0])
	e := ParseDesktopEntry(string(text), written[0])
	eq(t, e.Categories, []string{"Network", "Security"}, "cats")
	if !e.Terminal || !e.IsMenuVisible() {
		t.Fatal("terminal and visible")
	}
	removed := RemoveGenerated(target)
	eq(t, removed, written, "removed")
	left, _ := filepath.Glob(filepath.Join(target, "*.desktop"))
	if len(left) != 0 {
		t.Fatal("dir should be empty")
	}
}

func TestRemoveLeavesHandWrittenAlone(t *testing.T) {
	target := t.TempDir()
	p := write(t, target, "mine.desktop", "[Desktop Entry]\nType=Application\nName=Mine\nCategories=Utility;\n")
	if len(RemoveGenerated(target)) != 0 {
		t.Fatal("removed hand-written file")
	}
	if _, err := os.Stat(p); err != nil {
		t.Fatal("still exists")
	}
	if RemoveGenerated(filepath.Join(target, "missing")) != nil {
		t.Fatal("missing dir")
	}
}

func TestMinimumConfidenceSkips(t *testing.T) {
	m := &bagModel{vocab: []string{"image", "drawing", "network"}}
	a := AuditMenu(nil, []InstalledPackage{toolPkg("somedraw", "a program for drawing")}, AuditOptions{Classifier: classifier(t, m, 0.1, nil)})
	w, _ := ApplyProposals(a.Proposals, t.TempDir(), 0.99)
	if len(w) != 0 {
		t.Fatal("below minimum confidence")
	}
}

func TestRepairWritesUnderOriginalIDPreservingEverything(t *testing.T) {
	src := t.TempDir()
	p := write(t, src, "burpsuite.desktop", blackarchEntry+"\n[Desktop Action x]\nName=X\n")
	e := ParseDesktopEntry(blackarchEntry, p)
	a := AuditMenu([]DesktopEntry{*e}, nil, AuditOptions{})
	target := t.TempDir()
	written, _ := ApplyProposals(a.Proposals, target, 0)
	eq(t, filepath.Base(written[0]), "burpsuite.desktop", "name shadows original")
	body, _ := os.ReadFile(written[0])
	if !strings.Contains(string(body), "[Desktop Action x]") || !strings.Contains(string(body), "Categories=Development;Security;") {
		t.Fatalf("a repair is a rewritten COPY of the original:\n%s", body)
	}
	orig, _ := os.ReadFile(p)
	if !strings.Contains(string(orig), "X-BlackArch-Fuzzer") {
		t.Fatal("original untouched")
	}
}

func TestRepairFallsBackToFreshRenderWhenOriginalUnreadable(t *testing.T) {
	p := CategoryProposal{Action: "repair", Target: "x", Name: "x", Categories: []string{"Utility"}, ExecCommand: "x", SourcePath: filepath.Join(t.TempDir(), "gone.desktop")}
	if !strings.Contains(p.Render(), "Exec=x") {
		t.Fatal("fresh render")
	}
}

func TestReportText(t *testing.T) {
	a := AuditMenu(nil, realPackages()[:1], AuditOptions{})
	txt := a.AsText()
	for _, want := range []string{"# Desktop menu audit", "## Create -- installed tool with no desktop entry", "create  nmap", "[description, 1.00]", "Semantic inference unavailable:"} {
		if !strings.Contains(txt, want) {
			t.Errorf("missing %q in\n%s", want, txt)
		}
	}
	if !strings.Contains(AuditMenu(nil, nil, AuditOptions{}).AsText(), "No changes proposed.") {
		t.Fatal("no changes text")
	}
}

// ---- command ----

func runCmd(t *testing.T, d deps, home string, args ...string) (string, string, int) {
	t.Helper()
	var out, errb bytes.Buffer
	env := &cli.Env{Stdout: &out, Stderr: &errb, Home: home}
	code := runAudit(env, args, d)
	return out.String(), errb.String(), code
}

func fakeDeps() deps {
	return deps{
		entries:  func() []DesktopEntry { return nil },
		packages: func() []InstalledPackage { return []InstalledPackage{toolPkg("nmap", realTools[0].desc)} },
	}
}

func TestCommandReportsByDefaultAndWritesNothing(t *testing.T) {
	target := t.TempDir()
	out, _, code := runCmd(t, fakeDeps(), t.TempDir(), "--target-dir", target)
	if code != 0 || !strings.Contains(out, "nmap") || !strings.Contains(out, "Nothing was written") || !strings.Contains(out, "--apply") {
		t.Fatalf("%d\n%s", code, out)
	}
	if left, _ := filepath.Glob(filepath.Join(target, "*")); len(left) != 0 {
		t.Fatal("report-only must not write")
	}
}

func TestCommandApplyThenRemoveGenerated(t *testing.T) {
	target := t.TempDir()
	out, _, _ := runCmd(t, fakeDeps(), t.TempDir(), "--apply", "--target-dir", target)
	if !strings.Contains(out, "wrote "+filepath.Join(target, "sarina-nmap.desktop")) || !strings.Contains(out, "1 entries written. Undo with --remove-generated.") {
		t.Fatal(out)
	}
	out, _, code := runCmd(t, deps{}, t.TempDir(), "--remove-generated", "--target-dir", target)
	if code != 0 || !strings.Contains(out, "removed "+filepath.Join(target, "sarina-nmap.desktop")) || !strings.Contains(out, "1 generated entries removed.") {
		t.Fatal(out)
	}
	if strings.Contains(out, "Desktop menu audit") {
		t.Fatal("--remove-generated runs nothing else")
	}
}

func TestCommandMinConfidenceWithApply(t *testing.T) {
	target := t.TempDir()
	out, _, _ := runCmd(t, fakeDeps(), t.TempDir(), "--apply", "--min-confidence", "2", "--target-dir", target)
	if !strings.Contains(out, "0 entries written.") {
		t.Fatal(out)
	}
}

func TestCommandModelDirUnavailableFallsBackToDeterministic(t *testing.T) {
	out, _, code := runCmd(t, fakeDeps(), t.TempDir(), "--model-dir", "/some/model")
	if code != 0 || !strings.Contains(out, "semantic inference unavailable:") || !strings.Contains(out, "Python helper") || !strings.Contains(out, "nmap") {
		t.Fatal(out)
	}
}

func TestCommandWithHelperUsesCacheAndNoCacheSkipsIt(t *testing.T) {
	old := HelperScorer
	defer func() { HelperScorer = old }()
	sc := &countingScorer{label: "Graphics", score: 0.9}
	HelperScorer = func(string) (Scorer, error) { return sc, nil }
	d := deps{
		entries:  func() []DesktopEntry { return nil },
		packages: func() []InstalledPackage { return []InstalledPackage{toolPkg("somedraw", "a program for drawing")} },
	}
	home := t.TempDir()
	cacheFile := filepath.Join(home, ".sarina", "cache", "desktop_classify.json")

	out, _, _ := runCmd(t, d, home, "--model-dir", "m")
	if !strings.Contains(out, "semantic") {
		t.Fatal(out)
	}
	if _, err := os.Stat(cacheFile); err != nil {
		t.Fatalf("cache file not written at %s", cacheFile)
	}
	runCmd(t, d, home, "--model-dir", "m")
	eq(t, sc.calls, 1, "second run served from cache")

	os.Remove(cacheFile)
	runCmd(t, d, home, "--model-dir", "m", "--no-cache")
	if _, err := os.Stat(cacheFile); err == nil {
		t.Fatal("--no-cache must not write the cache")
	}
}

func TestCommandIsRegistered(t *testing.T) {
	var out, errb bytes.Buffer
	if code := cli.Main([]string{"help"}, cli.Env{Stdout: &out, Stderr: &errb}); code != 0 || !strings.Contains(out.String(), "desktop-audit") {
		t.Fatal(out.String())
	}
}
