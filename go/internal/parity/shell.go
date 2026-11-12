package parity

import (
	"math/rand"
	"path/filepath"
	"strings"
	"testing"
)

// ShellCorpus is the command lines the shell fixtures run: every string literal of the shell parser's and the bash
// guards' own tests, hand-written hostile ones, every prefix of a few long ones (each unterminated form), and generated
// ones (a fixed seed, so the corpus is the same on every run).
func ShellCorpus(t *testing.T) []string {
	t.Helper()
	tools := filepath.Join(Root(), "go", "tools")
	var out []string
	seen := map[string]bool{}
	add := func(s string) {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	for _, s := range StringLiterals(t,
		filepath.Join(tools, "shellparse", "shellparse_test.go"),
		filepath.Join(tools, "destructive_test.go"),
		filepath.Join(tools, "bash_test.go"),
		filepath.Join(tools, "exfil_test.go"),
	) {
		add(s)
	}
	for _, s := range hostileShell {
		add(s)
	}
	// the nesting limit from both sides, for each way of nesting
	for n := 37; n <= 42; n++ {
		add(strings.Repeat("(", n) + "a" + strings.Repeat(")", n))
		add(strings.Repeat("$(", n) + "a" + strings.Repeat(")", n))
		add(strings.Repeat("$(", n) + "`a`" + strings.Repeat(")", n))
		add(strings.Repeat("$(", n) + "`a <<E\n$(b)\nE\n`" + strings.Repeat(")", n))
		add(strings.Repeat("<(", n) + "a" + strings.Repeat(")", n))
		add(strings.Repeat("${x:-$(", n) + "a" + strings.Repeat(")}", n))
		add("a <<E" + strings.Repeat("$(", n) + "b" + strings.Repeat(")", n) + "\n" + strings.Repeat("$(", n) + "c" + strings.Repeat(")", n) + "\nE")
	}
	// here-documents whose body is read inside a substitution, for a command of an outer frame
	for _, s := range []string{
		"a <<E $(b\nbody $(c)\nE\n)", "a <<E $(b\nx\nE\n) d\ne", "a <<E <<F $(b\n1\nE\n2\nF\n)", "a <<E $(b <<F\n1\nE\n2\nF\n)",
		"a <<E | $(b\nx\nE\n)", "( a <<E\nx\nE\n)", "( a <<E )\nx\nE", "{ a <<E; }\nx\nE", "a $(b <<E)\nx $(c)\nE", "a `b <<E`\nx\nE",
		"a <<E $(b <<F\nf\nF\n)\ne\nE", "a <<E\n$(b <<F\nf\nF\n)\nE",
	} {
		add(s)
	}
	for _, s := range []string{
		"a=$(b \"c $(d 'e' `f \\` g`)\" ${h:-$(i)}) j <<-EOF k\n\tl $(m)\n\tEOF\n n",
		"cat <<'X' | sh -c \"$(echo $'\\x72m' x)\"\nrm y\nX\nfor i in a; do case $i in a) rm z;; esac; done",
		"f() { f | f & }; f",
		"x=1 y+=2 env -S 'rm a' sudo -u root -- timeout -s 9 5 find . -exec rm {} + 2>&1 >>/tmp/o",
	} {
		for i := 0; i <= len(s); i++ {
			add(s[:i])
		}
	}
	rng := rand.New(rand.NewSource(20261111))
	for len(out) < 6000 {
		add(genShell(rng))
	}
	for i := 0; i < 1500; i++ {
		add(genQuoted(rng))
	}
	return out
}

// genQuoted makes words built from quoting and escapes: ANSI-C escapes with digits running past their width, quotes inside
// quotes, substitutions inside them, here-documents with every delimiter form.
func genQuoted(rng *rand.Rand) string {
	const hexd = "0123456789abcdefABCDEFgG"
	digits := func(n int, set string) string {
		var b strings.Builder
		for i := 0; i < n; i++ {
			b.WriteByte(set[rng.Intn(len(set))])
		}
		return b.String()
	}
	esc := func() string {
		switch rng.Intn(8) {
		case 0:
			return `\x` + digits(rng.Intn(4), hexd)
		case 1:
			return `\u` + digits(rng.Intn(6), hexd)
		case 2:
			return `\U` + digits(rng.Intn(10), hexd)
		case 3:
			return `\` + digits(1+rng.Intn(4), "012345678")
		case 4:
			const named = "ntrabeEfvcz'\\\"?"
			return `\` + string(named[rng.Intn(len(named))])
		case 5:
			return string(rune(0x80 + rng.Intn(0x700)))
		case 6:
			return string([]byte{byte(0x80 + rng.Intn(0x80))})
		}
		return digits(1+rng.Intn(3), "az09 _-")
	}
	piece := func() string {
		switch rng.Intn(9) {
		case 0:
			var b strings.Builder
			b.WriteString("$'")
			for i := rng.Intn(5); i >= 0; i-- {
				b.WriteString(esc())
			}
			if rng.Intn(6) != 0 {
				b.WriteString("'")
			}
			return b.String()
		case 1:
			return "\"" + esc() + "$x\\\"" + "`a`" + "\""
		case 2:
			return "'" + esc() + "\"" + "'"
		case 3:
			return "\\" + esc()
		case 4:
			return "${" + digits(rng.Intn(4), "a{}$(`\\") + "}"
		case 5:
			return "$(" + digits(rng.Intn(5), "ab ;|&<>()`'\"\\") + ")"
		case 6:
			d := []string{"E", "'E'", "\"E\"", "\\E", "E'x'"}[rng.Intn(5)]
			op := []string{"<<", "<<-"}[rng.Intn(2)]
			return "cat " + op + d + "\n\t" + esc() + " $(rm a) `rm b`\n" + []string{"E", "\tE", "E\r", "x"}[rng.Intn(4)] + "\n"
		case 7:
			return digits(1, "0123456789") + []string{">", ">>", ">&", "<&", "<>", "<<<"}[rng.Intn(6)] + digits(rng.Intn(3), "-0123456789a")
		}
		return esc()
	}
	var b strings.Builder
	for i := 1 + rng.Intn(4); i > 0; i-- {
		b.WriteString(piece())
		if rng.Intn(2) == 0 {
			b.WriteByte(' ')
		}
	}
	return b.String()
}

