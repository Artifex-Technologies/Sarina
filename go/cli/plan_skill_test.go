package cli

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

// scriptedBackend answers each request with the next canned response body.
type scriptedBackend struct {
	*httptest.Server
	mu   sync.Mutex
	reqs []map[string]any
	left []map[string]any
}

func textReply(s string) map[string]any {
	return map[string]any{"choices": []any{map[string]any{"message": map[string]any{"role": "assistant", "content": s}, "finish_reason": "stop"}},
		"usage": map[string]any{"prompt_tokens": 5, "completion_tokens": 2}}
}

type call struct{ id, name, args string }

func callsReply(calls ...call) map[string]any {
	var tcs []any
	for _, c := range calls {
		tcs = append(tcs, map[string]any{"id": c.id, "type": "function", "function": map[string]any{"name": c.name, "arguments": c.args}})
	}
	return map[string]any{"choices": []any{map[string]any{"message": map[string]any{"role": "assistant", "content": "", "tool_calls": tcs}, "finish_reason": "tool_calls"}},
		"usage": map[string]any{"prompt_tokens": 5, "completion_tokens": 2}}
}

func newScripted(t *testing.T, replies ...map[string]any) *scriptedBackend {
	b := &scriptedBackend{left: replies}
	b.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		var req map[string]any
		json.Unmarshal(body, &req)
		b.mu.Lock()
		b.reqs = append(b.reqs, req)
		var rep map[string]any
		if len(b.left) > 0 {
			rep, b.left = b.left[0], b.left[1:]
		}
		b.mu.Unlock()
		if rep == nil {
			w.WriteHeader(400)
			io.WriteString(w, `{"error":"script exhausted"}`)
			return
		}
		j, _ := json.Marshal(rep)
		w.Write(j)
	}))
	t.Cleanup(b.Close)
	return b
}

func (b *scriptedBackend) n() int { b.mu.Lock(); defer b.mu.Unlock(); return len(b.reqs) }

func (b *scriptedBackend) toolNames(i int) []string {
	b.mu.Lock()
	defer b.mu.Unlock()
	var out []string
	list, _ := b.reqs[i]["tools"].([]any)
	for _, t := range list {
		out = append(out, t.(map[string]any)["function"].(map[string]any)["name"].(string))
	}
	return out
}

func (b *scriptedBackend) text(i int) string {
	b.mu.Lock()
	defer b.mu.Unlock()
	j, _ := json.Marshal(b.reqs[i]["messages"])
	return string(j)
}

func (h *harness) common2(b *scriptedBackend) []string {
	return []string{"--base-url", b.URL, "--cwd", h.dir, "--disable-claude-md", "--no-auto-memory-review", "--api-key", "cli-test-key-31337", "--allow-write", "--allow-shell"}
}

