// Command sarina is the Sarina agent's command-line entry point.
package main

import (
	"os"
	"path/filepath"
	"strings"

	_ "github.com/Artifex-Technologies/Sarina/go/chymaera"       // adds the chymaera-* commands
	_ "github.com/Artifex-Technologies/Sarina/go/chymaera/fleet" // adds the fleet-* commands (CLI only, never tools)
	"github.com/Artifex-Technologies/Sarina/go/cli"
	_ "github.com/Artifex-Technologies/Sarina/go/desktop" // adds the desktop-audit command
	_ "github.com/Artifex-Technologies/Sarina/go/doctor"  // adds the doctor command and the read-only sarina_doctor tool
	_ "github.com/Artifex-Technologies/Sarina/go/gui"     // adds the gui command
	_ "github.com/Artifex-Technologies/Sarina/go/helpers" // adds memory-search and voice-chat (Python helper)
	_ "github.com/Artifex-Technologies/Sarina/go/limbo"   // adds limbo_* tools and limbo-* commands
	_ "github.com/Artifex-Technologies/Sarina/go/service" // adds the service command
)

// aliasCommand maps the packaging names to commands: a binary (or link) called
// sarina-service behaves as `sarina service`, sarina-gui as `sarina gui`.
func aliasCommand(argv0 string) string {
	name := strings.TrimSuffix(strings.ToLower(filepath.Base(argv0)), ".exe")
	switch name {
	case "sarina-service":
		return "service"
	case "sarina-gui":
		return "gui"
	}
	return ""
}

func main() {
	args := os.Args[1:]
	if c := aliasCommand(os.Args[0]); c != "" {
		args = append([]string{c}, args...)
	}
	os.Exit(cli.Main(args, cli.Env{}))
}
