package cli

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"github.com/Artifex-Technologies/Sarina/go/flagx"
	"io"
	"os"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/mcp"
	"github.com/Artifex-Technologies/Sarina/go/policy"
	"github.com/Artifex-Technologies/Sarina/go/runkit"
	"github.com/Artifex-Technologies/Sarina/go/session"
	"github.com/Artifex-Technologies/Sarina/go/skills"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// Exit codes.
const (
	exitOK          = 0
	exitFailure     = 1
	exitUsage       = 2
	exitPlanPending = 3 // the model submitted a plan and nobody was there to decide
	exitInterrupted = 130
)

type stringList []string

func (s *stringList) String() string     { return strings.Join(*s, ",") }
func (s *stringList) Set(v string) error { *s = append(*s, v); return nil }

// opts holds every flag shared by chat and resume.
type opts struct {
	fs                                    *flag.FlagSet
	model, baseURL, apiKey                string
	temperature, timeout, inCost, outCost float64
	cwd                                   string
	maxTurns                              int
	stream                                bool
	snip, compact, preserve               int
	allowWrite, allowShell, unsafe        bool
	addDirs                               stringList
	noClaudeMd                            bool
	sessionDir, scratchRoot               string
	system, appendSystem, overrideSystem  string
	contextWindow                         int
	toolsList                             string
	noTools                               bool
	maxTotal, maxIn, maxOut, maxToolCalls int
	maxModelCalls, maxSessionTurns        int
	maxCost                               float64
	schemaFile, schemaName                string
	schemaStrict                          bool
	interactive                           bool
	trustWorkspaceSkills                  bool
	trustWorkspaceMCP                     bool
	mcpMgr                                *mcp.Manager // started MCP servers; closed when the command ends
	noNotes, noClear, noLLMCompact        bool
	noDiscovery                           bool
	notesEvery                            int
	noSlash                               bool            // the prompt is already expanded (skill run): never treat it as a slash command
	noAutoReview                          bool            // --no-auto-memory-review
	autoReview                            bool            // review memory when an interactive conversation ends; OFF unless a command turns it on
	client                                agent.Completer // the model transport the agent uses (also used for the exit-time memory review)
	cat                                   *skills.Catalog
	router                                *tiers.Router
	esc                                   runkit.EscalationFlags // --escalate-to-claude-code and friends
}