var hostileShell = []string{
	"", " ", "\n", "\\", "\\\n", "'", "\"", "`", "$", "$(", "${", "$'", "$\"", "((", "))", ")", "(", "<(", ">(", "<<", "<<-", "<<<",
	"a\x00b", "rm\x00", "$'\\x00'", "$'\\xff\\xfe' x", "$'\\u00e9\\U0001F600\\uD800\\U00110000\\Uffffffff' y", "$'\\777\\400\\08\\x'",
	"$'\\c\\z\\\\\\''", "echo \xff\xfe", "r\xc3m", "\xe2\x80\x8brm x", "rm\u200b x", "\\\xc3\xa9x", "\\\xff",
	"a 2>", "a >", "a >&", "a &>", "a <&-", "a >&2", "a >&-", "a 1>&2 3<>f 4>>g", "a >&99999999999999999999", "a >&+1", "a >&-1",
	"a >& file", "a >&\"2\"", "a &>> f", "a >| f", "a<>f", "a <<< w", "a <<E\nbody $(rm x) `rm y` ${z}\nE",
	"a <<'E'\n$(rm x)\nE", "a <<\"E\"\n$(rm x)\nE", "a <<E", "a <<E\r\nx\r\nE\r\n", "a <<-E\n\t\tE", "a <<E <<F\n1\nE\n2\nF",
	"a <<E; b <<F\nx\nE\ny\nF", "a <<E\n\\$(rm x)\nE", "a <<E\n${x:-$(rm y)}\nE", "a <<E\n$(\nE",
	"function f { rm x; }", "function", "function f() { :; }", "f(){ :;}", "$f() x", "\"f\"() x", "f ()", "a=b() x",
	"for", "for x", "for x in", "for x in a b; do c; done", "select x in a; do b; done", "case", "case x in", "case $x in a|b) c;; *) d;; esac",
	"if a; then b; elif c; then d; else e; fi", "while a; do b; done", "until a; do b; done", "! a", "time a", "coproc a", "{ a; }",
	"\"if\" x", "'then' y", "\\do z", "i\\f a", "in", "x in y", "a=1", "a+=1", "+=1", "=1", "_a1=2 b", "1a=2 b", "a-b=1 c", "é=1 x",
	"a;;b", "a;&b", "a;;&b", "a&&&b", "a|||b", "a||&b", "a | | b", "|a", "&a", ";a", "a\\\nb", "a \\\n b", "a#b #c\nd",
	"$((1+2))", "$((", "$[1]", "${#}", "${!x}", "${x#*/}", "${x/a/b}", "${x:-${y:-$(z)}}", "${", "${x", "${{}}", "$0 $9 $@ $* $# $? $- $$ $!",
	"$ x", "$", "a$", "a$b", "\"$\"", "\"$'x'\"", "\"$\\\"x\"", "\"\\$x\"", "\"\\`x\"", "\"\\\\\"", "\"\\a\"", "\"a\\\nb\"",
	"`a \\` b`", "`a \\\\ b`", "`a \\$ b`", "`a\\", "``", "$()", "$( )", "<()", "a <(b) >(c)", "<(a)b", "a<(b)",
	"*", "?", "[", "a*b", "a[bc]", "~", "~/x", "~root/x", "{a,b}", "a{b,c}d",
	"echo '\xff'", "echo \"\xff\"", "$'\xff'",
	strings.Repeat("(", 39) + strings.Repeat(")", 39), strings.Repeat("(", 41) + strings.Repeat(")", 41),
	strings.Repeat("$(", 39) + strings.Repeat(")", 39), strings.Repeat("$(", 41) + strings.Repeat(")", 41),
	strings.Repeat("`", 1) + strings.Repeat("\\`", 1) + "`", strings.Repeat("a | ", 200) + "b", strings.Repeat("x ", 500),
	strings.Repeat("${", 45) + strings.Repeat("}", 45), strings.Repeat("<(", 45) + strings.Repeat(")", 45),
}

