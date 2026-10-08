package shellparse

import (
	"strings"
	"testing"
)

func progs(t *testing.T, src string) []string {
	t.Helper()
	s, err := Parse(src)
	if err != nil {
		t.Fatalf("Parse(%q): %v", src, err)
	}
	var out []string
	for _, c := range s.Commands {
		if len(c.Words) > 0 && !c.Data {
			out = append(out, c.Words[0].Text)
		}
	}
	return out
}

func TestSeparatorsAndNesting(t *testing.T) {
	cases := []struct {
		src  string
		want string // programs found, in order, joined by space
	}{
		{"a; b", "a b"},
		{"a && b || c", "a b c"},
		{"a | b |& c", "a b c"},
		{"a & b", "a b"},
		{"a\nb\n\nc", "a b c"},
		{"( a ; b )", "a b"},
		{"{ a; b; }", "a b"},
		{"echo $(rm x)", "rm echo"},
		{"echo `rm x`", "rm echo"},
		{"echo \"$(rm x)\"", "rm echo"},
		{"echo \"`rm x`\"", "rm echo"},
		{"echo $(echo $(rm x))", "rm echo echo"},
		{"diff <(a) <(b)", "a b diff"},
		{"FOO=1 BAR=2 cmd arg", "cmd"},
		{"if x; then y; else z; fi", "x y z"},
		{"while a; do b; done", "a b"},
		{"for f in rm x; do echo $f; done", "echo"},
		{"x # y; z", "x"},
		{"x \\\n y", "x"},
		{"a |\n b", "a b"},
		{"case $x in a) b;; c) d;; esac", "a b c d"},
		{"echo 'a;b' \"c;d\" e\\;f", "echo"},
	}
	for _, c := range cases {
		got := strings.Join(progs(t, c.src), " ")
		if got != c.want {
			t.Errorf("%q: got %q want %q", c.src, got, c.want)
		}
	}
}

func TestQuotingResolvesToTheRealName(t *testing.T) {
	for src, want := range map[string]string{
		`r""m x`:        "rm",
		`"rm" x`:        "rm",
		`'r'm x`:        "rm",
		`\rm x`:         "rm",
		`r\m x`:         "rm",
		`$'\x72m' x`:    "rm",
		`$'\162\155' x`: "rm",
		`$'r\155' x`:    "rm",
	} {
		if got := strings.Join(progs(t, src), " "); got != want {
			t.Errorf("%q: got %q want %q", src, got, want)
		}
	}
}

func TestDynamicWordsAreMarked(t *testing.T) {
	for _, src := range []string{`$x a`, `${x} a`, `"$x" a`, `$(echo rm) a`, "`echo rm` a", `r${x}m a`} {
		s, err := Parse(src)
		if err != nil {
			t.Fatal(err)
		}
		last := s.Commands[len(s.Commands)-1]
		if !last.Words[0].Dynamic() {
			t.Errorf("%q: first word %q should be dynamic", src, last.Words[0].Text)
		}
	}
	s, _ := Parse(`echo '$x' plain`)
	if s.Commands[0].Words[1].Dynamic() {
		t.Error("a single-quoted $x is literal")
	}
}

func TestRedirects(t *testing.T) {
	s, err := Parse(`echo hi > out.txt 2>&1 >> log; cat < in; : > t; echo x &> all; echo y 2>/dev/null`)
	if err != nil {
		t.Fatal(err)
	}
	type rd struct{ op, target string }
	var all [][]rd
	for _, c := range s.Commands {
		var rs []rd
		for _, r := range c.Redirects {
			if r.Writes() {
				rs = append(rs, rd{r.Op, r.Target.Text})
			}
		}
		all = append(all, rs)
	}
	if len(all) != 5 {
		t.Fatalf("commands = %d", len(all))
	}
	if len(all[0]) != 2 || all[0][0] != (rd{">", "out.txt"}) || all[0][1] != (rd{">>", "log"}) {
		t.Errorf("first command writes %v (2>&1 must not count)", all[0])
	}
	if len(all[1]) != 0 {
		t.Errorf("an input redirect writes nothing: %v", all[1])
	}
	if len(all[2]) != 1 || all[2][0].target != "t" {
		t.Errorf(": > t = %v", all[2])
	}
	if len(all[3]) != 1 || all[3][0].op != "&>" {
		t.Errorf("&> = %v", all[3])
	}
	if len(all[4]) != 1 || all[4][0].target != "/dev/null" {
		t.Errorf("2>/dev/null = %v", all[4])
	}
}

