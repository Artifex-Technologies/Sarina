package gui

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/memory"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

func (s *Server) homeDir() string {
	if s.cfg.Home != "" {
		return s.cfg.Home
	}
	h, _ := os.UserHomeDir()
	return h
}

func (s *Server) settings() State {
	s.mu.Lock()
	defer s.mu.Unlock()
	st := s.st
	st.ExtraDirs = append([]string(nil), s.st.ExtraDirs...)
	return st
}

// ---- sessions ------------------------------------------------------------------------

func preview(msgs []session.Message) string {
	for _, m := range msgs {
		if m.Role == "user" && !m.IsReminder() && strings.TrimSpace(m.Content) != "" {
			return clip(strings.Join(strings.Fields(m.Content), " "), 120)
		}
	}
	return ""
}

func userText(msgs []session.Message) string {
	var b strings.Builder
	for _, m := range msgs {
		if m.Role == "user" && !m.IsReminder() {
			b.WriteString(strings.ToLower(m.Content))
			b.WriteByte('\n')
		}
	}
	return b.String()
}

func (s *Server) listSessions(w http.ResponseWriter, r *http.Request) {
	st := s.settings()
	scrub := s.scrubber()
	q := strings.ToLower(strings.TrimSpace(r.URL.Query().Get("q")))
	entries, err := os.ReadDir(st.SessionDir)
	out := []map[string]any{}
	if err != nil {
		s.writeJSON(w, 200, out)
		return
	}
	type row struct {
		m   map[string]any
		mod int64
	}
	var rows []row
	for _, e := range entries {
		name := e.Name()
		if e.IsDir() || !strings.HasSuffix(name, ".json") || !idRe.MatchString(strings.TrimSuffix(name, ".json")) {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		sess, err := session.Load(filepath.Join(st.SessionDir, name))
		if err != nil {
			continue // unreadable files are skipped silently
		}
		if q != "" && !strings.Contains(userText(sess.Messages), q) && !strings.Contains(strings.ToLower(sess.ID), q) {
			continue
		}
		rows = append(rows, row{map[string]any{
			"session_id": sess.ID, "turns": sess.Turns, "tool_calls": sess.ToolCalls,
			"preview": scrub(preview(sess.Messages)), "modified_at": info.ModTime().Unix(),
			"model": sess.Model.Model, "total_cost_usd": sess.TotalCostUSD,
		}, info.ModTime().UnixNano()})
	}
	sort.Slice(rows, func(i, j int) bool { return rows[i].mod > rows[j].mod })
	for i, rw := range rows {
		if i >= 500 {
			break
		}
		out = append(out, rw.m)
	}
	s.writeJSON(w, 200, out)
}

func (s *Server) getSession(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !idRe.MatchString(id) {
		s.detail(w, 404, "session not found")
		return
	}
	st := s.settings()
	sess, err := session.LoadByID(st.SessionDir, id)
	if err != nil {
		s.detail(w, 404, "session not found")
		return
	}
	scrub := s.scrubber()
	plan := sess.Plan()
	fh := make([]any, 0, len(sess.FileHistory))
	for _, e := range sess.FileHistory {
		fh = append(fh, runkit.ScrubValue(e, scrub))
	}
	s.writeJSON(w, 200, map[string]any{
		"session_id": sess.ID, "turns": sess.Turns, "tool_calls": sess.ToolCalls,
		"messages": runkit.Transcript(sess.Messages, scrub), "usage": sess.Usage,
		"total_cost_usd": sess.TotalCostUSD, "model": sess.Model.Model, "file_history": fh,
		"plan": map[string]any{"mode": plan.Mode, "pending": scrub(plan.Pending), "note": scrub(plan.Note)},
	})
}

func (s *Server) deleteSession(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !idRe.MatchString(id) {
		s.detail(w, 404, "session not found")
		return
	}
	s.mu.Lock()
	busy := s.run != nil && s.run.running() && s.run.currentSession() == id
	s.mu.Unlock()
	if busy {
		s.detail(w, http.StatusConflict, "that session is running; stop the run first")
		return
	}
	st := s.settings()
	if err := os.Remove(session.Path(st.SessionDir, id)); err != nil {
		s.detail(w, 404, "session not found")
		return
	}
	_ = os.Remove(filepath.Join(st.SessionDir, id+".notes.md"))
	s.mu.Lock()
	if s.st.ActiveSessionID == id {
		s.st.ActiveSessionID = ""
	}
	s.mu.Unlock()
	s.writeJSON(w, 200, map[string]any{"deleted": id})
}

func (s *Server) fileHistory(w http.ResponseWriter, r *http.Request) {
	limit := 200
	if n, err := strconv.Atoi(r.URL.Query().Get("limit")); err == nil && n > 0 && n <= 5000 {
		limit = n
	}
	st := s.settings()
	scrub := s.scrubber()
	var all []map[string]any
	if entries, err := os.ReadDir(st.SessionDir); err == nil {
		for _, e := range entries {
			name := e.Name()
			if e.IsDir() || !strings.HasSuffix(name, ".json") || !idRe.MatchString(strings.TrimSuffix(name, ".json")) {
				continue
			}
			sess, err := session.Load(filepath.Join(st.SessionDir, name))
			if err != nil {
				continue
			}
			for _, h := range sess.FileHistory {
				c := runkit.ScrubValue(h, scrub).(map[string]any)
				c["session_id"] = sess.ID
				all = append(all, c)
			}
		}
	}
	ts := func(m map[string]any) string { v, _ := m["timestamp"].(string); return v }
	sort.SliceStable(all, func(i, j int) bool { return ts(all[i]) > ts(all[j]) })
	total := len(all)
	if len(all) > limit {
		all = all[:limit]
	}
	if all == nil {
		all = []map[string]any{}
	}
	s.writeJSON(w, 200, map[string]any{"total": total, "returned": len(all), "entries": all})
}

func (s *Server) slashCommands(w http.ResponseWriter, _ *http.Request) {
	out := []map[string]any{}
	for _, c := range cli.SlashCommands() {
		out = append(out, map[string]any{"names": []string{c.Name}, "primary": c.Name, "description": c.Description})
	}
	s.writeJSON(w, 200, out)
}

// ---- plan and tasks (read only) ------------------------------------------------------

func (s *Server) getPlan(w http.ResponseWriter, _ *http.Request) {
	st := s.settings()
	scrub := s.scrubber()
	out := map[string]any{"session_id": nil, "mode": session.PlanNormal, "pending": "", "note": ""}
	if st.ActiveSessionID != "" && idRe.MatchString(st.ActiveSessionID) {
		if sess, err := session.LoadByID(st.SessionDir, st.ActiveSessionID); err == nil {
			p := sess.Plan()
			out["session_id"], out["mode"] = sess.ID, p.Mode
			out["pending"], out["note"] = scrub(p.Pending), scrub(p.Note)
		}
	}
	// A plan waiting for a decision right now is the live one.
	s.mu.Lock()
	for _, ap := range s.approvals {
		if ap.Kind == "plan" {
			out["pending"], out["mode"], out["approval_id"] = scrub(ap.Plan), session.PlanActive, ap.ID
		}
	}
	s.mu.Unlock()
	s.writeJSON(w, 200, out)
}

func (s *Server) getTasks(w http.ResponseWriter, _ *http.Request) {
	st := s.settings()
	scrub := s.scrubber()
	path := filepath.Join(st.CWD, ".port_sessions", "task_runtime.json")
	out := map[string]any{"tasks": []any{}, "counts": map[string]int{}}
	raw, err := os.ReadFile(path)
	if err != nil {
		s.writeJSON(w, 200, out)
		return
	}
	var f struct {
		Tasks []map[string]any `json:"tasks"`
	}
	if json.Unmarshal(raw, &f) != nil {
		s.writeJSON(w, 200, out)
		return
	}
	counts := map[string]int{}
	tasks := make([]any, 0, len(f.Tasks))
	for _, t := range f.Tasks {
		stt, _ := t["status"].(string)
		counts[stt]++
		tasks = append(tasks, runkit.ScrubValue(t, scrub))
	}
	out["tasks"], out["counts"] = tasks, counts
	s.writeJSON(w, 200, out)
}

// ---- curated memory and its review queue -----------------------------------------------

func (s *Server) getMemory(w http.ResponseWriter, _ *http.Request) {
	st := s.settings()
	scrub := s.scrubber()
	rt := memory.FromWorkspace(st.CWD, s.cfg.Home)
	scopes := map[string]any{}
	for _, sc := range memory.Scopes {
		es, err := rt.Entries(sc)
		list := make([]string, 0, len(es))
		for _, e := range es {
			list = append(list, scrub(e))
		}
		entry := map[string]any{"entries": list}
		if err != nil {
			entry["error"] = scrub(err.Error())
		}
		scopes[sc] = entry
	}
	pending := []map[string]any{}
	for _, p := range memory.QueueFromWorkspace(st.CWD).Pending() {
		pending = append(pending, map[string]any{
			"entry_id": p.EntryID, "session_id": p.SessionID, "scope": p.Scope, "action": p.Action,
			"old_text": ptrScrub(p.OldText, scrub), "content": ptrScrub(p.Content, scrub),
			"rationale": scrub(p.Rationale), "created_at": p.CreatedAt,
		})
	}
	s.writeJSON(w, 200, map[string]any{"scopes": scopes, "pending_review": pending,
		"limits": map[string]int{"entry_chars": memory.MaxEntryChars, "scope_chars": memory.MaxScopeChars}})
}

func ptrScrub(p *string, scrub func(string) string) any {
	if p == nil {
		return nil
	}
	return scrub(*p)
}

// reviewMemory is the human act: approving applies the proposal through the
// same scanned entry points a tool call uses; rejecting just closes it.
func (s *Server) reviewMemory(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Decision string `json:"decision"`
	}
	if !s.readJSON(w, r, &body) {
		return
	}
	st := s.settings()
	q := memory.QueueFromWorkspace(st.CWD)
	id := r.PathValue("id")
	switch body.Decision {
	case "approve":
		mut, err := q.Approve(id, memory.FromWorkspace(st.CWD, s.cfg.Home))
		if err != nil {
			s.detail(w, http.StatusConflict, s.scrubber()(err.Error()))
			return
		}
		s.writeJSON(w, 200, map[string]any{"entry_id": id, "status": "approved", "scope": mut.Scope, "action": mut.Action})
	case "reject":
		if err := q.Reject(id); err != nil {
			s.detail(w, http.StatusConflict, s.scrubber()(err.Error()))
			return
		}
		s.writeJSON(w, 200, map[string]any{"entry_id": id, "status": "rejected"})
	default:
		s.detail(w, 400, "decision must be approve or reject")
	}
}

