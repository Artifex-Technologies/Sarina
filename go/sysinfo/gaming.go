package sysinfo

// Gaming: Labyrinth OS is built to be a gaming machine first, and Sarina shares its graphics card with the games. This file adds
//   - the "gaming" topic of system_status (Steam, Proton, the card and who holds its memory, game mode, the installed games),
//   - GameRunning, so background jobs (reflect, vault-ingest) stay off the card while a game runs,
//   - system_fix: a few named, fixed, reversible repairs at user level, the first tool that changes the real system. It is shell-gated
//     (--allow-shell), so a spoken question never has it; the model picks a name from a table and nothing else.

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// steamRoot is Steam's data folder for this user ("" when Steam has never run).
func steamRoot() string {
	h, _ := os.UserHomeDir()
	for _, d := range []string{filepath.Join(h, ".local", "share", "Steam"), filepath.Join(h, ".steam", "steam")} {
		if st, err := os.Stat(filepath.Join(d, "steamapps")); err == nil && st.IsDir() {
			return d
		}
	}
	return ""
}

func init() {
	root := steamRoot()
	if root == "" {
		h, _ := os.UserHomeDir()
		root = filepath.Join(h, ".local", "share", "Steam")
	}
	Topics["gaming"] = []step{
		{title: "Gaming setup check (labyrinth-gaming check: PASS, WARN or FAIL per item)", argv: []string{"labyrinth-gaming", "check"}},
		{title: "Graphics card now (use %, memory used MiB, memory total MiB, temperature C, power W)",
			argv: []string{"nvidia-smi", "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader"}},
		{title: "Programs holding graphics memory (pid, program, MiB)",
			argv: []string{"nvidia-smi", "--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader"}, lines: 14, cols: 110},
		{title: "Game mode (gamemoded; it is active only while a game started with gamemoderun is running, so inactive is normal when no game runs)", argv: []string{"gamemoded", "-s"}},
		{title: "Steam, Proton and Wine processes", argv: []string{"pgrep", "-a", "-f", `steam|wine|proton|gamescope|umu-run|\.exe`}, lines: 12, cols: 140},
		{title: "Proton versions available (system package, then your own compatibilitytools.d)",
			argv: []string{"ls", "-1", "/usr/share/steam/compatibilitytools.d", filepath.Join(root, "compatibilitytools.d")}, lines: 20},
		{title: "Installed Steam games (steamapps/common)", argv: []string{"ls", "-1", filepath.Join(root, "steamapps", "common")}, lines: 40},
	}
}

var (
	gpuAppRE = regexp.MustCompile(`^\s*(\d+),\s*(.*?),\s*(\d+)\s*MiB`)
	appIDRE  = regexp.MustCompile(`SteamLaunch AppId=(\d+)`)
)

// gamingSummary is the top of the gaming report: whether a game runs, and who holds the card's memory, Sarina's own model included.
// A small model reads the top of a report and skims the rest, and the graphics memory is the one thing she and the games compete for.
func gamingSummary(ctx context.Context) string {
	var parts []string
	if why, yes := GameRunning(ctx); yes {
		parts = append(parts, "A game seems to be running ("+why+").")
	} else {
		parts = append(parts, "No game is running right now.")
	}
	card := run(ctx, step{argv: []string{"nvidia-smi", "--query-gpu=memory.used,memory.total", "--format=csv,noheader,nounits"}})
	if f := strings.Split(card, ","); len(f) == 2 {
		parts = append(parts, fmt.Sprintf("The graphics card has %s of %s MiB in use.", strings.TrimSpace(f[0]), strings.TrimSpace(f[1])))
	}
	apps := run(ctx, step{argv: []string{"nvidia-smi", "--query-compute-apps=pid,process_name,used_memory", "--format=csv,noheader"}})
	ollama := 0
	for _, l := range strings.Split(apps, "\n") {
		if m := gpuAppRE.FindStringSubmatch(l); m != nil && strings.Contains(strings.ToLower(m[2]), "ollama") {
			n, _ := strconv.Atoi(m[3])
			ollama += n
		}
	}
	if ollama > 0 {
		parts = append(parts, fmt.Sprintf("Ollama, the model server Sarina answers from, holds %d MiB of it (her model loads to answer and is unloaded by itself after a few idle minutes; `ollama stop MODEL` frees it at once, and it loads again on the next question). Part of the card's use right now is that model answering this very question, so a high use figure here is not a game's.", ollama))
	} else {
		parts = append(parts, "Sarina's model is not loaded, so it takes no graphics memory at the moment.")
	}
	return strings.Join(parts, " ")
}

// GameRunning says whether a game is running: game mode is active, Steam has launched a game (its reaper process carries the AppId),
// or gamescope is up. Background work that would take the graphics card checks this first.
func GameRunning(ctx context.Context) (string, bool) {
	if s := run(ctx, step{argv: []string{"gamemoded", "-s"}}); strings.Contains(strings.ToLower(s), "is active") {
		return "game mode is active", true
	}
	if out := run(ctx, step{argv: []string{"pgrep", "-a", "-f", "SteamLaunch AppId="}}); out != "" {
		for _, l := range strings.Split(out, "\n") {
			if strings.Contains(l, "pgrep") { // the pgrep itself, or a shell quoting it
				continue
			}
			if m := appIDRE.FindStringSubmatch(l); m != nil {
				return "Steam launched a game (AppId " + m[1] + ")", true
			}
		}
	}
	if out := run(ctx, step{argv: []string{"pgrep", "-x", "gamescope"}}); out != "" && out != "(not available here)" && out != "(nothing to report)" {
		return "gamescope is running", true
	}
	return "", false
}

