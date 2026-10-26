// Command sarina is the Sarina agent's command-line entry point.
package main

import (
	"os"
	"path/filepath"
	"strings"

	_ "github.com/Artifex-Technologies/Sarina/go/chymaera"       // adds the chymaera-* commands
	_ "github.com/Artifex-Technologies/Sarina/go/chymaera/fleet" // adds the fleet-* commands (CLI only, never tools)
	"github.com/Artifex-Technologies/Sarina/go/cli"
	_ "github.com/Artifex-Technologies/Sarina/go/council"     // adds the council command (CLI only, never a tool)
	_ "github.com/Artifex-Technologies/Sarina/go/cryptotools" // crypto_identify/decode/classic/hash/modern and sarina crypto when SARINA_CRYPTO=on
	_ "github.com/Artifex-Technologies/Sarina/go/daily"       // sarina daily-report and trends: the day's status in the vault, and how it changes
	_ "github.com/Artifex-Technologies/Sarina/go/desktop"     // adds the desktop-audit command
	_ "github.com/Artifex-Technologies/Sarina/go/doctor"      // adds the doctor command and the read-only sarina_doctor tool
	_ "github.com/Artifex-Technologies/Sarina/go/forge"       // adds forge_list/forge_write/forge_run when SARINA_FORGE=on: agents she writes, run only in Limbo
	_ "github.com/Artifex-Technologies/Sarina/go/gui"         // adds the gui command
	_ "github.com/Artifex-Technologies/Sarina/go/helpers"     // adds memory-search and voice-chat (Python helper)
	_ "github.com/Artifex-Technologies/Sarina/go/insight"     // sarina reflect: what she has learned about the owner from their talks
	_ "github.com/Artifex-Technologies/Sarina/go/limbo"       // adds limbo_* tools and limbo-* commands
	_ "github.com/Artifex-Technologies/Sarina/go/quantum"     // quantum_sim (SARINA_QUANTUM=on) and sarina quantum: exact circuits for teaching
	_ "github.com/Artifex-Technologies/Sarina/go/reverse"     // adds reverse_engineer (REA + Ghidra in a sandbox) when SARINA_REVERSE=on
	_ "github.com/Artifex-Technologies/Sarina/go/route"       // sarina route: what to look up before a spoken question (by meaning)
	_ "github.com/Artifex-Technologies/Sarina/go/service"     // adds the service command
	_ "github.com/Artifex-Technologies/Sarina/go/stego"       // stego_inspect and sarina stego when SARINA_CRYPTO=on
	_ "github.com/Artifex-Technologies/Sarina/go/sysinfo"     // adds system_status: how the computer is doing, read-only
	"github.com/Artifex-Technologies/Sarina/go/trust"
	_ "github.com/Artifex-Technologies/Sarina/go/vault" // adds the vault-* commands and, with SARINA_VAULT set, the vault tools
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
	// Heimdall's lock: a contained Sarina does not start (only "version" and "help" still answer)
	if on, why := trust.Contained(); on && !(len(args) > 0 && (args[0] == "version" || args[0] == "help" || args[0] == "--help")) {
		os.Stderr.WriteString("sarina: Heimdall has contained me (" + why + "). The owner releases me in Settings > Security, or: sudo heimdalld contain release\n")
		os.Exit(3)
	}
	os.Exit(cli.Main(args, cli.Env{}))
}
