package agent

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

func TestUnbackedFileClaim(t *testing.T) {
	wrote := []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/todo.md"}}
	read := []map[string]any{{"tool_name": "read_file", "ok": true, "path": "/w/todo.md"}}
	failed := []map[string]any{{"tool_name": "write_file", "ok": false, "path": "/w/todo.md"}}
	cases := []struct {
		answer string
		hist   []map[string]any
		want   string
	}{
		{"I added 'water the plants' to todo.md.", nil, "todo.md"},
		{"I added 'water the plants' to todo.md.", read, "todo.md"},
		{"I added 'water the plants' to todo.md.", failed, "todo.md"},
		{"I added 'water the plants' to todo.md.", wrote, ""},
		{"notes.txt says the dock is pinned. I did not change notes.txt.", nil, ""},
		{"The summary of notes.txt: Dana finishes the icons by Friday.", nil, ""},
		{"Nothing was changed in config.env.", nil, ""},
	}
	for _, c := range cases {
		if got := unbackedFileClaim(c.answer, c.hist); got != c.want {
			t.Errorf("%q: got %q want %q", c.answer, got, c.want)
		}
	}
}

func TestUnreportedChange(t *testing.T) {
	h := []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/also.txt"}}
	if got := unreportedChange("I cannot delete files. Would you like me to clear them?", h); got != "also.txt" {
		t.Fatalf("got %q", got)
	}
	if got := unreportedChange("I emptied also.txt.", h); got != "" {
		t.Fatalf("got %q", got)
	}
	if got := unreportedChange("Done.", nil); got != "" {
		t.Fatalf("got %q", got)
	}
}

func TestUngroundedQuote(t *testing.T) {
	tool := []session.Message{{Role: "tool", Content: "check_code: if (len == 6 && s[0] == 'L' && s[5] == '7') puts(\"access granted: labyrinth\")"}}
	if q := ungroundedQuote("The correct input is `admin123`.", "explain demo.bin", tool); q != "admin123" {
		t.Fatalf("got %q", q)
	}
	if q := ungroundedQuote("It prints \"access granted: labyrinth\".", "explain demo.bin", tool); q != "" {
		t.Fatalf("a real quote was flagged: %q", q)
	}
	if q := ungroundedQuote("Say `admin123` to me.", "hi", nil); q != "" {
		t.Fatal("without tools nothing is checked")
	}
	if q := ungroundedQuote("Read `/tmp/x/demo.bin` again.", "p", tool); q != "" {
		t.Fatal("paths are not quotes from evidence")
	}
}

func TestQuotesAreNotMispaired(t *testing.T) {
	tool := []session.Message{{Role: "tool", Content: "if (s[0] == 'L' && s[5] == '7') return 1;"}}
	ans := `It must start with "L" and end with "7" to pass.`
	if q := ungroundedQuote(ans, "p", tool); q == " and end with " {
		t.Fatal("the text between two quotes was taken for a quote")
	}
}

func TestClaimsToolWork(t *testing.T) {
	if !claimsToolWork("The reverse engineering tool analyzed demo.bin and found validate_input.", "p", nil) {
		t.Fatal("made-up tool work must be caught")
	}
	if claimsToolWork("The reverse engineering tool analyzed demo.bin.", "p", []session.Message{{Role: "tool", Content: "x"}}) {
		t.Fatal("real tool work must pass")
	}
	if claimsToolWork("Paris is the capital of France.", "p", nil) || claimsToolWork("I checked: memory is fine.", "What the computer reports right now (read-only, already looked up for you", nil) {
		t.Fatal("ordinary answers must pass")
	}
}

