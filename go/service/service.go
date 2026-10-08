// Package service is the integration service (`sarina service`, port 8899): a
// small bearer-authenticated JSON API that external callers (Chymaera_Sentry,
// scripts) use to run agent turns in pooled sessions.
//
// The wire contract is fixed by docs/spec/05-gui-service.md section 2. The
// pieces worth knowing about: tool_allowlist [] means zero tools and the
// create response reports the real count; the pool is a bounded LRU and an
// evicted id is a 404 (the caller resumes from disk with resume_session_id);
// and a caller can never be granted more than the operator's ceiling.
package service

import (
	"crypto/sha256"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"sync"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

// maxBody bounds a request body.
const maxBody = 8 << 20

// Config is the operator's configuration. Zero values mean "nothing granted".
type Config struct {
	Token string // bearer token; empty = no authentication

	Model, BaseURL, APIKey string // defaults for sessions that omit them
	SessionDir             string // default (and root) for session transcripts
	ScratchpadRoot         string // default (and root) for scratchpads
	MaxSessions            int    // pool bound (LRU); <= 0 means 20

	// The ceiling. A caller asking for more than this is refused with 403;
	// nothing is silently downgraded.
	PermitShell           bool     // callers may set allow_shell
	PermitWrite           bool     // callers may set allow_write
	PermitBackendOverride bool     // callers may set base_url / api_key
	CWDRoots              []string // when non-empty, a session's cwd must lie inside one

	Home      string              // "" = the user's real home
	Getenv    func(string) string // nil = os.Getenv
	NewClient func(config.ModelConfig) agent.Completer
	Discover  bool // let agents ask the backend for its context window

	// Escalation adds the claude_cli tier to every session. Operator flags only:
	// no request field can enable it or change the executable.
	Escalation *tiers.EscalationOptions
}

// Server holds the session pool.
type Server struct {
	cfg  Config
	mu   sync.Mutex
	pool map[string]*entry
	tick uint64
}

type entry struct {
	id         string
	agent      *agent.Agent
	sessionDir string
	mu         sync.Mutex // serialises turns on this session
	last       string     // agent session id of the previous run
	tools      int
	used       uint64
	scrub      func(string) string
}

// New builds a server.
func New(cfg Config) *Server {
	if cfg.MaxSessions <= 0 {
		cfg.MaxSessions = 20
	}
	if cfg.Getenv == nil {
		cfg.Getenv = os.Getenv
	}
	return &Server{cfg: cfg, pool: map[string]*entry{}}
}

// Handler is the HTTP surface.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", s.healthz)
	mux.HandleFunc("POST /v1/sessions", s.auth(s.create))
	mux.HandleFunc("POST /v1/sessions/{id}/chat", s.auth(s.chat))
	mux.HandleFunc("DELETE /v1/sessions/{id}", s.auth(s.evict))
	// Anything else (unknown path or method) is a plain 404/405 JSON detail.
	mux.HandleFunc("/", s.auth(func(w http.ResponseWriter, r *http.Request) {
		detail(w, http.StatusNotFound, "not found")
	}))
	return mux
}

// ---- helpers ---------------------------------------------------------------------

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func detail(w http.ResponseWriter, code int, msg string) {
	writeJSON(w, code, map[string]any{"detail": msg})
}

// invalid is the 422 body: a detail array describing fields.
func invalid(w http.ResponseWriter, field, msg, typ string) {
	writeJSON(w, http.StatusUnprocessableEntity, map[string]any{"detail": []map[string]any{
		{"loc": []string{"body", field}, "msg": msg, "type": typ}}})
}

func digest(s string) [32]byte { return sha256.Sum256([]byte(s)) }

// auth enforces the bearer token on everything but /healthz.
func (s *Server) auth(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.cfg.Token != "" {
			got := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
			ok := strings.HasPrefix(r.Header.Get("Authorization"), "Bearer ")
			want, have := digest(s.cfg.Token), digest(got)
			if !ok || subtle.ConstantTimeCompare(want[:], have[:]) != 1 {
				w.Header().Set("WWW-Authenticate", "Bearer")
				detail(w, http.StatusUnauthorized, "missing or invalid bearer token")
				return
			}
		}
		next(w, r)
	}
}

