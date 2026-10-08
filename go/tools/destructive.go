package tools

import (
	"fmt"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/tools/shellparse"
)

// Verdict is the outcome of the destructive-command check.
type Verdict struct {
	Blocked bool
	Reason  string
}

// CheckDestructive decides whether a bash command should be refused when
// destructive commands are not allowed. root is the workspace; redirections
// and tee targets outside it are refused.
//
// It reads the command with a real shell parser (package shellparse): every
// simple command is found whatever separates it, nested in a subshell, a
// substitution or a `-c` string, and the program is identified by name after
// removing env assignments, sudo/env/xargs-style wrappers, paths and quoting.
// Commands whose program or script only exists at run time are refused rather
// than guessed at.
//
// This is a guard-rail against a model's mistakes, not a sandbox. A determined
// caller can still do harm through a program it does not know about, and
// remote execution (ssh) is not inspected. The real boundaries are the shell
// permission itself and the Limbo sandbox.
func CheckDestructive(command, root string) Verdict {
	c := &checker{root: root}
	if why := c.script(command); why != "" {
		return Verdict{Blocked: true, Reason: why}
	}
	return Verdict{}
}

type checker struct {
	root  string
	depth int
}

type member struct {
	prog       string
	download   bool
	shellStdin bool
}

func (c *checker) script(src string) string {
	if c.depth > 6 {
		return "commands are nested too deeply to check"
	}
	s, err := shellparse.Parse(src)
	if err != nil {
		return fmt.Sprintf("the command could not be parsed safely (%v)", err)
	}
	funcs := map[string]bool{}
	for _, cmd := range s.Commands {
		if cmd.FuncName != "" {
			funcs[cmd.FuncName] = true
		}
	}
	pipes := map[int][]member{}
	for _, cmd := range s.Commands {
		why, m := c.command(cmd)
		if why != "" {
			return why
		}
		if cmd.PipeID != 0 {
			pipes[cmd.PipeID] = append(pipes[cmd.PipeID], m)
		}
	}
	for _, ms := range pipes {
		downloaded, self := false, 0
		for _, m := range ms {
			if downloaded && m.shellStdin {
				return "it pipes downloaded content straight into a shell or interpreter"
			}
			if m.download {
				downloaded = true
			}
			if funcs[m.prog] {
				self++
			}
		}
		if self >= 2 {
			return "it defines a function that pipes into itself (a fork bomb)"
		}
	}
	return ""
}

func (c *checker) command(cmd *shellparse.Command) (string, member) {
	if cmd.Data {
		return "", member{}
	}
	for _, r := range cmd.Redirects {
		if r.Writes() {
			if why := c.writeTarget(r.Target, "redirection"); why != "" {
				return why, member{}
			}
		}
	}
	if len(cmd.Words) == 0 {
		for _, r := range cmd.Redirects {
			if r.Writes() && r.Op != ">>" {
				return "a redirection with no command truncates a file", member{}
			}
		}
		return "", member{}
	}
	if w := cmd.Words[0].Text; (w == ":" || w == "true") && len(cmd.Words) == 1 {
		for _, r := range cmd.Redirects {
			if r.Writes() && r.Op != ">>" {
				return "': > file' truncates a file", member{}
			}
		}
	}
	return c.words(cmd.Words, cmd.Heredocs, 0)
}

// ---- program tables ---------------------------------------------------------

var destructiveProgs = map[string]string{
	"rm": "removes files", "rmdir": "removes directories", "unlink": "removes files", "shred": "destroys file contents",
	"srm": "destroys file contents", "truncate": "truncates files", "mv": "moves or overwrites files",
	"dd": "writes raw data to files or devices", "mke2fs": "creates a filesystem", "mkswap": "formats a device",
	"fdisk": "edits partition tables", "sfdisk": "edits partition tables", "cfdisk": "edits partition tables",
	"parted": "edits partition tables", "gdisk": "edits partition tables", "sgdisk": "edits partition tables",
	"wipefs": "erases filesystem signatures", "blkdiscard": "discards device contents",
	"shutdown": "powers the machine off", "reboot": "reboots the machine", "poweroff": "powers the machine off",
	"halt": "halts the machine",
}

