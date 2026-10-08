package tiers

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"os/exec"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Completer is the model transport; llm.Client and llm.AnthropicClient satisfy it.
type Completer interface {
	Complete(ctx context.Context, req llm.Request, on func(llm.StreamEvent)) (*llm.Turn, error)
}

// SessionKey is the plugin_state key the tier record is stored under.
const SessionKey = "compute_tiers"

const maxRecordedEvents = 200

// Options configures a Router.
type Options struct {
	Config     Config
	Base       config.ModelConfig // the CLI's model settings: tier 0's defaults
	BaseClient Completer          // transport for tier 0 when it names no base_url/model of its own
	Getenv     func(string) string
	HTTPClient *http.Client
	Sampler    Sampler         // nil: SystemSampler
	Workdir    string          // claude_cli: the child's working directory ("" = current directory)
	Environ    func() []string // claude_cli: the environment to filter for the child (nil = os.Environ)
	Wake       WakeFunc        // nil: wake entries are reported and skipped
	// Confirm asks the operator (once per session per tier) before first use of a
	// tier whose auto is false. Nil means no one can be asked: such a tier is refused.
	Confirm       func(prompt string) bool
	Now           func() time.Time
	HealthTimeout time.Duration // 0 = 2s
	HealthTTL     time.Duration // 0 = 30s
	WakePoll      time.Duration // 0 = 2s
}

type healthEntry struct {
	ok   bool
	why  string
	when time.Time
}

// Router is a model transport that picks the tier. It is the agent's Completer
// and its tier tracker (BeginTurn/Record).
type Router struct {
	opt Options

	mu       sync.Mutex
	cur      int
	pending  *int
	emit     func(map[string]any)
	clients  map[int]Completer
	health   map[int]healthEntry
	timeouts map[int]int
	wakeAt   map[int]time.Time
	approved map[int]bool
	denied   map[int]bool
	events   []map[string]any // escalations and refusals not yet recorded in a session
	redacted map[string]int   // redactions since the last Record, by tier name
	known    []string         // secret values this process holds, for exact-match redaction
	highest  int
	sentAny  bool        // a request body reached a transport during the current Complete
	cliRuns  map[int]int // claude_cli hand-offs started, by tier index
}

// New builds a router. Config must have at least one tier.
func New(opt Options) (*Router, error) {
	if len(opt.Config.Tiers) == 0 {
		return nil, errors.New("tiers: no tiers configured")
	}
	if opt.Getenv == nil {
		opt.Getenv = func(string) string { return "" }
	}
	if opt.Now == nil {
		opt.Now = time.Now
	}
	if opt.Sampler == nil {
		opt.Sampler = &SystemSampler{CPU: opt.Config.Resource.MaxCPUPercent > 0}
	}
	if opt.HealthTimeout <= 0 {
		opt.HealthTimeout = 2 * time.Second
	}
	if opt.HealthTTL <= 0 {
		opt.HealthTTL = 30 * time.Second
	}
	if opt.WakePoll <= 0 {
		opt.WakePoll = 2 * time.Second
	}
	r := &Router{opt: opt, clients: map[int]Completer{}, health: map[int]healthEntry{}, timeouts: map[int]int{},
		wakeAt: map[int]time.Time{}, approved: map[int]bool{}, denied: map[int]bool{}, redacted: map[string]int{}, cliRuns: map[int]int{}}
	if opt.Base.APIKey != "" {
		r.known = append(r.known, opt.Base.APIKey)
	}
	for _, t := range opt.Config.Tiers {
		if t.APIKeyEnv != "" {
			if v := opt.Getenv(t.APIKeyEnv); v != "" {
				r.known = append(r.known, v)
			}
		}
	}
	return r, nil
}

// Tiers returns the configured tiers.
func (r *Router) Tiers() []Tier { return r.opt.Config.Tiers }

// Current is the name of the tier the next call starts on.
func (r *Router) Current() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.opt.Config.Tiers[r.cur].Name
}

