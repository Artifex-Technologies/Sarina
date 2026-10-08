// Package firmware holds read-only advisory helpers for the Chymaera Zero
// firmware fork: parse device_info and report what a merge from upstream/dev
// would touch. Nothing here writes to a device; flashing stays a human action.
package firmware

import (
	"context"
	"fmt"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
	"unicode"

	"github.com/Artifex-Technologies/Sarina/go/chymaera/internal/pyfmt"
)

// DeviceInfoExpectations are the device_info fields that must hold a specific
// value on a healthy Chymaera build, in check order.
var DeviceInfoExpectations = []struct{ Key, Want string }{
	{"firmware_origin_fork", "Chymaera"},
	{"enclave_valid", "true"},
	{"radio_alive", "true"},
}

var riskRules = []struct{ prefix, area, why string }{
	{"assets/protobuf/", "wire-format", "RPC wire format - a unilateral change breaks stock qFlipper compatibility"},
	{"applications_user/", "external-app", "external app built against the published API; unaffected by upstream refactors"},
	{"furi/", "core", "OS core"},
	{"targets/", "core", "platform code"},
	{"applications/", "core", "firmware app tree"},
	{"lib/", "core", "vendored library"},
	{"site_scons/", "build", "build system"},
	{"scripts/", "build", "build system"},
	{"SConstruct", "build", "build system"},
	{"fbt", "build", "build system"},
	{"documentation/", "docs", "documentation"},
}

// ClassifyPath returns the descriptive (area, why) for one repo-relative path.
// It is a label only: merge risk comes from the upstream-existence test.
func ClassifyPath(path string) (area, why string) {
	for _, r := range riskRules {
		if strings.HasPrefix(path, r.prefix) {
			return r.area, r.why
		}
	}
	if strings.HasSuffix(path, ".md") {
		return "docs", "documentation"
	}
	return "other", "uncategorized"
}

// ParseDeviceInfo parses device_info serial output ("key<spaces>: value"
// lines). Unparseable lines are skipped: serial output carries banner text.
func ParseDeviceInfo(text string) map[string]string {
	fields := map[string]string{}
	for _, line := range splitLines(text) {
		i := strings.Index(line, ":")
		if i < 0 {
			continue
		}
		key := strings.TrimSpace(line[:i])
		if key == "" || strings.Contains(key, " ") {
			continue
		}
		fields[key] = strings.TrimSpace(line[i+1:])
	}
	return fields
}

func splitLines(s string) []string {
	s = strings.ReplaceAll(s, "\r\n", "\n")
	s = strings.ReplaceAll(s, "\r", "\n")
	return strings.Split(s, "\n")
}

// chymaeraVersion is the current format: <upstream>-CHY-<ours>[+ahead].
var chymaeraVersion = regexp.MustCompile(`^(\d+\.\d+\.\d+)-CHY-(dev|\d[\d.]*)(?:\+(\d+))?$`)

// legacyDevBuild is the previous `git describe --tags --always` format.
var legacyDevBuild = regexp.MustCompile(`-\d+-g[0-9a-f]{7,}$`)

// DeviceInfoReport is the result of checking parsed device_info fields.
// Warnings mean something is wrong and drive the exit code; Notes are
// observations that are not faults (a development build is normal).
type DeviceInfoReport struct {
	Fields   map[string]string
	Warnings []string
	Notes    []string
}

// OK reports whether there are no warnings.
func (r DeviceInfoReport) OK() bool { return len(r.Warnings) == 0 }

// AsText renders the report for humans.
func (r DeviceInfoReport) AsText() string {
	lines := []string{"# Chymaera Zero device", ""}
	if len(r.Fields) == 0 {
		lines = append(lines, "No device_info fields parsed.")
		return strings.Join(lines, "\n")
	}
	for _, key := range []string{"firmware_origin_fork", "firmware_version", "firmware_commit",
		"firmware_origin_git", "hardware_model", "enclave_valid", "radio_alive"} {
		if v, ok := r.Fields[key]; ok {
			lines = append(lines, key+": "+v)
		}
	}
	lines = append(lines, "")
	if len(r.Warnings) > 0 {
		lines = append(lines, "## Warnings")
		for _, w := range r.Warnings {
			lines = append(lines, "- "+w)
		}
	} else {
		lines = append(lines, "No warnings.")
	}
	if len(r.Notes) > 0 {
		lines = append(lines, "", "## Notes")
		for _, n := range r.Notes {
			lines = append(lines, "- "+n)
		}
	}
	return strings.Join(lines, "\n")
}

