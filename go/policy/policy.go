// Package policy is the operator's tool policy: a deny-list (exact names and
// prefixes), a trust switch and budget limits.
//
// Where it comes from is the security property. Operator files
// (~/.sarina/policy.json and the file named by SARINA_POLICY_FILE) may set
// anything. A policy file inside the workspace is repository-controlled, and a
// prompt-injected agent can write there, so it can only TIGHTEN: it can add
// denials, lower budgets and turn trust off. It cannot remove a denial, raise a
// limit, turn trust on, or contribute environment variables. Nothing is ever
// read from a parent directory.
//
// Message-injection hooks (before/after tool, before prompt, after turn) are
// not supported; a file that lists them gets a note and they do nothing.
package policy

import (
	"encoding/json"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// EnvFile names an extra operator policy file.
const EnvFile = "SARINA_POLICY_FILE"

// WorkspaceFile is the only name looked for inside the workspace.
const WorkspaceFile = ".sarina-policy.json"

const maxFileBytes = 256 << 10

// Options says where to look.
type Options struct {
	Home      string              // for ~/.sarina/policy.json; empty = none
	Getenv    func(string) string // default os.Getenv
	Workspace string              // the workspace directory; "" = no workspace file
}

type prefixRule struct{ prefix, source string }

// Policy is the merged result.
type Policy struct {
	// Trusted false means: no MCP server is started and no operator agent
	// profile is loaded. Default true.
	Trusted bool
	// SafeEnv names host variables the operator exports to tool subprocesses.
	// Only operator files can set it. Names only are ever displayed.
	SafeEnv []string
	// Sources lists the files that were read.
	Sources []string
	// Notices are lines the operator should see.
	Notices []string
	// Fatal is set when an OPERATOR policy file exists but cannot be used. The
	// caller should refuse to start: silently dropping a deny-list is worse than
	// stopping.
	Fatal []string

	exact    map[string]string // lower-case name -> source
	prefixes []prefixRule
	set      config.BudgetConfig // operator limits: override the command line
	cap      config.BudgetConfig // workspace limits: can only lower
}

// Load reads the operator files, then the workspace file (tighten-only).
func Load(o Options) *Policy {
	getenv := o.Getenv
	if getenv == nil {
		getenv = os.Getenv
	}
	p := &Policy{Trusted: true, exact: map[string]string{}}
	var files []string
	if o.Home != "" {
		files = append(files, filepath.Join(o.Home, ".sarina", "policy.json"))
	}
	for _, f := range filepath.SplitList(getenv(EnvFile)) {
		if f != "" {
			files = append(files, f)
		}
	}
	seen := map[string]bool{}
	for _, f := range files {
		abs, err := filepath.Abs(f)
		if err != nil {
			abs = f
		}
		if seen[abs] {
			continue
		}
		seen[abs] = true
		p.loadFile(abs, true)
	}
	if o.Workspace != "" {
		ws := filepath.Join(o.Workspace, WorkspaceFile)
		if abs, err := filepath.Abs(ws); err == nil && !seen[abs] {
			p.loadFile(abs, false)
		}
	}
	sort.Strings(p.SafeEnv)
	return p
}

// parsed is one file's content after key normalisation.
type parsed struct {
	trusted   *bool
	deny      []string
	prefixes  []string
	budget    config.BudgetConfig
	safeEnv   []string
	hasHooks  bool
	problems  []string
	budgetSet bool
}

func normKey(k string) string {
	return strings.ReplaceAll(strings.ToLower(k), "_", "")
}

func parseFile(raw []byte) (*parsed, error) {
	var top map[string]any
	if err := json.Unmarshal(raw, &top); err != nil {
		return nil, fmt.Errorf("not a JSON object: %v", err)
	}
	out := &parsed{}
	for k, v := range top {
		switch normKey(k) {
		case "trusted":
			if b, ok := v.(bool); ok {
				out.trusted = &b
			} else {
				out.problems = append(out.problems, "trusted must be true or false")
			}
		case "denytools":
			out.deny = append(out.deny, stringItems(v, "denyTools", &out.problems)...)
		case "denytoolprefixes":
			out.prefixes = append(out.prefixes, stringItems(v, "denyToolPrefixes", &out.problems)...)
		case "safeenv":
			out.safeEnv = append(out.safeEnv, stringItems(v, "safeEnv", &out.problems)...)
		case "budget":
			m, ok := v.(map[string]any)
			if !ok {
				out.problems = append(out.problems, "budget must be an object")
				continue
			}
			parseBudget(m, out)
		case "hooks":
			out.hasHooks = true
		}
	}
	return out, nil
}

func stringItems(v any, what string, problems *[]string) []string {
	var items []any
	switch t := v.(type) {
	case string:
		items = []any{t}
	case []any:
		items = t
	default:
		*problems = append(*problems, what+" must be a list of strings")
		return nil
	}
	var out []string
	for _, it := range items {
		if s, ok := it.(string); ok && strings.TrimSpace(s) != "" {
			out = append(out, strings.TrimSpace(s))
		} else {
			*problems = append(*problems, what+" has an entry that is not a non-empty string")
		}
	}
	return out
}

// budgetField maps a canonical key (lower case, no underscores, no "max"
// prefix) to the BudgetConfig field it sets.
type budgetField struct {
	name string
	i    func(*config.BudgetConfig) **int
	f    func(*config.BudgetConfig) **float64
}

var budgetFields = []budgetField{
	{name: "totaltokens", i: func(b *config.BudgetConfig) **int { return &b.MaxTotalTokens }},
	{name: "inputtokens", i: func(b *config.BudgetConfig) **int { return &b.MaxInputTokens }},
	{name: "outputtokens", i: func(b *config.BudgetConfig) **int { return &b.MaxOutputTokens }},
	{name: "reasoningtokens", i: func(b *config.BudgetConfig) **int { return &b.MaxReasoningTokens }},
	{name: "totalcostusd", f: func(b *config.BudgetConfig) **float64 { return &b.MaxTotalCostUSD }},
	{name: "toolcalls", i: func(b *config.BudgetConfig) **int { return &b.MaxToolCalls }},
	{name: "delegatedtasks", i: func(b *config.BudgetConfig) **int { return &b.MaxDelegatedTasks }},
	{name: "modelcalls", i: func(b *config.BudgetConfig) **int { return &b.MaxModelCalls }},
	{name: "sessionturns", i: func(b *config.BudgetConfig) **int { return &b.MaxSessionTurns }},
}

func parseBudget(m map[string]any, out *parsed) {
	for k, v := range m {
		name := strings.TrimPrefix(normKey(k), "max")
		var field *budgetField
		for i := range budgetFields {
			if budgetFields[i].name == name {
				field = &budgetFields[i]
			}
		}
		if field == nil {
			out.problems = append(out.problems, fmt.Sprintf("budget key %q is not recognised", k))
			continue
		}
		n, ok := v.(float64) // a JSON boolean (or string) is not a number
		if !ok || math.IsNaN(n) || math.IsInf(n, 0) || n < 0 {
			out.problems = append(out.problems, fmt.Sprintf("budget %q must be a non-negative number", k))
			continue
		}
		out.budgetSet = true
		if field.i != nil {
			x := int(n)
			*field.i(&out.budget) = &x
		} else {
			x := n
			*field.f(&out.budget) = &x
		}
	}
}

func (p *Policy) loadFile(path string, operator bool) {
	f, err := os.Open(path)
	if err != nil {
		if !os.IsNotExist(err) && operator {
			p.Fatal = append(p.Fatal, fmt.Sprintf("cannot read policy file %s: %v", path, err))
		}
		return
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil && st.IsDir() {
		return
	}
	raw, err := io.ReadAll(io.LimitReader(f, maxFileBytes+1))
	if err == nil && len(raw) > maxFileBytes {
		err = fmt.Errorf("larger than %d bytes", maxFileBytes)
	}
	var pf *parsed
	if err == nil {
		pf, err = parseFile(raw)
	}
	if err != nil {
		if operator {
			p.Fatal = append(p.Fatal, fmt.Sprintf("policy file %s is unusable (%v); fix or remove it", path, err))
		} else {
			p.Notices = append(p.Notices, fmt.Sprintf("workspace policy file %s ignored: %v", path, err))
		}
		return
	}
	p.Sources = append(p.Sources, path)
	for _, pr := range pf.problems {
		p.Notices = append(p.Notices, fmt.Sprintf("policy file %s: %s", path, pr))
	}
	if pf.hasHooks {
		p.Notices = append(p.Notices, fmt.Sprintf("policy file %s: hooks are not supported and were ignored", path))
	}
	for _, n := range pf.deny {
		key := strings.ToLower(n)
		if _, dup := p.exact[key]; !dup {
			p.exact[key] = path
		}
	}
	for _, pre := range pf.prefixes {
		p.prefixes = append(p.prefixes, prefixRule{strings.ToLower(pre), path})
	}
	if operator {
		if pf.trusted != nil {
			p.Trusted = *pf.trusted
		}
		p.SafeEnv = append(p.SafeEnv, pf.safeEnv...)
		overlay(&p.set, pf.budget)
		return
	}
	// Workspace file: tighten only.
	if pf.trusted != nil && *pf.trusted {
		p.Notices = append(p.Notices, fmt.Sprintf("workspace policy file %s: trusted=true ignored (a workspace file can only tighten)", path))
	} else if pf.trusted != nil {
		p.Trusted = false
	}
	if len(pf.safeEnv) > 0 {
		p.Notices = append(p.Notices, fmt.Sprintf("workspace policy file %s: safeEnv ignored (only operator files may export environment variables)", path))
	}
	lower(&p.cap, pf.budget)
	if n := len(pf.deny) + len(pf.prefixes); n > 0 || pf.budgetSet || (pf.trusted != nil && !*pf.trusted) {
		p.Notices = append(p.Notices, fmt.Sprintf("workspace policy file %s tightens this session (%d denial rule(s), budget limits: %v)", path, n, pf.budgetSet))
	}
}

// overlay copies every limit set in src into dst.
func overlay(dst *config.BudgetConfig, src config.BudgetConfig) {
	for _, bf := range budgetFields {
		if bf.i != nil {
			if v := *bf.i(&src); v != nil {
				x := *v
				*bf.i(dst) = &x
			}
		} else if v := *bf.f(&src); v != nil {
			x := *v
			*bf.f(dst) = &x
		}
	}
}

// lower copies a limit into dst only when it is lower than what dst has.
func lower(dst *config.BudgetConfig, src config.BudgetConfig) {
	for _, bf := range budgetFields {
		if bf.i != nil {
			if v := *bf.i(&src); v != nil {
				if cur := *bf.i(dst); cur == nil || *v < *cur {
					x := *v
					*bf.i(dst) = &x
				}
			}
		} else if v := *bf.f(&src); v != nil {
			if cur := *bf.f(dst); cur == nil || *v < *cur {
				x := *v
				*bf.f(dst) = &x
			}
		}
	}
}

// ApplyBudget folds the policy's limits into b: operator limits replace the
// value, then workspace limits lower it. A limit that is not mentioned is left
// alone.
func (p *Policy) ApplyBudget(b *config.BudgetConfig) {
	overlay(b, p.set)
	lower(b, p.cap)
}

// Denied returns why a tool may not be used, or "" when it may. Matching is
// case-insensitive.
func (p *Policy) Denied(tool string) string {
	if p == nil {
		return ""
	}
	key := strings.ToLower(tool)
	if src, ok := p.exact[key]; ok {
		return fmt.Sprintf("tool %s is denied by the operator policy (%s)", tool, src)
	}
	for _, r := range p.prefixes {
		if strings.HasPrefix(key, r.prefix) {
			return fmt.Sprintf("tool %s is denied by the operator policy (prefix %q in %s)", tool, r.prefix, r.source)
		}
	}
	return ""
}

// DenyRules is the number of exact and prefix rules.
func (p *Policy) DenyRules() int { return len(p.exact) + len(p.prefixes) }