// SetConfirm installs the operator-confirmation function.
func (r *Router) SetConfirm(f func(string) bool) {
	r.mu.Lock()
	r.opt.Confirm = f
	r.mu.Unlock()
}

// scrub removes any known secret value from text that may be shown or stored.
func (r *Router) scrub(s string) string {
	for _, k := range r.known {
		if len(k) >= 8 {
			s = strings.ReplaceAll(s, k, "[redacted-key]")
		}
	}
	return s
}

func (r *Router) name(i int) string { return r.opt.Config.Tiers[i].Name }

func (r *Router) now() string { return r.opt.Now().UTC().Format(time.RFC3339) }

// post shows an event to the operator and queues it for the session file.
func (r *Router) post(ev map[string]any, record bool) {
	ev["line"] = r.scrub(fmt.Sprint(ev["line"]))
	if reason, ok := ev["reason"].(string); ok {
		ev["reason"] = r.scrub(reason)
	}
	ev["time"] = r.now()
	if r.emit != nil {
		cp := make(map[string]any, len(ev))
		for k, v := range ev {
			cp[k] = v
		}
		r.emit(cp)
	}
	if record {
		r.events = append(r.events, ev)
	}
}

func (r *Router) announce(from, to int, reason string) {
	line := fmt.Sprintf("[tier] escalated %s -> %s: %s", r.name(from), r.name(to), reason)
	if r.opt.Config.Tiers[to].Kind == KindClaudeCLI {
		line += " (handing the task to the claude CLI)"
	}
	r.post(map[string]any{"type": "tier_escalation", "from": r.name(from), "to": r.name(to),
		"kind": r.opt.Config.Tiers[to].Kind, "reason": reason, "line": line}, true)
	r.cur = to
	r.highest = max(r.highest, to)
}

func (r *Router) refuse(i int, reason string) {
	r.post(map[string]any{"type": "tier_refused", "tier": r.name(i), "reason": reason,
		"line": fmt.Sprintf("[tier] not using %s: %s", r.name(i), reason)}, true)
}

// BeginTurn is called at the start of every turn: it de-escalates to the lowest
// tier (the next call picks the lowest healthy one), unless the operator asked
// for /tier up, which applies to this turn only.
func (r *Router) BeginTurn(emit func(map[string]any)) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.emit = emit
	if r.pending != nil {
		target := *r.pending
		r.pending = nil
		if target != r.cur {
			r.announce(r.cur, target, "operator ran /tier up")
		}
		r.cur = target
		return
	}
	r.cur = 0
}

// RequestUp is the operator's /tier up: the next turn starts one tier above the
// one the last turn ended on. Typing the command is the operator's consent for
// a tier whose auto is false. It returns the target tier's name.
func (r *Router) RequestUp() (string, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	base := r.cur
	if r.pending != nil {
		base = *r.pending
	}
	for next := base + 1; next < len(r.opt.Config.Tiers); next++ {
		t := r.opt.Config.Tiers[next]
		if !t.Enabled {
			continue
		}
		r.approved[next] = true
		r.pending = &next
		return t.Name, nil
	}
	return "", fmt.Errorf("no higher tier is available (on %s)", r.name(base))
}

// RequestDown is the operator's /tier down: cancel a pending /tier up and go
// back to the lowest tier now.
func (r *Router) RequestDown() string {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.pending, r.cur = nil, 0
	return r.name(0)
}

