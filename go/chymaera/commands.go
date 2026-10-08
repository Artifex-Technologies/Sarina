// Package chymaera registers the Chymaera ecosystem commands
// (chymaera-console, chymaera-analyze, chymaera-device-info,
// chymaera-merge-report). Importing it for its side effects adds them to the
// CLI. Everything here is read-only: nothing writes to a device or flashes.
package chymaera

import (
	"errors"
	"flag"
	"fmt"
	"io"
	"os"

	"github.com/Artifex-Technologies/Sarina/go/chymaera/analysis"
	"github.com/Artifex-Technologies/Sarina/go/chymaera/console"
	"github.com/Artifex-Technologies/Sarina/go/chymaera/firmware"
	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/helpers"
)

// TokenEnv names the environment variable holding the console token.
const TokenEnv = "CHYMAERA_CONSOLE_TOKEN"

// SemanticHelperReason is shown when --model-dir is given but the Python
// embedding helper (python/sarina_helpers) cannot be used; the detail follows.
const SemanticHelperReason = "semantic grouping needs the Python embedding helper"

func init() {
	cli.Register("chymaera-console", "show the Chymaera App console status and recent captures", consoleCommand)
	cli.Register("chymaera-analyze", "statistical triage of recent App captures (duplicate NFC UIDs, rare Sub-GHz frequencies)", analyzeCommand)
	cli.Register("chymaera-device-info", "check captured device_info text from a Chymaera Zero (--file F or stdin)", deviceInfoCommand)
	cli.Register("chymaera-merge-report", "report which firmware files a merge from upstream would touch", mergeReportCommand)
}

// parse runs fs over args; ok=false means the caller should return code.
func parse(fs *flag.FlagSet, args []string) (code int, ok bool) {
	if err := fs.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0, false
		}
		return 2, false
	}
	return 0, true
}

type connFlags struct {
	host  *string
	port  *int
	token *string
}

func addConnFlags(fs *flag.FlagSet, env *cli.Env) connFlags {
	return connFlags{
		host:  fs.String("host", console.DefaultHost, "console host (loopback)"),
		port:  fs.Int("port", console.DefaultPort, "console port"),
		token: fs.String("token", env.Getenv(TokenEnv), "console token (default $"+TokenEnv+")"),
	}
}

func (f connFlags) client() *console.Client {
	c := console.New()
	c.Host, c.Port, c.Token = *f.host, *f.port, *f.token
	c.Timeout = console.DefaultTimeout
	return c
}

func consoleCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("chymaera-console", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	cf := addConnFlags(fs, env)
	captures := fs.Int("captures", 10, "number of recent captures to list")
	if code, ok := parse(fs, args); !ok {
		return code
	}
	c := cf.client()
	if err := c.Connect(); err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	defer c.Close()
	status, err := c.Status()
	if err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	list, err := c.CaptureList(*captures)
	if err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	fmt.Fprintln(env.Stdout, console.RenderStatus(status, list))
	return 0
}

func analyzeCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("chymaera-analyze", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	cf := addConnFlags(fs, env)
	limit := fs.Int("limit", 100, "number of recent captures to analyse")
	modelDir := fs.String("model-dir", "", "embedding model directory (semantic grouping; needs the Python helper, see python/sarina_helpers)")
	threshold := fs.Float64("similarity-threshold", 0.75, "semantic grouping threshold")
	if code, ok := parse(fs, args); !ok {
		return code
	}
	c := cf.client()
	if err := c.Connect(); err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	defer c.Close()
	captures, err := c.CaptureList(*limit)
	if err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	reason := "no embedding model given (pass --model-dir; semantic grouping needs the Python embedding helper)"
	var groups [][]string
	if *modelDir != "" {
		reason = ""
		srv, err := helpers.StartServer(*modelDir)
		if err == nil {
			defer srv.Close()
			groups, err = srv.Group(captures, *threshold)
		}
		if err != nil {
			reason = fmt.Sprintf("%s (%s): %v", SemanticHelperReason, helpers.InstallHint, err)
			fmt.Fprintln(env.Stderr, "semantic grouping unavailable: "+reason)
		}
	}
	result := analysis.Analyze(captures, reason)
	result.SemanticGroups = groups
	fmt.Fprintln(env.Stdout, result.AsText())
	return 0
}

func deviceInfoCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("chymaera-device-info", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	file := fs.String("file", "", "file holding captured device_info text (default stdin)")
	expected := fs.String("expected-commit", "", "commit the device is expected to run")
	if code, ok := parse(fs, args); !ok {
		return code
	}
	var data []byte
	var err error
	if *file != "" {
		data, err = os.ReadFile(*file)
	} else {
		data, err = io.ReadAll(env.Stdin)
	}
	if err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	report := firmware.CheckDeviceInfo(firmware.ParseDeviceInfo(string(data)), *expected)
	fmt.Fprintln(env.Stdout, report.AsText())
	if report.OK() {
		return 0
	}
	return 1
}

func mergeReportCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("chymaera-merge-report", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	cwd := fs.String("cwd", ".", "firmware checkout")
	ref := fs.String("upstream-ref", "upstream/dev", "upstream ref to compare against")
	if code, ok := parse(fs, args); !ok {
		return code
	}
	report, err := firmware.MergeImpact(*cwd, *ref, "HEAD")
	if err != nil {
		fmt.Fprintln(env.Stdout, err)
		return 1
	}
	fmt.Fprintln(env.Stdout, report.AsText())
	return 0
}