func readBody(w http.ResponseWriter, r *http.Request, into any) bool {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxBody))
	if err != nil {
		detail(w, http.StatusRequestEntityTooLarge, "request body too large")
		return false
	}
	if err := json.Unmarshal(body, into); err != nil {
		var te *json.UnmarshalTypeError
		switch {
		case errors.As(err, &te):
			invalid(w, te.Field, "wrong type: expected "+te.Type.String(), "type_error")
		default:
			invalid(w, "", "request body is not valid JSON", "json_invalid")
		}
		return false
	}
	return true
}

func (s *Server) healthz(w http.ResponseWriter, _ *http.Request) {
	s.mu.Lock()
	n := len(s.pool)
	s.mu.Unlock()
	writeJSON(w, http.StatusOK, map[string]any{"status": "ok", "active_sessions": n})
}

// ---- create ------------------------------------------------------------------------

type createReq struct {
	CWD                *string  `json:"cwd"`
	Model              *string  `json:"model"`
	BaseURL            *string  `json:"base_url"`
	APIKey             *string  `json:"api_key"`
	AllowShell         *bool    `json:"allow_shell"`
	AllowWrite         *bool    `json:"allow_write"`
	Temperature        *float64 `json:"temperature"`
	TimeoutSeconds     *float64 `json:"timeout_seconds"`
	MaxTurns           *int     `json:"max_turns"`
	MaxTotalTokens     *int     `json:"max_total_tokens"`
	MaxInputTokens     *int     `json:"max_input_tokens"`
	MaxOutputTokens    *int     `json:"max_output_tokens"`
	MaxReasoningTokens *int     `json:"max_reasoning_tokens"`
	MaxToolCalls       *int     `json:"max_tool_calls"`
	MaxDelegatedTasks  *int     `json:"max_delegated_tasks"`
	MaxModelCalls      *int     `json:"max_model_calls"`
	MaxSessionTurns    *int     `json:"max_session_turns"`
	MaxTotalCostUSD    *float64 `json:"max_total_cost_usd"`
	SessionDirectory   *string  `json:"session_directory"`
	ScratchpadRoot     *string  `json:"scratchpad_root"`
	// ToolAllowlist: absent/null decodes to nil (all tools); [] decodes to an
	// empty non-nil slice (no tools at all).
	ToolAllowlist []string `json:"tool_allowlist"`
}

