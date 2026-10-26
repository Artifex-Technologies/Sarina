package reverse

import (
	"context"
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

func TestNetworkMapFindsAddressesNotFileNames(t *testing.T) {
	text := strings.Join([]string{
		"connecting to https://update.example-c2.ru/gate.php?id=1",
		"fallback 203.0.113.50:8443 and 2001:db8::7",
		"libc.so.6", "main.go", "setup.py", "version 1.2.3", "GCC: (GNU) 14.2.1",
		"http://www.w3.org/2000/svg", "beacon.badhost.top", "abcdefghijklmnop.onion",
		"0.0.0.0", "127.0.0.1",
	}, "\n")
	nm := FindAddresses(text)
	if len(nm.URLs) != 2 || nm.URLs[0] != "http://www.w3.org/2000/svg" || !strings.HasPrefix(nm.URLs[1], "https://update.example-c2.ru/gate.php") {
		t.Errorf("urls: %v", nm.URLs)
	}
	if strings.Join(nm.IPs, ",") != "2001:db8::7,203.0.113.50:8443" {
		t.Errorf("ips: %v", nm.IPs)
	}
	for _, h := range nm.Hosts {
		if strings.Contains(h, "libc") || strings.Contains(h, "main.go") || strings.Contains(h, "setup.py") || strings.Contains(h, "w3.org") {
			t.Errorf("a file name or standard taken for a host: %v", nm.Hosts)
		}
	}
	if strings.Join(nm.Hosts, ",") != "beacon.badhost.top,update.example-c2.ru" || len(nm.Onions) != 1 {
		t.Errorf("hosts %v onions %v", nm.Hosts, nm.Onions)
	}
	r := nm.Report(context.Background(), "x.bin")
	if !strings.Contains(r, "nothing was run and no name was looked up") || !strings.Contains(r, "hidden service") {
		t.Errorf("report: %s", r)
	}
	if r := (NetMap{}).Report(context.Background(), "y"); !strings.Contains(r, "No web addresses") {
		t.Errorf("empty: %s", r)
	}
	d := t.TempDir()
	t.Setenv("LABYRINTH_REA_SAMPLES", d)
	if a, err := Args("network-map", filepath.Join(d, "x.bin"), "", 0); err != nil || a[0] != "--exec" || a[len(a)-1] != "/samples/x.bin" {
		t.Errorf("args: %v %v", a, err)
	}
}

func TestTheOverviewIsCondensedAndNamesTheFunctions(t *testing.T) {
	md := "## evidence_id\n\nev_1\n\n## subject.name\n\ndemo.bin\n\n## analysis_profile.provider\n\n| id | ghidra |\n\n## normalized_result.segments\n\n| .text | 0x1000 |\n\n## normalized_result.procedure_count\n\n9\n\n## limitations\n\nlong text\n"
	got := condenseOverview(md)
	if !strings.Contains(got, "demo.bin") || !strings.Contains(got, "procedure_count\n\n9") || strings.Contains(got, "ghidra") || strings.Contains(got, ".text") || strings.Contains(got, "long text") {
		t.Fatalf("condensed: %q", got)
	}
	if condenseOverview("plain text") != "plain text" {
		t.Fatal("an overview in another shape is kept whole")
	}
	nm := "0000000000001000 T _init\n0000000000001070 t deregister_tm_clones\n0000000000001130 t frame_dummy\n0000000000001139 T main\n0000000000001150 t check_key\n0000000000002000 R _IO_stdin_used\n"
	f := ownFunctions(nm)
	if !strings.Contains(f, "main (1139), check_key (1150)") || strings.Contains(f, "frame_dummy") || strings.Contains(f, "_init") || !strings.Contains(f, "decompile") {
		t.Fatalf("functions: %q", f)
	}
	if !strings.Contains(ownFunctions(""), "stripped") {
		t.Fatal("a stripped program is said to be stripped")
	}
}
