package service

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
)

// Importing this package adds `sarina service`. The packaging name
// `sarina-service` is the same command: a wrapper script running
// `exec sarina service "$@"`, or a link to the binary named sarina-service
// (cmd/sarina maps that argv[0] to this command).
func init() {
	cli.Register("service", "run the integration service (default port 8899, loopback only by default)", command)
}

// IsLoopbackHost reports whether host names this machine only.
func IsLoopbackHost(host string) bool {
	h := strings.Trim(host, "[]")
	if strings.EqualFold(h, "localhost") {
		return true
	}
	ip := net.ParseIP(h)
	return ip != nil && ip.IsLoopback()
}

// triBool is a flag that is "unset" until given, so the default can depend on
// whether a token is configured.
type triBool struct {
	set bool
	val bool
}

func (t *triBool) String() string   { return strconv.FormatBool(t.val) }
func (t *triBool) IsBoolFlag() bool { return true }
func (t *triBool) Set(s string) error {
	v, err := strconv.ParseBool(s)
	if err != nil {
		return err
	}
	t.set, t.val = true, v
	return nil
}

func (t *triBool) resolve(env string, def bool) bool {
	if t.set {
		return t.val
	}
	if v, err := strconv.ParseBool(strings.TrimSpace(env)); err == nil && strings.TrimSpace(env) != "" {
		return v
	}
	return def
}

type pathList []string

func (p *pathList) String() string     { return strings.Join(*p, string(os.PathListSeparator)) }
func (p *pathList) Set(v string) error { *p = append(*p, v); return nil }

func command(env *cli.Env, args []string) int {
	getenv := env.Getenv
	d := config.ModelFromEnv(getenv)
	fs := flag.NewFlagSet("service", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	host := fs.String("host", "127.0.0.1", "address to bind (must stay loopback unless a token is set)")
	port := fs.Int("port", 8899, "port to listen on")
	// the token's and the key's defaults come from the environment after parsing, never as flag defaults: the usage
	// printed for -h or a mistyped flag (into the journal, for the unit) would show them
	token := fs.String("token", "", "bearer token required on every endpoint but /healthz (env SARINA_SERVICE_TOKEN); empty = no authentication")
	model := fs.String("model", d.Model, "default model (env OPENAI_MODEL)")
	baseURL := fs.String("base-url", d.BaseURL, "default OpenAI-compatible endpoint (env OPENAI_BASE_URL)")
	apiKey := fs.String("api-key", "", "default API key (env OPENAI_API_KEY)")
	sessionDir := fs.String("session-dir", filepath.Join(".port_sessions", "agent"), "where agent sessions persist")
	scratch := fs.String("scratchpad-root", filepath.Join(".port_sessions", "scratchpad"), "scratchpad root")
	maxSessions := fs.Int("max-sessions", 20, "pool bound (least recently used sessions are evicted)")
	var permitShell, permitWrite, permitBackend triBool
	fs.Var(&permitShell, "permit-shell", "let callers set allow_shell (default: only when a token is set; env SARINA_SERVICE_PERMIT_SHELL)")
	fs.Var(&permitWrite, "permit-write", "let callers set allow_write (default: only when a token is set; env SARINA_SERVICE_PERMIT_WRITE)")
	fs.Var(&permitBackend, "permit-backend-override", "let callers set base_url/api_key (default: only when a token is set; env SARINA_SERVICE_PERMIT_BACKEND)")
	var roots pathList
	fs.Var(&roots, "cwd-root", "restrict session cwd to this directory (repeatable; env SARINA_SERVICE_CWD_ROOTS, path list); none = anywhere")
	var esc runkit.EscalationFlags
	esc.Register(fs)
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	given := map[string]bool{}
	fs.Visit(func(f *flag.Flag) { given[f.Name] = true })
	if !given["token"] {
		*token = getenv("SARINA_SERVICE_TOKEN")
	}
	if !given["api-key"] {
		*apiKey = d.APIKey
	}
	for _, r := range filepath.SplitList(getenv("SARINA_SERVICE_CWD_ROOTS")) {
		if r != "" {
			roots = append(roots, r)
		}
	}
	if !IsLoopbackHost(*host) && *token == "" {
		fmt.Fprintf(env.Stderr, "sarina service: refusing to listen on %s without a token. Set --token (or SARINA_SERVICE_TOKEN).\n", *host)
		return 2
	}
	trusted := *token != ""
	absDir := func(p string) string {
		if a, err := filepath.Abs(p); err == nil {
			return a
		}
		return p
	}
	cfg := Config{
		Token: *token, Model: *model, BaseURL: *baseURL, APIKey: *apiKey,
		SessionDir: absDir(*sessionDir), ScratchpadRoot: absDir(*scratch), MaxSessions: *maxSessions,
		PermitShell:           permitShell.resolve(getenv("SARINA_SERVICE_PERMIT_SHELL"), trusted),
		PermitWrite:           permitWrite.resolve(getenv("SARINA_SERVICE_PERMIT_WRITE"), trusted),
		PermitBackendOverride: permitBackend.resolve(getenv("SARINA_SERVICE_PERMIT_BACKEND"), trusted),
		CWDRoots:              roots, Home: env.Home, Getenv: getenv, NewClient: env.NewClient, Discover: env.NewClient == nil,
		Escalation: esc.Options(),
	}
	srv := New(cfg)
	ln, err := net.Listen("tcp", net.JoinHostPort(*host, strconv.Itoa(*port)))
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina service: %v\n", err)
		return 1
	}
	banner(env.Stderr, cfg, *host, ln.Addr().String())
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if err := Serve(ctx, srv, ln); err != nil {
		fmt.Fprintf(env.Stderr, "sarina service: %v\n", err)
		return 1
	}
	return 0
}