// Record is called when a turn ends. It returns the value for the session
// file's plugin_state[SessionKey], or nil when there is nothing to record (a
// session that never left the first tier keeps a byte-identical file).
func (r *Router) Record(prev any) map[string]any {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.emit = nil
	pm, _ := prev.(map[string]any)
	if pm == nil && len(r.events) == 0 && r.cur == 0 && len(r.redacted) == 0 {
		return nil
	}
	var list []any
	if pm != nil {
		list, _ = pm["escalations"].([]any)
	}
	for _, ev := range r.events {
		list = append(list, ev)
	}
	if len(list) > maxRecordedEvents {
		list = list[len(list)-maxRecordedEvents:]
	}
	highest := r.highest
	if pm != nil {
		if n, ok := pm["highest_tier_index"].(float64); ok {
			highest = max(highest, int(n))
		}
		if n, ok := pm["highest_tier_index"].(int); ok {
			highest = max(highest, n)
		}
	}
	highest = min(highest, len(r.opt.Config.Tiers)-1)
	reds := map[string]any{}
	if pm != nil {
		if old, ok := pm["redactions"].(map[string]any); ok {
			for k, v := range old {
				reds[k] = v
			}
		}
	}
	for k, n := range r.redacted {
		prevN := 0
		switch v := reds[k].(type) {
		case int:
			prevN = v
		case float64:
			prevN = int(v)
		}
		reds[k] = prevN + n
	}
	rec := map[string]any{"current_tier": r.name(r.cur), "highest_tier": r.name(highest), "highest_tier_index": highest,
		"escalations": append([]any{}, list...)}
	if len(reds) > 0 {
		rec["redactions"] = reds
	}
	r.events, r.redacted = nil, map[string]int{}
	return rec
}

// ---- the model call --------------------------------------------------------

// estimateTokens is the chars/4 heuristic over everything the request carries.
func estimateTokens(req llm.Request) int {
	n := 0
	for _, m := range req.Messages {
		n += len(m.Content) + 16
		for _, tc := range m.ToolCalls {
			n += len(tc.Function.Name) + len(tc.Function.Arguments) + 16
		}
	}
	for _, t := range req.Tools {
		if b, err := json.Marshal(t); err == nil {
			n += len(b)
		}
	}
	return n / 4
}

func (r *Router) client(i int) (Completer, error) {
	if c, ok := r.clients[i]; ok {
		return c, nil
	}
	t := r.opt.Config.Tiers[i]
	cfg := r.opt.Base
	cfg.APIKey = ""
	var c Completer
	switch t.Kind {
	case KindClaudeCLI:
		c = r.cliClientFor(i)
	case KindClaude:
		key := r.opt.Getenv(t.APIKeyEnv)
		if key == "" {
			return nil, fmt.Errorf("environment variable %s is not set", t.APIKeyEnv)
		}
		cfg.APIKey, cfg.Model, cfg.BaseURL = key, t.Model, t.BaseURL
		c = &llm.AnthropicClient{Config: cfg, HTTPClient: r.opt.HTTPClient}
	default:
		if i == 0 && t.BaseURL == "" && t.Model == "" && r.opt.BaseClient != nil {
			c = r.opt.BaseClient
			break
		}
		if t.BaseURL != "" {
			cfg.BaseURL = t.BaseURL
		}
		if t.Model != "" {
			cfg.Model = t.Model
		}
		switch {
		case t.APIKeyEnv != "":
			cfg.APIKey = r.opt.Getenv(t.APIKeyEnv)
		case i == 0:
			cfg.APIKey = r.opt.Base.APIKey
		}
		c = &llm.Client{Config: cfg, HTTPClient: r.opt.HTTPClient}
	}
	r.clients[i] = c
	return c, nil
}

// cliTier builds the claude_cli client: it can never see an API key, and the
// literal secret values this process holds are kept out of its environment.
func (r *Router) cliClientFor(i int) *cliClient {
	return &cliClient{tier: r.opt.Config.Tiers[i], workdir: r.opt.Workdir, environ: r.opt.Environ, secrets: r.known}
}