func newOpts(name string, env *Env) *opts {
	o := &opts{fs: flag.NewFlagSet(name, flag.ContinueOnError)}
	o.fs.SetOutput(env.Stderr)
	d := config.ModelFromEnv(env.Getenv)
	f := o.fs
	f.StringVar(&o.model, "model", d.Model, "model name (env OPENAI_MODEL)")
	f.StringVar(&o.baseURL, "base-url", d.BaseURL, "OpenAI-compatible base URL (env OPENAI_BASE_URL)")
	f.StringVar(&o.apiKey, "api-key", d.APIKey, "API key (env OPENAI_API_KEY); never written to session files")
	f.Float64Var(&o.temperature, "temperature", 0, "sampling temperature")
	f.Float64Var(&o.timeout, "timeout-seconds", config.DefaultTimeoutSeconds, "per-request timeout")
	f.Float64Var(&o.inCost, "input-cost-per-million", 0, "USD per million input tokens")
	f.Float64Var(&o.outCost, "output-cost-per-million", 0, "USD per million output tokens")
	f.StringVar(&o.cwd, "cwd", ".", "workspace directory")
	f.IntVar(&o.maxTurns, "max-turns", 12, "model turns allowed per call")
	f.BoolVar(&o.stream, "stream", false, "stream the model's reply as it is produced")
	f.IntVar(&o.snip, "auto-snip-threshold", 0, "snip old bulky messages above this many tokens")
	f.IntVar(&o.compact, "auto-compact-threshold", 0, "compact history above this many tokens")
	f.IntVar(&o.preserve, "compact-preserve-messages", 4, "recent messages kept verbatim when compacting")
	f.BoolVar(&o.allowWrite, "allow-write", false, "permit file-writing tools")
	f.BoolVar(&o.allowShell, "allow-shell", false, "permit shell tools")
	f.BoolVar(&o.unsafe, "unsafe", false, "permit destructive shell commands")
	f.Var(&o.addDirs, "add-dir", "extra working directory (repeatable)")
	f.BoolVar(&o.noClaudeMd, "disable-claude-md", false, "skip CLAUDE.md discovery")
	f.StringVar(&o.sessionDir, "session-dir", "", "session directory (default <cwd>/.port_sessions/agent)")
	f.StringVar(&o.scratchRoot, "scratchpad-root", "", "scratchpad root (default <cwd>/.port_sessions/scratchpad)")
	f.StringVar(&o.system, "system-prompt", "", "replace the default system prompt")
	f.StringVar(&o.appendSystem, "append-system-prompt", "", "text added after the system prompt")
	f.StringVar(&o.overrideSystem, "override-system-prompt", "", "use only this as the system prompt")
	f.IntVar(&o.contextWindow, "context-window", 0, "context window in tokens (env SARINA_CONTEXT_WINDOW)")
	f.StringVar(&o.toolsList, "tools", "", "comma-separated tool allowlist")
	f.BoolVar(&o.noTools, "no-tools", false, "offer the model no tools at all")
	f.IntVar(&o.maxTotal, "max-total-tokens", 0, "stop above this many total tokens")
	f.IntVar(&o.maxIn, "max-input-tokens", 0, "input token limit")
	f.IntVar(&o.maxOut, "max-output-tokens", 0, "output token limit (also sent as max_tokens)")
	f.IntVar(&o.maxToolCalls, "max-tool-calls", 0, "stop above this many tool calls")
	f.IntVar(&o.maxModelCalls, "max-model-calls", 0, "stop above this many model calls")
	f.IntVar(&o.maxSessionTurns, "max-session-turns", 0, "stop above this many turns in the whole session")
	f.Float64Var(&o.maxCost, "max-cost-usd", 0, "stop above this estimated cost")
	f.StringVar(&o.schemaFile, "response-schema-file", "", "JSON schema file for structured output")
	f.StringVar(&o.schemaName, "response-schema-name", "response", "name of the response schema")
	f.BoolVar(&o.schemaStrict, "response-schema-strict", false, "ask for strict schema adherence")
	f.BoolVar(&o.interactive, "interactive", false, "hold a conversation even if a prompt is given")
	f.BoolVar(&o.trustWorkspaceSkills, "trust-workspace-skills", false, "also load skills from <cwd>/.sarina/skills (off by default: a repository must not inject prompts)")
	f.BoolVar(&o.trustWorkspaceMCP, "trust-workspace-mcp", false, "also load MCP server definitions from files in <cwd> (off by default: they start programs)")
	f.BoolVar(&o.noNotes, "no-session-notes", false, "do not keep running session notes (env SARINA_SESSION_NOTES=0)")
	f.IntVar(&o.notesEvery, "notes-every", 0, "refresh session notes every N tool-using turns (default 6; env SARINA_NOTES_EVERY)")
	f.BoolVar(&o.noAutoReview, "no-auto-memory-review", false, "do not review the session for memory proposals when an interactive conversation ends")
	f.BoolVar(&o.noClear, "no-clear-tool-outputs", false, "do not clear old bulky tool outputs as the context fills (env SARINA_CLEAR_TOOL_OUTPUTS=0)")
	f.BoolVar(&o.noLLMCompact, "no-llm-compaction", false, "compact with the heuristic digest only, never a summarising model call (env SARINA_LLM_COMPACTION=0)")
	f.BoolVar(&o.noDiscovery, "no-context-discovery", false, "do not ask the backend for its context window (env SARINA_CONTEXT_DISCOVERY=0)")
	o.esc.Register(f)
	return o
}

func (o *opts) explicit() map[string]bool {
	set := map[string]bool{}
	o.fs.Visit(func(f *flag.Flag) { set[f.Name] = true })
	return set
}

func ptrInt(set bool, v int) *int {
	if !set {
		return nil
	}
	return &v
}

