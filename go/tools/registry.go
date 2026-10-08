// Package tools defines the tool registry the agent loop dispatches to, the
// allowlist filter, and the extension point other packages use to add tools.
package tools

import (
	"context"
	"errors"
	"fmt"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// Context is what a handler receives besides its arguments.
type Context struct {
	Root             string // absolute, symlink-resolved workspace directory
	CommandTimeout   time.Duration
	MaxOutputChars   int
	Permissions      config.Permissions
	ExtraEnv         map[string]string
	Registry         *Registry
	SessionDirectory string
	// Runtimes lets other packages attach runtime handles (memory, limbo, ...)
	// that their own tools look up by name.
	Runtimes map[string]any

	// Environ returns the parent environment ("KEY=value" entries) that child
	// processes start from; nil means os.Environ. Secret-looking names are
	// always removed from it before a child sees it.
	Environ func() []string
	// SessionEnv are variables the operator set for this session. They are
	// applied to children after the secret filter and are never echoed.
	SessionEnv map[string]string

	// Approver, when set, is asked before a tool that needs write/shell
	// permission or that changes state runs (after the permission and schema
	// checks, before the handler). A refusal becomes a permission_denied
	// result. Interactive front ends use it for per-call consent; nil means no
	// per-call question (the permission flags alone decide).
	Approver Approver
}

// ApprovalRequest describes one call awaiting the operator's consent.
type ApprovalRequest struct {
	Tool      string
	Arguments map[string]any
	Needs     Permission // what the tool requires (PermNone for a mutating tool that needs no flag)
}

// Approver answers an ApprovalRequest. It may block until the operator
// decides; ctx is cancelled when the run is stopped. allow=false carries an
// optional reason the model will read.
type Approver func(ctx context.Context, req ApprovalRequest) (allow bool, reason string)

// needsApproval says whether a call to t must be put to the Approver.
func needsApproval(t Tool) bool {
	return t.Requires != PermNone || t.Class == ClassMutating || t.Class == ClassUnset
}

// Handler runs a tool. It returns text plus optional metadata. Returning a
// *PermissionDeniedError marks the call as denied by configuration; any other
// error becomes a tool_execution_error result. Handlers may also panic: the
// registry converts that to an error result.
type Handler func(ctx context.Context, args map[string]any, tc *Context) (string, map[string]any, error)

// Emit receives live output from a streaming tool: the stream label ("stdout"
// or "stderr") and a chunk of text.
type Emit func(stream, text string)

// StreamHandler is a Handler that can also report output while it runs.
type StreamHandler func(ctx context.Context, args map[string]any, tc *Context, emit Emit) (string, map[string]any, error)

// Tool is a name, a model-visible description and schema, a handler, and the
// permission the call needs. The permission is checked centrally before the
// handler runs, so no handler can forget it.
type Tool struct {
	Spec     llm.ToolSpec
	Handler  Handler
	Requires Permission
	// Class says whether the tool may run in plan mode. The zero value means
	// "undecided" and is treated as mutating, so a forgotten tool is never
	// offered in plan mode; a test fails for any default tool left undecided.
	Class Class
	// Stream, when set, is used instead of Handler by ExecuteStream.
	Stream StreamHandler
}

// Result of executing one tool call.
type Result struct {
	Name     string
	OK       bool
	Content  string
	Metadata map[string]any
}

// PermissionDeniedError is returned by handlers refused by configuration.
type PermissionDeniedError struct{ Message string }

func (e *PermissionDeniedError) Error() string { return e.Message }

// Denied builds a permission refusal.
func Denied(format string, a ...any) error {
	return &PermissionDeniedError{Message: fmt.Sprintf(format, a...)}
}

// Registry is an ordered set of tools.
type Registry struct {
	order []string
	tools map[string]Tool
	// deny, when set, names tools the operator's policy forbids: they are not
	// offered (Names, Specs, Len skip them) and a call is refused as
	// permission_denied. It returns the reason, or "" for an allowed name.
	deny func(name string) string
	// perms, when set, hides tools whose required permission is not granted.
	perms *config.Permissions
}

func (r *Registry) hidden(name string) bool {
	if r.deny != nil && r.deny(name) != "" {
		return true
	}
	if r.perms != nil {
		switch r.tools[name].Requires {
		case PermWrite:
			return !r.perms.AllowFileWrite
		case PermShell:
			return !r.perms.AllowShellCommands
		}
	}
	return false
}

// NewRegistry builds a registry; later tools replace earlier ones of the same name.
func NewRegistry(ts ...Tool) *Registry {
	r := &Registry{tools: map[string]Tool{}}
	for _, t := range ts {
		r.Register(t)
	}
	return r
}

// Register adds or replaces a tool.
func (r *Registry) Register(t Tool) {
	if _, exists := r.tools[t.Spec.Name]; !exists {
		r.order = append(r.order, t.Spec.Name)
	}
	r.tools[t.Spec.Name] = t
}

// Len is the number of tools.
func (r *Registry) Len() int {
	if r == nil {
		return 0
	}
	return len(r.Names())
}

// Names lists tool names in registration order.
func (r *Registry) Names() []string {
	if r == nil {
		return nil
	}
	out := make([]string, 0, len(r.order))
	for _, n := range r.order {
		if !r.hidden(n) {
			out = append(out, n)
		}
	}
	return out
}

// Get returns a tool by name.
func (r *Registry) Get(name string) (Tool, bool) {
	if r == nil {
		return Tool{}, false
	}
	t, ok := r.tools[name]
	return t, ok
}

// Specs returns the model-visible definitions in registration order.
func (r *Registry) Specs() []llm.ToolSpec {
	if r == nil {
		return nil
	}
	out := make([]llm.ToolSpec, 0, len(r.order))
	for _, n := range r.order {
		if !r.hidden(n) {
			out = append(out, r.tools[n].Spec)
		}
	}
	return out
}

// Filter returns a registry limited to the allowlist. A nil allowlist means
// "no restriction"; a non-nil one, even empty, keeps only the listed names, so
// an empty list yields zero tools. Unknown names grant nothing.
func (r *Registry) Filter(allow []string) *Registry {
	if allow == nil {
		return NewRegistry(r.list()...)
	}
	keep := map[string]bool{}
	for _, n := range allow {
		keep[n] = true
	}
	out := NewRegistry()
	for _, n := range r.order {
		if keep[n] {
			out.Register(r.tools[n])
		}
	}
	return out
}

func (r *Registry) list() []Tool {
	ts := make([]Tool, 0, len(r.order))
	for _, n := range r.order {
		ts = append(ts, r.tools[n])
	}
	return ts
}

// Execute runs one tool call. It never panics and never returns an error:
// failures are results the model can read. Order: unknown tool, permission
// gate, schema validation (types, required, enum, minimum, maximum), handler.
func (r *Registry) Execute(ctx context.Context, name string, args map[string]any, tc *Context) Result {
	return r.execute(ctx, name, args, tc, nil)
}

// ExecuteStream is Execute for a caller that wants live output. Tools with a
// Stream handler report through emit and the result carries streamed=true; all
// others behave exactly as in Execute. The permission gate is identical, and a
// refusal carries the same error_kind on both paths.
func (r *Registry) ExecuteStream(ctx context.Context, name string, args map[string]any, tc *Context, emit Emit) Result {
	return r.execute(ctx, name, args, tc, emit)
}

func (r *Registry) execute(ctx context.Context, name string, args map[string]any, tc *Context, emit Emit) (res Result) {
	t, ok := r.Get(name)
	if !ok {
		return Result{Name: name, Content: fmt.Sprintf("unknown tool %q", name), Metadata: map[string]any{}}
	}
	if r.deny != nil {
		if why := r.deny(name); why != "" {
			return Result{Name: name, Content: why, Metadata: map[string]any{"error_kind": "permission_denied"}}
		}
	}
	if args == nil {
		args = map[string]any{}
	}
	if tc == nil {
		tc = &Context{}
	}
	defer func() {
		if p := recover(); p != nil {
			res = Result{Name: name, Content: fmt.Sprintf("tool %s failed internally: %v", name, p),
				Metadata: map[string]any{"error_kind": "tool_execution_error"}}
		}
	}()
	if err := checkPermission(t, tc); err != nil {
		return Result{Name: name, Content: err.Error(), Metadata: map[string]any{"error_kind": "permission_denied"}}
	}
	args, err := ValidateArgs(t.Spec.Parameters, args)
	if err != nil {
		return Result{Name: name, Content: fmt.Sprintf("invalid arguments for %s: %v", name, err),
			Metadata: map[string]any{"error_kind": "tool_execution_error"}}
	}
	if tc.Approver != nil && needsApproval(t) {
		allow, why := tc.Approver(ctx, ApprovalRequest{Tool: name, Arguments: args, Needs: t.Requires})
		if !allow {
			if why == "" {
				why = "the operator did not approve this call"
			}
			return Result{Name: name, Content: "denied by the operator: " + why, Metadata: map[string]any{"error_kind": "permission_denied"}}
		}
	}
	var content string
	var md map[string]any
	streamed := false
	if emit != nil && t.Stream != nil {
		streamed = true
		content, md, err = t.Stream(ctx, args, tc, emit)
	} else {
		content, md, err = t.Handler(ctx, args, tc)
	}
	out := map[string]any{}
	for k, v := range md {
		out[k] = v
	}
	if streamed {
		out["streamed"] = true
	}
	if err != nil {
		var denied *PermissionDeniedError
		if errors.As(err, &denied) {
			out["error_kind"] = "permission_denied"
		} else {
			out["error_kind"] = "tool_execution_error"
		}
		return Result{Name: name, Content: err.Error(), Metadata: out}
	}
	return Result{Name: name, OK: true, Content: content, Metadata: out}
}

// ---- extension point ------------------------------------------------------

// Provider contributes tools to every default registry. Other packages call
// RegisterProvider from init() or from their setup code before agents are built.
type Provider func() []Tool

var (
	providerMu sync.Mutex
	providers  []Provider
)

// RegisterProvider appends a tool provider to the default registry.
func RegisterProvider(p Provider) {
	providerMu.Lock()
	defer providerMu.Unlock()
	providers = append(providers, p)
}

// Builtins returns the built-in tools followed by every provider's tools.
func Builtins() []Tool {
	ts := DefaultTools()
	providerMu.Lock()
	ps := append([]Provider(nil), providers...)
	providerMu.Unlock()
	for _, p := range ps {
		ts = append(ts, p()...)
	}
	return ts
}

// Builder assembles a registry and re-applies the allowlist on every Build,
// so a rebuild (for instance after the workspace changes) cannot undo it.
type Builder struct {
	// Allowlist: nil = all tools; non-nil (even empty) = exactly those names.
	Allowlist  []string
	Extra      []Tool // appended after the defaults
	NoDefaults bool   // skip Builtins (tests, custom setups)
	// Narrow, when non-nil, keeps only the listed names (empty = no tools). It
	// can only remove tools the allowlist left, never add one. Used by skills.
	Narrow []string
	// PlanMode limits the registry to read-only tools plus exit_plan_mode. It
	// is applied last and only removes tools.
	PlanMode bool
	// Deny, when non-nil, is the operator policy's deny check (name -> reason,
	// "" = allowed). Denied tools are neither offered nor runnable. It is applied
	// last and only removes tools.
	Deny func(name string) string
	// Perms, when non-nil, withholds every tool whose Requires permission the
	// operator has not granted (a denied tool is not offered at all). It only
	// removes tools from the offer; a call that arrives anyway is still refused at call time.
	Perms *config.Permissions
}

// Build returns a fresh registry.
func (b Builder) Build() *Registry {
	reg := NewRegistry()
	if !b.NoDefaults {
		for _, t := range Builtins() {
			reg.Register(t)
		}
	}
	for _, t := range b.Extra {
		reg.Register(t)
	}
	reg = reg.Filter(b.Allowlist)
	if b.Narrow != nil {
		reg = reg.Filter(b.Narrow)
	}
	reg = reg.ForMode(b.PlanMode)
	if b.Perms != nil {
		reg = reg.forPermissions(*b.Perms)
	}
	reg.deny = b.Deny
	return reg
}

// forPermissions stops offering the tools whose required permission is not
// granted. They stay known to the registry, so a call that arrives anyway
// reaches the permission gate and is refused with the usual explanation.
func (r *Registry) forPermissions(p config.Permissions) *Registry {
	r.perms = &p
	return r
}
