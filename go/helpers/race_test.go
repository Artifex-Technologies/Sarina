package helpers

import (
	"os"
	"path/filepath"
	"testing"
)

// A helper that dies at once ended start with "EOF" or with "read |0: file already closed": cmd.Wait closed the stdout
// pipe while the reader was still reading it (CI failed on it, 2026-11-17). Now the client owns its end of the pipe.
func TestHelperThatDiesReadsAsEOF(t *testing.T) {
	root := t.TempDir()
	bin := filepath.Join(root, "sarina-helpers")
	if err := os.WriteFile(bin, []byte(fakeScript), 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv(BinEnv, bin)
	dir := filepath.Join(root, "silent")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 300; i++ {
		_, err := startServer(dir, 0)
		if err == nil || err.Error() != "helper exited: EOF" {
			t.Fatalf("run %d: %v", i, err)
		}
	}
}