// build produces the model/runtime configuration. With a stored session its
// values are the base and only explicitly given flags override them
// (permissions are only ever widened).
func (o *opts) build(env *Env, stored *session.Session) (config.ModelConfig, config.RuntimeConfig, []string, error) {
	set := o.explicit()
	var m config.ModelConfig
	var rt config.RuntimeConfig
	if stored != nil {
		m, rt = stored.Model, stored.Runtime
		m.APIKey = config.ModelFromEnv(env.Getenv).APIKey
	} else {
		m = config.ModelFromEnv(env.Getenv)
		rt = config.DefaultRuntime(o.cwd)
	}
	pick := func(name string) bool { return stored == nil || set[name] }
	if pick("model") {
		m.Model = o.model
	}
	if pick("base-url") {
		m.BaseURL = o.baseURL
	}
	if set["api-key"] {
		m.APIKey = o.apiKey
	}
	if pick("temperature") {
		m.Temperature = o.temperature
	}
	if pick("timeout-seconds") {
		m.TimeoutSeconds = o.timeout
	}
	if set["input-cost-per-million"] {
		m.Pricing.InputPerMillion = o.inCost
	}
	if set["output-cost-per-million"] {
		m.Pricing.OutputPerMillion = o.outCost
	}
	if stored == nil || set["cwd"] {
		rt.CWD = o.cwd
		d := config.DefaultRuntime(o.cwd)
		rt.CWD = d.CWD
		if stored == nil || !set["session-dir"] {
			rt.SessionDirectory = d.SessionDirectory
		}
		if stored == nil || !set["scratchpad-root"] {
			rt.ScratchpadRoot = d.ScratchpadRoot
		}
	}
	if set["session-dir"] {
		rt.SessionDirectory = o.sessionDir
	}
	if set["scratchpad-root"] {
		rt.ScratchpadRoot = o.scratchRoot
	}
	if pick("max-turns") {
		rt.MaxTurns = o.maxTurns
	}
	if pick("stream") {
		rt.StreamModelResponses = o.stream
	}
	if set["auto-snip-threshold"] {
		rt.AutoSnipThresholdTokens = ptrInt(true, o.snip)
	}
	if set["auto-compact-threshold"] {
		rt.AutoCompactThresholdTokens = ptrInt(true, o.compact)
	}
	if pick("compact-preserve-messages") {
		rt.CompactPreserveMessages = max(o.preserve, 0)
	}
	rt.Permissions.AllowFileWrite = rt.Permissions.AllowFileWrite || o.allowWrite
	rt.Permissions.AllowShellCommands = rt.Permissions.AllowShellCommands || o.allowShell
	rt.Permissions.AllowDestructiveShellCommands = rt.Permissions.AllowDestructiveShellCommands || o.unsafe
	if len(o.addDirs) > 0 {
		rt.AdditionalWorkingDirectories = append([]string(nil), o.addDirs...)
	}
	rt.DisableClaudeMdDiscovery = rt.DisableClaudeMdDiscovery || o.noClaudeMd
	b := &rt.BudgetConfig
	if set["max-total-tokens"] {
		b.MaxTotalTokens = ptrInt(true, o.maxTotal)
	}
	if set["max-input-tokens"] {
		b.MaxInputTokens = ptrInt(true, o.maxIn)
	}
	if set["max-output-tokens"] {
		b.MaxOutputTokens = ptrInt(true, o.maxOut)
	}
	if set["max-tool-calls"] {
		b.MaxToolCalls = ptrInt(true, o.maxToolCalls)
	}
	if set["max-model-calls"] {
		b.MaxModelCalls = ptrInt(true, o.maxModelCalls)
	}
	if set["max-session-turns"] {
		b.MaxSessionTurns = ptrInt(true, o.maxSessionTurns)
	}
	if set["max-cost-usd"] {
		c := o.maxCost
		b.MaxTotalCostUSD = &c
	}
	if o.schemaFile != "" {
		raw, err := os.ReadFile(o.schemaFile)
		if err != nil {
			return m, rt, nil, fmt.Errorf("cannot read schema file: %w", err)
		}
		var schema map[string]any
		if err := json.Unmarshal(raw, &schema); err != nil {
			return m, rt, nil, fmt.Errorf("schema file is not a JSON object: %w", err)
		}
		rt.OutputSchema = &config.OutputSchema{Name: o.schemaName, Schema: schema, Strict: o.schemaStrict}
	}
	cm := config.ContextConfig{}
	if rt.ContextManagement != nil {
		cm = *rt.ContextManagement
	}
	cm.DisableNotes = cm.DisableNotes || o.noNotes
	cm.DisableClearing = cm.DisableClearing || o.noClear
	cm.DisableLLMCompaction = cm.DisableLLMCompaction || o.noLLMCompact
	cm.DisableDiscovery = cm.DisableDiscovery || o.noDiscovery
	if set["notes-every"] && o.notesEvery > 0 {
		cm.NotesEveryTurns = o.notesEvery
	}
	if cm.IsZero() {
		rt.ContextManagement = nil
	} else {
		rt.ContextManagement = &cm
	}
	var allow []string
	switch {
	case o.noTools:
		allow = []string{}
	case set["tools"]:
		allow = []string{}
		for _, n := range strings.Split(o.toolsList, ",") {
			if n = strings.TrimSpace(n); n != "" {
				allow = append(allow, n)
			}
		}
	}
	return m, rt, allow, nil
}

