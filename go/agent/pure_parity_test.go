package agent

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

func ip(n int) *int { return &n }

// paritySession is buildSessionSized's transcript (context_test.go) without an agent, plus odd shapes: an earlier boundary, a
// snipped message, a continuation request, a replay, a reminder with a kind at index 1, and no system message at all.
func paritySession(rounds, size, userSize int, shape string) *session.Session {
	s := &session.Session{ID: "sid", Model: config.ModelConfig{Model: "m"}, Runtime: config.RuntimeConfig{CWD: "/w"}}
	if shape != "no-system" {
		s.Append(session.Message{Role: "system", Content: "system prompt", MessageID: "system_0"})
	}
	if shape == "kind-reminder" {
		s.Append(session.Message{Role: "user", Content: session.ReminderOpen + "\nreplay\n" + session.ReminderClose, MessageID: "replay_0",
			Metadata: map[string]any{"kind": session.KindFileHistoryReplay}})
	} else {
		s.Append(session.Message{Role: "user", Content: session.ReminderOpen + "\ncontext\n" + session.ReminderClose, MessageID: "user_context_0"})
	}
	if shape == "nested" {
		s.Append(session.Message{Role: "user", Content: "earlier digest " + strings.Repeat("d", 900), MessageID: "compact_boundary_1_4",
			Metadata: map[string]any{"kind": session.KindCompactBoundary, "compaction_depth": 2}})
		s.Append(session.Message{Role: "tool", Name: "probe", ToolCallID: "old", Content: "tomb", MessageID: "tool_x",
			Metadata: map[string]any{"kind": session.KindSnipped}})
	}
	for i := 0; i < rounds; i++ {
		s.Append(session.Message{Role: "user", Content: fmt.Sprintf("request %d please ", i) + strings.Repeat("u", userSize), MessageID: s.NextMessageID("user")})
		am := session.Message{Role: "assistant", Content: fmt.Sprintf("thinking %d", i), MessageID: s.NextMessageID("assistant")}
		n := 1 + i%3
		for k := 0; k < n; k++ {
			am.ToolCalls = append(am.ToolCalls, call(fmt.Sprintf("c%d_%d", i, k)))
		}
		s.Append(am)
		for k := 0; k < n; k++ {
			content := strings.Repeat("r", size)
			if k == 1 {
				content = `{"tool":"edit_file","ok":true,"content":"Edited calc.go (1 replacement). Read back.\nCHECK FAILED (build, go vet ./...):\nx"}` + strings.Repeat(" é", size/2)
			}
			s.Append(session.Message{Role: "tool", Name: "probe", ToolCallID: fmt.Sprintf("c%d_%d", i, k),
				Content: content, MessageID: s.NextMessageID("tool"), Metadata: map[string]any{"tool_name": "probe"}})
		}
		if shape == "continuation" && i == 0 {
			s.Append(session.Message{Role: "user", Content: "Carry on " + strings.Repeat("c", 300), MessageID: s.NextMessageID("continuation"),
				Metadata: map[string]any{"kind": session.KindContinuation}})
		}
		s.Append(session.Message{Role: "assistant", Content: fmt.Sprintf("answer %d ", i) + strings.Repeat("a", userSize), MessageID: s.NextMessageID("assistant")})
	}
	s.Append(session.Message{Role: "user", Content: "latest request", MessageID: s.NextMessageID("user")})
	return s
}

// reparse gives the session as a file reader sees it (numbers in metadata as float64), the form both ports start from.
func reparse(t *testing.T, s *session.Session) *session.Session {
	b, err := s.Marshal()
	if err != nil {
		t.Fatal(err)
	}
	p, err := session.Parse(b)
	if err != nil {
		t.Fatal(err)
	}
	return p
}

func sessJSON(t *testing.T, s *session.Session) string {
	b, err := s.Marshal()
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

// digest is a session file as the fixture keeps it: its sha256 (the files are large; a difference is found by writing both).
func digest(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:])
}

func eventsJSON(evs []event) string {
	b, _ := json.Marshal(evs)
	return string(b)
}

