package memory

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/guard"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/session"
)

// Review limits.
const (
	MaxProposalsPerReview = 5
	MaxTranscriptChars    = 8000
)

// Proposal statuses.
const (
	StatusPending  = "pending"
	StatusApproved = "approved"
	StatusRejected = "rejected"
	StatusBlocked  = "blocked"
)

// ReviewSystemPrompt is the fixed prompt of the review pass.
const ReviewSystemPrompt = `You are reviewing a finished coding-agent session to decide what, if anything, is worth remembering for future sessions.

Propose at most 5 durable memory entries. A durable entry is a fact that will still be true and useful in a different session: an environment quirk, a project convention, a workaround for a specific bug, or a stated user preference. Do NOT propose entries for: task-specific details that won't recur, anything already obvious from reading the code, or instructions to change your own behavior.

Respond with ONLY a JSON array (no prose, no markdown fences). Each item:
{"scope": "agent"|"user", "action": "add"|"replace"|"remove", "old_text": string or null, "content": string or null, "rationale": string}

- scope "agent": project-specific facts. scope "user": facts about the operator that hold across projects.
- action "add": content is the new entry. action "replace": old_text is a substring uniquely identifying an existing entry to update, content is its replacement. action "remove": old_text identifies an entry to delete, content should be null.
- If nothing is worth remembering, respond with an empty array: []
`

// Completer is the model transport the review pass needs (agent.Completer satisfies it).
type Completer interface {
	Complete(ctx context.Context, req llm.Request, on func(llm.StreamEvent)) (*llm.Turn, error)
}

// Proposal is one queued suggestion. Field order and JSON keys are the
// on-disk format of pending_review.json.
type Proposal struct {
	EntryID       string  `json:"entry_id"`
	SessionID     string  `json:"session_id"`
	Scope         string  `json:"scope"`
	Action        string  `json:"action"`
	OldText       *string `json:"old_text"`
	Content       *string `json:"content"`
	Rationale     string  `json:"rationale"`
	Status        string  `json:"status"`
	BlockedReason *string `json:"blocked_reason"`
	CreatedAt     string  `json:"created_at"`
}

func str(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

// BuildTranscript renders the user/assistant/tool text of a session, keeping
// the most recent maxChars characters. System messages and the context
// reminder are left out: they are the same in every session and would eat the
// budget.
func BuildTranscript(msgs []session.Message, maxChars int) string {
	var parts []string
	for _, m := range msgs {
		c := strings.TrimSpace(m.Content)
		if m.Role == "" || m.Role == "system" || c == "" || m.IsReminder() {
			continue
		}
		parts = append(parts, m.Role+": "+c)
	}
	t := strings.Join(parts, "\n\n")
	if r := []rune(t); len(r) > maxChars {
		return "...(truncated)...\n" + string(r[len(r)-maxChars:])
	}
	return t
}

// Propose asks the model for memory proposals about a finished session. An
// empty transcript or an unparseable answer yields none. Each proposal's
// content is scanned now, so a human reviewing the queue never sees one that
// is already known to be unsafe (it is queued as "blocked").
func Propose(ctx context.Context, sessionID string, msgs []session.Message, client Completer, scanner guard.Scanner) ([]Proposal, error) {
	transcript := BuildTranscript(msgs, MaxTranscriptChars)
	if strings.TrimSpace(transcript) == "" {
		return nil, nil
	}
	if scanner == nil {
		scanner = guard.Memory()
	}
	turn, err := client.Complete(ctx, llm.Request{Messages: []llm.Message{
		{Role: "system", Content: ReviewSystemPrompt},
		{Role: "user", Content: transcript},
	}}, nil)
	if err != nil {
		return nil, err
	}
	items := extractJSONArray(turn.Content)
	var out []Proposal
	for i, it := range items {
		if i >= MaxProposalsPerReview {
			break
		}
		if p, ok := buildProposal(sessionID, it, scanner); ok {
			out = append(out, p)
		}
	}
	return out, nil
}

func optText(v any) *string {
	s, ok := v.(string)
	if !ok {
		return nil
	}
	if s = strings.TrimSpace(s); s == "" {
		return nil
	}
	return &s
}

func buildProposal(sessionID string, item any, scanner guard.Scanner) (Proposal, bool) {
	m, ok := item.(map[string]any)
	if !ok {
		return Proposal{}, false
	}
	scope, _ := m["scope"].(string)
	action, _ := m["action"].(string)
	if (scope != "agent" && scope != "user") || (action != "add" && action != "replace" && action != "remove") {
		return Proposal{}, false
	}
	old, content := optText(m["old_text"]), optText(m["content"])
	rationale := ""
	if m["rationale"] != nil {
		rationale = strings.TrimSpace(fmt.Sprint(m["rationale"]))
	}
	if (action == "add" || action == "replace") && content == nil {
		return Proposal{}, false
	}
	if (action == "replace" || action == "remove") && old == nil {
		return Proposal{}, false
	}
	p := Proposal{
		EntryID: "mem_" + randHex(12), SessionID: sessionID, Scope: scope, Action: action,
		OldText: old, Content: content, Rationale: rationale, Status: StatusPending, CreatedAt: now(),
	}
	if content != nil {
		if res := scanner.Scan(*content); res.Blocked {
			p.Status = StatusBlocked
			reason := res.Reason
			p.BlockedReason = &reason
		}
	}
	return p, true
}

func now() string { return time.Now().UTC().Format("2006-01-02T15:04:05.000000-07:00") }

func randHex(n int) string {
	b := make([]byte, (n+1)/2)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)[:n]
}