// ---- system_fix: named repairs ----------------------------------------------------------------------------------------------------

// Fix is one repair: fixed commands, run in order, at this user's level; nothing is deleted and each one is undone by time or a reboot.
type Fix struct {
	Name  string
	What  string     // for the owner and the model: what it does and what it costs
	Steps [][]string // the commands, in order
	Check []string   // run afterwards and shown, so the result is evidence, not a claim
	// Only, when set, must be true before the steps run (so a repair never turns on something the owner turned off).
	Only func(ctx context.Context) (string, bool)
}

// Fixes is the whole table. Adding a row is the only way a new action exists; the model cannot name a command.
var Fixes = []Fix{
	{
		Name:  "restart_audio",
		What:  "restart the sound system for this user (PipeWire, its PulseAudio bridge and WirePlumber): for no sound, a device stuck on the wrong output, or crackling after sleep. Audio stops for about a second and programs reconnect by themselves.",
		Steps: [][]string{{"systemctl", "--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"}},
		Check: []string{"systemctl", "--user", "is-active", "pipewire", "pipewire-pulse", "wireplumber"},
	},
	{
		Name:  "restart_bluetooth",
		What:  "turn the Bluetooth controller off and on again (bluetoothctl): for a controller or headset that will not reconnect. Paired devices reconnect by themselves. Only when Bluetooth is already on: turning it on is the owner's choice.",
		Steps: [][]string{{"bluetoothctl", "power", "off"}, {"bluetoothctl", "power", "on"}},
		Check: []string{"bluetoothctl", "show"},
		Only: func(ctx context.Context) (string, bool) {
			s := run(ctx, step{argv: []string{"bluetoothctl", "show"}})
			if !strings.Contains(s, "Powered: yes") {
				return "Bluetooth is off (or there is no controller); turning it on is the owner's choice, not a repair", false
			}
			return "", true
		},
	},
}

var fixNameRE = regexp.MustCompile(`^[a-z_]{3,40}$`)

// execFix runs one command of a repair; a test replaces it so the developer's own audio is not restarted.
var execFix = func(ctx context.Context, argv []string) string { return run(ctx, step{argv: argv}) }

func findFix(name string) (Fix, bool) {
	name = strings.ToLower(strings.TrimSpace(name))
	if !fixNameRE.MatchString(name) {
		return Fix{}, false
	}
	for _, f := range Fixes {
		if f.Name == name {
			return f, true
		}
	}
	return Fix{}, false
}

func fixNames() []string {
	out := make([]string, 0, len(Fixes))
	for _, f := range Fixes {
		out = append(out, f.Name)
	}
	return out
}

// ApplyFix runs one named repair and reports each command's output and the check afterwards.
func ApplyFix(ctx context.Context, name string) (string, error) {
	f, ok := findFix(name)
	if !ok {
		return "", fmt.Errorf("no such repair %q: the repairs are %s", name, strings.Join(fixNames(), ", "))
	}
	if f.Only != nil {
		if why, ok := f.Only(ctx); !ok {
			return "", fmt.Errorf("%s not done: %s", f.Name, why)
		}
	}
	var b strings.Builder
	fmt.Fprintf(&b, "## %s\n%s\n\n", f.Name, f.What)
	for _, argv := range f.Steps {
		out := execFix(ctx, argv)
		fmt.Fprintf(&b, "$ %s\n%s\n\n", strings.Join(argv, " "), out)
	}
	if len(f.Check) > 0 {
		fmt.Fprintf(&b, "## Check afterwards\n$ %s\n%s\n\n", strings.Join(f.Check, " "), execFix(ctx, f.Check))
	}
	b.WriteString("[Evidence only: say what the check shows. If it does not say the parts are active, the repair did not take; say so.]")
	return b.String(), nil
}

func fixTool() tools.Tool {
	var desc strings.Builder
	desc.WriteString("Do one named repair on this computer (user level, reversible), then report what its check shows. Repairs: ")
	for i, f := range Fixes {
		if i > 0 {
			desc.WriteString("; ")
		}
		desc.WriteString(f.Name + " (" + strings.SplitN(f.What, ":", 2)[0] + ")")
	}
	desc.WriteString(". Nothing else can be run through it.")
	return tools.Tool{
		Requires: tools.PermShell, Class: tools.ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "system_fix",
			Description: desc.String(),
			Parameters: map[string]any{"type": "object", "required": []string{"action"}, "properties": map[string]any{
				"action": map[string]any{"type": "string", "enum": fixNames()},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			name, _ := a["action"].(string)
			text, err := ApplyFix(ctx, name)
			if err != nil {
				return "", nil, err
			}
			return text, map[string]any{"action": "system_fix", "fix": name}, nil
		},
	}
}

func init() {
	cli.Register("system-fix", "do one named, reversible repair at user level (the same table the system_fix tool uses): system-fix --list | ACTION", fixCommand)
}

func fixCommand(env *cli.Env, args []string) int {
	if len(args) == 1 && args[0] == "--list" {
		for _, f := range Fixes {
			fmt.Fprintf(env.Stdout, "%-20s %s\n", f.Name, f.What)
		}
		return 0
	}
	if len(args) != 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina system-fix --list | ACTION ("+strings.Join(fixNames(), ", ")+")")
		return 2
	}
	out, err := ApplyFix(context.Background(), args[0])
	if err != nil {
		fmt.Fprintln(env.Stderr, err)
		return 2
	}
	fmt.Fprintln(env.Stdout, out)
	return 0
}