func banner(w io.Writer, cfg Config, host, addr string) {
	if cfg.Token == "" {
		fmt.Fprintln(w, "WARNING: no token is configured. Unauthenticated requests are accepted.")
		fmt.Fprintln(w, "         Set SARINA_SERVICE_TOKEN or --token. Without a token callers cannot be granted shell, write or backend overrides.")
	} else if !IsLoopbackHost(host) {
		fmt.Fprintf(w, "WARNING: listening on non-loopback address %s. There is no TLS; put this behind something that provides it.\n", host)
	}
	auth := "on"
	if cfg.Token == "" {
		auth = "OFF"
	}
	yn := func(b bool) string {
		if b {
			return "yes"
		}
		return "no"
	}
	fmt.Fprintf(w, "sarina service listening on http://%s  auth=%s\n", addr, auth)
	fmt.Fprintf(w, "  model=%s base_url=%s\n  session_dir=%s pool=%d\n", cfg.Model, cfg.BaseURL, cfg.SessionDir, cfg.MaxSessions)
	if cfg.Escalation != nil {
		fmt.Fprintln(w, "  claude_cli escalation tier: on (operator flag)")
	}
	fmt.Fprintf(w, "  callers may set: shell=%s write=%s backend=%s; cwd roots: %s\n",
		yn(cfg.PermitShell), yn(cfg.PermitWrite), yn(cfg.PermitBackendOverride), rootsText(cfg.CWDRoots))
}

func rootsText(r []string) string {
	if len(r) == 0 {
		return "anywhere"
	}
	return strings.Join(r, ", ")
}

// idleTimeout bounds how long a kept-alive connection may wait for its next request. Without it (net/http falls back
// to ReadTimeout, which is unset) an idle connection was kept for ever, so any local program could hold as many open
// connections, each a goroutine and a file descriptor, as it liked.
var idleTimeout = 120 * time.Second

// Serve runs the server on ln until ctx is cancelled, then shuts down.
func Serve(ctx context.Context, s *Server, ln net.Listener) error {
	// DisableGeneralOptionsHandler: "OPTIONS *" goes through the handler (the browser guard and its body deadline)
	// instead of net/http's own, which read up to 4 KiB of a body without any deadline
	hs := &http.Server{Handler: s.Handler(), ReadHeaderTimeout: 15 * time.Second, IdleTimeout: idleTimeout,
		DisableGeneralOptionsHandler: true}
	errc := make(chan error, 1)
	go func() { errc <- hs.Serve(ln) }()
	select {
	case err := <-errc:
		if errors.Is(err, http.ErrServerClosed) {
			return nil
		}
		return err
	case <-ctx.Done():
		sc, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = hs.Shutdown(sc)
		return nil
	}
}