func TestTaskInsteadOfFile(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "todo.md"), []byte("- [ ] milk\n"), 0o644)
	used := []session.Message{{Role: "assistant", ToolCalls: []llm.WireToolCall{{Function: llm.FunctionCall{Name: "task_create"}}}}}
	if f := taskInsteadOfFile("Add an item to the todo list in todo.md", dir, used, nil); f != "todo.md" {
		t.Fatalf("got %q", f)
	}
	wrote := []map[string]any{{"tool_name": "edit_file", "ok": true, "path": filepath.Join(dir, "todo.md")}}
	if f := taskInsteadOfFile("Add an item to the todo list in todo.md", dir, used, wrote); f != "" {
		t.Fatalf("got %q", f)
	}
	if f := taskInsteadOfFile("Add an item to the todo list in todo.md", dir, nil, nil); f != "" {
		t.Fatal("no planning tool, nothing to say")
	}
}

func TestSuggestsSecurityOff(t *testing.T) {
	for _, s := range []string{"Disable ufw.service (firewall) if not needed.", "You could turn off Heimdall while gaming.", "Stop the clamav freshclam service."} {
		if suggestsSecurityOff(s) == "" {
			t.Fatalf("missed: %s", s)
		}
	}
	for _, s := range []string{"Keep the firewall on.", "Disable the splash screen to boot faster.", "The firewall is active."} {
		if got := suggestsSecurityOff(s); got != "" {
			t.Fatalf("false alarm %q on %s", got, s)
		}
	}
}

func TestCommandsMustComeFromTheReportWhenItSaysSo(t *testing.T) {
	fix := `printf 'KERNEL=="kvm"\n' | sudo tee /etc/udev/rules.d/65-kvm.rules && sudo usermod -aG kvm $USER && sudo udevadm control --reload`
	prompt := "### Summary\n- Package lua51: severity LOW\n- HELD /x: confirm it (sudo heimdalld immunity confirm 5f5f5f5f5f5f)\n\nCommands to copy exactly, whole:\n- kvm-world-writable:\n  " + fix
	for answer, want := range map[string]string{
		"Run `" + fix + "`.": "",
		"Run `sudo heimdalld immunity confirm 5f5f5f5f5f5f`":           "",
		"Then `sudo pacman -Syu` and `pacman -Qi lua51`.":              "",
		"Update it: `sudo pacman -Syu lua51`":                          "sudo pacman -Syu lua51",
		"`sudo udevadm trigger --everything`":                          "sudo udevadm trigger --everything",
		"Release it with `sudo heimdalld immunity release deadbeef00`": "sudo heimdalld immunity release deadbeef00",
	} {
		if got := unsourcedCommand(answer, prompt, nil); got != want {
			t.Errorf("%q: got %q, want %q", answer, got, want)
		}
	}
	if unsourcedCommand("`sudo pacman -Syu lua51`", "a chat without a report", nil) != "" {
		t.Error("other tasks are not checked")
	}
}

func TestInventedHistoryIsCaught(t *testing.T) {
	sys := session.Message{Role: "system", Content: "You are Sarina. Curated memory: the owner likes strong coffee."}
	ask := func(q string) []session.Message { return []session.Message{sys, {Role: "user", Content: q}} }
	// nothing said about breakfast before: invented
	all := ask("What did I have for breakfast this morning?")
	if got := inventedHistory("You had avocado toast, as you told me last week. Enjoy!", all, 1); !strings.Contains(got, "avocado") {
		t.Fatalf("got %q", got)
	}
	// the user's own false premise in this question is not evidence
	all = ask("You told me yesterday that my processor was running at 300 degrees. Is that still true?")
	if got := inventedHistory("Nothing is overheating; those 300 degree figures were a joke from earlier.", all, 1); got == "" {
		t.Fatal("a joke 'from earlier' that never happened")
	}
	// grounded in memory: fine
	all = ask("How do I take my coffee?")
	if got := inventedHistory("Strong, as you told me before.", all, 1); got != "" {
		t.Fatalf("memory backs it: %q", got)
	}
	// grounded in an earlier turn of this conversation: fine
	all = []session.Message{sys, {Role: "user", Content: "I'm learning to draw with charcoal."}, {Role: "assistant", Content: "Nice."}, {Role: "user", Content: "Any tips?"}}
	if got := inventedHistory("Since you mentioned charcoal, try blending with your finger.", all, 3); got != "" {
		t.Fatalf("an earlier turn backs it: %q", got)
	}
	// "as you said" pointing at the question itself: fine
	if got := inventedHistory("As you said, Firefox has no sound; Spotify plays.", ask("Firefox plays no sound but Spotify does"), 1); got != "" {
		t.Fatalf("the question backs it: %q", got)
	}
	// no claim of the past: nothing to check
	if got := inventedHistory("I don't know what you had for breakfast.", ask("What did I eat?"), 1); got != "" {
		t.Fatal(got)
	}
}

