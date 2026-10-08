// Package fleet is Sarina's side of the agent fleet (Chymaera_Linux contract
// item 7): fleet-usage writes Sarina's record for Omarchy's agents panel,
// fleet-status reads the other agents' records (untrusted data), and
// fleet-launch proposes launching another agent and runs it only after a human
// confirms. Standard library only.
//
// HARD RULE: nothing in this package may ever be registered as a model-callable
// tool. fleet-launch in particular is CLI-only; a test scans the tool registry
// and the Go source to enforce it.
package fleet

import (
	"bytes"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"unicode/utf16"
)

// compactJSON marshals v like Python's json.dumps(v, separators=(',', ':')):
// no spaces, no HTML escaping, and every non-ASCII character as a \uXXXX escape.
func compactJSON(v any) (string, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return "", err
	}
	s := strings.TrimRight(buf.String(), "\n")
	var out strings.Builder
	for _, r := range s {
		switch {
		case r < 0x80:
			out.WriteRune(r)
		case r > 0xFFFF:
			hi, lo := utf16.EncodeRune(r)
			fmt.Fprintf(&out, "\\u%04x\\u%04x", hi, lo)
		default:
			fmt.Fprintf(&out, "\\u%04x", r)
		}
	}
	return out.String(), nil
}

// orderedInts is a JSON object of ints that keeps insertion order (as a Python
// dict does), so the record's key order is stable and predictable.
type orderedInts struct {
	keys []string
	vals map[string]int
}

func newOrderedInts() *orderedInts { return &orderedInts{vals: map[string]int{}} }

func (o *orderedInts) add(key string, n int) {
	if _, ok := o.vals[key]; !ok {
		o.keys = append(o.keys, key)
	}
	o.vals[key] += n
}

// Get returns the value for key (0 when absent).
func (o *orderedInts) Get(key string) int { return o.vals[key] }

// Map returns a copy as a plain map.
func (o *orderedInts) Map() map[string]int {
	m := make(map[string]int, len(o.vals))
	for k, v := range o.vals {
		m[k] = v
	}
	return m
}

func (o *orderedInts) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, k := range o.keys {
		if i > 0 {
			b.WriteByte(',')
		}
		kb, _ := json.Marshal(k)
		b.Write(kb)
		fmt.Fprintf(&b, ":%d", o.vals[k])
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

// ModelTokens is one model's cumulative token counts in the record.
type ModelTokens struct {
	InputTokens              int `json:"inputTokens"`
	OutputTokens             int `json:"outputTokens"`
	CacheReadInputTokens     int `json:"cacheReadInputTokens"`
	CacheCreationInputTokens int `json:"cacheCreationInputTokens"`
}

// orderedModels is the modelUsage object, insertion-ordered.
type orderedModels struct {
	keys []string
	vals map[string]*ModelTokens
}

func newOrderedModels() *orderedModels { return &orderedModels{vals: map[string]*ModelTokens{}} }

func (o *orderedModels) bucket(key string) *ModelTokens {
	if b, ok := o.vals[key]; ok {
		return b
	}
	b := &ModelTokens{}
	o.keys = append(o.keys, key)
	o.vals[key] = b
	return b
}

// Get returns the model's counts, or the zero value.
func (o *orderedModels) Get(key string) ModelTokens {
	if b, ok := o.vals[key]; ok {
		return *b
	}
	return ModelTokens{}
}

func (o *orderedModels) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, k := range o.keys {
		if i > 0 {
			b.WriteByte(',')
		}
		kb, _ := json.Marshal(k)
		b.Write(kb)
		b.WriteByte(':')
		vb, _ := json.Marshal(o.vals[k])
		b.Write(vb)
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