var powerSubcommands = map[string]bool{
	"poweroff": true, "reboot": true, "halt": true, "kexec": true, "suspend": true, "hibernate": true,
	"hybrid-sleep": true, "suspend-then-hibernate": true, "rescue": true, "emergency": true, "isolate": true,
}

type wrapSpec struct {
	short      string
	long       []string
	positional int
	assigns    bool
}

var wrappers = map[string]wrapSpec{
	"sudo":    {short: "ughprtUCDRT", long: []string{"user", "group", "host", "prompt", "role", "type", "other-user", "chdir", "chroot", "close-from", "command-timeout"}},
	"doas":    {short: "uC"},
	"env":     {short: "uCS", long: []string{"unset", "chdir", "split-string"}, assigns: true},
	"command": {}, "builtin": {}, "nohup": {}, "setsid": {}, "busybox": {},
	"exec":    {short: "a"},
	"time":    {short: "fo", long: []string{"format", "output"}},
	"nice":    {short: "n", long: []string{"adjustment"}},
	"ionice":  {short: "cnpPu", long: []string{"class", "classdata", "pid", "pgid", "uid"}},
	"timeout": {short: "sk", long: []string{"signal", "kill-after"}, positional: 1},
	"stdbuf":  {short: "ioe", long: []string{"input", "output", "error"}},
	"xargs":   {short: "InPLsdEa", long: []string{"max-args", "max-procs", "max-lines", "max-chars", "delimiter", "arg-file", "eof"}},
	"watch":   {short: "n", long: []string{"interval"}},
	"flock":   {short: "wE", positional: 1},
	"chroot":  {positional: 1},
}

var shells = map[string]bool{
	"sh": true, "bash": true, "zsh": true, "dash": true, "ksh": true, "mksh": true, "ash": true,
	"csh": true, "tcsh": true, "fish": true, "rbash": true,
}

var downloaders = map[string]bool{
	"curl": true, "wget": true, "fetch": true, "aria2c": true, "nc": true, "ncat": true, "netcat": true, "lwp-download": true,
}

var inlineRiskTokens = []string{
	"rmtree", "os.remove", "os.unlink", "os.rmdir", "os.removedirs", "os.rename", "os.replace", "os.system",
	"os.popen", "os.exec", "os.spawn", "os.kill", "subprocess", "shutil", "__import__", "importlib", "eval(", "exec(",
	"unlink", "rmdir", "rmsync", "writefile", "appendfile", "renamesync", ".write_text(", ".write_bytes(", "child_process", "execsync", "spawnsync", "fs.rm", "fs.unlink",
	"fs.write", "fs.rename", "system(", "popen(", "file.delete", "fileutils", "`", "mkfs", "dd if",
}

var rmWord = regexp.MustCompile(`(^|[^a-z0-9_])rm(\s|$)`)

// ---- command analysis -------------------------------------------------------