var shellAtoms = []string{
	"rm", "x", "-rf", "sudo", "env", "bash", "sh", "-c", "python3", "eval", "find", "-exec", "{}", ";", "+", "git", "push", "-f",
	"tee", "/etc/x", "../y", "~", "/dev/sda", "/dev/null", "curl", "|", "||", "&&", "&", ";", "\n", " ", " ", " ", "\t",
	"'", "\"", "`", "$", "$(", ")", "(", "${", "}", "{", "$'", "\\", "\\\n", "<", ">", ">>", ">&", "2>", "&>", "<<", "<<-", "<<<",
	"<(", ">(", "EOF", "#", "=", "a=1", "x=", "*", "?", "[", "!", "if", "then", "fi", "for", "in", "do", "done", "case", "esac",
	"function", "f", "()", "\\x72", "\\162", "\\u00e9", "é", "\xff", "\x00", "\r", "0", "1", "99", "-", "--", "+o", "-o", "-S",
	"xargs", "timeout", "5", "nice", "-n", "su", "-", "chmod", "-R", "awk", "system(", "perl", "-e", "node", "--eval", "systemd-run",
	"--user", "claude-sudo", "pkexec", "run0", "doas", "runuser", "alias", "source", ".", "dd", "mv", ":", "true", "init", "6",
}

func genShell(rng *rand.Rand) string {
	var b strings.Builder
	n := 1 + rng.Intn(14)
	for i := 0; i < n; i++ {
		if rng.Intn(9) == 0 {
			b.WriteByte(byte(rng.Intn(256)))
			continue
		}
		b.WriteString(shellAtoms[rng.Intn(len(shellAtoms))])
		if rng.Intn(3) != 0 {
			b.WriteByte(' ')
		}
	}
	return b.String()
}
