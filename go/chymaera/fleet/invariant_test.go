package fleet

import (
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// The guarantee about fleet-launch is structural: the model can only call what
// the tool registry holds, and a launcher that is not importable from the tool
// layer cannot be exposed. These tests are source- and registry-level checks, not
// behaviour checks, for that reason. If one fails, do not weaken it: a
// prompt-injected session must never be able to start another agent.

var fleetToolName = regexp.MustCompile(`(?i)fleet[-_ ]?(launch|usage|status)|agent[-_]?launch`)

func TestNoFleetToolIsInTheToolRegistry(t *testing.T) {
	// This package is linked into this test binary, so its init() has run: if it
	// had registered a provider, Builtins() would show it.
	for _, tool := range tools.Builtins() {
		if fleetToolName.MatchString(tool.Spec.Name) {
			t.Errorf("tool %q is registered; fleet-launch must be CLI-only", tool.Spec.Name)
		}
	}
	names := tools.NewRegistry(tools.Builtins()...).Names()
	for _, n := range names {
		if fleetToolName.MatchString(n) {
			t.Errorf("registry exposes %q", n)
		}
	}
}

func TestFleetPackageRegistersNoToolProvider(t *testing.T) {
	// A synthetic registry must not contain a launcher even by description.
	for _, tool := range tools.Builtins() {
		d := strings.ToLower(tool.Spec.Description)
		if strings.Contains(d, "fleet-launch") || strings.Contains(d, "fleet_launch") || strings.Contains(d, "launch another agent") {
			t.Errorf("tool %q advertises the launcher", tool.Spec.Name)
		}
	}
}

func goRoot(t *testing.T) string {
	t.Helper()
	dir, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for {
		if _, err := os.Stat(filepath.Join(dir, "go.mod")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatal("go.mod not found")
		}
		dir = parent
	}
}

func TestNothingOutsideTheFleetPackageImportsOrNamesTheLauncher(t *testing.T) {
	root := goRoot(t)
	fleetDir := filepath.Join(root, "chymaera", "fleet")
	allowedImporter := filepath.Join(root, "cmd", "sarina", "main.go")
	scanned := 0
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			if path == fleetDir || d.Name() == ".git" || d.Name() == "vendor" {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") {
			return nil
		}
		raw, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		scanned++
		text := string(raw)
		rel, _ := filepath.Rel(root, path)
		// Only the binary's main package may import the fleet package (for its
		// registration side effect), and only as a blank import.
		if strings.Contains(text, "chymaera/fleet") && path != allowedImporter {
			t.Errorf("%s imports the fleet package; only cmd/sarina/main.go may", rel)
		}
		if path == allowedImporter {
			for _, line := range strings.Split(text, "\n") {
				if strings.Contains(line, "chymaera/fleet") && !strings.Contains(line, "_ \"") {
					t.Errorf("%s must import the fleet package only as a blank import: %s", rel, strings.TrimSpace(line))
				}
			}
		}
		// Test files may legitimately assert the absence of the name.
		if strings.HasSuffix(path, "_test.go") {
			return nil
		}
		for _, needle := range []string{"fleet-launch", "fleet_launch", "FleetLaunch", "fleetLaunch"} {
			if strings.Contains(text, needle) {
				t.Errorf("%s mentions %q outside the fleet package: the launcher must not be reachable from the tool/agent layer", rel, needle)
			}
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if scanned < 20 {
		t.Fatalf("only %d Go files scanned under %s; the scan is not looking at the tree", scanned, root)
	}
}

func TestToolLayerDoesNotDependOnTheLauncherPackage(t *testing.T) {
	// The tool registry and agent loop sit below the CLI; they must not even be
	// able to import this package (it imports cli, which imports them: a cycle
	// would be a compile error, but check the source anyway).
	root := goRoot(t)
	for _, pkg := range []string{"tools", "agent", "mcp", "skills", "memory", "limbo", "runkit", "service", "gui", "agents", "policy"} {
		entries, err := os.ReadDir(filepath.Join(root, pkg))
		if err != nil {
			continue // package may not exist yet
		}
		for _, e := range entries {
			if e.IsDir() || !strings.HasSuffix(e.Name(), ".go") {
				continue
			}
			raw, _ := os.ReadFile(filepath.Join(root, pkg, e.Name()))
			if strings.Contains(string(raw), "chymaera/fleet") {
				t.Errorf("%s/%s imports the fleet package", pkg, e.Name())
			}
		}
	}
}

func TestLauncherIsRegisteredAsACLICommandNotATool(t *testing.T) {
	out, _, code := runCLI(t, nil, "help")
	if code != 0 {
		t.Fatal(code)
	}
	for _, c := range []string{"fleet-usage", "fleet-status", "fleet-launch"} {
		if !strings.Contains(out, c) {
			t.Errorf("command %s not registered", c)
		}
	}
}