// CheckDeviceInfo checks parsed fields, optionally against an expected commit.
func CheckDeviceInfo(fields map[string]string, expectedCommit string) DeviceInfoReport {
	var warnings, notes []string
	for _, e := range DeviceInfoExpectations {
		actual, ok := fields[e.Key]
		switch {
		case !ok:
			warnings = append(warnings, e.Key+" missing from device_info")
		case strings.ToLower(actual) != strings.ToLower(e.Want):
			warnings = append(warnings, fmt.Sprintf("%s is %s, expected %s", e.Key, pyfmt.ReprString(actual), pyfmt.ReprString(e.Want)))
		}
	}

	if version, ok := fields["firmware_version"]; ok {
		if version == "unknown" || version == "" {
			warnings = append(warnings, "firmware_version is unknown - the build could not describe itself from git at all")
		} else if m := chymaeraVersion.FindStringSubmatch(version); m == nil {
			if legacyDevBuild.MatchString(version) {
				notes = append(notes, version+" uses the pre-2026-09-12 version format - the device predates the current firmware")
			} else {
				notes = append(notes, version+" is not a recognised Chymaera version string")
			}
		} else {
			notes = append(notes, "tracks upstream release "+m[1])
			if m[2] == "dev" {
				notes = append(notes, "no chy-* tag exists yet, so this build is not a release")
			} else if m[3] != "" {
				notes = append(notes, fmt.Sprintf("development build: %s commit(s) since chy-%s", m[3], m[2]))
			}
		}
	}

	if expectedCommit != "" {
		actual := fields["firmware_commit"]
		if actual != "" && !(strings.HasPrefix(expectedCommit, actual) || strings.HasPrefix(actual, expectedCommit)) {
			warnings = append(warnings, fmt.Sprintf("firmware_commit %s does not match expected %s - the device is not running this tree",
				pyfmt.ReprString(actual), pyfmt.ReprString(expectedCommit)))
		}
	}
	return DeviceInfoReport{Fields: fields, Warnings: warnings, Notes: notes}
}

// ---------------------------------------------------------------------------
// upstream merge impact
// ---------------------------------------------------------------------------

// MergeTierOrder lists risk tiers, worst first. Risk is decided by whether a
// path exists upstream, not by which directory it sits in.
var MergeTierOrder = []string{"high", "medium", "low"}

// RebasePolicyBudget: more non-additive first-party files than this means the
// "add directories, do not edit upstream files" rebase policy is violated.
const RebasePolicyBudget = 12

// MergeTierDescriptions explains each tier.
var MergeTierDescriptions = map[string]string{
	"high":   "changed on both sides - a merge will conflict here",
	"medium": "ours, and the file exists upstream - conflicts if upstream moves it",
	"low":    "does not exist upstream - additive, cannot conflict",
}

// FileRisk is one locally-changed path and what it would cost in a merge.
type FileRisk struct {
	Path            string
	Tier            string
	Area            string
	ExistsUpstream  bool
	ChangedUpstream bool
}

// MergeImpactReport is what merging UpstreamRef into the local tree would touch.
type MergeImpactReport struct {
	UpstreamRef     string
	Base            string
	UpstreamChanged []string
	LocalChanged    []string
	Risks           []FileRisk
}

// ByTier groups risks by tier.
func (r MergeImpactReport) ByTier() map[string][]FileRisk {
	g := map[string][]FileRisk{}
	for _, k := range r.Risks {
		g[k.Tier] = append(g[k.Tier], k)
	}
	return g
}

// Conflicting are the files that would actually conflict (tier high).
func (r MergeImpactReport) Conflicting() []FileRisk { return r.filter("high") }

// NonAdditive are the files we changed that exist upstream (high or medium).
func (r MergeImpactReport) NonAdditive() []FileRisk { return r.filter("high", "medium") }

func (r MergeImpactReport) filter(tiers ...string) []FileRisk {
	var out []FileRisk
	for _, k := range r.Risks {
		for _, t := range tiers {
			if k.Tier == t {
				out = append(out, k)
				break
			}
		}
	}
	return out
}

// PolicyBreached reports whether the non-additive count is over budget.
func (r MergeImpactReport) PolicyBreached() bool { return len(r.NonAdditive()) > RebasePolicyBudget }