func TestTheDefaultOutputIsLeftAloneWhenAnotherAppPlays(t *testing.T) {
	if !defaultOutputForOneApp("3. If using the wrong sink: `wpctl set-default 52`", "Firefox plays no sound but Spotify does") {
		t.Fatal("caught")
	}
	if defaultOutputForOneApp("Run `wpctl set-default 52` to use the headphones.", "Nothing plays any sound since I plugged in headphones") {
		t.Fatal("no other program plays: set-default may be right")
	}
}

func TestUngroundedQuoteIgnoresTheQuoteMarks(t *testing.T) {
	tool := []session.Message{{Role: "tool", Content: `{"tool":"read_file","ok":true,"content":"assert!(protected(\"yggdrasil-minos\"))"}`}}
	if q := ungroundedQuote("It protects `\"yggdrasil-minos\"` for example.", "which prefixes?", tool); q != "" {
		t.Fatalf("a quote that is in the output, with its own quote marks, was flagged: %q", q)
	}
}

func TestUngroundedIdentifier(t *testing.T) {
	tool := []session.Message{{Role: "tool", Content: "kernel 6.18.55-2-cachyos-lts has CVE-2026-72493 (9.9), fixed in 6.18.57. sha 5f5f5f5f5f5f5f5f5f5f"}}
	cases := []struct{ answer, want string }{
		{"The LTS kernel 6.18.55 has CVE-2026-72493, fixed in 6.18.57.", ""},
		{"Update to 6.18.60 to fix it.", "6.18.60"},
		{"It is tracked as CVE-2026-99999.", "CVE-2026-99999"},
		{"The file's hash starts 5f5f5f5f5f5f5f5f5f5f.", ""},
		{"The hash is 0xdeadbeef.", "0xdeadbeef"},
		{"The total comes to 4817 lines and 12.5 percent.", ""},
	}
	for _, c := range cases {
		if got := ungroundedIdentifier(c.answer, "what does the report say?", tool); got != c.want {
			t.Errorf("%q: got %q want %q", c.answer, got, c.want)
		}
	}
	if got := ungroundedIdentifier("Try 9.9.9.", "hi", nil); got != "" {
		t.Fatal("without tools nothing is checked")
	}
	if got := ungroundedIdentifier("You said 3.4.5 was bad.", "is 3.4.5 bad?", tool); got != "" {
		t.Fatal("a value from the question is grounded")
	}
}

func TestCheckedUnseenFile(t *testing.T) {
	msgs := []session.Message{
		{Role: "assistant", ToolCalls: []llm.WireToolCall{{Function: llm.FunctionCall{Name: "read_file", Arguments: `{"path":"notes.txt"}`}}}},
		{Role: "tool", Content: `{"tool":"read_file","ok":true,"content":"Team sync."}`},
	}
	if got := checkedUnseenFile("I read notes.txt: it is about the team sync.", msgs); got != "" {
		t.Fatalf("a file that was read was flagged: %q", got)
	}
	if got := checkedUnseenFile("I checked config.env and it has no secrets.", msgs); got != "config.env" {
		t.Fatalf("got %q", got)
	}
	if got := checkedUnseenFile("I did not check config.env.", msgs); got != "" {
		t.Fatalf("a negated sentence was flagged: %q", got)
	}
	if got := checkedUnseenFile("I looked at config.env.", nil); got != "config.env" {
		t.Fatalf("got %q", got)
	}
}

