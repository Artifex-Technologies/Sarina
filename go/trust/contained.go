package trust

import (
	"encoding/json"
	"os"
)

// ContainedPath is Heimdall's lock (Heimdall's docs/CONTAIN.md): while it exists Sarina does not start.
const ContainedPath = "/etc/heimdall/contained"

// EnvContained overrides the lock's path (tests).
const EnvContained = "SARINA_CONTAINED_PATH"

// Contained reports whether Heimdall has contained Sarina, and why.
func Contained() (bool, string) {
	p := os.Getenv(EnvContained)
	if p == "" {
		p = ContainedPath
	}
	b, err := os.ReadFile(p)
	if err != nil {
		return false, ""
	}
	var m struct {
		Reason string `json:"reason"`
	}
	if json.Unmarshal(b, &m) != nil || m.Reason == "" {
		return true, "no reason given"
	}
	return true, m.Reason
}
