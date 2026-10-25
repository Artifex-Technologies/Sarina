package skills

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// longSkill is a skill far over the prompt limit: an introduction and sections about different things.
func longSkill() string {
	var b strings.Builder
	b.WriteString("---\nname: fixit\ndescription: Fixing a Linux desktop\nmodel_invocable: true\n---\nHow to find and fix problems.\n\n")
	topics := []string{"Sound", "Wi-Fi", "Bluetooth", "Printing"}
	for _, t := range topics {
		fmt.Fprintf(&b, "## %s\n\n", t)
		for i := 0; b.Len() < (len(topics)+1)*30<<10 && i < 400; i++ {
			fmt.Fprintf(&b, "When %s fails, step %d: check the %s service and its log.\n\n", strings.ToLower(t), i, strings.ToLower(t))
			if b.Len() > 30<<10*(indexOf(topics, t)+1) {
				break
			}
		}
	}
	b.WriteString("## PipeWire\n\nIf no sound comes out, restart pipewire and wireplumber: systemctl --user restart pipewire wireplumber.\n")
	return b.String()
}

func indexOf(list []string, s string) int {
	for i, x := range list {
		if x == s {
			return i
		}
	}
	return -1
}

func TestALongSkillIsSplitIntoSectionsNotRefused(t *testing.T) {
	e := newEnv(t)
	text := longSkill()
	if len(text) <= maxSkillBytes {
		t.Fatalf("the test skill is only %d bytes", len(text))
	}
	write(t, filepath.Join(e.home, ".sarina", "skills", "fixit.md"), text)
	c := Discover(e.opts(false))
	s, ok := c.Get("fixit")
	if !ok {
		t.Fatalf("not loaded: %v", c.Problems)
	}
	if !s.IsLibrary() || len(s.Body) > maxSkillBytes {
		t.Fatalf("library %v, body %d bytes", s.IsLibrary(), len(s.Body))
	}
	if !strings.Contains(s.Body, "How to find and fix problems.") || !strings.Contains(s.Body, "skill_section") || !strings.Contains(s.Body, "$ARGUMENTS") {
		t.Fatalf("the prompt has the introduction, the way to read on, and the task: %q", s.Body[:200])
	}
	for _, sec := range s.Sections {
		if len(sec.Text) > maxSectionBytes {
			t.Fatalf("section %q is %d bytes", sec.Title, len(sec.Text))
		}
	}
	n, sec, ok := s.Section("pipewire")
	if !ok || !strings.Contains(sec.Text, "wireplumber") || n != len(s.Sections) {
		t.Fatalf("by title: %d %v", n, ok)
	}
	if _, sec, ok := s.Section("1"); !ok || !strings.HasPrefix(sec.Title, "Sound") {
		t.Fatalf("by number: %+v", sec)
	}
	if _, _, ok := s.Section("99999"); ok {
		t.Fatal("a section that is not there")
	}
}

func TestASkillFolderMayKeepItsSectionsAsFiles(t *testing.T) {
	e := newEnv(t)
	dir := filepath.Join(e.home, ".sarina", "skills", "netfix")
	write(t, filepath.Join(dir, "SKILL.md"), "---\ndescription: network\n---\nStart with the basics.\n")
	write(t, filepath.Join(dir, "sections", "01-wifi.md"), "# Wi-Fi drops\n\nnmcli device wifi rescan")
	write(t, filepath.Join(dir, "sections", "02-dns.md"), "# DNS\n\nresolvectl status")
	s, ok := Discover(e.opts(false)).Get("netfix")
	if !ok || len(s.Sections) != 2 || s.Sections[1].Title != "DNS" {
		t.Fatalf("%+v", s)
	}
	if !strings.HasPrefix(s.Body, "Start with the basics.") {
		t.Fatalf("the authored text stays whole: %q", s.Body)
	}
}

func TestSearchByWordsWithoutTheModel(t *testing.T) {
	e := newEnv(t)
	write(t, filepath.Join(e.home, ".sarina", "skills", "fixit.md"), longSkill())
	c := Discover(e.opts(false))
	lib := NewLibrary(c, nil, "")
	found, how, err := lib.Search("fixit", "no sound, restart wireplumber", 3)
	if err != nil || len(found) == 0 {
		t.Fatal(err, found)
	}
	if !strings.Contains(found[0].Text, "wireplumber") || how != "by words" {
		t.Fatalf("%q %s", found[0].Text, how)
	}
	if _, _, err := lib.Search("nope", "x", 3); err == nil {
		t.Fatal("an unknown skill is an error")
	}
}

// fakeEmbedder makes a vector from which of a few words a text has, so meaning is testable without a model.
type fakeEmbedder struct {
	calls *int
	fail  bool
}

var fakeWords = []string{"sound", "wi-fi", "bluetooth", "printing", "wireplumber", "pipewire"}

func (f fakeEmbedder) Name() string { return "fake" }
func (f fakeEmbedder) Embed(texts []string) ([][]float32, error) {
	if f.fail {
		return nil, errors.New("no model")
	}
	*f.calls += len(texts)
	out := make([][]float32, len(texts))
	for i, t := range texts {
		v := make([]float32, 8)
		low := strings.ToLower(t)
		for k, w := range fakeWords {
			if strings.Contains(low, w) {
				v[k] = 1
			}
		}
		v[7] = 0.01
		out[i] = v
	}
	return out, nil
}