func (o *opts) agent(env *Env, stored *session.Session) (*agent.Agent, error) {
	m, rt, allow, err := o.build(env, stored)
	if err != nil {
		return nil, err
	}
	var client agent.Completer
	if env.NewClient != nil {
		client = env.NewClient(m)
	} else {
		client = llm.NewClient(m)
	}
	var tracker agent.TierTracker
	o.router = o.tierRouter(env, m, client, rt)
	if o.router != nil {
		client, tracker = o.router, o.router
	}
	o.client = client
	pol := policy.Load(policy.Options{Home: env.homeDir(), Getenv: env.Getenv, Workspace: rt.CWD})
	if len(pol.Fatal) > 0 {
		return nil, errors.New(strings.Join(pol.Fatal, "; "))
	}
	for _, n := range pol.Notices {
		fmt.Fprintf(env.Stderr, "[notice] %s\n", n)
	}
	pol.ApplyBudget(&rt.BudgetConfig)
	var sessionEnv map[string]string
	for _, name := range pol.SafeEnv {
		if v := env.Getenv(name); v != "" {
			if sessionEnv == nil {
				sessionEnv = map[string]string{}
			}
			sessionEnv[name] = v
		}
	}
	builder := tools.Builder{Allowlist: allow, Deny: pol.Denied}
	if st, ok := skills.Tool(o.catalog(env)); ok {
		builder.Extra = append(builder.Extra, st)
	}
	if mgr := o.startMCP(env, pol, rt, sessionEnv); mgr != nil {
		builder.Extra = append(builder.Extra, mgr.Tools()...)
	}
	profiles := agents.Load(agents.Options{Home: env.homeDir(), Getenv: env.Getenv, Trusted: pol.Trusted})
	for _, p := range profiles.Problems {
		fmt.Fprintf(env.Stderr, "[agents] %s\n", p)
	}
	var discoverer agent.WindowDiscoverer
	if env.NewClient == nil { // a real backend; tests that inject a client never probe the network
		discoverer = llm.DefaultDiscoverer
	}
	a, err := agent.New(agent.Options{
		Subagents: profiles, SessionEnv: sessionEnv,
		Model: m, Runtime: rt, Client: client, Discoverer: discoverer,
		Tools:         builder,
		Prompt:        agent.PromptOptions{System: o.system, Append: o.appendSystem, Override: o.overrideSystem},
		ContextWindow: o.contextWindow, Getenv: env.Getenv,
		Home:  env.Home,
		Tiers: tracker,
	})
	if err != nil {
		o.closeMCP()
	}
	return a, err
}

// startMCP connects the operator's MCP servers (under the trust gate) and
// reports notices and failures on stderr. It returns nil when nothing started.
func (o *opts) startMCP(env *Env, pol *policy.Policy, rt config.RuntimeConfig, sessionEnv map[string]string) *mcp.Manager {
	if !pol.Trusted {
		fmt.Fprintln(env.Stderr, "[notice] MCP servers are disabled: the operator policy sets trusted=false")
		return nil
	}
	cfg := mcp.Load(mcp.LoadOptions{Home: env.homeDir(), Getenv: env.Getenv, Workspace: rt.CWD, TrustWorkspace: o.trustWorkspaceMCP})
	for _, n := range cfg.Notices {
		fmt.Fprintf(env.Stderr, "[notice] %s\n", n)
	}
	for _, p := range cfg.Problems {
		fmt.Fprintf(env.Stderr, "[mcp] %s\n", p)
	}
	if len(cfg.Servers) == 0 {
		return nil
	}
	mgr := mcp.NewManager(cfg, mcp.Options{
		Env:            tools.ChildEnv(&tools.Context{SessionEnv: sessionEnv}),
		MaxResultChars: rt.MaxOutputChars,
	})
	mgr.Connect(context.Background())
	for _, p := range mgr.Problems {
		fmt.Fprintf(env.Stderr, "[mcp] %s\n", p)
	}
	o.mcpMgr = mgr
	return mgr
}