// TestParityAgentPure records the agent's pure helpers (testdata/parity/agentpure.json): token counting and the window,
// the budgets, the system prompt with lessons, guidance and the git snapshot (a stand-in git in bash), curated memory's
// block, clearing tool outputs, snipping and compacting without a model, session notes' coverage and format, the
// transcript rendering and the summary's cleaning. Temporary folders are written as $T.
func TestParityAgentPure(t *testing.T) {
	top := t.TempDir()
	clean := func(s string) string { return strings.ReplaceAll(s, top, "$T") }
	var cases []parity.Case
	add := func(fn string, in, out any) { cases = append(cases, parity.Case{Fn: fn, In: in, Out: out}) }

	texts := []string{"", "a", "abcd", "abcde", "ééééé", "日本語のテキスト", strings.Repeat("x", 4001), "\x00\x01", "🙂🙂🙂"}
	for _, s := range texts {
		add("countTokens", s, CountTokens(s))
	}
	for _, m := range []string{"", "Qwen3-Coder:30b", "devstral", "qwen3:8b", "QWEN", "claude-opus", "llama3", "gpt-4o"} {
		add("inferWindow", m, InferContextWindow(m))
	}
	specs := []llm.ToolSpec{{Name: "read_file", Description: "Read a file <x> & more", Parameters: map[string]any{"type": "object",
		"properties": map[string]any{"path": map[string]any{"type": "string"}}, "required": []any{"path"}}}, {Name: "bare", Description: "no params"}}
	s1 := paritySession(3, 400, 50, "")
	budgets := []config.BudgetConfig{{}, {MaxOutputTokens: ip(500)}, {MaxOutputTokens: ip(-3), MaxInputTokens: ip(100)}, {MaxInputTokens: ip(100000)}}
	for _, w := range []int{0, 1000, 8192, 32768, 200000} {
		for bi, bc := range budgets {
			for _, schema := range []bool{false, true} {
				rt := config.RuntimeConfig{BudgetConfig: bc}
				if schema {
					rt.OutputSchema = &config.OutputSchema{Name: "x"}
				}
				for _, sp := range [][]llm.ToolSpec{nil, specs} {
					snap := TakeSnapshot(w, s1.Messages, sp, rt)
					b, _ := json.Marshal(snap)
					add("snapshot", map[string]any{"window": w, "budget": bi, "schema": schema, "specs": len(sp)}, string(b))
				}
			}
		}
	}
	add("messageTokens", sessJSON(t, s1), []int{MessageTokens(s1.Messages), toolSpecTokens(specs)})
	pr := config.Pricing{InputPerMillion: 3, OutputPerMillion: 15, CacheCreationPerMillion: 3.75, CacheReadPerMillion: 0.3}
	u := llm.Usage{InputTokens: 123456, OutputTokens: 7890, CacheCreationInputTokens: 11, CacheReadInputTokens: 222, ReasoningTokens: 5}
	add("cost", nil, fmt.Sprintf("%.17g", Cost(pr, u)))
	f := 0.0001
	exact := Cost(pr, u)
	for i, b := range []config.BudgetConfig{{}, {MaxTotalTokens: ip(100)}, {MaxInputTokens: ip(1000000), MaxOutputTokens: ip(7000)},
		{MaxReasoningTokens: ip(4)}, {MaxTotalCostUSD: &f}, {MaxToolCalls: ip(2)}, {MaxDelegatedTasks: ip(0)}, {MaxModelCalls: ip(3)},
		{MaxSessionTurns: ip(9)}, {MaxSessionTurns: ip(10), MaxToolCalls: ip(3), MaxTotalTokens: ip(1 << 30)}, {MaxTotalCostUSD: &exact}} {
		c := counters{Usage: u, Cost: Cost(pr, u), ToolCalls: 3, Delegated: 1, ModelCalls: 4, SessionTurns: 10}
		add("checkBudget", i, checkBudget(b, c))
	}
	for _, src := range []string{"flag", "env", "backend:ollama /api/show", "backend:", "inferred", "other"} {
		add("windowSource", src, describeWindowSource(src))
	}

	// the system prompt
	t.Setenv("SHELL", "/usr/bin/zsh")
	lessons := filepath.Join(top, "lessons.md")
	long := strings.Repeat("ab", 140) + "ééééééééééééééééééé"
	body := "# Lessons\n- Read before you write.\n-not a rule\n  - Indented rule\x07 with a bell\n- \xe2\x80\x8bzero width\n- \n- " + long + "\n"
	for i := 0; i < 25; i++ {
		body += fmt.Sprintf("- rule %d\n", i)
	}
	if err := os.WriteFile(lessons, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	open := filepath.Join(top, "open.md")
	os.WriteFile(open, []byte("- x\n"), 0o644)
	os.Chmod(open, 0o666)
	group := filepath.Join(top, "group.md")
	os.WriteFile(group, []byte("- g\n"), 0o644)
	os.Chmod(group, 0o620)
	big := filepath.Join(top, "big.md")
	os.WriteFile(big, []byte(strings.Repeat("- y\n", 20000)), 0o644)
	for _, lf := range []string{"off", lessons, open, group, big, top, filepath.Join(top, "missing.md")} {
		t.Setenv("SARINA_LESSONS_FILE", lf)
		add("lessons", clean(lf), lessonsPart())
	}
	add("readLessonsDefault", nil, readLessons(lessons, false) == nil)
	t.Setenv("SARINA_LESSONS_FILE", lessons)
	env := envInfo{CWD: "/w/proj", IsGit: true, ExtraDirs: []string{"/a", "/b"}, Model: "qwen3:8b"}
	for i, o := range []PromptOptions{{}, {System: "Custom body."}, {Append: "Appended."}, {Override: "Only this."}, {Override: "  ", System: " ", Append: " "}} {
		for j, tn := range [][]string{nil, {"bash"}, {"read_file", "bash"}} {
			for k, p := range []config.Permissions{{}, {AllowFileWrite: true}, {AllowFileWrite: true, AllowShellCommands: true}} {
				b, _ := json.Marshal(BuildSystemPromptParts(o, env, tn, p))
				add("promptParts", []int{i, j, k}, string(b))
			}
		}
	}
	t.Setenv("SHELL", "")
	add("environment", nil, environmentSection(envInfo{CWD: "/x"}))
	var uc, sc session.OrderedMap
	uc.Set("currentDate", "Today's date is 2026-11-12.")
	uc.Set("empty", "  ")
	uc.Set("claudeMd", "guidance <b>")
	sc.Set("k", "v")
	sc.Set("blank", "")
	add("userContext", nil, renderUserContext(uc))
	add("systemMessage", nil, []string{renderSystemMessage([]string{"a", "b"}, sc), renderSystemMessage(nil, nil)})

	// guidance
	home := filepath.Join(top, "home")
	cwd := filepath.Join(top, "proj", "sub")
	extra := filepath.Join(top, "extra")
	for p, c := range map[string]string{
		filepath.Join(home, ".claude", "CLAUDE.md"):             "global",
		filepath.Join(top, "proj", "CLAUDE.md"):                 "proj guidance",
		filepath.Join(top, "proj", ".claude", "CLAUDE.md"):      "  \n ",
		filepath.Join(top, "proj", "CLAUDE.local.md"):           "local\r\n",
		filepath.Join(top, "proj", ".claude", "rules", "b.md"):  "rule b",
		filepath.Join(top, "proj", ".claude", "rules", "a.md"):  "rule a",
		filepath.Join(top, "proj", ".claude", "rules", "c.txt"): "not md",
		filepath.Join(cwd, ".claude", "CLAUDE.md"):              strings.Repeat("é", 40010),
		filepath.Join(extra, "CLAUDE.md"):                       "bad \xff\xfe bytes",
		filepath.Join(extra, ".claude", "rules", "dir.md", "x"): "in a folder",
	} {
		os.MkdirAll(filepath.Dir(p), 0o755)
		if err := os.WriteFile(p, []byte(c), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	os.Symlink(filepath.Join("..", "CLAUDE.md"), filepath.Join(cwd, "CLAUDE.md"))
	os.Mkdir(filepath.Join(cwd, "CLAUDE.local.md"), 0o755)
	add("guidance", nil, []string{clean(discoverGuidance(cwd, []string{extra, cwd}, home)), clean(discoverGuidance(extra, nil, "")),
		clean(discoverGuidance(filepath.Join(top, "nothing"), nil, filepath.Join(top, "nohome")))})

	// the git snapshot, against a stand-in git per case
	gitScripts := map[string]string{
		"not a repo": `case "$*" in *"rev-parse --is-inside-work-tree"*) echo false ;; *) exit 1 ;; esac`,
		"inside":     `case "$*" in *"rev-parse --is-inside-work-tree"*) echo true ;; *"config --local"*) echo core.bare ;; *"rev-parse --abbrev-ref HEAD"*) echo feature ;; *"symbolic-ref"*) echo origin/develop ;; *"config user.name"*) echo Ada ;; *"status --short"*) printf ' M a.go\n?? b.txt\n' ;; *"log --oneline"*) printf 'abc1234 first\ndef5678 second\n' ;; *) exit 1 ;; esac`,
		"risky":      `case "$*" in *"config --local"*) printf 'core.bare\nfilter.lfs.clean\nINCLUDE.path\n' ;; *) echo true ;; esac`,
		"fallbacks":  `case "$*" in *"config --local"*) exit 0 ;; *"rev-parse --abbrev-ref HEAD"*) exit 1 ;; *"symbolic-ref"*) exit 1 ;; *"refs/heads/main"*) exit 1 ;; *"refs/heads/master"*) exit 0 ;; *"config user.name"*) exit 1 ;; *"status --short"*) exit 0 ;; *"log --oneline"*) exit 1 ;; *) exit 1 ;; esac`,
		"both":       `case "$*" in *"config --local"*) exit 0 ;; *"symbolic-ref"*) echo ;; *"refs/heads/"*) exit 0 ;; *) exit 1 ;; esac`,
		"unknown":    `case "$*" in *"config --local"*) exit 0 ;; *"rev-parse --abbrev-ref HEAD"*) echo ;; *"status --short"*) for i in {1..300}; do echo " M dir/fileé$i.go"; done ;; *"log --oneline"*) echo one ;; *) exit 1 ;; esac`,
	}
	for _, name := range []string{"not a repo", "inside", "risky", "fallbacks", "both", "unknown"} {
		dir := filepath.Join(top, "git", strings.ReplaceAll(name, " ", "-"))
		os.MkdirAll(filepath.Join(dir, "bin"), 0o755)
		os.MkdirAll(filepath.Join(dir, "ws"), 0o755)
		if name != "not a repo" {
			os.Mkdir(filepath.Join(dir, "ws", ".git"), 0o755)
		}
		script := "#!/bin/bash\n" + gitScripts[name] + "\n"
		os.WriteFile(filepath.Join(dir, "bin", "git"), []byte(script), 0o755)
		t.Setenv("PATH", filepath.Join(dir, "bin"))
		add("gitSnapshot", map[string]any{"name": name, "script": script}, []any{isGitRepo(filepath.Join(dir, "ws")), clean(gitSnapshot(filepath.Join(dir, "ws")))})
	}

	// curated memory
	mws := filepath.Join(top, "mem", "ws")
	mhome := filepath.Join(top, "mem", "home")
	os.MkdirAll(filepath.Join(mws, ".port_sessions", "memory"), 0o755)
	os.MkdirAll(filepath.Join(mhome, ".sarina", "memory"), 0o755)
	var agentMem []string
	for i := 0; i < 7; i++ {
		agentMem = append(agentMem, fmt.Sprintf("Project rule %d: use cargo for builds number %d", i, i))
	}
	os.WriteFile(filepath.Join(mws, ".port_sessions", "memory", "AGENT_MEMORY.md"), []byte(strings.Join(agentMem, "\n§\n")), 0o644)
	os.WriteFile(filepath.Join(mhome, ".sarina", "memory", "USER.md"), []byte("The owner likes short answers\n§\nPrefers Rust over Go\n§\nUses Hyprland"), 0o644)
	for _, p := range []string{"", "  ", "how do I build with cargo?", "rust please", "nothing in common"} {
		add("curatedMemory", p, curatedMemory(mws, mhome, p))
	}

	// clearing
	clearBase := reparse(t, paritySession(4, 900, 40, ""))
	add("clearSession", nil, sessJSON(t, clearBase))
	for i, st := range []config.ContextSettings{
		{Clear: true, ClearWindowFraction: 0.5, ClearKeepRecent: 2, ClearMinTokens: 50},
		{Clear: true, ClearWindowFraction: 0.01, ClearKeepRecent: 0, ClearMinTokens: 0},
		{Clear: true, ClearWindowFraction: 0.9, ClearKeepRecent: 1, ClearMinTokens: 10},
		{Clear: false, ClearWindowFraction: 0.01},
		{Clear: true, ClearWindowFraction: 0.01, ClearKeepRecent: -4, ClearMinTokens: 100000},
		{Clear: true, ClearWindowFraction: 0.01, ClearKeepRecent: 0, ClearMinTokens: 225},
	} {
		for _, w := range []int{0, 4000, 64000} {
			s := reparse(t, clearBase)
			ev, ok := clearToolOutputs(s, w, 30, st, 7)
			if !ok {
				add("clear", []int{i, w}, nil)
				continue
			}
			s2 := s
			ev2, ok2 := clearToolOutputs(s2, w, 30, st, 8) // idempotent
			add("clear", []int{i, w}, []any{eventsJSON([]event{ev}), digest(sessJSON(t, s)), ok2, eventsJSON([]event{ev2})})
		}
	}
	for _, c := range []string{"", `{"tool":"read_file","content":"package x"}`,
		`{"tool":"edit_file","ok":true,"content":"Edited text_test.go (1 replacement). Read back from disk: it matches.\nCHECK FAILED (build, go vet ./...):\ntext_test.go:22:4: string literal not terminated\n..."}`,
		"Wrote a.txt (12 characters). Check passed: go vet ./... | refused: no | Edited b (2 replacements) Edited c (3 replacements) Wrote d (1 characters)",
		"Wrote a (1 characters)\nWrote b (2 characters)\nWrote c (3 characters)\nWrote d (4 characters)\nWrote e (5 characters)",
		"CHECK FAILED (tests)xxxxxxxxxxxxxxxxxx\\n refused: " + strings.Repeat("z", 200)} {
		add("outcome", c, outcomeLines(c))
	}

	// snipping and compacting without a model
	shapes := []struct {
		rounds, size, user int
		shape              string
	}{{3, 2000, 100, ""}, {6, 800, 2500, ""}, {2, 50, 10, ""}, {4, 600, 700, "nested"}, {3, 900, 400, "continuation"},
		{3, 700, 300, "kind-reminder"}, {3, 700, 300, "no-system"}, {2, 900, 592, ""}}
	opts := []reduceOpts{
		{target: 0, preserve: 4, turn: 3},
		{target: 500, preserve: 0, turn: 3},
		{target: 3000, allowCompaction: true, preserve: 4, turn: 5},
		{target: 0, allowCompaction: true, preserve: 50, turn: 5, notes: "NOTES <kept>"},
		{target: 1 << 30, allowCompaction: true, preserve: 4, turn: 5},
		{target: 0, allowCompaction: true, reactive: true, preserve: 2, turn: 6},
		{target: 200, allowCompaction: true, reactive: true, preserve: 0, turn: 6},
		{target: 1500, allowCompaction: true, preserve: 3, turn: 4},
	}
	for si, sh := range shapes {
		base := reparse(t, paritySession(sh.rounds, sh.size, sh.user, sh.shape))
		if si == 1 {
			setNotesCoverage(base, 9)
		}
		if si == 3 {
			base.PluginState = map[string]any{notesStateKey: map[string]any{"last_summarised_index": 7, "last_summarised_message_id": "nope"}}
		}
		add("session", si, sessJSON(t, base))
		add("prefix", si, []int{protectedPrefix(base.Messages), pairSafeCut(base.Messages, protectedPrefix(base.Messages), 4),
			pairSafeCut(base.Messages, 1, 0), pairSafeCut(base.Messages, 0, 1000), pairSafeCut(base.Messages, 3, 5)})
		for oi, o := range opts {
			s := reparse(t, base)
			changed, evs := reduce(s, o)
			add("reduce", []int{si, oi}, []any{changed, eventsJSON(evs), digest(sessJSON(t, s)), len(s.Messages)})
		}
		cov, ok := notesCoverage(base)
		add("coverage", si, []any{cov, ok})
		b, _ := json.Marshal(groupMessages(base.Messages))
		add("groups", si, len(b))
		add("render", si, []string{renderTranscript(base.Messages, 1500), renderTranscript(base.Messages[:min(3, len(base.Messages))], 5)})
		var tails []int
		for _, mx := range []int{0, 100, 2000, 20000, 1 << 30} {
			tails = append(tails, len(tailWithin(base.Messages, 1500, mx)))
		}
		add("tail", si, tails)
	}
	if b, _ := json.Marshal(groupMessages(nil)); true {
		add("groupsEmpty", nil, string(b))
	}
	cs := reparse(t, paritySession(2, 10, 10, ""))
	add("coverageSession", nil, sessJSON(t, cs))
	for _, c := range []int{-1, 0, 1, 5, 1000} {
		s := reparse(t, cs)
		setNotesCoverage(s, c)
		cov, ok := notesCoverage(s)
		b, _ := json.Marshal(s.PluginState)
		add("setCoverage", c, []any{string(b), cov, ok})
	}
	for _, pc := range [][3]int{{0, 2, 9}, {5, 2, 9}, {9, 2, 9}, {12, 2, 9}, {1, 2, 9}, {3, 3, 3}} {
		add("remap", pc, remapCoverage(pc[0], pc[1], pc[2]))
	}
	for _, c := range []string{"hi", "   ", strings.Repeat("ab ", 100), "a\n\tb  c", "ééé", ""} {
		add("collapse", c, []string{collapse(c, 5), collapse(c, 160)})
	}
	for _, c := range []string{"short", strings.Repeat("é", 10)} {
		add("truncate", c, []string{truncateChars(c, 3), truncateChars(c, 10)})
	}
	for _, h := range []string{"## Task / goal", "### Goal", "**Files touched**", "# current state:", "## Decisions", "## Open questions", "## Questions",
		"## Next steps", "## State", "## Other", "#", ""} {
		add("section", h, sectionIndex(h))
	}
	for _, raw := range []string{validNotes, "```markdown\n# My notes\n\n### Goal\nA\n## current state:\nB\n**Files touched**\nignored heading style\n## Decisions\nD\n```",
		"just some prose\n## Goal\nonly one", "## Goal\ng\n## State\ns", "## Goal\n" + strings.Repeat("g", 9000) + "\n## State\ns\n## Files\nf",
		"## Goal\n" + strings.Repeat("é", 3000) + "  \t\r\n## State\n\n## Files\n  f  \n## Next\nn\n## Next\nmore", ""} {
		out, ok := normaliseNotes(raw)
		add("normalise", raw, []any{out, ok})
	}
	for _, w := range []int{0, 100, 1024, 8192, 1 << 20} {
		add("sideMax", w, []int{sideMaxTokens(w, 2048), sideMaxTokens(w, 3000)})
	}
	for _, s := range []string{okSummary, "<summary>a</summary><summary>b</summary>", "no tags \n\n\n\n here <summary> x",
		"<analysis>\nmulti\nline</analysis>  " + session.ReminderOpen + "body" + session.ReminderClose + "  ", ""} {
		add("cleanSummary", s, cleanSummary(s))
	}
	for _, s := range []string{"Prompt is too long", "context_length_exceeded!", "fine", "the CONTEXT WINDOW is full", "prompt was long"} {
		add("tooLong", s, []bool{replyTooLong(s), errTooLong(errors.New(s)), isPromptTooLong(errors.New(s))})
	}
	add("foldedBody", nil, foldedBody(7, "text", "closing"))
	add("reminder", nil, []string{reminder(planModeNotice), reminder("")})
	parity.Fixture(t, "agentpure", cases)
}