func (h *harness) skill(t *testing.T, rel, content string) {
	t.Helper()
	p := filepath.Join(h.env.Home, ".sarina", "skills", rel)
	os.MkdirAll(filepath.Dir(p), 0o755)
	if err := os.WriteFile(p, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

const triage = "---\nname: triage\ndescription: Triage alerts\n---\nTriage this: $ARGUMENTS\n"

// ---- slash dispatch ----------------------------------------------------------

func TestSlashUnknownIsLocalAndPathsGoToTheModel(t *testing.T) {
	b := newScripted(t, textReply("model saw it"))
	h := newHarness(t, strings.NewReader("/nope\n/etc/passwd\n/HELP\n/exit\n"))
	if code := h.run(append([]string{"chat"}, h.common2(b)...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if b.n() != 1 || !strings.Contains(b.text(0), "/etc/passwd") {
		t.Fatalf("only the path-like input should reach the model; requests=%d", b.n())
	}
	out := h.out.String()
	for _, want := range []string{"Unknown skill: nope", "# Slash Commands", "/plan", "/skill"} {
		if !strings.Contains(out, want) {
			t.Errorf("output lacks %q:\n%s", want, out)
		}
	}
}

// ---- skills through the CLI ---------------------------------------------------

func TestWorkspaceSkillNeedsTheFlagAndIsAnnounced(t *testing.T) {
	h := newHarness(t, strings.NewReader(""))
	p := filepath.Join(h.dir, ".sarina", "skills", "planted.md")
	os.MkdirAll(filepath.Dir(p), 0o755)
	os.WriteFile(p, []byte("---\ndescription: planted\nmodel_invocable: true\n---\nbody"), 0o644)

	if code := h.run("skill", "list", "--cwd", h.dir); code != 0 || strings.Contains(h.out.String(), "planted") {
		t.Fatalf("workspace skill listed without the flag (code %d):\n%s", code, h.out.String())
	}
	h.out.Reset()
	if code := h.run("skill", "list", "--cwd", h.dir, "--trust-workspace-skills"); code != 0 {
		t.Fatal(code)
	}
	out := h.out.String()
	if !strings.Contains(out, "planted") || !strings.Contains(out, "NOTICE") || !strings.Contains(out, "source: workspace") {
		t.Fatalf("flagged workspace skill must be shown with a notice:\n%s", out)
	}

	// And in a chat run: not offered to the model without the flag, announced with it.
	b := newScripted(t, textReply("a"), textReply("b"))
	h2 := newHarness(t, strings.NewReader(""))
	h2.dir = h.dir
	if code := h2.run(append([]string{"chat"}, append(h2.common2(b), "hi")...)...); code != 0 {
		t.Fatal(h2.errb.String())
	}
	if slices.Contains(b.toolNames(0), "skill") || strings.Contains(h2.errb.String(), "notice") {
		t.Fatalf("workspace skill leaked into a run without the flag: %v / %s", b.toolNames(0), h2.errb.String())
	}
	h3 := newHarness(t, strings.NewReader(""))
	h3.dir = h.dir
	if code := h3.run(append([]string{"chat", "--trust-workspace-skills"}, append(h3.common2(b), "hi")...)...); code != 0 {
		t.Fatal(h3.errb.String())
	}
	if !slices.Contains(b.toolNames(1), "skill") || !strings.Contains(h3.errb.String(), "[notice] workspace skills are enabled") {
		t.Fatalf("flag must enable the skill and say so: %v / %s", b.toolNames(1), h3.errb.String())
	}
}

func TestSlashSkillExpandsAndNarrowsOnly(t *testing.T) {
	b := newScripted(t, textReply("done"), textReply("later"))
	h := newHarness(t, strings.NewReader("/skill triage the audit\nplain follow-up\n/exit\n"))
	h.skill(t, "triage.md", "---\nname: triage\ndescription: Triage alerts\nallowed_tools: [read_file, bash]\n---\nTriage this: $ARGUMENTS\n")
	code := h.run(append([]string{"chat", "--tools", "read_file,write_file"}, h.common2(b)...)...)
	if code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if !strings.Contains(b.text(0), "Triage this: the audit") {
		t.Fatalf("not expanded: %s", b.text(0))
	}
	// allowlist {read_file, write_file} narrowed by {read_file, bash}: bash must not appear.
	if got := b.toolNames(0); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("skill tools = %v; it can only narrow", got)
	}
	// The narrowing lasts for that turn sequence only.
	if got := b.toolNames(1); !slices.Equal(got, []string{"read_file", "write_file"}) {
		t.Fatalf("narrowing leaked into the next turn: %v", got)
	}
}

func TestSlashSkillErrors(t *testing.T) {
	b := newScripted(t)
	h := newHarness(t, strings.NewReader("/skill missing\n/skill hidden\n/skill\n/exit\n"))
	h.skill(t, "triage.md", triage)
	h.skill(t, "hidden.md", "---\ndescription: model only\nuser_invocable: false\nmodel_invocable: true\n---\nx")
	h.run(append([]string{"chat"}, h.common2(b)...)...)
	out := h.out.String()
	if b.n() != 0 {
		t.Fatal("skill errors must not reach the model")
	}
	for _, want := range []string{`Unknown skill "missing"`, "not user-invocable", "- triage: Triage alerts"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	if strings.Contains(strings.SplitN(out, "Skills (use", 2)[1], "hidden") {
		t.Errorf("a non-user-invocable skill is listed for the operator:\n%s", out)
	}
}

func TestSkillRunAndList(t *testing.T) {
	b := newScripted(t, textReply("ran"))
	h := newHarness(t, strings.NewReader(""))
	h.skill(t, "triage.md", "---\nname: triage\ndescription: Triage alerts\nallowed_tools: []\n---\n/etc/not-a-slash $ARGUMENTS\n")
	if code := h.run(append([]string{"skill", "run"}, append(h.common2(b), "triage", "now")...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if !strings.Contains(b.text(0), "/etc/not-a-slash now") {
		t.Fatalf("%s", b.text(0))
	}
	if len(b.toolNames(0)) != 0 {
		t.Fatalf("empty allowed_tools must mean no tools: %v", b.toolNames(0))
	}
	if code := h.run("skill", "run", "--cwd", h.dir, "nope"); code != 1 || !strings.Contains(h.errb.String(), `unknown skill "nope"`) {
		t.Fatalf("unknown skill: code %d %s", code, h.errb.String())
	}
	h.out.Reset()
	if code := h.run("skill", "list", "--cwd", h.dir); code != 0 || !strings.Contains(h.out.String(), "triage") ||
		!strings.Contains(h.out.String(), "model-invocable: no") {
		t.Fatalf("list: %s", h.out.String())
	}
	if code := h.run("skill"); code != 2 {
		t.Fatal(code)
	}
}

func TestModelCanCallOnlyModelInvocableSkills(t *testing.T) {
	b := newScripted(t,
		callsReply(call{"c1", "skill", `{"name":"open","arguments":"X"}`}, call{"c2", "skill", `{"name":"private"}`}),
		textReply("done"))
	h := newHarness(t, strings.NewReader(""))
	h.skill(t, "open.md", "---\ndescription: open\nmodel_invocable: true\nallowed_tools: [read_file]\n---\nDo X: $ARGUMENTS")
	h.skill(t, "private.md", "---\ndescription: private\n---\nPRIVATE-STEPS")
	if code := h.run(append([]string{"chat"}, append(h.common2(b), "go")...)...); code != 0 {
		t.Fatal(h.errb.String())
	}
	first := b.toolNames(0)
	if !slices.Contains(first, "skill") || !slices.Contains(first, "write_file") {
		t.Fatalf("%v", first)
	}
	txt := b.text(1)
	if !strings.Contains(txt, "Do X: X") || strings.Contains(txt, "PRIVATE-STEPS") {
		t.Fatalf("%s", txt)
	}
	if got := b.toolNames(1); !slices.Equal(got, []string{"read_file"}) {
		t.Fatalf("after the skill call the registry must be narrowed to its allowed_tools: %v", got)
	}
}

// ---- plan mode through the CLI --------------------------------------------------

func readSession(t *testing.T, h *harness) *session.Session {
	t.Helper()
	files := h.sessionFiles(t)
	if len(files) != 1 {
		t.Fatalf("session files: %v", files)
	}
	s, err := session.Load(files[0])
	if err != nil {
		t.Fatal(err)
	}
	return s
}

const submitPlan = `{"plan":"1. edit a.txt"}`

func TestOneShotPlanPendingThenResumeFlags(t *testing.T) {
	b := newScripted(t,
		callsReply(call{"c1", "enter_plan_mode", "{}"}, call{"c2", "exit_plan_mode", submitPlan}),
		textReply("working"), textReply("replanned"), textReply("rejected ack"))
	h := newHarness(t, strings.NewReader(""))
	code := h.run(append([]string{"chat"}, append(h.common2(b), "make a plan")...)...)
	if code != exitPlanPending {
		t.Fatalf("code %d (want %d) %s", code, exitPlanPending, h.errb.String())
	}
	if !strings.Contains(h.out.String(), "1. edit a.txt") || !strings.Contains(h.errb.String(), "plan pending approval") {
		t.Fatalf("out=%q err=%q", h.out.String(), h.errb.String())
	}
	s := readSession(t, h)
	if p := s.Plan(); !p.Active() || p.Pending != "1. edit a.txt" {
		t.Fatalf("%+v", p)
	}

	// conflicting or impossible decisions
	if code := h.run("resume", "--approve-plan", "--reject-plan", "--cwd", h.dir, s.ID); code != 2 {
		t.Fatalf("two decisions must be a usage error, got %d", code)
	}
	if b.n() != 1 {
		t.Fatal("no model call expected so far")
	}

	// revise: stays in plan mode
	h.out.Reset()
	if code := h.run(append([]string{"resume", "--revise-plan", "add tests"}, append(h.common2(b), s.ID)...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if slices.Contains(b.toolNames(1), "write_file") || !strings.Contains(b.text(1), "add tests") {
		t.Fatalf("revise must stay in plan mode and carry the feedback: %v", b.toolNames(1))
	}
	if !readSession(t, h).Plan().Active() {
		t.Fatal("plan mode left on revise")
	}

	// nothing pending now: approving must fail rather than silently leave plan mode
	if code := h.run(append([]string{"resume", "--approve-plan"}, append(h.common2(b), s.ID)...)...); code != 1 {
		t.Fatalf("approve with no pending plan = %d", code)
	}
	if !readSession(t, h).Plan().Active() {
		t.Fatal("plan mode must not end without a real decision")
	}
	if b.n() != 2 {
		t.Fatal("a refused decision must not call the model")
	}

	// reject: operator leaves plan mode
	if code := h.run(append([]string{"resume", "--reject-plan"}, append(h.common2(b), s.ID)...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if !slices.Contains(b.toolNames(2), "write_file") || readSession(t, h).Plan().Active() {
		t.Fatalf("reject must restore normal mode: %v", b.toolNames(2))
	}
}

func TestResumeApprovalContinuesWithTools(t *testing.T) {
	b := newScripted(t,
		callsReply(call{"c1", "enter_plan_mode", "{}"}, call{"c2", "exit_plan_mode", submitPlan}),
		textReply("on it"))
	h := newHarness(t, strings.NewReader(""))
	if code := h.run(append([]string{"chat"}, append(h.common2(b), "plan it")...)...); code != exitPlanPending {
		t.Fatal(code)
	}
	id := readSession(t, h).ID
	if code := h.run(append([]string{"resume", "--approve-plan"}, append(h.common2(b), id)...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if !slices.Contains(b.toolNames(1), "write_file") || !strings.Contains(b.text(1), "approved the plan") || !strings.Contains(b.text(1), "1. edit a.txt") {
		t.Fatalf("%v\n%s", b.toolNames(1), b.text(1))
	}
	if p := readSession(t, h).Plan(); p.Active() || p.Note != "1. edit a.txt" {
		t.Fatalf("%+v", p)
	}
}

func TestInteractiveApprovalPrompt(t *testing.T) {
	b := newScripted(t,
		callsReply(call{"c1", "enter_plan_mode", "{}"}, call{"c2", "exit_plan_mode", submitPlan}),
		callsReply(call{"c3", "exit_plan_mode", `{"plan":"1. edit a.txt, 2. add tests"}`}),
		textReply("all done"))
	h := newHarness(t, strings.NewReader("do the work\nr\nadd tests\na\n/exit\n"))
	if code := h.run(append([]string{"chat"}, h.common2(b)...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if !strings.Contains(h.out.String(), "plan awaiting your decision") {
		t.Fatalf("no approval prompt:\n%s", h.out.String())
	}
	if !strings.Contains(b.text(1), "add tests") || slices.Contains(b.toolNames(1), "write_file") {
		t.Fatalf("revise must return feedback and keep plan mode: %v", b.toolNames(1))
	}
	if !slices.Contains(b.toolNames(2), "write_file") || !strings.Contains(b.text(2), "approved the plan") {
		t.Fatalf("approve must restore tools in the same run: %v", b.toolNames(2))
	}
	if readSession(t, h).Plan().Active() {
		t.Fatal("still in plan mode after approval")
	}
}

func TestInteractiveEndOfInputLeavesPlanPending(t *testing.T) {
	b := newScripted(t, callsReply(call{"c1", "enter_plan_mode", "{}"}, call{"c2", "exit_plan_mode", submitPlan}))
	h := newHarness(t, strings.NewReader("do the work\n"))
	h.run(append([]string{"chat"}, h.common2(b)...)...)
	if p := readSession(t, h).Plan(); !p.Active() || p.Pending == "" {
		t.Fatalf("no answer must never approve: %+v", p)
	}
}

func TestOperatorPlanCommands(t *testing.T) {
	b := newScripted(t, textReply("planning"), textReply("normal again"))
	h := newHarness(t, strings.NewReader("/plan\nhello\n/plan approve\n/plan reject\nagain\n/exit\n"))
	if code := h.run(append([]string{"chat"}, h.common2(b)...)...); code != 0 {
		t.Fatalf("code %d %s", code, h.errb.String())
	}
	if slices.Contains(b.toolNames(0), "write_file") || !slices.Contains(b.toolNames(0), "exit_plan_mode") {
		t.Fatalf("/plan must withhold mutating tools from the first request: %v", b.toolNames(0))
	}
	out := h.out.String()
	for _, want := range []string{"Plan mode is on", "Cannot approve", "Plan rejected"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q:\n%s", want, out)
		}
	}
	if !slices.Contains(b.toolNames(1), "write_file") || !strings.Contains(b.text(1), "rejected the plan") {
		t.Fatalf("after /plan reject: %v", b.toolNames(1))
	}
}