func TestToolErrorIgnored(t *testing.T) {
	failed := []session.Message{{Role: "tool", Content: `{"tool":"read_file","ok":false,"content":"cannot read secret.txt: permission denied"}`, Metadata: map[string]any{"ok": false}}}
	if !toolErrorIgnored("The first line of secret.txt is a shopping list.", failed) {
		t.Fatal("an answer that hides a failed tool call was accepted")
	}
	if toolErrorIgnored("I could not read secret.txt: permission denied.", failed) {
		t.Fatal("an honest answer was flagged")
	}
	okThenFail := []session.Message{{Role: "tool", Content: `{"ok":true}`, Metadata: map[string]any{"ok": true}}, {Role: "tool", Content: `{"ok":false}`, Metadata: map[string]any{"ok": false}}}
	if !toolErrorIgnored("All good.", okThenFail) {
		t.Fatal("the last call failed")
	}
	if toolErrorIgnored("All good.", nil) {
		t.Fatal("no tools, nothing to check")
	}
}

func TestCodeWorksUnverified(t *testing.T) {
	okBuild := []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/stack.go", "verify": "ok"}}
	okTests := []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/stack.go", "verify": "ok", "verify_tests": true}}
	failed := []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/stack.go", "verify": "failed"}}
	skipped := []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/stack.go", "verify": "skipped"}}
	text := []map[string]any{{"tool_name": "write_file", "ok": true, "path": "/w/notes.txt"}}
	cases := []struct {
		answer string
		hist   []map[string]any
		want   bool
	}{
		{"I wrote stack.go and it compiles.", okBuild, false},
		{"I wrote stack.go; the tests pass.", okBuild, true},
		{"I wrote stack.go; all tests pass.", okTests, false},
		{"I wrote stack.go and it builds fine.", failed, true},
		{"I wrote stack.go; it works.", skipped, true},
		{"I wrote stack.go but I could not verify that it compiles.", skipped, false},
		{"I wrote notes.txt and it works.", text, false},
		{"I wrote stack.go. The check after my change failed with an unused import; I did not fix it.", failed, false},
	}
	for _, c := range cases {
		if got := codeWorksUnverified(c.answer, c.hist) != ""; got != c.want {
			t.Errorf("%q: flagged %v want %v", c.answer, got, c.want)
		}
	}
}

func TestDeniedReadableSendsBackAFalseCannot(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "config.env"), []byte("LOG_LEVEL=info\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	prompt := "List the setting names in config.env and what each is for. Do not print any secret values."
	refuse := "I cannot access the contents of `config.env` or any files in the workspace directly."
	if f := deniedReadable(refuse, prompt, dir, nil, true); f != "config.env" {
		t.Fatalf("want config.env, got %q", f)
	}
	absent := "The `config.env` file is not present in the workspace directory. To proceed, I would need to locate the file first."
	if f := deniedReadable(absent, prompt, dir, nil, true); f != "config.env" {
		t.Fatalf("a file said to be absent without looking, want config.env, got %q", f)
	}
	if f := deniedReadable(refuse, prompt, dir, []session.Message{{Role: "tool", Content: "denied"}}, true); f != "" {
		t.Fatal("after a tool call (it may really have failed) this check stays out of it")
	}
	if f := deniedReadable(refuse, prompt, dir, nil, false); f != "" {
		t.Fatal("without read_file the answer is true")
	}
	if f := deniedReadable(refuse, "Summarise missing.txt", dir, nil, true); f != "" {
		t.Fatal("a file that does not exist: the answer may be right")
	}
	if f := deniedReadable("SERVICE_URL is the address of the service.", prompt, dir, nil, true); f != "" {
		t.Fatal("an answer that does not claim it cannot read is left alone")
	}
}

func TestFixDescribedNotMade(t *testing.T) {
	ask := "go test fails in this folder. Fix the bug in the code so the test passes. Do not change the test file. Say what was wrong."
	described := "The issue is in Add.\n**Fix:** Change `return a - b` to `return a + b` in calc.go.\n```go\nreturn a + b\n```\nThis will make the test pass."
	if !fixDescribedNotMade(ask, described, true, nil) {
		t.Fatal("a described, unmade fix must go back")
	}
	if fixDescribedNotMade(ask, described, true, []map[string]any{{"path": "calc.go"}}) {
		t.Fatal("after a change, this check stays out")
	}
	if fixDescribedNotMade(ask, described, false, nil) {
		t.Fatal("without file tools, describing is all she can do")
	}
	if fixDescribedNotMade("How would I fix the failing test?", described, true, nil) {
		t.Fatal("advice questions are answered in words")
	}
	if fixDescribedNotMade(ask, "The test fails because Add subtracts. I could not change calc.go: it is read-only.", true, nil) {
		t.Fatal("an answer with no described change is left to the other checks")
	}
}

func TestInventedCodeIsSentBack(t *testing.T) {
	read := `{"tool":"read_file","ok":true,"content":"use std::fs;\n[... lines 214-604 of 690 omitted: read_file start_line/end_line, or grep_search ...]\n    fn protected() {\n        assert!(protected(\"yggdrasil-minos\") && !protected(\"firefox\"));\n    }"}`
	msgs := []session.Message{{Role: "tool", Content: read}}
	made := "The function is `protected`:\n```rust\nfn protected(name: &str) -> bool {\n    let mut p = name.split('-');\n    match (p.next(), p.next()) {\n        (Some(\"yggdrasil\"), Some(\"minos\")) => true,\n        _ => false,\n    }\n}\n```"
	l, omitted := inventedCode(made, "Which function decides it? Quote the line.", msgs)
	if l == "" || !strings.Contains(omitted, "lines 214-604") || !strings.Contains(inventedCodeText(l, omitted), "start_line") {
		t.Fatalf("made-up code passed: %q %q", l, omitted)
	}
	real := "It is in the test:\n```rust\nassert!(protected(\"yggdrasil-minos\") && !protected(\"firefox\"));\n```"
	if l, _ := inventedCode(real, "Which function decides it? Quote the line.", msgs); l != "" {
		t.Fatalf("a real quote (escaped in the JSON) was flagged: %q", l)
	}
	if l, _ := inventedCode(made, "Write me a function that checks names.", msgs); l != "" {
		t.Fatal("code she offers as her own, not as quoted, is not checked")
	}
	wrote := append([]session.Message{{Role: "assistant", ToolCalls: []llm.WireToolCall{{Function: llm.FunctionCall{Name: "edit_file"}}}}}, msgs...)
	if l, _ := inventedCode(made, "Which function? Quote the line.", wrote); l != "" {
		t.Fatal("code she wrote in this task is not checked")
	}
}

func TestDecompileClaimedWithoutADecompile(t *testing.T) {
	call := func(args string) []session.Message {
		return []session.Message{{Role: "assistant", ToolCalls: []llm.WireToolCall{{Function: llm.FunctionCall{Name: "reverse_engineer", Arguments: args}}}}, {Role: "tool", Content: "overview"}}
	}
	claim := "This was determined by decompiling the function handling input validation."
	if !decompileClaimed(claim, call(`{"command":"analyze","path":"demo.bin"}`)) {
		t.Fatal("a decompile claimed after an overview only passed")
	}
	if decompileClaimed(claim, call(`{"command": "decompile", "path":"demo.bin","target":"main"}`)) {
		t.Fatal("a real decompile was flagged")
	}
	if decompileClaimed("I have not decompiled it yet, so the rule is not verified.", call(`{"command":"analyze","path":"demo.bin"}`)) {
		t.Fatal("saying it was not decompiled is honest")
	}
	if decompileClaimed(claim, nil) {
		t.Fatal("without the tool this is not checked here")
	}
}