// closeMCP stops the MCP server processes. It is safe to call twice.
func (o *opts) closeMCP() {
	if o.mcpMgr != nil {
		o.mcpMgr.Close()
		o.mcpMgr = nil
	}
}

// ---- running a turn with Ctrl-C support ------------------------------------

type chatIO struct {
	env      *Env
	streamed bool
}

func (c *chatIO) onEvent(e map[string]any) {
	switch e["type"] {
	case "content_delta":
		c.streamed = true
		fmt.Fprint(c.env.Stdout, e["delta"])
	case "tool_start":
		fmt.Fprintf(c.env.Stderr, "[tool] %v\n", e["tool_name"])
	case "tier_escalation", "tier_refused", "tier_redaction", "tier_wake", "tier_notice":
		fmt.Fprintln(c.env.Stderr, e["line"])
	}
}

// runTurn executes fn under a context that Ctrl-C cancels.
func runTurn(env *Env, fn func(ctx context.Context) (*agent.Result, error)) (*agent.Result, error) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan struct{})
	go func() {
		select {
		case <-env.Interrupts:
			cancel()
		case <-done:
		}
	}()
	res, err := fn(ctx)
	close(done)
	return res, err
}

func report(env *Env, c *chatIO, res *agent.Result, err error) int {
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
	}
	if res == nil {
		return exitFailure
	}
	if c.streamed {
		fmt.Fprintln(env.Stdout)
	} else if res.FinalOutput != "" {
		fmt.Fprintln(env.Stdout, res.FinalOutput)
	}
	c.streamed = false
	switch res.StopReason {
	case agent.StopInterrupted:
		fmt.Fprintln(env.Stderr, "[interrupted]")
		return exitInterrupted
	case agent.StopPlanPending:
		fmt.Fprintf(env.Stderr, "[plan pending approval] decide with: sarina resume --approve-plan|--reject-plan|--revise-plan \"feedback\" %s\n", res.SessionID)
		return exitPlanPending
	case agent.StopBackendError, agent.StopPromptTooLong, agent.StopBudgetExceeded, agent.StopMaxTurns:
		fmt.Fprintf(env.Stderr, "[stopped: %s]\n", res.StopReason)
		return exitFailure
	}
	if err != nil {
		return exitFailure
	}
	return exitOK
}

func chatCommand(env *Env, args []string) int {
	o := newOpts("chat", env)
	positional, err := flagx.Parse(o.fs, args)
	if err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	o.autoReview = !o.noAutoReview
	a, err := o.agent(env, nil)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitUsage
	}
	defer o.closeMCP()
	prompt := strings.TrimSpace(strings.Join(positional, " "))
	return conversation(env, a, o, nil, prompt)
}

func resumeCommand(env *Env, args []string) int {
	o := newOpts("resume", env)
	approve := o.fs.Bool("approve-plan", false, "approve the plan this session is waiting on, then continue")
	reject := o.fs.Bool("reject-plan", false, "reject the plan this session is waiting on (plan mode ends), then continue")
	revise := o.fs.String("revise-plan", "", "ask for changes to the pending plan (feedback text); plan mode stays on")
	rest, err := flagx.Parse(o.fs, args)
	if err != nil {
		if err == flag.ErrHelp {
			return exitOK
		}
		return exitUsage
	}
	if len(rest) == 0 {
		fmt.Fprintln(env.Stderr, "usage: sarina resume [flags] <session_id> [prompt...]")
		return exitUsage
	}
	id := rest[0]
	dir := o.sessionDir
	if dir == "" {
		dir = config.DefaultRuntime(o.cwd).SessionDirectory
	}
	stored, err := session.LoadByID(dir, id)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: cannot load session %s: %v\n", id, err)
		return exitFailure
	}
	o.autoReview = !o.noAutoReview
	a, err := o.agent(env, stored)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitUsage
	}
	defer o.closeMCP()
	prompt := strings.TrimSpace(strings.Join(rest[1:], " "))
	d, given, err := planFlagDecision(o, *approve, *reject, *revise)
	if err != nil {
		fmt.Fprintf(env.Stderr, "sarina: %v\n", err)
		return exitUsage
	}
	if given {
		a.AdoptPlan(stored)
		if _, err := a.DecidePlan(stored, d); err != nil {
			fmt.Fprintf(env.Stderr, "sarina: cannot apply the plan decision: %v\n", err)
			return exitFailure
		}
		fmt.Fprintf(env.Stderr, "[plan decision: %s]\n", d.Action)
		if prompt == "" {
			prompt = "Carry on, following the operator's plan decision above."
		}
	}
	return conversation(env, a, o, stored, prompt)
}

