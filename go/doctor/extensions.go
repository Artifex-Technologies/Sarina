package doctor

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/mcp"
	"github.com/Artifex-Technologies/Sarina/go/policy"
	"github.com/Artifex-Technologies/Sarina/go/skills"
)

func presence(path string) string {
	if st, err := os.Stat(path); err == nil && !st.IsDir() {
		return "present"
	}
	return "absent"
}

func (r *runner) extensions() {
	// skills
	opt := skills.Options{Home: r.home, Getenv: r.env, Workspace: r.o.Cwd, SystemDir: r.o.SystemSkillsDir}
	cat := skills.Discover(opt)
	n := 0
	for _, root := range skills.Roots(opt) {
		if st, err := os.Stat(root.Dir); err == nil && st.IsDir() {
			n++
		}
	}
	finding := fmt.Sprintf("%d skill(s) loaded from %d existing director(ies)", len(cat.Skills), n)
	if len(cat.Problems) > 0 {
		var ps []string
		for _, p := range cat.Problems {
			ps = append(ps, filepath.Base(p.Path)+": "+p.Message)
		}
		r.add("ext_skills", Warn, fmt.Sprintf("%s; %d unusable: %s", finding, len(ps), nameList(ps)),
			"fix or remove the named skill files (`sarina skill list` shows the same problems)")
	} else {
		r.add("ext_skills", Pass, finding, "")
	}

	// mcp.json
	mc := mcp.Load(mcp.LoadOptions{Home: r.home, Getenv: r.env, Workspace: r.o.Cwd})
	mpath := filepath.Join(r.home, ".sarina", "mcp.json")
	if len(mc.Problems) > 0 {
		r.add("ext_mcp", Warn, fmt.Sprintf("mcp.json %s; problems: %s", presence(mpath), strings.Join(mc.Problems, "; ")),
			"fix the JSON in the named file; a broken MCP file only disables MCP servers")
	} else {
		r.add("ext_mcp", Pass, fmt.Sprintf("mcp.json %s; %d server(s) defined", presence(mpath), len(mc.Servers)), "")
	}

	// policy.json: a malformed operator file stops startup
	pol := policy.Load(policy.Options{Home: r.home, Getenv: r.env, Workspace: r.o.Cwd})
	ppath := filepath.Join(r.home, ".sarina", "policy.json")
	switch {
	case len(pol.Fatal) > 0:
		r.add("ext_policy", Fail, "operator policy would stop startup: "+strings.Join(pol.Fatal, "; "),
			"repair the JSON in the named policy file; do not delete denials just to get started")
	case len(pol.Notices) > 0:
		r.add("ext_policy", Warn, fmt.Sprintf("policy.json %s; %d notice(s): %s", presence(ppath), len(pol.Notices), strings.Join(pol.Notices, "; ")),
			"read the notices; a workspace policy file can only tighten the operator policy")
	default:
		r.add("ext_policy", Pass, fmt.Sprintf("policy.json %s; %d denial rule(s), trusted=%v", presence(ppath), pol.DenyRules(), pol.Trusted), "")
	}
}
