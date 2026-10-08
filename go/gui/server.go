// Package gui is the single-operator web console (`sarina gui`, port 8765).
//
// One workspace, one agent run at a time, loopback only. Because a page the
// operator visits could otherwise drive an agent that may have shell and write
// access, every request is checked: the Host must be a loopback name (DNS
// rebinding), an Origin must be this origin, mutations must be JSON, and the
// API needs a random per-launch token that only the page served to the
// operator carries. The page is embedded in the binary and loads nothing from
// the network.
package gui

import (
	"crypto/rand"
	"crypto/subtle"
	"embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"mime"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
)

//go:embed static
var staticFS embed.FS

const (
	tokenHeader = "X-Sarina-Token"
	cookieName  = "sarina_gui"
	maxBody     = 8 << 20
	tokenMarker = "__SARINA_TOKEN__"
)

// Config configures a Server.
type Config struct {
	Initial State  // settings the console starts with
	Token   string // per-launch token; generated when empty

	Home            string              // "" = the user's real home
	Getenv          func(string) string // nil = os.Getenv
	NewClient       func(config.ModelConfig) agent.Completer
	Discover        bool          // let agents ask the backend for its context window
	ApprovalTimeout time.Duration // unanswered approvals are denied after this; 0 = 5 minutes

	// Escalation adds the claude_cli tier. Operator flags only: it is not part
	// of State, so the page and its API cannot enable it or change the path.
	Escalation *tiers.EscalationOptions
}

// Server is the console's state: settings, the current run, pending approvals.
type Server struct {
	cfg   Config
	token string

	mu        sync.Mutex
	st        State
	run       *run
	seq       int
	events    []map[string]any
	notify    chan struct{}
	approvals map[string]*approval
	nextApp   int
}

// New builds a server.
func New(cfg Config) *Server {
	if cfg.Getenv == nil {
		cfg.Getenv = func(string) string { return "" }
	}
	if cfg.ApprovalTimeout <= 0 {
		cfg.ApprovalTimeout = 5 * time.Minute
	}
	tok := cfg.Token
	if tok == "" {
		var b [24]byte
		if _, err := rand.Read(b[:]); err != nil {
			panic(err)
		}
		tok = hex.EncodeToString(b[:])
	}
	return &Server{cfg: cfg, token: tok, st: cfg.Initial, notify: make(chan struct{}),
		approvals: map[string]*approval{}}
}

// Token is the per-launch token (printed in the startup URL).
func (s *Server) Token() string { return s.token }

// ---- request checks ------------------------------------------------------------------

func hostname(hostport string) string {
	h := hostport
	if hh, _, err := net.SplitHostPort(hostport); err == nil {
		h = hh
	}
	return strings.Trim(h, "[]")
}

func loopbackName(h string) bool {
	if strings.EqualFold(h, "localhost") {
		return true
	}
	ip := net.ParseIP(h)
	return ip != nil && ip.IsLoopback()
}

func sameToken(want, got string) bool {
	if got == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(want), []byte(got)) == 1
}

func (s *Server) writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func (s *Server) detail(w http.ResponseWriter, code int, msg string) {
	s.writeJSON(w, code, map[string]any{"detail": msg})
}

func (s *Server) fail(w http.ResponseWriter, err error) {
	var ae *apiErr
	if errors.As(err, &ae) {
		if ae.code == 422 {
			s.writeJSON(w, 422, map[string]any{"detail": []map[string]any{{"msg": ae.msg, "type": "type_error"}}})
			return
		}
		s.detail(w, ae.code, ae.msg)
		return
	}
	s.detail(w, 500, err.Error())
}

// guard wraps the whole mux with the per-request checks.
func (s *Server) guard(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("Content-Security-Policy", "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("X-Frame-Options", "DENY")
		h.Set("Referrer-Policy", "no-referrer")
		h.Set("Cache-Control", "no-store")

		if !loopbackName(hostname(r.Host)) {
			s.detail(w, http.StatusForbidden, "unexpected Host header: this console only answers on a loopback address")
			return
		}
		if site := r.Header.Get("Sec-Fetch-Site"); site != "" && site != "same-origin" && site != "none" {
			s.detail(w, http.StatusForbidden, "cross-site request refused")
			return
		}
		if origin := r.Header.Get("Origin"); origin != "" {
			u, err := url.Parse(origin)
			if err != nil || u.Scheme != "http" || u.Host != r.Host {
				s.detail(w, http.StatusForbidden, "cross-origin request refused")
				return
			}
		}
		if strings.HasPrefix(r.URL.Path, "/api/") {
			if !sameToken(s.token, r.Header.Get(tokenHeader)) {
				s.detail(w, http.StatusUnauthorized, "missing or invalid console token; open the console from the URL printed at startup")
				return
			}
			if r.Method != http.MethodGet && r.Method != http.MethodHead {
				mt, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type"))
				if mt != "application/json" {
					s.detail(w, http.StatusUnsupportedMediaType, "requests that change anything must be application/json")
					return
				}
			}
		}
		next.ServeHTTP(w, r)
	})
}

