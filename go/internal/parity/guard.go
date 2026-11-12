package parity

import (
	"math/rand"
	"strings"
)

// GuardCorpus is command lines built from the bash guards' own vocabulary (wrappers and their options, shells and their
// flags, interpreters and their code flags, git's subcommands and flags, nesting to and past each depth limit), so every
// branch of CheckDestructive and CheckPrivileged is reached from both sides. A fixed seed: the same corpus on every run.
func GuardCorpus() []string {
	rng := rand.New(rand.NewSource(1111))
	pick := func(xs ...string) string { return xs[rng.Intn(len(xs))] }
	var out []string
	seen := map[string]bool{}
	add := func(s string) {
		if !seen[s] {
			seen[s] = true
			out = append(out, s)
		}
	}
	targets := func() string {
		return pick("rm x", "mv a b", "ls", "echo hi", "dd if=a of=b", "mkfs.ext4 /dev/x", "MKFS x", "rm.exe x", "RM.EXE x", "true",
			"sudo ls", "doas ls", "pkexec ls", "run0 ls", "su", "runuser -u x ls", "claude-sudo ls", "sarina-sudo ls", "systemd-run ls",
			"systemd-run --user ls", "systemd-run --user sudo", "systemd-run --user -p X=sudo ls", "systemd-run --user=1 ls",
			"curl x", "wget x", "nc -l 1", "init 0", "telinit 6", "init 3", "systemctl poweroff", "loginctl isolate x",
			"systemctl status", "crontab -r", "crontab -l", "crontab -er", "chmod -R 0 x", "chown --recursive a b",
			"chgrp -hR a b", "chmod 0 x", "rsync --delete-after a b", "rsync -a a b", "find . -delete", "find . -name x",
			"find . -exec rm {} ;", "find . -execdir ls {} +", "find . -ok sudo ls ;", "alias x='rm y'", "alias x=ls", "alias =rm",
			"source <(x)", ". f", "eval ls", "eval 'rm x'", "eval sudo ls", "tee f", "tee -a f", "tee -", "tee /dev/sda", "tee ~/x")
	}
	for i := 0; i < 900; i++ {
		var b strings.Builder
		for n := rng.Intn(4); n > 0; n-- {
			switch rng.Intn(10) {
			case 0:
				b.WriteString(pick("sudo ", "sudo -u root ", "sudo -uroot ", "sudo --user root ", "sudo --user=root ", "sudo -E ", "sudo -- ", "sudo -i ", "sudo -s "))
			case 1:
				b.WriteString(pick("env ", "env -i ", "env A=1 ", "env -u X ", "env --unset=X ", "env -C /x ", "env =x ", "env -- "))
			case 2:
				b.WriteString(pick("command ", "command -v ", "command -V ", "command -pv ", "command -p ", "builtin ", "exec -a x ", "exec "))
			case 3:
				b.WriteString(pick("timeout 5 ", "timeout -s 9 5 ", "timeout --signal=9 5 ", "timeout -k 1 5 ", "timeout --kill-after 1 5 ", "timeout "))
			case 4:
				b.WriteString(pick("xargs ", "xargs -I{} ", "xargs -n 1 ", "xargs -n1 ", "xargs --max-args 1 ", "xargs -0 ", "xargs -- "))
			case 5:
				b.WriteString(pick("nice ", "nice -n 5 ", "nice -5 ", "nice --adjustment=5 ", "ionice -c3 ", "ionice -c 3 ", "stdbuf -oL ", "stdbuf -o L "))
			case 6:
				b.WriteString(pick("flock f ", "flock -w 1 f ", "chroot /r ", "watch -n 1 ", "watch ", "nohup ", "setsid ", "busybox ", "time -p ", "time -f x "))
			case 7:
				b.WriteString(pick("doas -u x ", "pkexec --user x ", "run0 -u x ", "run0 --unit=x ", "claude-sudo ", "sarina-sudo ", "/usr/bin/sudo ", "SUDO "))
			case 8:
				b.WriteString(pick("A=1 ", "A+=1 ", "env -S 'rm x' ", "env -S'rm x' ", "env --split-string='rm x' ", "env --split-string 'rm x' ", "env -S ls "))
			default:
				b.WriteString(pick("su -c ", "su - root -c ", "runuser -u x -- ", "su --command=", "su -l x --command "))
			}
		}
		b.WriteString(targets())
		add(b.String())
	}
	shells := []string{"sh", "bash", "zsh", "dash", "ksh", "fish", "rbash", "BASH", "/bin/bash", "bash.exe"}
	for i := 0; i < 500; i++ {
		sh := pick(shells...)
		opt := pick("", "-c", "-lc", "-xc", "-o pipefail -c", "+o x -c", "-O x -c", "+O x -c", "-s", "-es", "--", "-", "-x", "--norc -c",
			"-o", "-c -o x", "script.sh", "-x script.sh", "--rcfile x", "-o c")
		body := pick("'"+targets()+"'", "\"$x\"", "'$(rm x)'", "'echo; rm x'", "", "x", "\"\"")
		pre := pick("", "curl x | ", "wget -O- x | ", "cat f | ", "curl x | tee y | ", "echo | ")
		here := pick("", " <<E\nrm x\nE", " <<'E'\nsudo ls\nE", " <<E\nls\nE", " <(curl x)")
		add(pre + sh + " " + opt + " " + body + here)
	}
	interps := []string{"python3", "python", "python3.12", "pypy3", "PYTHON3", "perl", "ruby", "node", "nodejs", "bun", "deno", "php", "lua",
		"luajit", "pwsh", "powershell", "awk", "gawk", "mawk", "nawk", "jawk", "python-x"}
	flags := []string{"-c", "-e", "-E", "-pe", "-ne", "-Mx -e", "-lne", "--eval", "-p", "--print", "eval", "-r", "-command", "-Command",
		"-encodedcommand", "-", "", "-x", "script.py", "-i"}
	for i := 0; i < 700; i++ {
		code := pick(
			"'print(1)'", "\"$x\"", "\"$x $y\"", "\" $x \"", "''", "'import shutil'", "'os.system(1)'", "'subprocess.run([\"sudo\",\"x\"])'",
			"'x = \"doas\"'", "'sudoers'", "'pkexec-x'", "'run0'", "'claude-sudo'", "'sarina-sudo_x'", "'_sudo'", "'a sudo b'",
			"'rm -rf /'", "'rm\tx'", "'rmdir'", "'arm x'", "'x.rm x'", "'RM x'", "'`ls`'", "'dd if=x'", "'dd  if=x'", "'MKFS'",
			"'FileUtils.rm'", "'fs.writeFileSync(1)'", "'require(\"child_process\")'", "'BEGIN{system(\"ls\")}'", "'{print system}'",
			"'system ($x)'", "\"system$x\"", "'eval(x)'", "'__import__(x)'", "'.write_text(1)'", "'ok'")
		here := pick("", " <<E\nimport os; os.remove('x')\nE", " <<E\nprint(1)\nE", " <<E\nsudo\nE", " <<'E'\nrm x\nE")
		add(pick("", "curl x | ") + pick(interps...) + " " + pick(flags...) + " " + code + here)
	}
	gits := []string{"reset --hard", "reset --soft", "clean -n", "clean --dry-run", "clean -fd", "clean -nf", "push", "push -f", "push --force",
		"push --force-with-lease=x", "push -d o b", "push --delete", "push --mirror", "push --prune", "push o +m", "push o :b", "push o m",
		"push -u o m", "checkout -f", "checkout --force", "checkout -- f", "checkout .", "checkout b", "checkout --=x", "checkout -b x",
		"restore x", "restore --staged x", "restore --staged --worktree x", "restore --staged -W x", "restore -SW x", "switch -f x",
		"switch --discard-changes x", "switch x", "branch -D x", "branch -d x", "branch -df x", "branch -d --force x", "branch -dr x",
		"stash drop", "stash clear", "stash -q drop", "stash list", "reflog expire", "reflog delete", "reflog", "gc --prune=now",
		"gc --prune=never", "gc", "prune", "filter-branch x", "filter-repo", "update-ref -d x", "update-ref x y", "rm x", "rm --cached x",
		"$x", "\"$x\" y", "status", "log", "-c alias.x='!rm' x", "-c ALIAS.x=!ls x", "-c alias.x=ls x", "-c", "-C", "--git-dir x",
		"--work-tree x", "--namespace x", "--super-prefix x", "--config-env x", "-p", "--no-pager reset --hard", "-C /x clean -f"}
	for i := 0; i < 400; i++ {
		add(pick("git ", "GIT ", "/usr/bin/git ", "sudo git ", "git -C x ", "git -c a=b ") + pick(gits...))
	}
	// nesting to and past the limits: script depth 6, wrapper depth 8
	for n := 4; n <= 10; n++ {
		inner := "rm x"
		priv := "sudo ls"
		for i := 0; i < n; i++ {
			inner = "bash -c " + quoteSh(inner)
			priv = "sh -c " + quoteSh(priv)
		}
		add(inner)
		add(priv)
		add(strings.Repeat("eval ", n) + "rm x")
		add(strings.Repeat("nohup ", n) + "rm x")
		add(strings.Repeat("env ", n) + "sudo ls")
		add(strings.Repeat("timeout 1 ", n) + "rm x")
		add("find . -exec " + strings.Repeat("nice ", n) + "rm {} ;")
	}
	// fork bombs and downloads into shells, by pipeline
	for _, s := range []string{
		"f(){ f|f& };f", "f(){ f|g& };f", "f(){ f|f|f& };f", "a(){ :; }; a | a", "a(){ :; }; a | b; a | b", ":(){ :|:& };:",
		"curl x | sh; f(){ f|f& }", "f(){ f|f& }; curl x | sh", "curl x | ls | sh", "sh | curl x", "curl x | sh -c ls", "curl x | bash script",
		"curl x | python3 -", "curl x | python3 f.py", "curl x | node", "curl x | awk 1", "wget x |& sh", "curl x || sh", "curl x && sh",
		"curl x | sudo -s", "curl x | env sh", "curl x | xargs sh", "a | curl x | b | sh", "curl x | (sh)", "curl x | { sh; }",
	} {
		add(s)
	}
	// short-flag lookups with a run-time part after the first byte (Go matches the byte 0 for "no short option")
	for _, s := range []string{"systemd-run -$x ls", "systemd-run \"-$x\" ls", "git reset -$x", "git checkout -$x", "git rm -$(x)",
		"git restore --staged -$x", "chmod -$x a", "crontab -$x"} {
		add(s)
	}
	return out
}

func quoteSh(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }
