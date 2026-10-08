// Package analysis does statistical analysis of Chymaera_Zero_App captures.
//
// It works only with what the App already provides through capture.list: each
// record's "meta" JSON field (see that repo's docs/SARINA_API.md). Sarina does
// not parse .nfc/.sub files itself; that stays the App's job.
//
// Semantic grouping needs an embedding model (onnxruntime) and is NOT done in
// Go: the caller fills SemanticGroups from the Python helper, or supplies the
// reason it is unavailable.
package analysis

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"unicode"

	"github.com/Artifex-Technologies/Sarina/go/chymaera/internal/pyfmt"
)

// NameList is a key with the capture names that carry it, in input order.
type NameList struct {
	Key   string
	Names []string
}

// CaptureAnalysis is the result of Analyze. DuplicateUIDs and FrequencyGroups
// keep first-seen order.
type CaptureAnalysis struct {
	Total           int
	ByType          map[string]int
	DuplicateUIDs   []NameList
	FrequencyGroups []NameList
	RareFrequencies []string
	// SemanticGroups are sets of similar captures (names), from the Python helper.
	SemanticGroups [][]string
	// SemanticUnavailableReason is shown in the report when non-empty.
	SemanticUnavailableReason string
}

// AsText renders the report for humans.
func (a CaptureAnalysis) AsText() string {
	lines := []string{"# Capture analysis", "", fmt.Sprintf("total captures: %d", a.Total)}
	kinds := make([]string, 0, len(a.ByType))
	for k := range a.ByType {
		kinds = append(kinds, k)
	}
	sort.Strings(kinds)
	for _, k := range kinds {
		lines = append(lines, fmt.Sprintf("  %s: %d", k, a.ByType[k]))
	}
	lines = append(lines, "")
	if len(a.DuplicateUIDs) > 0 {
		lines = append(lines, "## Duplicate NFC UIDs",
			"Same UID read more than once - could be a re-read, or a cloned/replayed tag.")
		for _, d := range a.DuplicateUIDs {
			lines = append(lines, fmt.Sprintf("- %s: %s", d.Key, strings.Join(d.Names, ", ")))
		}
		lines = append(lines, "")
	}
	if len(a.RareFrequencies) > 0 {
		lines = append(lines, "## Rare Sub-GHz frequencies (seen once)")
		for _, f := range a.RareFrequencies {
			lines = append(lines, fmt.Sprintf("- %s Hz", f))
		}
		lines = append(lines, "")
	}
	if len(a.SemanticGroups) > 0 {
		lines = append(lines, "## Semantic groups (similar captures)")
		for _, g := range a.SemanticGroups {
			lines = append(lines, "- "+strings.Join(g, ", "))
		}
		lines = append(lines, "")
	} else if a.SemanticUnavailableReason != "" {
		lines = append(lines, "Semantic grouping unavailable: "+a.SemanticUnavailableReason)
	}
	return strings.TrimRightFunc(strings.Join(lines, "\n"), unicode.IsSpace)
}

// parseMeta decodes a capture's meta field; anything unusable yields nil.
func parseMeta(capture map[string]any) map[string]any {
	raw, ok := capture["meta"].(string)
	if !ok || raw == "" {
		return nil
	}
	dec := json.NewDecoder(strings.NewReader(raw))
	dec.UseNumber() // keep 433920000 as written, not 4.3392e+08
	var v any
	if err := dec.Decode(&v); err != nil {
		return nil
	}
	if _, err := dec.Token(); err == nil {
		return nil // trailing data: not valid JSON
	}
	m, _ := v.(map[string]any)
	return m
}

func orDefault(capture map[string]any, key, def string) string {
	if v, ok := capture[key]; ok {
		return pyfmt.Str(v)
	}
	return def
}

type ordered struct {
	idx  map[string]int
	list []NameList
}

func (o *ordered) add(key, name string) {
	if o.idx == nil {
		o.idx = map[string]int{}
	}
	i, ok := o.idx[key]
	if !ok {
		i = len(o.list)
		o.idx[key] = i
		o.list = append(o.list, NameList{Key: key})
	}
	o.list[i].Names = append(o.list[i].Names, name)
}

// Analyze analyses capture records as returned by console.Client.CaptureList.
// semanticReason is why semantic grouping did not run.
func Analyze(captures []map[string]any, semanticReason string) CaptureAnalysis {
	byType := map[string]int{}
	var uids, freqs ordered
	for _, c := range captures {
		byType[orDefault(c, "type", "unknown")]++
		name := orDefault(c, "name", "?")
		meta := parseMeta(c)
		if uid := meta["uid"]; pyfmt.Truthy(uid) {
			uids.add(pyfmt.Str(uid), name)
		}
		if f := meta["frequency"]; pyfmt.Truthy(f) {
			freqs.add(pyfmt.Str(f), name)
		}
	}
	var dups []NameList
	for _, u := range uids.list {
		if len(u.Names) > 1 {
			dups = append(dups, u)
		}
	}
	var rare []string
	for _, f := range freqs.list {
		if len(f.Names) == 1 {
			rare = append(rare, f.Key)
		}
	}
	sort.Strings(rare)
	return CaptureAnalysis{
		Total:                     len(captures),
		ByType:                    byType,
		DuplicateUIDs:             dups,
		FrequencyGroups:           freqs.list,
		RareFrequencies:           rare,
		SemanticUnavailableReason: semanticReason,
	}
}
