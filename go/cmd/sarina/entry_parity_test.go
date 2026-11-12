package main

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityEntry records how the program starts (testdata/parity/entry.json), for sarina-rs: the command each name it
// may be started as stands for, and which command lines a contained Sarina refuses, with its message and exit status (the
// built program run with a made-up lock; a command line it lets through is recorded as such, not run further).
func TestParityEntry(t *testing.T) {
	var cases []parity.Case
	for _, n := range []string{"sarina", "/usr/bin/sarina", "sarina-service", "/usr/bin/sarina-service", "SARINA-GUI", "sarina-gui.exe",
		"sarina-gui.EXE", "./x/Sarina-Service.exe", "sarina-service.exe.exe", "sarina-services", "", "/", "sarina-gui/", "sarina\xffgui"} {
		cases = append(cases, parity.Case{Fn: "alias", In: parity.Text(n), Out: aliasCommand(n)})
	}
	dir := t.TempDir()
	bin := filepath.Join(dir, "sarina")
	build := exec.Command("go", "build", "-o", bin, ".")
	if out, err := build.CombinedOutput(); err != nil {
		t.Fatalf("go build: %v\n%s", err, out)
	}
	lock := filepath.Join(dir, "contained")
	if err := os.WriteFile(lock, []byte(`{"reason":"a test lock: <b> & é"}`), 0o644); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"sarina", "sarina-service", "sarina-gui"} {
		link := filepath.Join(dir, name)
		if name != "sarina" {
			if err := os.Symlink(bin, link); err != nil {
				t.Fatal(err)
			}
		}
		for _, args := range [][]string{{}, {"chat", "hi"}, {"version"}, {"help"}, {"--help"}, {"-h"}, {"Version"}, {"version", "--json"}, {"--", "version"}} {
			cmd := exec.Command(link, args...)
			cmd.Env = append(os.Environ(), "SARINA_CONTAINED_PATH="+lock, "HOME="+dir)
			var stderr bytes.Buffer
			cmd.Stderr = &stderr
			cmd.Stdout = nil
			err := cmd.Run()
			code := 0
			var ee *exec.ExitError
			if errors.As(err, &ee) {
				code = ee.ExitCode()
			}
			out := map[string]any{"refused": code == 3, "code": code}
			if code == 3 {
				out["stderr"] = stderr.String()
			}
			cases = append(cases, parity.Case{Fn: "contained", In: map[string]any{"name": name, "args": args}, Out: out})
		}
	}
	parity.Fixture(t, "entry", cases)
}
