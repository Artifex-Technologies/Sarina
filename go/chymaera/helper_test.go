package chymaera

import (
	"fmt"
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

func TestAnalyzeWithHelperPrintsSemanticGroups(t *testing.T) {
	exe, _ := os.Executable()
	t.Setenv(helpers.PythonEnv, exe)
	t.Setenv(fakehelper.EnvOn, "1")
	log := filepath.Join(t.TempDir(), "log")
	t.Setenv(fakehelper.EnvLog, log)
	port, _ := serve(t, "", []any{
		map[string]any{"type": "nfc", "name": "a.nfc", "meta": `{"uid":"AA"}`},
		map[string]any{"type": "nfc", "name": "b.nfc", "meta": `{"uid":"BB"}`},
	})
	code, out, errOut := run([]string{"chymaera-analyze", "--port", fmt.Sprint(port), "--model-dir", "m", "--similarity-threshold", "0.6"}, "", nil)
	if code != 0 || errOut != "" {
		t.Fatalf("code %d err %q", code, errOut)
	}
	for _, want := range []string{"total captures: 2", "## Semantic groups (similar captures)", "- a.nfc, b.nfc"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	if strings.Contains(out, "unavailable") {
		t.Fatalf("must not claim unavailable:\n%s", out)
	}
	sent, _ := os.ReadFile(log)
	if !strings.Contains(string(sent), `"op":"group"`) || !strings.Contains(string(sent), `"threshold":0.6`) {
		t.Fatalf("helper request log: %s", sent)
	}
}

func TestAnalyzeHelperLoadFailureKeepsStatisticsAndSaysWhy(t *testing.T) {
	exe, _ := os.Executable()
	t.Setenv(helpers.PythonEnv, exe)
	t.Setenv(fakehelper.EnvOn, "1")
	port, _ := serve(t, "", []any{map[string]any{"type": "nfc", "name": "a.nfc", "meta": `{"uid":"AA"}`}})
	code, out, errOut := run([]string{"chymaera-analyze", "--port", fmt.Sprint(port), "--model-dir", "bad"}, "", nil)
	if code != 0 || !strings.Contains(errOut, "no model.onnx in bad") || !strings.Contains(out, "total captures: 1") ||
		!strings.Contains(out, "Semantic grouping unavailable: semantic grouping needs the Python embedding helper") {
		t.Fatalf("code %d err %q out %q", code, errOut, out)
	}
}