var fencedArray = regexp.MustCompile("```(?:json)?\\s*(\\[[\\s\\S]*?\\])\\s*```")

func extractJSONArray(text string) []any {
	text = strings.TrimSpace(text)
	var candidates []string
	if m := fencedArray.FindStringSubmatch(text); m != nil {
		candidates = append(candidates, m[1])
	}
	candidates = append(candidates, text)
	if s, e := strings.Index(text, "["), strings.LastIndex(text, "]"); s != -1 && e > s {
		candidates = append(candidates, text[s:e+1])
	}
	for _, c := range candidates {
		var v []any
		if json.Unmarshal([]byte(c), &v) == nil && v != nil {
			return v
		}
	}
	return nil
}

// Queue is the pending-review file, <workspace>/.port_sessions/memory/pending_review.json.
type Queue struct{ Path string }

// QueueFromWorkspace places the queue next to the agent-scope memory file.
func QueueFromWorkspace(cwd string) *Queue {
	abs, err := filepath.Abs(cwd)
	if err != nil {
		abs = cwd
	}
	return &Queue{Path: filepath.Join(abs, ".port_sessions", "memory", "pending_review.json")}
}

// Load reads every entry; a missing or corrupt file is an empty queue.
func (q *Queue) Load() []Proposal {
	raw, err := os.ReadFile(q.Path)
	if err != nil {
		return nil
	}
	var file struct {
		Entries []map[string]any `json:"entries"`
	}
	if json.Unmarshal(raw, &file) != nil {
		return nil
	}
	var out []Proposal
	for _, e := range file.Entries {
		out = append(out, proposalFromMap(e))
	}
	return out
}

func text(v any) string {
	if v == nil {
		return ""
	}
	if s, ok := v.(string); ok {
		return s
	}
	return fmt.Sprint(v)
}

func ptr(v any) *string {
	if v == nil {
		return nil
	}
	s := text(v)
	return &s
}

func proposalFromMap(m map[string]any) Proposal {
	status := text(m["status"])
	if status == "" {
		status = StatusPending
	}
	return Proposal{
		EntryID: text(m["entry_id"]), SessionID: text(m["session_id"]), Scope: text(m["scope"]),
		Action: text(m["action"]), OldText: ptr(m["old_text"]), Content: ptr(m["content"]),
		Rationale: text(m["rationale"]), Status: status, BlockedReason: ptr(m["blocked_reason"]),
		CreatedAt: text(m["created_at"]),
	}
}

