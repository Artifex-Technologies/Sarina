package desktop

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/helpers"
	"github.com/Artifex-Technologies/Sarina/go/helpers/fakehelper"
)

func TestMain(m *testing.M) {
	fakehelper.MaybeRun()
	os.Exit(m.Run())
}

func useFakeHelper(t *testing.T, fingerprint string) string {
	t.Helper()
	exe, _ := os.Executable()
	t.Setenv(helpers.PythonEnv, exe)
	t.Setenv(fakehelper.EnvOn, "1")
	t.Setenv(fakehelper.EnvLabel, "Graphics")
	t.Setenv(fakehelper.EnvFingerprint, fingerprint)
	log := filepath.Join(t.TempDir(), "log")
	t.Setenv(fakehelper.EnvLog, log)
	return log
}

func drawDeps() deps {
	return deps{
		entries:  func() []DesktopEntry { return nil },
		packages: func() []InstalledPackage { return []InstalledPackage{toolPkg("somedraw", "a program for drawing")} },
	}
}

func TestRealHelperScorerClassifiesCachesAndClosesViaProtocol(t *testing.T) {
	log := useFakeHelper(t, PrototypesFingerprint())
	home := t.TempDir()
	out, _, code := runCmd(t, drawDeps(), home, "--model-dir", "m")
	if code != 0 || !strings.Contains(out, "somedraw") || !strings.Contains(out, "semantic") || strings.Contains(out, "unavailable") {
		t.Fatalf("code %d\n%s", code, out)
	}
	runCmd(t, drawDeps(), home, "--model-dir", "m")
	sent, _ := os.ReadFile(log)
	if n := strings.Count(string(sent), `"op":"classify"`); n != 1 {
		t.Fatalf("classify requests across two runs = %d, want 1 (second served from cache)\n%s", n, sent)
	}
}

func TestHelperWithDifferentPrototypesIsRefused(t *testing.T) {
	useFakeHelper(t, "some-other-set")
	out, _, code := runCmd(t, fakeDeps(), t.TempDir(), "--model-dir", "m")
	if code != 0 || !strings.Contains(out, "semantic inference unavailable:") || !strings.Contains(out, "prototypes differ") || !strings.Contains(out, "nmap") {
		t.Fatalf("code %d\n%s", code, out)
	}
}

func TestHelperModelLoadFailureFallsBack(t *testing.T) {
	useFakeHelper(t, PrototypesFingerprint())
	out, _, code := runCmd(t, fakeDeps(), t.TempDir(), "--model-dir", "bad")
	if code != 0 || !strings.Contains(out, "no model.onnx in bad") || !strings.Contains(out, "Python helper") {
		t.Fatalf("code %d\n%s", code, out)
	}
}
