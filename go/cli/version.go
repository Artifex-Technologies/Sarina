package cli

import (
	"encoding/json"
	"fmt"
	"runtime/debug"
)

func init() {
	Register("version", "which build this is: the commit it was built from, when, and whether the tree had changes (--json)", versionCommand)
}

// BuildInfo is what the Go toolchain recorded about this binary: a build from a git checkout carries its commit, the commit's time
// and whether uncommitted changes were built in. An eval report names it, so a result is tied to the code that produced it.
type BuildInfo struct {
	Commit   string `json:"commit"`
	Time     string `json:"time"`
	Modified bool   `json:"modified"`
	Go       string `json:"go"`
}

func ReadBuild() BuildInfo {
	b := BuildInfo{Commit: "unknown"}
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return b
	}
	b.Go = info.GoVersion
	for _, s := range info.Settings {
		switch s.Key {
		case "vcs.revision":
			b.Commit = s.Value
		case "vcs.time":
			b.Time = s.Value
		case "vcs.modified":
			b.Modified = s.Value == "true"
		}
	}
	return b
}

func (b BuildInfo) String() string {
	c := b.Commit
	if len(c) > 12 {
		c = c[:12]
	}
	if b.Modified {
		c += " with uncommitted changes"
	}
	s := "sarina " + c
	if b.Time != "" {
		s += ", committed " + b.Time
	}
	if b.Go != "" {
		s += ", " + b.Go
	}
	return s
}

func versionCommand(env *Env, args []string) int {
	b := ReadBuild()
	if len(args) > 0 && args[0] == "--json" {
		out, _ := json.Marshal(b)
		fmt.Fprintln(env.Stdout, string(out))
		return 0
	}
	fmt.Fprintln(env.Stdout, b.String())
	return 0
}