// ---- instruction files (CLAUDE.md and friends) ------------------------------------------

type instrFile struct {
	Path     string `json:"path"`
	Name     string `json:"name"`
	Parent   string `json:"parent"`
	Size     int64  `json:"size"`
	Modified int64  `json:"modified_at"`
	Writable bool   `json:"writable"`
	Exists   bool   `json:"exists"`
}

func guidanceNames(dir string) []string {
	files := []string{
		filepath.Join(dir, "CLAUDE.md"),
		filepath.Join(dir, ".claude", "CLAUDE.md"),
		filepath.Join(dir, "CLAUDE.local.md"),
	}
	if es, err := os.ReadDir(filepath.Join(dir, ".claude", "rules")); err == nil {
		var names []string
		for _, e := range es {
			if !e.IsDir() && strings.HasSuffix(e.Name(), ".md") {
				names = append(names, e.Name())
			}
		}
		sort.Strings(names)
		for _, n := range names {
			files = append(files, filepath.Join(dir, ".claude", "rules", n))
		}
	}
	return files
}

// instrSet is the only set of files the console reads or writes: the standard
// instruction file names in the workspace, the extra directories and the
// global config directory (writable), plus those in the workspace's ancestors
// (read only, as the agent also reads them).
func (s *Server) instrSet(st State) (map[string]instrFile, []string) {
	set := map[string]instrFile{}
	var order []string
	add := func(p string, writable bool, boundary string) {
		p = filepath.Clean(p)
		if _, dup := set[p]; dup {
			return
		}
		f := instrFile{Path: p, Name: filepath.Base(p), Parent: filepath.Dir(p), Writable: writable}
		if info, err := os.Stat(p); err == nil && info.Mode().IsRegular() {
			f.Exists, f.Size, f.Modified = true, info.Size(), info.ModTime().Unix()
		}
		set[p] = f
		order = append(order, p)
	}
	for _, d := range append([]string{st.CWD}, st.ExtraDirs...) {
		for _, p := range guidanceNames(d) {
			add(p, true, d)
		}
		// a new file is always possible at the standard names
	}
	add(filepath.Join(s.homeDir(), ".claude", "CLAUDE.md"), true, filepath.Join(s.homeDir(), ".claude"))
	for d := filepath.Dir(st.CWD); d != filepath.Dir(d); d = filepath.Dir(d) {
		for _, p := range guidanceNames(d) {
			if _, err := os.Stat(p); err == nil {
				add(p, false, d)
			}
		}
	}
	return set, order
}

