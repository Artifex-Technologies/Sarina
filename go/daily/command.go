package daily

import (
	"context"
	"flag"
	"fmt"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/vault"
)

func init() {
	cli.Register("daily-report", "the day's status in her vault (Daily/YYYY-MM-DD.md): probe the computer and write it, or print it as it is when fresh enough: daily-report [--path VAULT] [--max-age MINUTES] [--no-write]", reportCommand)
	cli.Register("trends", "how the computer has been doing day by day, from the daily notes: disks, crashes, failed services: trends [--path VAULT] [--days N]", trendsCommand)
}

func reportCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("daily-report", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	path := fs.String("path", env.Getenv(vault.EnvVar), "the vault folder (default $"+vault.EnvVar+")")
	maxAge := fs.Int("max-age", 0, "print the note's status when it is at most this many minutes old; 0 probes every time")
	noWrite := fs.Bool("no-write", false, "probe and print; change nothing in the vault")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	now := time.Now()
	var n Note
	if *path != "" {
		n = Load(*path, now)
		if n.Fresh(now, time.Duration(*maxAge)*time.Minute) {
			fmt.Fprint(env.Stdout, n.Print(now, true))
			return 0
		}
	}
	s := Probe(context.Background())
	n.SetStatus(s)
	if *path != "" && !*noWrite {
		if err := Save(*path, n); err != nil {
			fmt.Fprintln(env.Stderr, "sarina daily-report:", err)
			return 1
		}
	}
	fmt.Fprint(env.Stdout, n.Print(now, false))
	return 0
}

// Print is the status for a reader (or the model): when it was taken, the lines, and the check-up's own notes of the day.
func (n Note) Print(now time.Time, fromNote bool) string {
	var b strings.Builder
	src := "just now"
	if fromNote {
		src = "from the day's note, no new probe"
	}
	fmt.Fprintf(&b, "Status of the computer as of %s today (%s; read-only facts, say what they show in plain words):\n", n.StatusAt, src)
	for _, l := range n.Status {
		b.WriteString(l + "\n")
	}
	var checkups []string
	for _, l := range n.Head {
		if t := strings.TrimSpace(l); strings.HasPrefix(t, "- ") && strings.Contains(t, "check-up:") {
			checkups = append(checkups, t)
		}
	}
	if len(checkups) > 0 {
		b.WriteString("Today's check-up notes:\n" + strings.Join(checkups, "\n") + "\n")
	}
	return b.String()
}

func trendsCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("trends", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	path := fs.String("path", env.Getenv(vault.EnvVar), "the vault folder (default $"+vault.EnvVar+")")
	days := fs.Int("days", 30, "how many days back")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *path == "" {
		fmt.Fprintln(env.Stderr, "sarina trends: no vault: give --path or set "+vault.EnvVar)
		return 2
	}
	fmt.Fprint(env.Stdout, Trends(ReadDays(*path, *days)))
	return 0
}