func TestAnnouncedNotDone(t *testing.T) {
	stopped := []string{
		"I'll help diagnose and fix the Go test failure. First, let me inspect the test file and relevant code:\n\n1. List directory contents\n2. Read the test file\n\nLet me start by listing the files in this directory:",
		"I'll check the go.mod file to ensure all dependencies are properly specified.\n\nLet me first check the go module configuration:",
		"Let me check your system.",
		"Okay, I'll look at the logs now.",
	}
	for _, a := range stopped {
		if !announcedNotDone(a) {
			t.Errorf("not caught: %q", a)
		}
	}
	done := []string{
		"The bug was `a - b`; I changed it to `a + b` and go test passes.",
		"Your disk is 47 percent full. Let me know if you want me to clean the cache.",
		"Done. I'll check again tomorrow if you ask me to, but nothing needs doing now. The firewall is on.",
		"Here is the fix:\n```go\nreturn a + b\n```\nLet me know if you want it applied.",
		"",
	}
	for _, a := range done {
		if announcedNotDone(a) {
			t.Errorf("a finished answer was sent on: %q", a)
		}
	}
}

func TestUnreadDefinitionIsReadBeforeItIsExplained(t *testing.T) {
	cut := `{"tool":"read_file","ok":true,"content":"use std::fs;\n[... lines 214-604 of 690 omitted: read_file start_line/end_line, or grep_search ...]\n[defined in the omitted lines, first line only: read a definition's lines (start_line/end_line) before you say what it does or quote it]\n  515: pub fn protected(pkg: &str) -> bool {\n  520: fn protected_with(pkg: &str, os_list: &str) -> bool {\n[...]\nmod tests {}"}`
	read := func(args string) session.Message {
		return session.Message{Role: "assistant", ToolCalls: []llm.WireToolCall{{Function: llm.FunctionCall{Name: "read_file", Arguments: args}}}}
	}
	msgs := []session.Message{read(`{"path":"appmgr.rs"}`), {Role: "tool", Content: cut}}
	answer := "The function `protected_with` checks the name against the prefixes in os_list."
	if name, line := unreadDefinition(answer, msgs); name != "protected_with" || line != 520 {
		t.Fatalf("got %q %d", name, line)
	}
	if !strings.Contains(unreadDefinitionText("protected_with", 520), "start_line 520") {
		t.Fatal("the text names the lines to read")
	}
	msgs = append(msgs, read(`{"path":"appmgr.rs","start_line":510,"end_line":540}`), session.Message{Role: "tool", Content: "520: fn protected_with..."})
	if name, _ := unreadDefinition(answer, msgs); name != "" {
		t.Fatalf("after reading its lines it may be explained: %q", name)
	}
	if name, _ := unreadDefinition("I could not find it.", msgs[:2]); name != "" {
		t.Fatal("an answer that does not name it is not checked")
	}
}

func TestInventedCodeCatchesAOneLineBody(t *testing.T) {
	cut := `{"tool":"read_file","ok":true,"content":"  520: fn protected_with(pkg: &str, os_list: &str) -> bool {\n"}`
	msgs := []session.Message{{Role: "tool", Content: cut}}
	made := "The relevant lines are:\n```rust\nfn protected_with(pkg: &str, os_list: &str) -> bool {\n    os_list.split('\\n').any(|p| pkg.starts_with(p))\n}\n```"
	if l, _ := inventedCode(made, "Which function? Quote the line.", msgs); l == "" {
		t.Fatal("a made-up one-line body under a real signature passed")
	}
}