func str(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

func (s *Server) homeDir() string {
	if s.cfg.Home != "" {
		return s.cfg.Home
	}
	h, _ := os.UserHomeDir()
	return h
}

func (s *Server) expand(p string) string {
	if p == "~" || strings.HasPrefix(p, "~/") || strings.HasPrefix(p, `~\`) {
		return filepath.Join(s.homeDir(), p[1:])
	}
	return p
}

// resolve makes a path absolute and follows symlinks through its deepest
// existing ancestor, so a link cannot be used to step outside a root.
func resolve(p string) string {
	abs, err := filepath.Abs(p)
	if err != nil {
		return filepath.Clean(p)
	}
	rest := ""
	cur := abs
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

// inside reports whether p equals root or lies below it.
func inside(root, p string) bool {
	rel, err := filepath.Rel(resolve(root), resolve(p))
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) && !filepath.IsAbs(rel))
}

func (s *Server) create(w http.ResponseWriter, r *http.Request) {
	var req createReq
	if !readBody(w, r, &req) {
		return
	}
	if req.CWD == nil {
		invalid(w, "cwd", "field required", "value_error.missing")
		return
	}
	cwd := s.expand(strings.TrimSpace(*req.CWD))
	if cwd == "" {
		detail(w, http.StatusBadRequest, "cwd must be a non-empty directory path")
		return
	}
	if st, err := os.Stat(cwd); err != nil || !st.IsDir() {
		detail(w, http.StatusBadRequest, "cwd does not exist or is not a directory")
		return
	}

	// The ceiling.
	if req.AllowShell != nil && *req.AllowShell && !s.cfg.PermitShell {
		detail(w, http.StatusForbidden, "allow_shell is not permitted by this service's configuration")
		return
	}
	if req.AllowWrite != nil && *req.AllowWrite && !s.cfg.PermitWrite {
		detail(w, http.StatusForbidden, "allow_write is not permitted by this service's configuration")
		return
	}
	model, baseURL, apiKey := s.cfg.Model, s.cfg.BaseURL, s.cfg.APIKey
	if v := str(req.Model); v != "" {
		model = v
	}
	if v := str(req.BaseURL); v != "" && v != s.cfg.BaseURL {
		if !s.cfg.PermitBackendOverride {
			detail(w, http.StatusForbidden, "base_url is not permitted by this service's configuration")
			return
		}
		baseURL = v
	}
	if v := str(req.APIKey); v != "" && v != s.cfg.APIKey {
		if !s.cfg.PermitBackendOverride {
			detail(w, http.StatusForbidden, "api_key is not permitted by this service's configuration")
			return
		}
		apiKey = v
	}
	if len(s.cfg.CWDRoots) > 0 {
		ok := false
		for _, root := range s.cfg.CWDRoots {
			if inside(root, cwd) {
				ok = true
				break
			}
		}
		if !ok {
			detail(w, http.StatusForbidden, "cwd is outside the directories this service may work in")
			return
		}
	}
	sessionDir, scratch := s.cfg.SessionDir, s.cfg.ScratchpadRoot
	if v := str(req.SessionDirectory); v != "" {
		v = s.expand(v)
		if !inside(s.cfg.SessionDir, v) {
			detail(w, http.StatusForbidden, "session_directory must be inside the service's session directory")
			return
		}
		sessionDir = v
	}
	if v := str(req.ScratchpadRoot); v != "" {
		v = s.expand(v)
		if !inside(s.cfg.ScratchpadRoot, v) {
			detail(w, http.StatusForbidden, "scratchpad_root must be inside the service's scratchpad root")
			return
		}
		scratch = v
	}

	m := config.ModelConfig{Model: model, BaseURL: baseURL, APIKey: apiKey, TimeoutSeconds: config.DefaultTimeoutSeconds}
	if req.Temperature != nil {
		m.Temperature = *req.Temperature
	}
	if req.TimeoutSeconds != nil {
		m.TimeoutSeconds = *req.TimeoutSeconds
	}
	rt := config.DefaultRuntime(cwd)
	rt.SessionDirectory, rt.ScratchpadRoot = sessionDir, scratch
	if req.MaxTurns != nil {
		rt.MaxTurns = *req.MaxTurns
	}
	rt.Permissions.AllowShellCommands = req.AllowShell != nil && *req.AllowShell
	rt.Permissions.AllowFileWrite = req.AllowWrite != nil && *req.AllowWrite
	b := &rt.BudgetConfig
	b.MaxTotalTokens, b.MaxInputTokens, b.MaxOutputTokens = req.MaxTotalTokens, req.MaxInputTokens, req.MaxOutputTokens
	b.MaxReasoningTokens, b.MaxToolCalls, b.MaxDelegatedTasks = req.MaxReasoningTokens, req.MaxToolCalls, req.MaxDelegatedTasks
	b.MaxModelCalls, b.MaxSessionTurns, b.MaxTotalCostUSD = req.MaxModelCalls, req.MaxSessionTurns, req.MaxTotalCostUSD

	built, err := runkit.Build(runkit.Spec{Model: m, Runtime: rt, Allowlist: req.ToolAllowlist,
		Home: s.cfg.Home, Getenv: s.cfg.Getenv, NewClient: s.cfg.NewClient, Discover: s.cfg.Discover,
		Escalation: s.cfg.Escalation})
	if err != nil {
		detail(w, http.StatusBadRequest, runkit.Scrubber(apiKey, s.cfg.Token)(err.Error()))
		return
	}
	e := &entry{id: session.NewID(), agent: built.Agent, sessionDir: sessionDir, tools: built.Agent.ToolCount(),
		scrub: runkit.Scrubber(apiKey, s.cfg.APIKey, s.cfg.Token)}
	s.add(e)
	writeJSON(w, http.StatusOK, map[string]any{"session_id": e.id, "tools": e.tools})
}

// add puts an entry in the pool, evicting the least recently used one(s) when
// the pool is full.
func (s *Server) add(e *entry) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for len(s.pool) >= s.cfg.MaxSessions {
		var oldest *entry
		for _, c := range s.pool {
			if oldest == nil || c.used < oldest.used {
				oldest = c
			}
		}
		delete(s.pool, oldest.id)
	}
	s.tick++
	e.used = s.tick
	s.pool[e.id] = e
}

// get resolves an id and marks it used.
func (s *Server) get(id string) *entry {
	s.mu.Lock()
	defer s.mu.Unlock()
	e := s.pool[id]
	if e != nil {
		s.tick++
		e.used = s.tick
	}
	return e
}

// ---- chat --------------------------------------------------------------------------

type chatReq struct {
	Prompt          *string `json:"prompt"`
	ResumeSessionID *string `json:"resume_session_id"`
}

var idRe = regexp.MustCompile(`^[A-Za-z0-9_-]{1,64}$`)

func (s *Server) chat(w http.ResponseWriter, r *http.Request) {
	e := s.get(r.PathValue("id"))
	if e == nil {
		detail(w, http.StatusNotFound, "session not found (it may have been evicted); create a new one and pass the agent session_id as resume_session_id to continue")
		return
	}
	var req chatReq
	if !readBody(w, r, &req) {
		return
	}
	if req.Prompt == nil {
		invalid(w, "prompt", "field required", "value_error.missing")
		return
	}
	if *req.Prompt == "" {
		invalid(w, "prompt", "ensure this value has at least 1 characters", "value_error.any_str.min_length")
		return
	}
	prompt := strings.TrimSpace(*req.Prompt)
	if prompt == "" {
		detail(w, http.StatusBadRequest, "prompt must not be blank")
		return
	}

	e.mu.Lock() // turns on one session are serialised
	defer e.mu.Unlock()

	resumeID := e.last
	if req.ResumeSessionID != nil && *req.ResumeSessionID != "" {
		resumeID = *req.ResumeSessionID
	}
	var stored *session.Session
	if resumeID != "" {
		if !idRe.MatchString(resumeID) {
			detail(w, http.StatusNotFound, "session to resume not found")
			return
		}
		var err error
		if stored, err = session.LoadByID(e.sessionDir, resumeID); err != nil {
			detail(w, http.StatusNotFound, "session to resume not found in this session's directory")
			return
		}
	}

	if text, ok := cli.LocalSlash(prompt); ok {
		res := map[string]any{
			"final_output": text, "turns": 0, "tool_calls": 0, "session_id": resumeID,
			"usage": map[string]int{"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0,
				"cache_read_input_tokens": 0, "reasoning_tokens": 0, "total_tokens": 0},
			"total_cost_usd": 0, "stop_reason": "slash_command",
			"transcript": []map[string]any{{"role": "user", "content": prompt}, {"role": "assistant", "content": text}},
		}
		writeJSON(w, http.StatusOK, res)
		return
	}

	var res *agent.Result
	var err error
	if stored != nil {
		res, err = e.agent.Resume(r.Context(), prompt, stored)
	} else {
		res, err = e.agent.Run(r.Context(), prompt)
	}
	if err != nil || res == nil {
		if err == nil {
			err = errors.New("the run produced no result")
		}
		writeJSON(w, http.StatusInternalServerError, map[string]any{
			"error": e.scrub(err.Error()), "error_type": runkit.ErrorType(err)})
		return
	}
	e.last = res.SessionID
	writeJSON(w, http.StatusOK, runkit.Result(res, e.scrub))
}

// ---- delete ------------------------------------------------------------------------

func (s *Server) evict(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	s.mu.Lock()
	_, ok := s.pool[id]
	delete(s.pool, id)
	s.mu.Unlock()
	if !ok {
		detail(w, http.StatusNotFound, "session not found")
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"evicted": true})
}

// Active is the number of pooled sessions (for the banner and tests).
func (s *Server) Active() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.pool)
}