func (s *Server) listInstructions(w http.ResponseWriter, _ *http.Request) {
	st := s.settings()
	set, order := s.instrSet(st)
	files := []instrFile{}
	for _, p := range order {
		f := set[p]
		// existing files, plus the two top-level names so a new one can be created
		if f.Exists || (f.Writable && (f.Name == "CLAUDE.md" && (f.Parent == st.CWD || f.Parent == filepath.Join(s.homeDir(), ".claude")))) {
			files = append(files, f)
		}
	}
	s.writeJSON(w, 200, map[string]any{"cwd": st.CWD, "additional_working_directories": st.ExtraDirs,
		"global_dir": filepath.Join(s.homeDir(), ".claude"), "files": files})
}

// resolveInstr maps ?path= onto the allowed set; anything else is refused.
func (s *Server) resolveInstr(r *http.Request, write bool) (instrFile, *apiErr) {
	p := r.URL.Query().Get("path")
	if p == "" || strings.ContainsRune(p, 0) {
		return instrFile{}, bad("path is required")
	}
	st := s.settings()
	set, _ := s.instrSet(st)
	f, ok := set[filepath.Clean(p)]
	if !ok && write {
		f, ok = newRuleFile(st, filepath.Clean(p))
	}
	if !ok {
		return instrFile{}, &apiErr{http.StatusForbidden, "that path is not one of the instruction files this console manages"}
	}
	if write {
		if !f.Writable {
			return f, &apiErr{http.StatusForbidden, "that file is outside the workspace, extra directories and global config directory"}
		}
		// a symlinked directory must not lead the write elsewhere
		root := f.Parent
		if filepath.Base(root) == "rules" {
			root = filepath.Dir(filepath.Dir(root))
		} else if filepath.Base(root) == ".claude" {
			root = filepath.Dir(root)
		}
		if info, err := os.Lstat(f.Path); err == nil && info.Mode()&os.ModeSymlink != 0 {
			return f, &apiErr{http.StatusForbidden, "refusing to write through a symbolic link"}
		}
		if !pathInside(root, f.Path) && !pathInside(filepath.Join(s.homeDir(), ".claude"), f.Path) {
			return f, &apiErr{http.StatusForbidden, "the path resolves outside its directory"}
		}
	}
	return f, nil
}