func baseName(text string) string {
	if i := strings.LastIndexAny(text, `/\`); i >= 0 {
		return text[i+1:]
	}
	return text
}

func (c *checker) words(ws []shellparse.Word, heredocs []string, depth int) (string, member) {
	if len(ws) == 0 {
		return "", member{}
	}
	if depth > 8 {
		return "commands are wrapped too deeply to check", member{}
	}
	base := baseName(ws[0].Text)
	if strings.ContainsAny(base, shellparse.Dyn+"*?[{") {
		return "the command name is built at run time (variable, substitution, glob or brace expansion), so it cannot be checked", member{}
	}
	prog := strings.TrimSuffix(strings.ToLower(base), ".exe")
	args := ws[1:]
	m := member{prog: prog, download: downloaders[prog]}

	if why, ok := destructiveProgs[prog]; ok {
		return fmt.Sprintf("%s %s", prog, why), m
	}
	if strings.HasPrefix(prog, "mkfs") {
		return "mkfs creates a filesystem, destroying what is on the device", m
	}
	switch prog {
	case "eval":
		var parts []string
		for _, a := range args { // run-time parts stay marked, so a command made of them is refused
			parts = append(parts, a.Text)
		}
		return c.nested(strings.Join(parts, " ")), m
	case "init", "telinit":
		for _, a := range args {
			if a.Text == "0" || a.Text == "6" {
				return prog + " would shut down or reboot the machine", m
			}
		}
	case "systemctl", "loginctl":
		for _, a := range args {
			if powerSubcommands[a.Text] {
				return fmt.Sprintf("%s %s would change the machine's power state", prog, a.Text), m
			}
		}
	case "crontab":
		if hasFlag(args, 'r') {
			return "crontab -r deletes all scheduled jobs", m
		}
	case "chmod", "chown", "chgrp":
		if hasFlag(args, 'R', "--recursive") {
			return prog + " -R changes permissions or ownership of a whole tree", m
		}
	case "git":
		return gitRisk(args), m
	case "rsync":
		for _, a := range args {
			if strings.HasPrefix(a.Text, "--del") || strings.HasPrefix(a.Text, "--remove-source") {
				return "rsync with deletion options removes files", m
			}
		}
	case "tee":
		for _, a := range args {
			if !strings.HasPrefix(a.Text, "-") || a.Text == "-" {
				if why := c.writeTarget(a, "tee"); why != "" {
					return why, m
				}
			}
		}
	case "alias":
		for _, a := range args {
			if i := strings.IndexByte(a.Text, '='); i > 0 {
				if why := c.nested(a.Text[i+1:]); why != "" {
					return why, m
				}
			}
		}
	case "source", ".":
		for _, a := range args {
			if a.ProcSub {
				return "it sources a process substitution, which cannot be checked", m
			}
		}
	case "find":
		return c.find(args), m
	case "su", "runuser":
		if script, ok := findDashC(args, true); ok {
			return c.nested(script), m
		}
		return "", m
	}

	if spec, ok := wrappers[prog]; ok {
		rest, scripts, noExec := unwrap(prog, spec, args)
		for _, s := range scripts {
			if why := c.nested(s.Text); why != "" {
				return why, m
			}
		}
		if noExec || len(rest) == 0 {
			return "", m
		}
		return c.words(rest, heredocs, depth+1)
	}

	if shells[prog] {
		return c.shell(prog, args, heredocs, m)
	}
	if why, handled, stdin := c.interpreter(prog, args, heredocs); handled {
		m.shellStdin = stdin
		return why, m
	}
	return "", m
}

func (c *checker) nested(src string) string {
	sub := &checker{root: c.root, depth: c.depth + 1}
	return sub.script(src)
}

// hasFlag reports whether args hold the short option letter (alone or in a
// cluster such as -rf) or one of the long options (also as --name=value).
func hasFlag(args []shellparse.Word, short byte, long ...string) bool {
	for _, a := range args {
		t := a.Text
		if t == "--" {
			return false
		}
		if strings.HasPrefix(t, "--") {
			for _, l := range long {
				if t == l || strings.HasPrefix(t, l+"=") {
					return true
				}
			}
			continue
		}
		if strings.HasPrefix(t, "-") && len(t) > 1 && strings.IndexByte(t, short) > 0 {
			return true
		}
	}
	return false
}

func hasLong(args []shellparse.Word, names ...string) bool { return hasFlag(args, 0, names...) }

func texts(ws []shellparse.Word) []string {
	out := make([]string, len(ws))
	for i, w := range ws {
		out[i] = w.Text
	}
	return out
}

// unwrap strips a wrapper's own options (and env assignments) and returns the
// command it would run, any command strings it takes directly (env -S), and
// whether it only looks a command up without running it (command -v).
func unwrap(prog string, spec wrapSpec, args []shellparse.Word) (rest, scripts []shellparse.Word, noExec bool) {
	i := 0
	for i < len(args) {
		t := args[i].Text
		switch {
		case t == "--":
			i++
			goto done
		case strings.HasPrefix(t, "--") && len(t) > 2:
			name, hasEq := t[2:], false
			if j := strings.IndexByte(name, '='); j >= 0 {
				name, hasEq = name[:j], true
			}
			i++
			for _, l := range spec.long {
				if l == name && !hasEq && i < len(args) {
					if name == "split-string" {
						scripts = append(scripts, args[i])
					}
					i++
					break
				}
			}
			if hasEq && name == "split-string" {
				scripts = append(scripts, shellparse.Word{Text: t[strings.IndexByte(t, '=')+1:]})
			}
		case strings.HasPrefix(t, "-") && len(t) > 1:
			consumed := false
			for j := 1; j < len(t) && !consumed; j++ {
				if prog == "command" && (t[j] == 'v' || t[j] == 'V') {
					return nil, nil, true
				}
				if strings.IndexByte(spec.short, t[j]) >= 0 {
					consumed = true
					if j == len(t)-1 {
						if i+1 < len(args) {
							if prog == "env" && t[j] == 'S' {
								scripts = append(scripts, args[i+1])
							}
							i++
						}
					} else if prog == "env" && t[j] == 'S' {
						scripts = append(scripts, shellparse.Word{Text: t[j+1:]})
					}
				}
			}
			i++
		case spec.assigns && strings.Contains(t, "=") && !strings.HasPrefix(t, "="):
			i++
		default:
			goto done
		}
	}
done:
	i += spec.positional
	if i > len(args) {
		i = len(args)
	}
	return args[i:], scripts, false
}

// findDashC finds the script of a "-c" style invocation.
func findDashC(args []shellparse.Word, anywhere bool) (script string, found bool) {
	seenC := false
	for i := 0; i < len(args); i++ {
		t := args[i].Text
		if t == "--command" || strings.HasPrefix(t, "--command=") {
			if strings.HasPrefix(t, "--command=") {
				return t[len("--command="):], true
			}
			seenC = true
			continue
		}
		if strings.HasPrefix(t, "-") && !strings.HasPrefix(t, "--") && len(t) > 1 {
			if strings.ContainsRune(t[1:], 'c') {
				seenC = true
			}
			if t == "-o" || t == "+o" || t == "-O" || t == "+O" {
				i++
			}
			continue
		}
		if seenC {
			return t, true
		}
		if !anywhere {
			return "", false
		}
	}
	return "", false
}

func (c *checker) shell(prog string, args []shellparse.Word, heredocs []string, m member) (string, member) {
	for _, a := range args {
		if a.ProcSub {
			return "a shell is run on a process substitution, which cannot be checked", m
		}
	}
	if script, ok := findDashC(args, false); ok {
		return c.nested(script), m
	}
	// No -c: a script file runs unseen; otherwise the shell reads stdin.
	fileArg := false
	for i := 0; i < len(args); i++ {
		t := args[i].Text
		if strings.HasPrefix(t, "-") && len(t) > 1 || strings.HasPrefix(t, "+") && len(t) > 1 {
			if t == "-o" || t == "+o" || t == "-O" || t == "+O" {
				i++
			}
			if strings.HasPrefix(t, "-") && !strings.HasPrefix(t, "--") && strings.ContainsRune(t[1:], 's') {
				break
			}
			continue
		}
		fileArg = true
		break
	}
	if fileArg {
		return "", m
	}
	m.shellStdin = true
	for _, h := range heredocs {
		if why := c.nested(h); why != "" {
			return why, m
		}
	}
	return "", m
}

var (
	pythonLike = regexp.MustCompile(`^(python[0-9.]*|pypy[0-9]*)$`)
	nodeLike   = regexp.MustCompile(`^(node|nodejs|bun|deno)$`)
)

func (c *checker) interpreter(prog string, args []shellparse.Word, heredocs []string) (why string, handled, stdin bool) {
	var flags []string
	switch {
	case pythonLike.MatchString(prog):
		flags = []string{"-c"}
	case prog == "perl":
		flags = []string{"-e", "-E"}
	case prog == "ruby":
		flags = []string{"-e"}
	case nodeLike.MatchString(prog):
		flags = []string{"-e", "--eval", "-p", "--print", "eval"}
	case prog == "php":
		flags = []string{"-r"}
	case prog == "lua" || prog == "luajit":
		flags = []string{"-e"}
	case prog == "pwsh" || prog == "powershell":
		flags = []string{"-c", "-command", "-encodedcommand"}
	case prog == "awk" || prog == "gawk" || prog == "mawk" || prog == "nawk":
		for _, a := range args {
			if strings.Contains(a.Text, "system(") || a.Dynamic() && strings.Contains(a.Text, "system") {
				return "an awk program calls system()", true, false
			}
		}
		return "", true, false
	default:
		return "", false, false
	}
	for i, a := range args {
		t := strings.ToLower(a.Text)
		for _, f := range flags {
			cluster := (prog == "perl" || prog == "ruby") && len(t) > 2 && strings.HasPrefix(t, "-") &&
				!strings.HasPrefix(t, "--") && len(f) == 2 && t[len(t)-1] == f[1]
			if (t == f || cluster) && i+1 < len(args) {
				code := args[i+1]
				if strings.TrimSpace(strings.ReplaceAll(code.Text, shellparse.Dyn, "")) == "" {
					return fmt.Sprintf("%s %s is given code built at run time, so it cannot be checked", prog, f), true, false
				}
				if tok := riskyInline(code.Text); tok != "" {
					return fmt.Sprintf("inline %s code uses %q to touch files or processes; put it in a script file with write_file and run that, or re-run with --unsafe", prog, tok), true, false
				}
				return "", true, false
			}
		}
	}
	// Reads its program from stdin when no script file or inline code is given.
	nonOpt := 0
	for _, a := range args {
		if !strings.HasPrefix(a.Text, "-") || a.Text == "-" {
			nonOpt++
		}
	}
	if nonOpt == 0 || len(args) == 1 && args[0].Text == "-" {
		for _, h := range heredocs {
			if tok := riskyInline(h); tok != "" {
				return fmt.Sprintf("%s reads a program from stdin that uses %q to touch files or processes", prog, tok), true, true
			}
		}
		return "", true, true
	}
	return "", true, false
}

func riskyInline(code string) string {
	lc := strings.ToLower(code)
	for _, t := range inlineRiskTokens {
		if strings.Contains(lc, t) {
			return strings.TrimSpace(t)
		}
	}
	if rmWord.MatchString(lc) {
		return "rm"
	}
	return ""
}

func (c *checker) find(args []shellparse.Word) string {
	for i := 0; i < len(args); i++ {
		switch args[i].Text {
		case "-delete":
			return "find -delete removes files"
		case "-exec", "-execdir", "-ok", "-okdir":
			j := i + 1
			for j < len(args) && args[j].Text != ";" && args[j].Text != "+" {
				j++
			}
			if why, _ := c.words(args[i+1:j], nil, 1); why != "" {
				return why
			}
			i = j
		}
	}
	return ""
}

func gitRisk(args []shellparse.Word) string {
	i := 0
	for i < len(args) {
		t := args[i].Text
		if !strings.HasPrefix(t, "-") {
			break
		}
		switch t {
		case "-C", "--git-dir", "--work-tree", "--namespace", "--super-prefix", "--config-env":
			i++
		case "-c":
			if i+1 < len(args) {
				kv := args[i+1].Text
				if strings.HasPrefix(strings.ToLower(kv), "alias.") && strings.Contains(kv, "=!") {
					return "a git alias is defined to run a shell command"
				}
			}
			i++
		}
		i++
	}
	if i >= len(args) {
		return ""
	}
	if args[i].Dynamic() {
		return "the git subcommand is built at run time, so it cannot be checked"
	}
	sub, rest := args[i].Text, args[i+1:]
	firstPositional := ""
	for _, a := range rest {
		if !strings.HasPrefix(a.Text, "-") {
			firstPositional = a.Text
			break
		}
	}
	switch sub {
	case "reset":
		if hasLong(rest, "--hard") {
			return "git reset --hard discards uncommitted work"
		}
	case "clean":
		if !hasFlag(rest, 'n', "--dry-run") {
			return "git clean deletes untracked files"
		}
	case "push":
		if hasFlag(rest, 'f', "--force", "--force-with-lease", "--force-if-includes", "--delete", "--mirror", "--prune") || hasFlag(rest, 'd') {
			return "git push with force or delete rewrites or removes remote history"
		}
		for _, a := range rest {
			if !strings.HasPrefix(a.Text, "-") && (strings.HasPrefix(a.Text, "+") || strings.HasPrefix(a.Text, ":")) {
				return "git push with a forcing or deleting refspec rewrites or removes remote history"
			}
		}
	case "checkout":
		if hasFlag(rest, 'f', "--force") || hasLong(rest, "--") || firstPositional == "." {
			return "git checkout would discard working-tree changes"
		}
		for _, a := range rest {
			if a.Text == "--" {
				return "git checkout would discard working-tree changes"
			}
		}
	case "restore":
		if !(hasLong(rest, "--staged") && !hasLong(rest, "--worktree") && !hasFlag(rest, 'W')) {
			return "git restore would discard working-tree changes"
		}
	case "switch":
		if hasFlag(rest, 'f', "--force", "--discard-changes") {
			return "git switch would discard working-tree changes"
		}
	case "branch":
		if hasFlag(rest, 'D') || hasFlag(rest, 'd') && hasFlag(rest, 'f', "--force") {
			return "git branch -D deletes a branch regardless of its merge state"
		}
	case "stash":
		if firstPositional == "drop" || firstPositional == "clear" {
			return "git stash drop/clear destroys saved work"
		}
	case "reflog":
		if firstPositional == "expire" || firstPositional == "delete" {
			return "git reflog expire/delete destroys recovery history"
		}
	case "gc":
		for _, a := range rest {
			if strings.HasPrefix(a.Text, "--prune=") && a.Text != "--prune=never" {
				return "git gc --prune discards unreachable objects"
			}
		}
	case "prune", "filter-branch", "filter-repo":
		return "git " + sub + " rewrites or destroys history"
	case "update-ref":
		if hasFlag(rest, 'd') {
			return "git update-ref -d deletes a ref"
		}
	case "rm":
		if !hasLong(rest, "--cached") {
			return "git rm deletes files from the working tree"
		}
	}
	return ""
}

// writeTarget refuses a write that is built at run time, goes to a device, or
// leaves the workspace.
func (c *checker) writeTarget(w shellparse.Word, what string) string {
	t := w.Text
	if w.Dynamic() {
		return fmt.Sprintf("a %s target is built at run time, so it cannot be checked", what)
	}
	if t == "" {
		return ""
	}
	if strings.HasPrefix(t, "/dev/") {
		switch t {
		case "/dev/null", "/dev/zero", "/dev/stdout", "/dev/stderr", "/dev/tty":
			return ""
		}
		if strings.HasPrefix(t, "/dev/fd/") {
			return ""
		}
		return fmt.Sprintf("a %s writes to the device %s", what, t)
	}
	if c.root == "" {
		return ""
	}
	if c.outsideRoot(t) {
		return fmt.Sprintf("a %s writes to %s, outside the workspace", what, t)
	}
	return ""
}

func (c *checker) outsideRoot(t string) bool {
	if strings.HasPrefix(t, "~") {
		return true
	}
	p := t
	if runtime.GOOS == "windows" {
		// Git Bash spells C:\x as /c/x
		if len(p) >= 2 && p[0] == '/' && (len(p) == 2 || p[2] == '/') && (p[1]|0x20) >= 'a' && (p[1]|0x20) <= 'z' {
			p = strings.ToUpper(p[1:2]) + ":" + p[2:]
			if len(p) == 2 {
				p += `\`
			}
		}
		if strings.HasPrefix(p, "/") || strings.HasPrefix(p, `\`) {
			return true
		}
	}
	if !filepath.IsAbs(p) {
		p = filepath.Join(c.root, p)
	}
	p = filepath.Clean(p)
	return !within(c.root, p) && !within(realRoot(c.root), p)
}