// AsText renders the report for humans.
func (r MergeImpactReport) AsText() string {
	g := r.ByTier()
	lines := []string{
		fmt.Sprintf("# Merge impact: %s -> local firmware tree", r.UpstreamRef),
		"",
		"merge base: " + r.Base,
		fmt.Sprintf("upstream changed since base: %d file(s)", len(r.UpstreamChanged)),
		fmt.Sprintf("local changed since base: %d file(s)", len(r.LocalChanged)),
		fmt.Sprintf("would conflict (high): %d", len(g["high"])),
		fmt.Sprintf("ours, exists upstream (medium): %d", len(g["medium"])),
		fmt.Sprintf("additive, cannot conflict (low): %d", len(g["low"])),
		"",
	}
	if r.PolicyBreached() {
		lines = append(lines,
			fmt.Sprintf("## POLICY ALARM: %d first-party files exist upstream, over the budget of %d", len(r.NonAdditive()), RebasePolicyBudget),
			`The "add directories, do not edit upstream files" rebase policy is being violated. That matters more than any single file below.`,
			"")
	}
	if len(r.Risks) == 0 {
		lines = append(lines, "Nothing changed locally since the merge base - the merge should apply cleanly.")
		return rstrip(strings.Join(lines, "\n"))
	}
	for _, tier := range MergeTierOrder {
		items := g[tier]
		if len(items) == 0 {
			continue
		}
		lines = append(lines, fmt.Sprintf("## %s (%d)", tier, len(items)), MergeTierDescriptions[tier])
		sorted := append([]FileRisk(nil), items...)
		sort.SliceStable(sorted, func(i, j int) bool { return sorted[i].Path < sorted[j].Path })
		for _, k := range sorted {
			lines = append(lines, fmt.Sprintf("- %s [%s]", k.Path, k.Area))
		}
		lines = append(lines, "")
	}
	if len(r.Conflicting()) == 0 {
		lines = append(lines, "No file was changed on both sides - the merge should apply cleanly.")
	}
	return rstrip(strings.Join(lines, "\n"))
}

func rstrip(s string) string { return strings.TrimRightFunc(s, unicode.IsSpace) }

// git runs git in repo and returns stdout with surrounding whitespace trimmed.
func git(repo string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "git", args...)
	cmd.Dir = repo
	var stdout, stderr strings.Builder
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		detail := strings.TrimSpace(stderr.String())
		if detail == "" {
			detail = err.Error()
		}
		return "", fmt.Errorf("git %s failed: %s", strings.Join(args, " "), detail)
	}
	return strings.TrimSpace(stdout.String()), nil
}

func nonEmptyLines(s string) []string {
	var out []string
	for _, l := range splitLines(s) {
		if l = strings.TrimSpace(l); l != "" {
			out = append(out, l)
		}
	}
	return out
}

// MergeImpact reports what merging upstreamRef would touch, without merging.
//
// Risk is decided by whether a path exists upstream (git ls-tree), not by
// directory: a file absent from upstream cannot conflict wherever it lives.
func MergeImpact(repo, upstreamRef, head string) (MergeImpactReport, error) {
	if upstreamRef == "" {
		upstreamRef = "upstream/dev"
	}
	if head == "" {
		head = "HEAD"
	}
	abs, err := filepath.Abs(repo)
	if err == nil {
		if r, e := filepath.EvalSymlinks(abs); e == nil {
			abs = r
		}
	}
	base, err := git(abs, "merge-base", head, upstreamRef)
	if err != nil {
		return MergeImpactReport{}, err
	}
	diff := func(ref string) ([]string, error) {
		out, err := git(abs, "diff", "--name-only", base+".."+ref)
		return nonEmptyLines(out), err
	}
	upstreamChanged, err := diff(upstreamRef)
	if err != nil {
		return MergeImpactReport{}, err
	}
	localChanged, err := diff(head)
	if err != nil {
		return MergeImpactReport{}, err
	}
	tree, err := git(abs, "ls-tree", "-r", "--name-only", upstreamRef)
	if err != nil {
		return MergeImpactReport{}, err
	}
	upstreamFiles := map[string]bool{}
	for _, p := range nonEmptyLines(tree) {
		upstreamFiles[p] = true
	}
	moved := map[string]bool{}
	for _, p := range upstreamChanged {
		moved[p] = true
	}

	sorted := append([]string(nil), localChanged...)
	sort.Strings(sorted)
	var risks []FileRisk
	for _, p := range sorted {
		exists, changed := upstreamFiles[p], moved[p]
		tier := "medium"
		if !exists {
			tier = "low"
		} else if changed {
			tier = "high"
		}
		area, _ := ClassifyPath(p)
		risks = append(risks, FileRisk{Path: p, Tier: tier, Area: area, ExistsUpstream: exists, ChangedUpstream: changed})
	}
	return MergeImpactReport{UpstreamRef: upstreamRef, Base: base, UpstreamChanged: upstreamChanged, LocalChanged: localChanged, Risks: risks}, nil
}