func (q *Queue) save(entries []Proposal) error {
	if entries == nil {
		entries = []Proposal{}
	}
	data, err := session.EncodeJSON(map[string]any{"entries": entries})
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(q.Path), 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(q.Path), ".review-*.tmp")
	if err != nil {
		return err
	}
	name := tmp.Name()
	_, werr := tmp.Write(data)
	cerr := tmp.Close()
	if werr != nil || cerr != nil {
		os.Remove(name)
		return fmt.Errorf("cannot write review queue: %v %v", werr, cerr)
	}
	_ = os.Chmod(name, 0o600)
	if err := os.Rename(name, q.Path); err != nil {
		os.Remove(name)
		return err
	}
	return nil
}

// Add appends proposals.
func (q *Queue) Add(ps []Proposal) error {
	if len(ps) == 0 {
		return nil
	}
	writeMu.Lock()
	defer writeMu.Unlock()
	return q.save(append(q.Load(), ps...))
}

// Pending lists proposals awaiting a decision.
func (q *Queue) Pending() []Proposal {
	var out []Proposal
	for _, e := range q.Load() {
		if e.Status == StatusPending {
			out = append(out, e)
		}
	}
	return out
}

// Get finds a proposal by id.
func (q *Queue) Get(id string) (Proposal, bool) {
	for _, e := range q.Load() {
		if e.EntryID == id {
			return e, true
		}
	}
	return Proposal{}, false
}

func (q *Queue) setStatus(id, status string, apply func(Proposal) (Mutation, error)) (Mutation, error) {
	entries := q.Load()
	for i, e := range entries {
		if e.EntryID != id {
			continue
		}
		if e.Status != StatusPending {
			return Mutation{}, fail("entry %s is not pending (status=%s)", id, e.Status)
		}
		var mut Mutation
		if apply != nil {
			m, err := apply(e)
			if err != nil {
				return Mutation{}, err
			}
			mut = m
		}
		entries[i].Status = status
		return mut, q.save(entries)
	}
	return Mutation{}, fail("no pending review entry with id %s", id)
}

// Approve applies a pending proposal through the same entry points an
// interactive tool call uses, so it is scanned again here.
func (q *Queue) Approve(id string, rt *Runtime) (Mutation, error) {
	return q.setStatus(id, StatusApproved, func(e Proposal) (Mutation, error) {
		switch e.Action {
		case "add":
			return rt.Add(e.Scope, str(e.Content))
		case "replace":
			return rt.Replace(e.Scope, str(e.OldText), str(e.Content))
		}
		return rt.Remove(e.Scope, str(e.OldText))
	})
}

// Reject marks a pending proposal rejected.
func (q *Queue) Reject(id string) error {
	_, err := q.setStatus(id, StatusRejected, nil)
	return err
}

// RenderSummary is the `memory-review-list` rendering.
func (q *Queue) RenderSummary() string {
	entries := q.Load()
	if len(entries) == 0 {
		return "# Memory Review Queue\n\nNo review proposals are currently stored."
	}
	lines := []string{"# Memory Review Queue", ""}
	for _, e := range entries {
		lines = append(lines, fmt.Sprintf("- %s; status=%s; scope=%s; action=%s", e.EntryID, e.Status, e.Scope, e.Action))
		if e.Rationale != "" {
			lines = append(lines, "  rationale: "+e.Rationale)
		}
		if c := str(e.Content); c != "" {
			lines = append(lines, "  content: "+c)
		}
		if o := str(e.OldText); o != "" {
			lines = append(lines, "  old_text: "+o)
		}
		if b := str(e.BlockedReason); b != "" {
			lines = append(lines, "  blocked: "+b)
		}
	}
	return strings.Join(lines, "\n")
}
