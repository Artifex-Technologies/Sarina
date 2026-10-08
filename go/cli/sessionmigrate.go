package cli

import (
	"fmt"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

func init() {
	Register("session-migrate", "rewrite retired tool names in saved sessions: session-migrate [--session-dir D] [--cwd D] [--dry-run]", sessionMigrateCommand)
}

func sessionMigrateCommand(env *Env, args []string) int {
	fs, cwd := cwdFlag("session-migrate", env)
	dir := fs.String("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)")
	dry := fs.Bool("dry-run", false, "report what would change without writing anything")
	if code, ok := parseOrExit(fs, args); !ok {
		return code
	}
	if *dir == "" {
		*dir = config.DefaultRuntime(*cwd).SessionDirectory
	}
	r, err := session.MigrateDir(*dir, *dry)
	if err != nil {
		fmt.Fprintf(env.Stderr, "session-migrate: %v\n", err)
		return exitFailure
	}
	verb := "changed"
	if *dry {
		verb = "would change"
	}
	for _, f := range r.Files {
		fmt.Fprintf(env.Stdout, "%s: %s\n", verb, f)
	}
	fmt.Fprintf(env.Stdout, "session-migrate: %d files scanned, %d %s, %d calls rewritten, %d results renamed, %d unreadable skipped\n",
		r.Scanned, r.Changed, verb, r.Calls, r.Results, r.Skipped)
	return exitOK
}
