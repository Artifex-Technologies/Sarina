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
