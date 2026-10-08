package doctor

import (
	"context"
	"flag"
	"fmt"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Importing this package adds the doctor command and the sarina_doctor tool.
func init() {
	tools.RegisterProvider(Tools)
	cli.Register("doctor", "read-only checkup of the whole setup: doctor [--json] [--cwd D] [--session-dir D]", command)
}

func command(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("doctor", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	asJSON := fs.Bool("json", false, "print the report as a JSON array of {id,status,finding,remedy}")
	cwd := fs.String("cwd", ".", "workspace directory")
	dir := fs.String("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)")
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	if fs.NArg() > 0 {
		fmt.Fprintf(env.Stderr, "sarina doctor: unexpected argument %q\n", fs.Arg(0))
		return 2
	}
	cs := Run(context.Background(), Options{Cwd: *cwd, SessionDir: *dir, Home: env.Home, Getenv: env.Getenv})
	if *asJSON {
		fmt.Fprintln(env.Stdout, JSON(cs))
	} else {
		fmt.Fprint(env.Stdout, Text(cs))
	}
	if HasFailure(cs) {
		return 1
	}
	return 0
}