// readJSON decodes a request body (an empty body is an empty object).
func (s *Server) readJSON(w http.ResponseWriter, r *http.Request, into any) bool {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxBody))
	if err != nil {
		s.detail(w, http.StatusRequestEntityTooLarge, "request body too large")
		return false
	}
	if strings.TrimSpace(string(body)) == "" {
		body = []byte("{}")
	}
	if err := json.Unmarshal(body, into); err != nil {
		var te *json.UnmarshalTypeError
		if errors.As(err, &te) {
			s.fail(w, wrongType(te.Field, te.Type.String()))
		} else {
			s.fail(w, wrongType("body", "a JSON object"))
		}
		return false
	}
	return true
}

// ---- routes --------------------------------------------------------------------------

// Handler is the whole HTTP surface.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /{$}", s.index)
	sub, _ := fs.Sub(staticFS, "static")
	mux.Handle("GET /static/", http.StripPrefix("/static/", http.FileServerFS(sub)))

	mux.HandleFunc("GET /api/state", s.getState)
	mux.HandleFunc("POST /api/state", s.postState)
	mux.HandleFunc("POST /api/clear", s.clear)
	mux.HandleFunc("GET /api/backend", s.backend)

	mux.HandleFunc("POST /api/chat", s.chat)
	mux.HandleFunc("POST /api/chat/cancel", s.cancel)
	mux.HandleFunc("GET /api/run", s.runStatus)
	mux.HandleFunc("GET /api/events", s.eventStream)
	mux.HandleFunc("GET /api/approvals", s.listApprovals)
	mux.HandleFunc("POST /api/approvals/{id}", s.answerApproval)

	mux.HandleFunc("GET /api/sessions", s.listSessions)
	mux.HandleFunc("GET /api/sessions/{id}", s.getSession)
	mux.HandleFunc("DELETE /api/sessions/{id}", s.deleteSession)
	mux.HandleFunc("GET /api/file-history", s.fileHistory)
	mux.HandleFunc("GET /api/slash-commands", s.slashCommands)

	mux.HandleFunc("GET /api/plan", s.getPlan)
	mux.HandleFunc("GET /api/tasks", s.getTasks)
	mux.HandleFunc("GET /api/memory", s.getMemory)
	mux.HandleFunc("POST /api/memory/review/{id}", s.reviewMemory)
	mux.HandleFunc("GET /api/instructions", s.listInstructions)
	mux.HandleFunc("GET /api/instructions/file", s.readInstruction)
	mux.HandleFunc("PUT /api/instructions/file", s.writeInstruction)
	mux.HandleFunc("DELETE /api/instructions/file", s.deleteInstruction)
	mux.HandleFunc("GET /api/tiers", s.tierStatus)

	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) { s.detail(w, http.StatusNotFound, "not found") })
	return s.guard(mux)
}

// index serves the page with the console token embedded. The page itself needs
// the token too (query string on first load, then a cookie), so another local
// user or a rebinding page cannot fetch it.
func (s *Server) index(w http.ResponseWriter, r *http.Request) {
	ok := sameToken(s.token, r.URL.Query().Get("t"))
	if !ok {
		if c, err := r.Cookie(cookieName); err == nil {
			ok = sameToken(s.token, c.Value)
		}
	}
	if !ok {
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		w.WriteHeader(http.StatusForbidden)
		io.WriteString(w, "This console needs the link printed when it started (it carries a one-time token).\n")
		return
	}
	page, err := staticFS.ReadFile("static/index.html")
	if err != nil {
		s.detail(w, 500, "page missing")
		return
	}
	http.SetCookie(w, &http.Cookie{Name: cookieName, Value: s.token, Path: "/", HttpOnly: true, SameSite: http.SameSiteStrictMode})
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	io.WriteString(w, strings.Replace(string(page), tokenMarker, s.token, 1))
}

// ---- state ---------------------------------------------------------------------------

func (s *Server) snapshot() map[string]any {
	s.mu.Lock()
	defer s.mu.Unlock()
	snap := s.st.Snapshot()
	snap["busy"] = s.run != nil && s.run.running()
	return snap
}

func (s *Server) getState(w http.ResponseWriter, _ *http.Request) { s.writeJSON(w, 200, s.snapshot()) }

func (s *Server) postState(w http.ResponseWriter, r *http.Request) {
	var patch map[string]json.RawMessage
	if !s.readJSON(w, r, &patch) {
		return
	}
	confirmed := false
	if raw, ok := patch["confirm_permissions"]; ok {
		_ = json.Unmarshal(raw, &confirmed)
		delete(patch, "confirm_permissions")
	}
	s.mu.Lock()
	err := s.st.apply(patch, confirmed)
	s.mu.Unlock()
	if err != nil {
		s.fail(w, err)
		return
	}
	s.writeJSON(w, 200, s.snapshot()) // a run in flight keeps the settings it started with
}

// clear forgets the active session so the next prompt starts fresh.
func (s *Server) clear(w http.ResponseWriter, _ *http.Request) {
	s.mu.Lock()
	s.st.ActiveSessionID = ""
	s.mu.Unlock()
	s.writeJSON(w, 200, s.snapshot())
}

func (s *Server) scrubber() func(string) string {
	s.mu.Lock()
	key := s.st.APIKey
	s.mu.Unlock()
	return runkit.Scrubber(key, s.token)
}

func (s *Server) now() string { return time.Now().UTC().Format(time.RFC3339) }