func TestSearchByMeaningIsIndexedOnceAndKept(t *testing.T) {
	e := newEnv(t)
	write(t, filepath.Join(e.home, ".sarina", "skills", "fixit.md"), longSkill())
	c := Discover(e.opts(false))
	calls := 0
	dir := t.TempDir()
	lib := NewLibrary(c, fakeEmbedder{calls: &calls}, dir)
	found, how, err := lib.Search("", "pipewire wireplumber", 2)
	if err != nil || how != "by meaning" || len(found) == 0 || !strings.Contains(found[0].Text, "wireplumber") {
		t.Fatalf("%v %s %+v", err, how, found)
	}
	first := calls
	// a new library (a new run) loads the kept index: only the question is embedded
	calls = 0
	again := NewLibrary(c, fakeEmbedder{calls: &calls}, dir)
	if _, _, err := again.Search("fixit", "pipewire", 2); err != nil || calls != 1 {
		t.Fatalf("embedded %d texts again (first time %d)", calls, first)
	}
	// without the model it still answers, by words
	broken := NewLibrary(c, fakeEmbedder{fail: true}, "")
	if found, how, _ := broken.Search("fixit", "wireplumber", 2); len(found) == 0 || !strings.HasPrefix(how, "by words") {
		t.Fatalf("%s %v", how, found)
	}
	if _, err := os.Stat(dir); err != nil {
		t.Fatal(err)
	}
}

func TestTheToolsReadAndFindSections(t *testing.T) {
	e := newEnv(t)
	write(t, filepath.Join(e.home, ".sarina", "skills", "fixit.md"), longSkill())
	write(t, filepath.Join(e.home, ".sarina", "skills", "short.md"), good)
	c := Discover(e.opts(false))
	lib := NewLibrary(c, nil, "")
	ts := LibraryTools(c, lib)
	if len(ts) != 2 {
		t.Fatalf("%d tools", len(ts))
	}
	reg := tools.NewRegistry(ts...)
	res := reg.Execute(context.Background(), SectionToolName, map[string]any{"skill": "fixit", "section": "PipeWire"}, nil)
	if !res.OK || !strings.Contains(res.Content, "wireplumber") {
		t.Fatalf("%+v", res)
	}
	res = reg.Execute(context.Background(), SectionToolName, map[string]any{"skill": "triage", "section": "1"}, nil)
	if res.OK {
		t.Fatal("a short skill has no sections")
	}
	res = reg.Execute(context.Background(), SearchToolName, map[string]any{"query": "bluetooth service"}, nil)
	if !res.OK || !strings.Contains(strings.ToLower(res.Content), "bluetooth") {
		t.Fatalf("%+v", res)
	}
	if LibraryTools(Discover(newEnv(t).opts(false)), nil) != nil {
		t.Fatal("no long skills, no tools")
	}
}

func TestRunningALongSkillWithATaskBringsTheRightPassages(t *testing.T) {
	e := newEnv(t)
	write(t, filepath.Join(e.home, ".sarina", "skills", "fixit.md"), longSkill())
	s, _ := Discover(e.opts(false)).Get("fixit")
	lib := NewLibrary(Discover(e.opts(false)), nil, "")
	text := s.ExpandWith("there is no sound from pipewire", lib)
	if !strings.Contains(text, "wireplumber") || !strings.Contains(text, "there is no sound") {
		t.Fatalf("%q", text[len(text)-300:])
	}
	if len(text) > maxSkillBytes+passageBudget+1024 {
		t.Fatalf("%d bytes", len(text))
	}
	if s.ExpandWith("", lib) != s.Expand("") {
		t.Fatal("no task, no passages")
	}
}

func TestASkillFolderWithoutHeaderStillLoads(t *testing.T) {
	e := newEnv(t)
	write(t, filepath.Join(e.home, ".sarina", "skills", "oauth-checks", "SKILL.md"), "# OAuth: checking a login flow\n\nLook at the redirect URIs first.\n")
	write(t, filepath.Join(e.home, ".sarina", "skills", "loose.md"), "just text")
	c := Discover(e.opts(false))
	s, ok := c.Get("oauth-checks")
	if !ok || s.Description != "OAuth - checking a login flow" || s.ModelInvocable {
		t.Fatalf("%+v %v", s, c.Problems)
	}
	if _, ok := c.Get("loose"); ok {
		t.Fatal("a loose file still needs its header")
	}
}

func TestPrefetchedContextGoesFirst(t *testing.T) {
	RegisterContext("test-status", func(task string) string { return "The sound card reports: muted (for " + task + ")" })
	s, err := Parse([]byte("---\nname: p\nprefetch: [test-status, unknown]\n---\nFix: $ARGUMENTS"), "p")
	if err != nil {
		t.Fatal(err)
	}
	got := s.ExpandWith("no sound", nil)
	if !strings.HasPrefix(got, "The sound card reports: muted (for no sound)") || !strings.HasSuffix(got, "Fix: no sound") {
		t.Fatalf("%q", got)
	}
	if s.ExpandWith("", nil) != "Fix: " {
		t.Fatal("no task, nothing looked up")
	}
}