// cliGate decides whether a claude_cli tier may take this request: the trigger
// must be enabled for it and its hand-off cap not used up. "" means allowed.
func (r *Router) cliGate(i int, trigger string) string {
	t := r.opt.Config.Tiers[i]
	ok := false
	for _, tr := range t.Triggers {
		if tr == trigger {
			ok = true
		}
	}
	if !ok {
		return fmt.Sprintf("%s is not one of its escalation triggers", trigger)
	}
	if t.MaxEscalations > 0 && r.cliRuns[i] >= t.MaxEscalations {
		return fmt.Sprintf("escalation cap reached (%d hand-off(s) this session)", t.MaxEscalations)
	}
	return ""
}

// probe is one health request: the tier's models endpoint, short timeout.
func (r *Router) probe(ctx context.Context, i int) (bool, string) {
	t := r.opt.Config.Tiers[i]
	var target string
	hdr := http.Header{}
	switch t.Kind {
	case KindClaudeCLI:
		p := t.CLIPath
		if p == "" {
			p = defaultCLIPath
		}
		if _, err := exec.LookPath(p); err != nil {
			return false, fmt.Sprintf("claude CLI %q not found", p)
		}
		return true, ""
	case KindClaude:
		c, err := r.client(i)
		if err != nil {
			return false, err.Error()
		}
		ac := c.(*llm.AnthropicClient)
		target = ac.ModelsEndpoint()
		ac.SetHeaders(hdr)
	default:
		base := t.BaseURL
		if base == "" {
			base = r.opt.Base.BaseURL
		}
		if base == "" {
			return true, "" // nothing to probe (an injected transport)
		}
		target = strings.TrimRight(base, "/") + "/models"
		key := ""
		if t.APIKeyEnv != "" {
			key = r.opt.Getenv(t.APIKeyEnv)
		} else if i == 0 {
			key = r.opt.Base.APIKey
		}
		if key != "" {
			hdr.Set("Authorization", "Bearer "+key)
		}
	}
	host := target
	if u, err := url.Parse(target); err == nil && u.Host != "" {
		host = u.Host
	}
	cctx, cancel := context.WithTimeout(ctx, r.opt.HealthTimeout)
	defer cancel()
	req, err := http.NewRequestWithContext(cctx, http.MethodGet, target, nil)
	if err != nil {
		return false, "bad base_url"
	}
	for k, v := range hdr {
		req.Header[k] = v
	}
	hc := r.opt.HTTPClient
	if hc == nil {
		hc = http.DefaultClient
	}
	resp, err := hc.Do(req)
	if err != nil {
		if ctx.Err() != nil {
			return false, "cancelled"
		}
		return false, "unreachable (" + host + ")"
	}
	resp.Body.Close()
	switch {
	case resp.StatusCode >= 500:
		return false, fmt.Sprintf("unhealthy (HTTP %d from %s)", resp.StatusCode, host)
	case t.Kind == KindClaude && (resp.StatusCode == 401 || resp.StatusCode == 403):
		return false, fmt.Sprintf("API key rejected (HTTP %d)", resp.StatusCode)
	}
	return true, ""
}

// healthy is probe with a cache (default 30 s, successes and failures alike).
func (r *Router) healthy(ctx context.Context, i int) (bool, string) {
	if e, ok := r.health[i]; ok && r.opt.Now().Sub(e.when) < r.opt.HealthTTL {
		return e.ok, e.why
	}
	ok, why := r.probe(ctx, i)
	r.health[i] = healthEntry{ok, why, r.opt.Now()}
	return ok, why
}

func (r *Router) markDown(i int, why string) {
	r.health[i] = healthEntry{false, why, r.opt.Now()}
}

