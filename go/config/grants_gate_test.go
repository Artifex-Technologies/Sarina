package config

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/heimdall"
)

// With Heimdall's grant gate on, sarina.env cannot raise the tier past the owner's cap in the root-owned file.
func TestAccessTierCappedByHeimdallsGate(t *testing.T) {
	dir := t.TempDir()
	os.Chmod(dir, 0o755)
	p := filepath.Join(dir, "grants.json")
	os.WriteFile(p, []byte(`{"access_tier": 1}`), 0o644)
	oldP, oldO := heimdall.GrantsPath, heimdall.GrantsOwner
	heimdall.GrantsPath, heimdall.GrantsOwner = p, uint32(os.Getuid())
	defer func() { heimdall.GrantsPath, heimdall.GrantsOwner = oldP, oldO }()
	if got := AccessTierFromEnv(func(string) string { return "3" }); got != 1 {
		t.Fatalf("tier 3 in sarina.env with a cap of 1: got %d", got)
	}
	if got := AccessTierFromEnv(func(string) string { return "0" }); got != 0 {
		t.Fatalf("the cap never raises: got %d", got)
	}
}
