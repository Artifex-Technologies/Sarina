package gui

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"runtime"
	"strconv"
	"syscall"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
	"github.com/Artifex-Technologies/Sarina/go/service"
)

// Importing this package adds `sarina gui`. The packaging name `sarina-gui` is
// the same command: a wrapper script running `exec sarina gui "$@"`, or a link
// to the binary named sarina-gui (cmd/sarina maps that argv[0] to this command).
func init() {
	cli.Register("gui", "run the web console (default port 8765, loopback only)", command)
}

type dirList []string

func (d *dirList) String() string     { return fmt.Sprint([]string(*d)) }
func (d *dirList) Set(v string) error { *d = append(*d, v); return nil }

func command(env *cli.Env, args []string) int {
	getenv := env.Getenv
	d := config.ModelFromEnv(getenv)
	fs := flag.NewFlagSet("gui", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	host := fs.String("host", "127.0.0.1", "address to bind (loopback only)")
	port := fs.Int("port", 8765, "port to listen on")
	cwd := fs.String("cwd", ".", "workspace directory the agent works in")
	model := fs.String("model", d.Model, "model name (env OPENAI_MODEL)")
	baseURL := fs.String("base-url", d.BaseURL, "OpenAI-compatible base URL (env OPENAI_BASE_URL)")
	apiKey := fs.String("api-key", d.APIKey, "API key (env OPENAI_API_KEY); never returned by the API")
	sessionDir := fs.String("session-dir", "", "session directory (default <cwd>/.port_sessions/agent)")
	allowShell := fs.Bool("allow-shell", false, "start with shell commands permitted")
	allowWrite := fs.Bool("allow-write", false, "start with file writes permitted")
	autoApprove := fs.Bool("auto-approve", false, "do not ask before each permitted shell/write action")
	temperature := fs.Float64("temperature", 0, "sampling temperature")
	timeout := fs.Float64("timeout-seconds", config.DefaultTimeoutSeconds, "per model call timeout")
	stream := fs.Bool("stream", true, "stream the model's reply to the page as it is produced")
	maxTurns := fs.Int("max-turns", 12, "model turns per run")
	appendSystem := fs.String("append-system-prompt", "", "text added after the system prompt")
	noBrowser := fs.Bool("no-browser", false, "do not open the default browser on start")
	var addDirs dirList
	fs.Var(&addDirs, "add-dir", "extra working directory (repeatable)")
	var esc runkit.EscalationFlags
	esc.Register(fs)
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	if !service.IsLoopbackHost(*host) {
		fmt.Fprintf(env.Stderr, "sarina gui: refusing to listen on %s. The console has no accounts and may hold shell access, so it only binds to a loopback address.\n", *host)
		return 2
	}
	st := DefaultState(getenv, *cwd)
	if _, e := absDir(*cwd); e != nil {
		fmt.Fprintf(env.Stderr, "sarina gui: --cwd: %v\n", e)
		return 2
	}
	st.Model, st.BaseURL, st.APIKey = *model, *baseURL, *apiKey
	st.AllowShell, st.AllowWrite = *allowShell, *allowWrite
	if *autoApprove {
		st.ApprovalMode = ApprovalAuto
	}
	st.Temperature, st.TimeoutSeconds, st.Stream, st.MaxTurns = *temperature, *timeout, *stream, *maxTurns
	st.AppendSystemPrompt = *appendSystem
	if *sessionDir != "" {
		a, _ := filepath.Abs(*sessionDir)
		st.SessionDir, st.sessionDirExplicit = a, true
	}
	for _, dd := range addDirs {
		a, e := absDir(dd)
		if e != nil {
			fmt.Fprintf(env.Stderr, "sarina gui: --add-dir: %v\n", e)
			return 2
		}
		st.ExtraDirs = append(st.ExtraDirs, a)
	}
	if abs, e := absDir(*cwd); e == nil {
		st.CWD = abs
		if !st.sessionDirExplicit {
			st.SessionDir = config.DefaultRuntime(abs).SessionDirectory
		}
	}

	srv := New(Config{Initial: st, Home: env.Home, Getenv: getenv, NewClient: env.NewClient, Discover: env.NewClient == nil,
		Escalation: esc.Options()})
	ln, err := net.Listen("tcp", net.JoinHostPort(*host, strconv.Itoa(*port)))
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina gui: %v\n", err)
		return 1
	}
	url := fmt.Sprintf("http://%s/?t=%s", ln.Addr().String(), srv.Token())
	fmt.Fprintf(env.Stderr, "sarina gui: %s\n", url)
	fmt.Fprintf(env.Stderr, "  workspace=%s\n  model=%s base_url=%s\n  session_dir=%s\n  permissions=%s approval=%s\n",
		st.CWD, st.Model, st.BaseURL, st.SessionDir, st.PermissionMode(), st.ApprovalMode)
	fmt.Fprintln(env.Stderr, "  The link carries a token for this launch; keep it to yourself.")
	if !*noBrowser {
		openBrowser(url)
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	hs := &http.Server{Handler: srv.Handler(), ReadHeaderTimeout: 15 * time.Second}
	errc := make(chan error, 1)
	go func() { errc <- hs.Serve(ln) }()
	select {
	case err := <-errc:
		if !errors.Is(err, http.ErrServerClosed) {
			fmt.Fprintf(env.Stderr, "sarina gui: %v\n", err)
			return 1
		}
	case <-ctx.Done():
		sc, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		_ = hs.Shutdown(sc)
	}
	return 0
}

// openBrowser is a convenience only; failure is ignored.
func openBrowser(url string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	case "darwin":
		cmd = exec.Command("open", url)
	default:
		cmd = exec.Command("xdg-open", url)
	}
	_ = cmd.Start()
}