var ruleNameRe = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*\.md$`)

// newRuleFile admits a not-yet-existing .claude/rules/<name>.md in the
// workspace or an extra directory, so a new rule can be created.
func newRuleFile(st State, p string) (instrFile, bool) {
	if !ruleNameRe.MatchString(filepath.Base(p)) {
		return instrFile{}, false
	}
	dir := filepath.Dir(p)
	for _, root := range append([]string{st.CWD}, st.ExtraDirs...) {
		if dir == filepath.Join(root, ".claude", "rules") {
			return instrFile{Path: p, Name: filepath.Base(p), Parent: dir, Writable: true}, true
		}
	}
	return instrFile{}, false
}

func pathInside(root, p string) bool {
	rel, err := filepath.Rel(resolvePath(root), resolvePath(p))
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) && !filepath.IsAbs(rel)
}

func resolvePath(p string) string {
	abs, err := filepath.Abs(p)
	if err != nil {
		return filepath.Clean(p)
	}
	rest, cur := "", abs
	for {
		if real, err := filepath.EvalSymlinks(cur); err == nil {
			return filepath.Join(real, rest)
		}
		parent := filepath.Dir(cur)
		if parent == cur {
			return abs
		}
		rest = filepath.Join(filepath.Base(cur), rest)
		cur = parent
	}
}

func (s *Server) readInstruction(w http.ResponseWriter, r *http.Request) {
	f, e := s.resolveInstr(r, false)
	if e != nil {
		s.fail(w, e)
		return
	}
	raw, err := os.ReadFile(f.Path)
	if err != nil {
		s.detail(w, 404, "file not found")
		return
	}
	if len(raw) > 400<<10 {
		raw = raw[:400<<10]
	}
	scrub := s.scrubber()
	text := string(raw)
	clean := scrub(text)
	s.writeJSON(w, 200, map[string]any{"path": f.Path, "content": clean, "redacted": clean != text, "writable": f.Writable})
}

func (s *Server) writeInstruction(w http.ResponseWriter, r *http.Request) {
	f, e := s.resolveInstr(r, true)
	if e != nil {
		s.fail(w, e)
		return
	}
	var body struct {
		Content *string `json:"content"`
	}
	if !s.readJSON(w, r, &body) {
		return
	}
	if body.Content == nil {
		s.fail(w, wrongType("content", "a string"))
		return
	}
	if len(*body.Content) > 200<<10 {
		s.detail(w, http.StatusRequestEntityTooLarge, "file too large")
		return
	}
	if strings.Contains(*body.Content, "[REDACTED") {
		s.detail(w, 400, "the text contains a [REDACTED] marker; saving would overwrite the real value")
		return
	}
	if err := os.MkdirAll(filepath.Dir(f.Path), 0o755); err != nil {
		s.detail(w, 500, "cannot create the directory")
		return
	}
	tmp, err := os.CreateTemp(filepath.Dir(f.Path), ".sarina-*.tmp")
	if err != nil {
		s.detail(w, 500, "cannot write the file")
		return
	}
	_, werr := io.WriteString(tmp, *body.Content)
	cerr := tmp.Close()
	if werr != nil || cerr != nil || os.Rename(tmp.Name(), f.Path) != nil {
		os.Remove(tmp.Name())
		s.detail(w, 500, "cannot write the file")
		return
	}
	s.writeJSON(w, 200, map[string]any{"path": f.Path, "size": len(*body.Content)})
}

func (s *Server) deleteInstruction(w http.ResponseWriter, r *http.Request) {
	f, e := s.resolveInstr(r, true)
	if e != nil {
		s.fail(w, e)
		return
	}
	if err := os.Remove(f.Path); err != nil {
		s.detail(w, 404, "file not found")
		return
	}
	s.writeJSON(w, 200, map[string]any{"deleted": f.Path})
}

// ---- tiers and backend ---------------------------------------------------------------

func (s *Server) tierStatus(w http.ResponseWriter, r *http.Request) {
	scrub := s.scrubber()
	st := s.settings()
	cfg, err := tiers.Load(s.cfg.Home, s.cfg.Getenv)
	if err == nil && s.cfg.Escalation != nil {
		o := *s.cfg.Escalation
		o.AcceptEdits = st.AllowWrite || st.AllowShell
		cfg = tiers.FromEscalation(cfg, config.EscalationConfig{}, o, s.cfg.Getenv)
	}
	out := map[string]any{"configured": false, "config_path": cfg.Path, "tiers": []any{}, "warnings": cfg.Warnings}
	if err != nil {
		out["error"] = scrub(err.Error())
		s.writeJSON(w, 200, out)
		return
	}
	m, _ := st.ModelRuntime()
	router, err := tiers.New(tiers.Options{Config: cfg, Base: m, Getenv: s.cfg.Getenv})
	if err != nil {
		out["error"] = scrub(err.Error())
		s.writeJSON(w, 200, out)
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), 15*time.Second)
	defer cancel()
	rows := []map[string]any{}
	for _, t := range router.Status(ctx) {
		row := map[string]any{
			"name": t.Name, "kind": t.Kind, "base_url": scrub(t.BaseURL), "model": t.Model,
			"api_key_env": t.APIKeyEnv, "api_key_set": t.KeySet, "allow_data": t.AllowData,
			"max_context": t.MaxContext, "enabled": t.Enabled, "auto": t.Auto, "healthy": t.Healthy,
			"why": scrub(t.Why), "current": t.Current,
		}
		if t.Kind == tiers.KindClaudeCLI {
			row["cli_path"], row["cli_found"] = scrub(t.CLIPath), t.CLIFound
		}
		rows = append(rows, row)
	}
	out["configured"], out["tiers"] = cfg.Multi(), rows
	if _, why := router.Pressure(ctx); why != "" {
		out["pressure"] = why
	}
	s.writeJSON(w, 200, out)
}

// backend reports whether the model backend answers (GET <base_url>/models).
func (s *Server) backend(w http.ResponseWriter, r *http.Request) {
	st := s.settings()
	scrub := s.scrubber()
	out := map[string]any{"base_url": scrub(st.BaseURL), "model": st.Model, "ok": false}
	ctx, cancel := context.WithTimeout(r.Context(), 4*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(st.BaseURL, "/")+"/models", nil)
	if err != nil {
		out["error"] = "the base URL is not valid"
		s.writeJSON(w, 200, out)
		return
	}
	if st.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+st.APIKey)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		out["error"] = scrub(err.Error())
		s.writeJSON(w, 200, out)
		return
	}
	defer resp.Body.Close()
	io.Copy(io.Discard, io.LimitReader(resp.Body, 1<<20))
	out["status"] = resp.StatusCode
	out["ok"] = resp.StatusCode >= 200 && resp.StatusCode < 300
	if !out["ok"].(bool) {
		out["error"] = "the backend answered HTTP " + strconv.Itoa(resp.StatusCode)
	}
	s.writeJSON(w, 200, out)
}