// tryWake runs the tier's configured wake entry (never anything unconfigured)
// and waits for the tier to answer.
func (r *Router) tryWake(ctx context.Context, i int) bool {
	t := r.opt.Config.Tiers[i]
	if strings.TrimSpace(t.Wake) == "" {
		return false
	}
	if last, ok := r.wakeAt[i]; ok && r.opt.Now().Sub(last) < 2*time.Minute {
		return false
	}
	r.wakeAt[i] = r.opt.Now()
	if r.opt.Wake == nil {
		r.post(map[string]any{"type": "tier_wake", "tier": t.Name, "ok": false, "reason": "no wake runner available",
			"line": fmt.Sprintf("[tier] %s is down and has a wake entry, but it was not run: no wake runner", t.Name)}, false)
		return false
	}
	if err := r.opt.Wake(ctx, t); err != nil {
		r.post(map[string]any{"type": "tier_wake", "tier": t.Name, "ok": false, "reason": err.Error(),
			"line": fmt.Sprintf("[tier] wake for %s failed: %v", t.Name, err)}, false)
		return false
	}
	r.post(map[string]any{"type": "tier_wake", "tier": t.Name, "ok": true,
		"line": fmt.Sprintf("[tier] wake sent to %s; waiting for it to answer", t.Name)}, false)
	wait := time.Duration(t.WakeWait * float64(time.Second))
	if wait <= 0 {
		wait = 30 * time.Second
	}
	deadline := r.opt.Now().Add(wait)
	for {
		if ok, why := r.probe(ctx, i); ok {
			r.health[i] = healthEntry{true, "", r.opt.Now()}
			return true
		} else if why == "cancelled" {
			return false
		}
		if !r.opt.Now().Before(deadline) {
			return false
		}
		select {
		case <-time.After(r.opt.WakePoll):
		case <-ctx.Done():
			return false
		}
	}
}

type verdict int

const (
	usableOK verdict = iota
	unusable
	pressureOnly // fine except for resource pressure (advisory: last resort)
)

func (r *Router) usable(ctx context.Context, i int, req llm.Request) (verdict, string) {
	t := r.opt.Config.Tiers[i]
	if !t.Enabled {
		return unusable, "disabled in config"
	}
	if t.Kind == KindClaude {
		if _, err := r.client(i); err != nil {
			return unusable, err.Error()
		}
	}
	pressure := ""
	if i == 0 {
		pressure = r.opt.Config.Resource.Pressure(r.opt.Sampler.Sample(ctx))
	}
	if ok, why := r.healthy(ctx, i); !ok {
		if !r.tryWake(ctx, i) {
			return unusable, why
		}
	}
	if t.MaxContext > 0 {
		if est := estimateTokens(req); est > t.MaxContext {
			return unusable, fmt.Sprintf("the request (~%d tokens) does not fit its %d-token context window", est, t.MaxContext)
		}
	}
	if pressure != "" {
		return pressureOnly, "resource pressure: " + pressure
	}
	return usableOK, ""
}

