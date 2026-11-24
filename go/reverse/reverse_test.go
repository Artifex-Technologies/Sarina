package reverse

import (
	"context"
	"os"
	"os/exec"
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

// Heimdall names a held file it lent her by where her sandbox sees it (/quarantine/<sha256>.sample): such a path is read as
// the folder it shows and checked like any other, so it cannot reach anything the folders do not hold.
func TestSandboxPathsAreReadAsTheirFoldersAndNothingElse(t *testing.T) {
	d := t.TempDir()
	t.Setenv("LABYRINTH_REA_SAMPLES", d)
	t.Setenv("HOME", t.TempDir())
	q := QuarantineDir()
	_ = os.MkdirAll(q, 0o755)
	_ = os.WriteFile(filepath.Join(q, "abc.sample"), []byte("x"), 0o440)
	_ = os.WriteFile(filepath.Join(d, "a.bin"), []byte("x"), 0o644)
	for in, want := range map[string]string{
		"/quarantine/abc.sample":            "/quarantine/abc.sample",
		"/quarantine//./abc.sample":         "/quarantine/abc.sample",
		"/samples/a.bin":                    "/samples/a.bin",
		"/samples/sub/../a.bin":             "/samples/a.bin",
		"/quarantine/x/../../samples/a.bin": "/samples/a.bin",
	} {
		if got, err := SandboxPath(in); err != nil || got != want {
			t.Errorf("%q: %q %v, want %q", in, got, err, want)
		}
	}
	_ = os.Symlink("/etc/passwd", filepath.Join(q, "link"))
	for _, bad := range []string{"/quarantine/../etc/passwd", "/quarantine/../../etc/shadow", "/samples/../x", "/quarantine/link",
		"/quarantinex/abc.sample", "/etc/passwd", "/proc/self/environ", filepath.Join(os.Getenv("HOME"), ".ssh", "id_ed25519")} {
		if got, err := SandboxPath(bad); err == nil {
			t.Errorf("%q was accepted as %q", bad, got)
		}
	}
	// the arguments labyrinth-rea gets name the folder inside the sandbox
	if a, err := Args("analyze", "/quarantine/abc.sample", "", 0); err != nil || a[1] != "/quarantine/abc.sample" {
		t.Fatal(a, err)
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
	if len(ts) != 3 || ts[2].Spec.Name != "reverse_engineer" || ts[2].Requires != 0 || ts[0].Spec.Name != "stage_binary" ||
		ts[1].Spec.Name != "draft_yara_rule" || ts[1].Requires != 0 {
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

// similar: the arguments name the file as the sandbox sees it and carry the Ghidra script; the script around it is run here
// with its sandbox paths (/work, /opt/ghidra, /tmp) moved into a test folder and a stand-in analyzer: no library, nothing
// runs; a report is all that is shown; a failure shows the log's error lines, never the rest; the throwaway folder goes.
func TestSimilarRunsOnlyWithALibraryAndShowsOnlyTheReport(t *testing.T) {
	d := t.TempDir()
	t.Setenv("LABYRINTH_REA_SAMPLES", d)
	_ = os.WriteFile(filepath.Join(d, "x.bin"), []byte("x"), 0o644)
	a, err := Args("similar", filepath.Join(d, "x.bin"), "ignored", 0)
	if err != nil || len(a) != 7 || strings.Join(a[:6], " ") != "--exec sh -c "+similarScript+" bsim /samples/x.bin" || a[6] != bsimScript {
		t.Fatalf("args: %q %v", a, err)
	}
	if !strings.Contains(bsimScript, "public class LabyrinthBSim extends GhidraScript") || !strings.Contains(bsimScript, "new InsertRequest()") {
		t.Fatal("the Ghidra script is not embedded")
	}
	root := t.TempDir()
	for _, p := range []string{"work/bsim", "opt/ghidra/support", "tmp"} {
		_ = os.MkdirAll(filepath.Join(root, p), 0o755)
	}
	moved := strings.NewReplacer("/work/", root+"/work/", "/opt/ghidra/", root+"/opt/ghidra/", "/tmp/", root+"/tmp/").Replace(similarScript)
	analyzer := filepath.Join(root, "opt/ghidra/support/analyzeHeadless")
	run := func(fake string) (string, string, error) {
		_ = os.WriteFile(analyzer, []byte("#!/bin/sh\n"+fake), 0o755)
		cmd := exec.Command("sh", "-c", moved, "bsim", "/samples/x.bin", "public class LabyrinthBSim {}")
		var out, errb strings.Builder
		cmd.Stdout, cmd.Stderr = &out, &errb
		err := cmd.Run()
		return out.String(), errb.String(), err
	}
	// the analyzer's arguments: "$d" p -import FILE ... -postScript LabyrinthBSim.java LIB OUT (OUT is the last one)
	report := `for a; do out=$a; done; echo "ghidra log line"; echo "args: $*" > "$1/../args"; cat "$1/LabyrinthBSim.java" > "$1/../java"; echo "1. alpha.bin: 6 of 8 functions" > "$out"` + "\n"
	if out, _, err := run(report); err != nil || !strings.Contains(out, "no similarity library yet") {
		t.Fatalf("no library: %q %v", out, err)
	}
	if _, err := os.Stat(filepath.Join(root, "tmp/args")); err == nil {
		t.Fatal("the analyzer ran without a library")
	}
	_ = os.WriteFile(filepath.Join(root, "work/bsim/library.mv.db"), []byte("db"), 0o640)
	out, errb, err := run(report)
	if err != nil || out != "1. alpha.bin: 6 of 8 functions\n" || errb != "" {
		t.Fatalf("report: %q %q %v", out, errb, err)
	}
	args, _ := os.ReadFile(filepath.Join(root, "tmp/args"))
	java, _ := os.ReadFile(filepath.Join(root, "tmp/java"))
	f := strings.Fields(strings.TrimPrefix(string(args), "args: "))
	if len(f) < 4 || !strings.HasPrefix(f[0], root+"/tmp/bsim.") || f[len(f)-1] != f[0]+"/out" ||
		!strings.Contains(string(args), " p -import /samples/x.bin -readOnly -deleteProject ") ||
		f[len(f)-2] != root+"/work/bsim/library.mv.db" || string(java) != "public class LabyrinthBSim {}" {
		t.Errorf("analyzer got %q, script %q", args, java)
	}
	out, errb, err = run("echo 'INFO fine'; echo 'ERROR the import failed'; echo 'java.lang.IllegalStateException: library busy'; exit 0\n")
	if err == nil || out != "" || errb != "ERROR the import failed\njava.lang.IllegalStateException: library busy\n" {
		t.Errorf("a failure: %q %q %v", out, errb, err)
	}
	if left, _ := filepath.Glob(filepath.Join(root, "tmp/bsim.*")); len(left) != 0 {
		t.Errorf("throwaway folders left: %v", left)
	}
}

// Heimdall's warnings on a draft are passed on, one per line; blank ones are dropped and none gives no heading (SAR-5).
func TestDraftNotes(t *testing.T) {
	for _, c := range []struct {
		in   []string
		want string
	}{
		{nil, ""},
		{[]string{" ", ""}, ""},
		{[]string{"the string $a is short", " YARA-X: slow pattern\n"}, "\nHeimdall's notes on it (the draft is kept):\n- the string $a is short\n- YARA-X: slow pattern"},
	} {
		if got := draftNotes(c.in); got != c.want {
			t.Errorf("draftNotes(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}