// planFlagDecision reads the --approve-plan/--reject-plan/--revise-plan flags;
// at most one may be given.
func planFlagDecision(o *opts, approve, reject bool, revise string) (agent.PlanDecision, bool, error) {
	n := 0
	var d agent.PlanDecision
	if approve {
		n, d = n+1, agent.PlanDecision{Action: agent.PlanApprove}
	}
	if reject {
		n, d = n+1, agent.PlanDecision{Action: agent.PlanReject}
	}
	if o.explicit()["revise-plan"] {
		n, d = n+1, agent.PlanDecision{Action: agent.PlanRevise, Feedback: revise}
	}
	if n > 1 {
		return d, false, fmt.Errorf("give only one of --approve-plan, --reject-plan, --revise-plan")
	}
	return d, n == 1, nil
}

// conversation runs a one-shot when a prompt is given (unless --interactive),
// otherwise a read-eval loop on stdin. Later prompts resume the session file
// written by the previous one. /exit, /quit and EOF end the loop; Ctrl-C stops
// a running turn, or ends the loop when idle (exit 130).
func conversation(env *Env, a *agent.Agent, o *opts, stored *session.Session, first string) int {
	defer tools.StopAllBackground() // background processes do not outlive the session
	defer o.closeMCP()              // nor do MCP servers
	c := &chatIO{env: env}
	if o.router != nil {
		o.router.SetConfirm(stdinConfirm(env))
	}
	// live printing of streamed tokens and tool starts
	a.SetEventHook(c.onEvent)

	var current *session.Session = stored
	sc := &SlashContext{Env: env, Agent: a, opts: o, Tiers: o.router, Current: func() *session.Session { return current }}
	turn := func(prompt string) int {
		if !o.noSlash {
			if r, handled := dispatchSlash(sc, prompt); handled {
				if r.Text != "" {
					fmt.Fprintln(env.Stdout, r.Text)
				}
				if r.Prompt == "" {
					return exitOK
				}
				prompt = r.Prompt
				a.NarrowTools(r.Narrow)
			}
		}
		res, err := runTurn(env, func(ctx context.Context) (*agent.Result, error) {
			if current == nil {
				return a.Run(ctx, prompt)
			}
			return a.Resume(ctx, prompt, current)
		})
		if res != nil && res.SessionPath != "" {
			if s, lerr := session.Load(res.SessionPath); lerr == nil {
				current = s
			}
		}
		return report(env, c, res, err)
	}

	if first != "" && !o.interactive {
		return turn(first)
	}

	lines := make(chan string)
	go func() {
		defer close(lines)
		br := bufio.NewReader(env.Stdin)
		for {
			line, err := br.ReadString('\n')
			if line != "" {
				lines <- strings.TrimRight(line, "\r\n")
			}
			if err != nil {
				if err != io.EOF {
					fmt.Fprintf(env.Stderr, "sarina: reading input: %v\n", err)
				}
				return
			}
		}
	}()
	a.SetPlanDecider(func(plan string) agent.PlanDecision { return askPlanDecision(env, lines, plan) })
	if o.router != nil {
		o.router.SetConfirm(linesConfirm(env, lines))
	}
	// finish runs when the interactive loop ends (/exit, /quit, EOF or Ctrl-C).
	finish := func(code int) int {
		if o.autoReview {
			autoMemoryReview(env, o, current)
		}
		return code
	}
	last := exitOK
	pending := first
	for {
		if pending == "" {
			fmt.Fprint(env.Stdout, "> ")
			select {
			case <-env.Interrupts:
				fmt.Fprintln(env.Stdout)
				return finish(exitInterrupted)
			case l, ok := <-lines:
				if !ok {
					return finish(last)
				}
				pending = strings.TrimSpace(l)
			}
		}
		switch pending {
		case "":
			continue
		case "/exit", "/quit":
			return finish(last)
		}
		p := pending
		pending = ""
		if code := turn(p); code == exitInterrupted {
			last = exitOK // the turn was interrupted; the conversation continues
		} else {
			last = code
		}
	}
}