// attempt guards and sends one request to tier i. done=false means "try the
// next tier"; the reason has been recorded in reasons.
func (r *Router) attempt(ctx context.Context, i int, req llm.Request, on func(llm.StreamEvent), reasons map[int]string) (*llm.Turn, bool, error) {
	t := r.opt.Config.Tiers[i]
	if !t.Auto && !r.approved[i] {
		switch {
		case r.denied[i]:
			reasons[i] = "the operator declined to use it this session"
			return nil, false, nil
		case r.opt.Confirm == nil:
			reasons[i] = "needs operator confirmation (auto is false) and none can be asked"
			r.refuse(i, reasons[i])
			return nil, false, nil
		case r.opt.Confirm(fmt.Sprintf("Send this conversation to tier %q (%s)? [y/N] ", t.Name, t.Kind)):
			r.approved[i] = true
		default:
			r.denied[i] = true
			reasons[i] = "the operator declined to use it this session"
			r.refuse(i, reasons[i])
			return nil, false, nil
		}
	}
	sent, rep, err := Sanitize(req, t.AllowData, r.known)
	if err != nil {
		reasons[i] = "refused: " + err.Error()
		r.refuse(i, reasons[i])
		return nil, false, nil
	}
	cl, err := r.client(i)
	if err != nil {
		reasons[i] = err.Error()
		return nil, false, nil
	}
	if rep.Redactions > 0 {
		r.redacted[t.Name] += rep.Redactions
		r.post(map[string]any{"type": "tier_redaction", "tier": t.Name, "count": rep.Redactions,
			"line": fmt.Sprintf("[tier] %s: %s", t.Name, describeGuard(rep))}, false)
	}
	if t.Kind == KindClaudeCLI {
		r.cliRuns[i]++
		defer r.recordCLI(i, cl)
	}
	for {
		r.sentAny = true
		started := false
		wrapped := on
		if on != nil {
			wrapped = func(e llm.StreamEvent) { started = true; on(e) }
		}
		turn, err := cl.Complete(ctx, sent, wrapped)
		if err == nil {
			r.timeouts[i] = 0
			r.health[i] = healthEntry{true, "", r.opt.Now()}
			return turn, true, nil
		}
		if ctx.Err() != nil || started {
			return nil, true, err
		}
		var be *llm.BackendError
		if !errors.As(err, &be) {
			return nil, true, err
		}
		switch {
		case be.Status == 0 && strings.Contains(be.Message, "timed out"):
			r.timeouts[i]++
			if r.timeouts[i] < 2 {
				continue // one more try on the same tier before calling it repeated
			}
			r.markDown(i, "repeated timeouts")
			reasons[i] = "repeated timeouts"
		case be.Status == 0:
			r.markDown(i, "unreachable")
			reasons[i] = "backend unreachable"
		case be.Status == 502 || be.Status == 503 || be.Status == 504:
			r.markDown(i, fmt.Sprintf("HTTP %d", be.Status))
			reasons[i] = fmt.Sprintf("backend unavailable (HTTP %d)", be.Status)
		default:
			return nil, true, err
		}
		return nil, false, nil
	}
}

// recordCLI puts the outcome of a claude_cli hand-off in the session record
// (the original's escalate_claude_code history entry) and shows it to the operator.
func (r *Router) recordCLI(i int, cl Completer) {
	c, ok := cl.(*cliClient)
	if !ok {
		return
	}
	t := r.opt.Config.Tiers[i]
	ev := map[string]any{"type": "tier_cli_result", "action": "escalate_claude_code", "tier": t.Name, "ok": c.last.OK,
		"cli_session_id": c.last.SessionID, "total_cost_usd": c.last.CostUSD,
		"stdout_preview": c.last.Stdout, "stderr_preview": c.last.Stderr}
	if c.last.OK {
		ev["line"] = fmt.Sprintf("[tier] %s: the claude CLI finished the task (cost $%.4f)", t.Name, c.last.CostUSD)
	} else {
		ev["error"] = c.last.Err
		ev["line"] = fmt.Sprintf("[tier] %s: the claude CLI hand-off failed: %s", t.Name, c.last.Err)
	}
	r.post(ev, true)
}

// Complete sends req to the lowest usable tier at or above the current one.
func (r *Router) Complete(ctx context.Context, req llm.Request, on func(llm.StreamEvent)) (*llm.Turn, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	tiers := r.opt.Config.Tiers
	reasons := map[int]string{}
	r.sentAny = false
	soft := -1
	from := r.cur
	for i := r.cur; i < len(tiers); i++ {
		v, why := r.usable(ctx, i, req)
		if v == unusable {
			reasons[i] = why
			continue
		}
		if v == pressureOnly {
			reasons[i], soft = why, i
			continue
		}
		tctx, trigger := ctx, TriggerBackendError
		if soft >= 0 {
			trigger = TriggerResourceExhausted
		}
		if tiers[i].Kind == KindClaudeCLI {
			if why := r.cliGate(i, trigger); why != "" {
				reasons[i] = why
				continue
			}
		}
		reason := "operator asked for this tier"
		if i > r.cur {
			reason = reasons[r.cur]
			for k := r.cur + 1; k < i; k++ {
				reason += fmt.Sprintf("; %s skipped: %s", r.name(k), reasons[k])
			}
			r.announce(r.cur, i, reason)
		}
		if tiers[i].Kind == KindClaudeCLI {
			tctx = withReason(ctx, trigger, reason)
		}
		turn, done, err := r.attempt(tctx, i, req, on, reasons)
		if done {
			if err == nil {
				r.cur = i
			}
			return turn, err
		}
	}
	if soft >= 0 {
		// Resource pressure is advisory: with nowhere better to go, carry on here.
		r.post(map[string]any{"type": "tier_notice", "tier": r.name(soft), "reason": reasons[soft],
			"line": fmt.Sprintf("[tier] %s; no higher tier could take the request, so staying on %s", reasons[soft], r.name(soft))}, false)
		turn, done, err := r.attempt(ctx, soft, req, on, reasons)
		if done {
			if err == nil {
				r.cur = soft
			}
			return turn, err
		}
	}
	var parts []string
	for i := from; i < len(tiers); i++ {
		parts = append(parts, fmt.Sprintf("%s: %s", r.name(i), reasons[i]))
	}
	msg := "no compute tier could run this request (" + strings.Join(parts, "; ") + ")"
	if !r.sentAny {
		msg += ". Nothing was sent to any tier."
	}
	return nil, &llm.BackendError{Message: r.scrub(msg)}
}