func TestPipelineIDs(t *testing.T) {
	s, _ := Parse("curl x | sh; ls | wc | cat; true")
	ids := []int{}
	for _, c := range s.Commands {
		ids = append(ids, c.PipeID)
	}
	if ids[0] == 0 || ids[0] != ids[1] || ids[2] == 0 || ids[2] != ids[3] || ids[3] != ids[4] || ids[0] == ids[2] || ids[5] != 0 {
		t.Fatalf("pipeline ids = %v", ids)
	}
}

func TestHeredocs(t *testing.T) {
	s, err := Parse("cat <<EOF\nrm -rf /\nEOF\necho after")
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.Join(progs(t, "cat <<EOF\nrm -rf /\nEOF\necho after"), " "); got != "cat echo" {
		t.Errorf("a heredoc body is data, got %q", got)
	}
	if len(s.Commands[0].Heredocs) != 1 || !strings.Contains(s.Commands[0].Heredocs[0], "rm -rf /") {
		t.Errorf("heredoc body not kept: %+v", s.Commands[0].Heredocs)
	}
	// an unquoted delimiter expands substitutions, which do run
	if got := strings.Join(progs(t, "cat <<EOF\n$(rm x)\nEOF"), " "); got != "cat rm" {
		t.Errorf("unquoted heredoc substitution: %q", got)
	}
	if got := strings.Join(progs(t, "cat <<'EOF'\n$(rm x)\nEOF"), " "); got != "cat" {
		t.Errorf("quoted heredoc is literal: %q", got)
	}
	if got := strings.Join(progs(t, "cat <<-EOF\n\tbody\n\tEOF\nnext"), " "); got != "cat next" {
		t.Errorf("<<- strips tabs: %q", got)
	}
	if got := strings.Join(progs(t, "cat <<< \"$(rm x)\""), " "); got != "rm cat" {
		t.Errorf("here-string: %q", got)
	}
}

func TestFunctionDefinition(t *testing.T) {
	s, err := Parse(`:(){ :|:& };:`)
	if err != nil {
		t.Fatal(err)
	}
	if s.Commands[0].FuncName != ":" {
		t.Fatalf("function header not found: %+v", s.Commands[0])
	}
	s, _ = Parse(`f() { rm x; }; f`)
	if got := strings.Join(progs(t, `f() { rm x; }; f`), " "); got != "rm f" {
		t.Errorf("body commands must be visible: %q", got)
	}
	_ = s
}

func TestParseErrors(t *testing.T) {
	for _, src := range []string{`echo "abc`, `echo 'abc`, `echo $(abc`, "echo `abc", `echo ${abc`, `(echo`, `echo >`} {
		if _, err := Parse(src); err == nil {
			t.Errorf("%q should not parse", src)
		}
	}
	if _, err := Parse(strings.Repeat("$(", 200) + strings.Repeat(")", 200)); err == nil {
		t.Error("absurd nesting must fail rather than recurse forever")
	}
}

func TestArithmeticAndStrayParens(t *testing.T) {
	if _, err := Parse(`echo $((1+2)) $(( (3) * 4 ))`); err != nil {
		t.Errorf("arithmetic: %v", err)
	}
	if _, err := Parse(`echo a) b`); err != nil {
		t.Errorf("a stray ')' must be tolerated: %v", err)
	}
}
