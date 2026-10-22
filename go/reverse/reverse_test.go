package reverse

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestPathsMapIntoTheSandboxOrAreRefused(t *testing.T) {
	d := t.TempDir()
	t.Setenv("LABYRINTH_REA_SAMPLES", d)
	t.Setenv("HOME", t.TempDir())
	if err := os.WriteFile(filepath.Join(d, "a.bin"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got, err := SandboxPath(filepath.Join(d, "a.bin")); err != nil || got != "/samples/a.bin" {
		t.Errorf("a sample: %q %v", got, err)
	}
	q := QuarantineDir()
	_ = os.MkdirAll(filepath.Join(q, "abc123def456"), 0o755)
	if got, err := SandboxPath(filepath.Join(q, "abc123def456", "payload")); err != nil || got != "/quarantine/abc123def456/payload" {
		t.Errorf("quarantine: %q %v", got, err)
	}
	for _, bad := range []string{"/etc/passwd", filepath.Join(d, "..", "x"), filepath.Join(os.Getenv("HOME"), ".ssh", "id_ed25519"), "/"} {
		if _, err := SandboxPath(bad); err == nil {
			t.Errorf("%q was accepted", bad)
		}
	}
	// a symlink inside the folder that points out of it
	_ = os.Symlink("/etc/passwd", filepath.Join(d, "link"))
	if _, err := SandboxPath(filepath.Join(d, "link")); err == nil {
		t.Error("a symlink out of the folder was followed")
	}
}

func TestArgsAreChecked(t *testing.T) {
	d := t.TempDir()
	t.Setenv("LABYRINTH_REA_SAMPLES", d)
	a, err := Args("decompile", filepath.Join(d, "x.sys"), "main", 0)
	if err != nil || strings.Join(a, " ") != "decompile /samples/x.sys main --format md --token-limit 3000" {
		t.Errorf("args: %v %v", a, err)
	}
	if a, err := Args("functions", filepath.Join(d, "x.sys"), "", 0); err != nil || strings.Join(a, " ") != "--exec nm -C --defined-only -n /samples/x.sys" {
		t.Errorf("functions: %v %v", a, err)
	}
	for _, c := range []string{"capture-process", "debug-with-gdb", "setup", "mcp", ""} {
		if _, err := Args(c, filepath.Join(d, "x"), "", 0); err == nil {
			t.Errorf("command %q was allowed", c)
		}
	}
	for _, target := range []string{"--procedure=x", "-h", "a;b", "a$(id)", "x\ny", strings.Repeat("a", 300)} {
		if _, err := Args("function", filepath.Join(d, "x"), target, 0); err == nil {
			t.Errorf("target %q was allowed", target)
		}
	}
}

func TestToolIsOfferedOnlyWithTheSandboxInstalled(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	t.Setenv("LABYRINTH_REA", "")
	t.Setenv("SARINA_REVERSE", "on")
	if len(Tools()) != 0 {
		t.Error("offered without labyrinth-rea")
	}
	t.Setenv("LABYRINTH_REA", "/bin/true")
	ts := Tools()
	if len(ts) != 2 || ts[1].Spec.Name != "reverse_engineer" || ts[1].Requires != 0 || ts[0].Spec.Name != "stage_binary" {
		t.Errorf("tool: %+v", ts)
	}
	t.Setenv("SARINA_REVERSE", "")
	if len(Tools()) != 0 {
		t.Error("offered without SARINA_REVERSE=on")
	}
}

func TestStageCopiesInstalledProgramsOnly(t *testing.T) {
	t.Setenv("LABYRINTH_REA_SAMPLES", t.TempDir())
	dst, err := Stage("/usr/bin/true")
	if err != nil {
		t.Skipf("no /usr/bin/true here: %v", err)
	}
	fi, err := os.Stat(dst)
	if err != nil || fi.Mode().Perm() != 0o400 {
		t.Fatalf("copy %v %v", fi, err)
	}
	if _, err := SandboxPath(dst); err != nil {
		t.Fatalf("the copy must be inside the analysis folder: %v", err)
	}
	for _, bad := range []string{"/etc/shadow", "/etc/passwd", "/proc/self/environ", "/home/nobody/secret", "/usr/../etc/passwd", "/nonexistent"} {
		if _, err := Stage(bad); err == nil {
			t.Fatalf("%s must be refused", bad)
		}
	}
}

func TestCalledFunctions(t *testing.T) {
	code := "int main(int param_1,long param_2){ if (1 < param_1) { iVar1 = check_code(*(char **)(param_2 + 8)); if (iVar1 != 0) { puts(\"access granted\"); } } return 0; }"
	got := calledFunctions(code, "main")
	if len(got) != 1 || got[0] != "check_code" {
		t.Fatalf("%v", got)
	}
}