// ---- status ----------------------------------------------------------------

// TierStatus is one row of `sarina tiers status`.
type TierStatus struct {
	Name, Kind, BaseURL, Model string
	APIKeyEnv                  string
	KeySet                     bool
	AllowData                  string
	MaxContext                 int
	Wake                       bool
	Enabled, Auto              bool
	Healthy                    bool
	Why                        string
	Current                    bool
	CLIPath                    string // claude_cli: the executable as configured
	CLIFound                   bool   // claude_cli: it resolves on this machine
}

// Status probes every enabled tier (cache bypassed) and reports. It sends only
// health requests, never conversation data.
func (r *Router) Status(ctx context.Context) []TierStatus {
	r.mu.Lock()
	defer r.mu.Unlock()
	var out []TierStatus
	for i, t := range r.opt.Config.Tiers {
		s := TierStatus{Name: t.Name, Kind: t.Kind, BaseURL: t.BaseURL, Model: t.Model, APIKeyEnv: t.APIKeyEnv,
			AllowData: t.AllowData.String(), MaxContext: t.MaxContext, Wake: strings.TrimSpace(t.Wake) != "",
			Enabled: t.Enabled, Auto: t.Auto, Current: i == r.cur}
		if t.APIKeyEnv != "" {
			s.KeySet = r.opt.Getenv(t.APIKeyEnv) != ""
		}
		if t.Kind == KindClaudeCLI {
			s.CLIPath = t.CLIPath
			if s.CLIPath == "" {
				s.CLIPath = defaultCLIPath
			}
			_, lerr := exec.LookPath(s.CLIPath)
			s.CLIFound = lerr == nil
		}
		if s.BaseURL == "" && i == 0 {
			s.BaseURL = r.opt.Base.BaseURL
		}
		if s.Model == "" && t.Kind != KindClaude && t.Kind != KindClaudeCLI {
			s.Model = r.opt.Base.Model
		}
		switch {
		case !t.Enabled:
			s.Why = "disabled in config"
		case t.Kind == KindClaude && !s.KeySet:
			s.Why = "environment variable " + t.APIKeyEnv + " is not set"
		default:
			ok, why := r.probe(ctx, i)
			r.health[i] = healthEntry{ok, why, r.opt.Now()}
			s.Healthy, s.Why = ok, why
		}
		out = append(out, s)
	}
	return out
}

// Pressure takes one resource sample and evaluates it against the thresholds.
func (r *Router) Pressure(ctx context.Context) (Sample, string) {
	s := r.opt.Sampler.Sample(ctx)
	return s, r.opt.Config.Resource.Pressure(s)
}

// Thresholds returns the configured resource limits.
func (r *Router) Thresholds() Thresholds { return r.opt.Config.Resource }
