// Package memory is agent-curated persistent memory: two scopes of short
// entries the agent writes itself and that are injected into every future
// session. Because accepted text comes back every session, every write passes
// the one shared security gate (package guard) first.
//
//   - "agent" scope: project facts, <workspace>/.port_sessions/memory/AGENT_MEMORY.md
//   - "user" scope: operator facts across projects, ~/.sarina/memory/USER_MEMORY.md
//
// On-disk format (unchanged from the Python original so existing files keep
// working): plain text, entries separated by the line "---", trimmed.
package memory

import (
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"

	"github.com/Artifex-Technologies/Sarina/go/guard"
	"github.com/Artifex-Technologies/Sarina/go/testguard"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Limits.
const (
	MaxEntryChars = 1500
	MaxScopeChars = 4000
	// DefaultMaxRelevantEntries bounds how many entries (across both scopes)
	// RenderRelevantPromptBlock keeps.
	DefaultMaxRelevantEntries = 8
)

const entryDelimiter = "\n---\n"

// Scopes in rendering order.
var Scopes = []string{"agent", "user"}

// PromptHeader opens the injected block.
const PromptHeader = "The entries below are notes you saved for yourself in earlier sessions " +
	"(the \"agent\" scope) and notes about the operator carried across projects " +
	"(the \"user\" scope). This is a frozen snapshot captured at the start of " +
	"this session -- changes you make with memory_add/memory_replace/" +
	"memory_remove during this session take effect for future sessions, not " +
	"this one."

// Error is an expected memory failure (capacity, duplicate, not found, blocked).
type Error struct{ Msg string }

func (e *Error) Error() string { return e.Msg }

func fail(format string, a ...any) error { return &Error{fmt.Sprintf(format, a...)} }

// Mutation reports what a write did.
type Mutation struct {
	Scope       string
	Action      string
	Path        string
	BeforeCount int
	AfterCount  int
}

// Runtime reads and writes the two memory files.
type Runtime struct {
	AgentPath string
	UserPath  string
	// Root is the workspace AgentPath lies in (FromWorkspace sets it): the agent file is then resolved inside it on every
	// use, so a link planted in the workspace cannot lead it to a file elsewhere (whose text would be put in front of the
	// model every session) or to a secret or private file.
	Root string
	// Scanner vets every write; nil means guard.Memory() (local checks, then Heimdall).
	Scanner guard.Scanner
}

var writeMu sync.Mutex

// FromWorkspace builds the runtime for a workspace. home "" means the user's
// real home directory.
func FromWorkspace(cwd, home string) *Runtime {
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	abs, err := filepath.Abs(cwd)
	if err != nil {
		abs = cwd
	}
	return &Runtime{
		AgentPath: filepath.Join(abs, ".port_sessions", "memory", "AGENT_MEMORY.md"),
		UserPath:  filepath.Join(home, ".sarina", "memory", "USER_MEMORY.md"),
		Root:      abs,
	}
}

// maxMemoryFileBytes bounds what is read of a memory or review file (a scope holds at most 4000 characters).
const maxMemoryFileBytes = 1 << 20

// workspaceFile resolves p (a path under root) as a file tool's write is: a link cannot lead it out of the workspace,
// and a secret or private file is never used (found by the Rust port, 2026-11-12: a planted AGENT_MEMORY.md link put
// the file it named into every session's prompt).
func workspaceFile(root, p string) (string, error) {
	rel, err := filepath.Rel(root, p)
	if err != nil {
		return "", err
	}
	resolved, err := tools.ResolveForWrite(root, rel)
	if err != nil {
		return "", err
	}
	if tools.IsSecretPath(resolved) || tools.IsPrivate(resolved) {
		return "", fmt.Errorf("%s leads to a file the file tools never use", rel)
	}
	return resolved, nil
}

// readStateFile reads a memory or review file: nil for none; an error for anything but a regular file of at most
// maxMemoryFileBytes (a named pipe would wait forever, a huge file would be held whole).
func readStateFile(p string) ([]byte, error) {
	st, err := os.Stat(p)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	if !st.Mode().IsRegular() {
		return nil, fmt.Errorf("%s is not a regular file", p)
	}
	if st.Size() > maxMemoryFileBytes {
		return nil, fmt.Errorf("%s is larger than %d bytes", p, maxMemoryFileBytes)
	}
	f, err := os.Open(p)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	raw, err := io.ReadAll(io.LimitReader(f, maxMemoryFileBytes+1))
	if err != nil {
		return nil, err
	}
	if len(raw) > maxMemoryFileBytes {
		return nil, fmt.Errorf("%s is larger than %d bytes", p, maxMemoryFileBytes)
	}
	return raw, nil
}

func (r *Runtime) scanner() guard.Scanner {
	if r.Scanner != nil {
		return r.Scanner
	}
	return guard.Memory()
}

func (r *Runtime) path(scope string) (string, error) {
	switch scope {
	case "agent":
		if r.Root != "" {
			return workspaceFile(r.Root, r.AgentPath)
		}
		return r.AgentPath, nil
	case "user":
		return r.UserPath, nil
	}
	return "", fail("unknown memory scope '%s' (expected 'agent' or 'user')", scope)
}

// Entries returns the stored entries of a scope (nil when there are none); an error when the file cannot be used.
func (r *Runtime) Entries(scope string) ([]string, error) {
	p, err := r.path(scope)
	if err != nil {
		return nil, err
	}
	raw, err := readStateFile(p)
	if err != nil {
		return nil, err
	}
	var out []string
	for _, e := range strings.Split(strings.ToValidUTF8(string(raw), "�"), entryDelimiter) {
		if e = strings.TrimSpace(e); e != "" {
			out = append(out, e)
		}
	}
	return out, nil
}

func runeLen(s string) int { return len([]rune(s)) }

func used(entries []string, skip int) int {
	n := 0
	for i, e := range entries {
		if i != skip {
			n += runeLen(e)
		}
	}
	return n
}

func (r *Runtime) checkContent(content string) (string, error) {
	content = strings.TrimSpace(content)
	if content == "" {
		return "", fail("memory entry content must not be empty")
	}
	if n := runeLen(content); n > MaxEntryChars {
		return "", fail("entry is %d chars, over the %d-char limit per entry", n, MaxEntryChars)
	}
	if res := r.scanner().Scan(content); res.Blocked {
		return "", fail("entry rejected: %s", res.Reason)
	}
	return content, nil
}

// Add stores a new entry.
func (r *Runtime) Add(scope, content string) (Mutation, error) {
	writeMu.Lock()
	defer writeMu.Unlock()
	if _, err := r.path(scope); err != nil {
		return Mutation{}, err
	}
	content, err := r.checkContent(content)
	if err != nil {
		return Mutation{}, err
	}
	existing, err := r.Entries(scope)
	if err != nil {
		return Mutation{}, unreadable(err)
	}
	norm := normalize(content)
	for _, e := range existing {
		if normalize(e) == norm {
			return Mutation{}, fail("an identical entry is already stored; nothing to add")
		}
	}
	if u := used(existing, -1); u+runeLen(content) > MaxScopeChars {
		return Mutation{}, fail("memory scope \"%s\" is full (%d/%d chars); replace or remove an existing entry before adding more", scope, u, MaxScopeChars)
	}
	updated := append(append([]string(nil), existing...), content)
	return r.commit(scope, "add", len(existing), updated)
}

// Replace swaps the one entry containing oldText for content.
func (r *Runtime) Replace(scope, oldText, content string) (Mutation, error) {
	writeMu.Lock()
	defer writeMu.Unlock()
	if _, err := r.path(scope); err != nil {
		return Mutation{}, err
	}
	content, err := r.checkContent(content)
	if err != nil {
		return Mutation{}, err
	}
	existing, err := r.Entries(scope)
	if err != nil {
		return Mutation{}, unreadable(err)
	}
	idx, err := findUnique(existing, oldText)
	if err != nil {
		return Mutation{}, err
	}
	if used(existing, idx)+runeLen(content) > MaxScopeChars {
		return Mutation{}, fail("replacement would grow memory scope \"%s\" past the %d-char limit", scope, MaxScopeChars)
	}
	updated := append([]string(nil), existing...)
	updated[idx] = content
	return r.commit(scope, "replace", len(existing), updated)
}

// Remove deletes the one entry containing oldText.
func (r *Runtime) Remove(scope, oldText string) (Mutation, error) {
	writeMu.Lock()
	defer writeMu.Unlock()
	if _, err := r.path(scope); err != nil {
		return Mutation{}, err
	}
	existing, err := r.Entries(scope)
	if err != nil {
		return Mutation{}, unreadable(err)
	}
	idx, err := findUnique(existing, oldText)
	if err != nil {
		return Mutation{}, err
	}
	updated := append(append([]string(nil), existing[:idx]...), existing[idx+1:]...)
	return r.commit(scope, "remove", len(existing), updated)
}

// unreadable refuses a write over a memory file that exists but cannot be read: writing would replace what it holds.
func unreadable(err error) error {
	return fail("cannot use the stored memory, so nothing was changed: %v", err)
}

func (r *Runtime) commit(scope, action string, before int, entries []string) (Mutation, error) {
	p, err := r.path(scope)
	if err != nil {
		return Mutation{}, unreadable(err)
	}
	if err := testguard.Allow(p); err != nil {
		return Mutation{}, fail("%v", err)
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return Mutation{}, fail("cannot write memory: %v", err)
	}
	tmp, err := os.CreateTemp(filepath.Dir(p), ".mem-*.tmp")
	if err != nil {
		return Mutation{}, fail("cannot write memory: %v", err)
	}
	name := tmp.Name()
	_, werr := tmp.WriteString(strings.Join(entries, entryDelimiter))
	cerr := tmp.Close()
	if werr != nil || cerr != nil {
		os.Remove(name)
		return Mutation{}, fail("cannot write memory: %v", errors.Join(werr, cerr))
	}
	_ = os.Chmod(name, 0o600)
	if err := os.Rename(name, p); err != nil {
		os.Remove(name)
		return Mutation{}, fail("cannot write memory: %v", err)
	}
	return Mutation{Scope: scope, Action: action, Path: p, BeforeCount: before, AfterCount: len(entries)}, nil
}

func normalize(s string) string { return strings.ToLower(strings.Join(strings.Fields(s), " ")) }

func findUnique(entries []string, oldText string) (int, error) {
	oldText = strings.TrimSpace(oldText)
	if oldText == "" {
		return 0, fail("old_text must not be empty")
	}
	var matches []int
	for i, e := range entries {
		if strings.Contains(e, oldText) {
			matches = append(matches, i)
		}
	}
	switch len(matches) {
	case 0:
		return 0, fail("no stored entry contains that text")
	case 1:
		return matches[0], nil
	}
	return 0, fail("that text matches %d stored entries; give a more specific substring", len(matches))
}

// RenderPromptBlock is the full frozen-snapshot block ("" when there is no memory).
func (r *Runtime) RenderPromptBlock() string {
	var blocks []string
	for _, scope := range Scopes {
		entries, _ := r.Entries(scope)
		if len(entries) == 0 {
			continue
		}
		lines := []string{fmt.Sprintf("## %s memory (%d/%d chars, %d entries)", scope, used(entries, -1), MaxScopeChars, len(entries))}
		for _, e := range entries {
			lines = append(lines, "- "+e)
		}
		blocks = append(blocks, strings.Join(lines, "\n"))
	}
	if len(blocks) == 0 {
		return ""
	}
	return strings.Join(append([]string{PromptHeader}, blocks...), "\n\n")
}

var wordRE = regexp.MustCompile(`[a-z0-9']+`)

func tokenize(s string) map[string]bool {
	out := map[string]bool{}
	for _, w := range wordRE.FindAllString(strings.ToLower(s), -1) {
		out[w] = true
	}
	return out
}

func overlap(entry string, task map[string]bool) int {
	n := 0
	for w := range tokenize(entry) {
		if task[w] {
			n++
		}
	}
	return n
}

// RenderRelevantPromptBlock is RenderPromptBlock limited to the maxEntries
// entries (across both scopes) sharing the most words with taskText, kept in
// their original order. When everything already fits it returns exactly the
// full block. maxEntries <= 0 means DefaultMaxRelevantEntries.
func (r *Runtime) RenderRelevantPromptBlock(taskText string, maxEntries int) string {
	if maxEntries <= 0 {
		maxEntries = DefaultMaxRelevantEntries
	}
	type item struct{ scope, entry string }
	var all []item
	for _, scope := range Scopes {
		es, _ := r.Entries(scope)
		for _, e := range es {
			all = append(all, item{scope, e})
		}
	}
	if len(all) == 0 {
		return ""
	}
	if len(all) <= maxEntries {
		return r.RenderPromptBlock()
	}
	task := tokenize(taskText)
	order := make([]int, len(all))
	scores := make([]int, len(all))
	for i := range all {
		order[i] = i
		scores[i] = overlap(all[i].entry, task)
	}
	sort.SliceStable(order, func(a, b int) bool { return scores[order[a]] > scores[order[b]] })
	keep := append([]int(nil), order[:maxEntries]...)
	sort.Ints(keep)
	omitted := len(all) - len(keep)

	var blocks []string
	for _, scope := range Scopes {
		var lines []string
		total := 0
		for _, it := range all {
			if it.scope == scope {
				total++
			}
		}
		for _, i := range keep {
			if all[i].scope == scope {
				lines = append(lines, "- "+all[i].entry)
			}
		}
		if len(lines) == 0 {
			continue
		}
		head := fmt.Sprintf("## %s memory (showing %d of %d most relevant entries)", scope, len(lines), total)
		blocks = append(blocks, strings.Join(append([]string{head}, lines...), "\n"))
	}
	noun := "entries"
	if omitted == 1 {
		noun = "entry"
	}
	header := fmt.Sprintf("%s\n\n%d less-relevant %s omitted for this task; ask the operator if you need the full list.", PromptHeader, omitted, noun)
	return strings.Join(append([]string{header}, blocks...), "\n\n")
}

// RenderSummary is the human-readable `memory-show` rendering.
func (r *Runtime) RenderSummary() string {
	lines := []string{"# Curated Memory", ""}
	for _, scope := range Scopes {
		entries, _ := r.Entries(scope)
		p := r.AgentPath
		if scope == "user" {
			p = r.UserPath
		}
		lines = append(lines, "## "+scope, "- Path: "+p,
			fmt.Sprintf("- Usage: %d/%d chars, %d entries", used(entries, -1), MaxScopeChars, len(entries)))
		for _, e := range entries {
			lines = append(lines, "  - "+snapshot(e, 140))
		}
		lines = append(lines, "")
	}
	return strings.TrimRight(strings.Join(lines, "\n"), "\n ")
}

func snapshot(s string, limit int) string {
	n := strings.Join(strings.Fields(s), " ")
	if r := []rune(n); len(r) > limit {
		return string(r[:limit-3]) + "..."
	}
	return n
}
