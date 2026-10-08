// Package tiers implements compute tiers: run on the local backend first, move
// to the operator's desktop when the device is not enough, and to Claude only
// as a last resort. The configuration is operator-owned (~/.sarina/tiers.json,
// or the file named by SARINA_TIERS_FILE) and is never read from a workspace.
// Moving up a tier changes where the model runs, never the tools or permissions.
package tiers

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Tier kinds.
const (
	KindOpenAICompat = "openai_compat"
	KindClaude       = "claude"
	// KindClaudeCLI hands the task to the local `claude` executable (print
	// mode), as the original escalation did. Opt-in: see ParseEscalation.
	KindClaudeCLI = "claude_cli"
)

// Data classes an allow_data list can restrict.
const (
	ClassNoSecrets  = "no_secrets"  // credential-shaped material is redacted before sending
	ClassNoCaptures = "no_captures" // RF/NFC capture dumps are never sent (the tier is refused)
)

// AllowData says what may be sent to a tier. All means no restriction;
// otherwise each listed class is enforced.
type AllowData struct {
	All     bool
	Classes []string
}

// Has reports whether the class restriction is in force.
func (a AllowData) Has(class string) bool {
	if a.All {
		return false
	}
	for _, c := range a.Classes {
		if c == class {
			return true
		}
	}
	return false
}

// String renders for status output.
func (a AllowData) String() string {
	if a.All {
		return "all"
	}
	if len(a.Classes) == 0 {
		return "(nothing restricted)"
	}
	return strings.Join(a.Classes, ",")
}

// MarshalJSON writes "all" or the class list.
func (a AllowData) MarshalJSON() ([]byte, error) {
	if a.All {
		return []byte(`"all"`), nil
	}
	return json.Marshal(append([]string{}, a.Classes...))
}

func parseAllow(raw json.RawMessage, def AllowData) (AllowData, []string, error) {
	if len(raw) == 0 || string(raw) == "null" {
		return def, nil, nil
	}
	var one string
	if json.Unmarshal(raw, &one) == nil {
		if strings.EqualFold(strings.TrimSpace(one), "all") {
			return AllowData{All: true}, nil, nil
		}
		return AllowData{}, nil, fmt.Errorf("allow_data must be \"all\" or a list of classes, got %q", one)
	}
	var list []string
	if err := json.Unmarshal(raw, &list); err != nil {
		return AllowData{}, nil, errors.New("allow_data must be \"all\" or a list of classes")
	}
	out := AllowData{}
	var warns []string
	for _, c := range list {
		switch c = strings.ToLower(strings.TrimSpace(c)); c {
		case "all":
			return AllowData{All: true}, nil, nil
		case ClassNoSecrets, ClassNoCaptures:
			out.Classes = append(out.Classes, c)
		default:
			warns = append(warns, fmt.Sprintf("unknown allow_data class %q ignored", c))
		}
	}
	return out, warns, nil
}

// Tier is one compute tier.
type Tier struct {
	Name       string
	Kind       string
	BaseURL    string
	Model      string
	APIKeyEnv  string // the NAME of an environment variable; the key itself is never stored
	AllowData  AllowData
	MaxContext int // tokens; 0 = unknown/unlimited
	Wake       string
	WakeWait   float64 // seconds to wait for the tier to come up after wake
	Enabled    bool
	Auto       bool // false: ask the operator once per session before first use

	// claude_cli only (the original escalation_config).
	CLIPath        string   // executable, default "claude"
	TimeoutSeconds float64  // default 300
	MaxEscalations int      // hand-offs per router (session), default 1
	Triggers       []string // backend_error and/or resource_exhausted; default both
	ExtraArgs      []string
	ExtraDirs      []string // passed as --add-dir
	AcceptEdits    bool     // --permission-mode acceptEdits; never a bypass mode
}

// Thresholds are the resource-pressure limits on the tier-0 device, in percent.
// Zero disables a check.
type Thresholds struct {
	MaxMemoryPercent    float64
	MaxCPUPercent       float64 // 0 = off (opt-in, as spec 01 section 11.2 recommends)
	MaxGPUMemoryPercent float64
}

// Config is the whole tier setup.
type Config struct {
	Tiers    []Tier
	Resource Thresholds
	Warnings []string
	Path     string // where it was read from; empty for the default
}

// Multi reports whether any escalation is possible.
func (c Config) Multi() bool { return len(c.Tiers) > 1 }

// Default is the no-file configuration: nothing but the local backend.
func Default() Config {
	return Config{Tiers: []Tier{{Name: "local", Kind: KindOpenAICompat, AllowData: AllowData{All: true}, Enabled: true, Auto: true}},
		Resource: Thresholds{MaxMemoryPercent: 90, MaxGPUMemoryPercent: 90}}
}

type tierFile struct {
	Name       string          `json:"name"`
	Kind       string          `json:"kind"`
	BaseURL    string          `json:"base_url"`
	Model      string          `json:"model"`
	APIKeyEnv  string          `json:"api_key_env"`
	AllowData  json.RawMessage `json:"allow_data"`
	MaxContext int             `json:"max_context"`
	Wake       string          `json:"wake"`
	WakeWait   float64         `json:"wake_wait_seconds"`
	Enabled    *bool           `json:"enabled"`
	Auto       *bool           `json:"auto"`

	CLIPath        string   `json:"cli_path"`
	TimeoutSeconds float64  `json:"timeout_seconds"`
	MaxEscalations *int     `json:"max_escalations"`
	Triggers       []string `json:"trigger_stop_reasons"`
	ExtraArgs      []string `json:"extra_cli_args"`
	ExtraDirs      []string `json:"add_dirs"`
	AcceptEdits    bool     `json:"accept_edits"`
}

type configFile struct {
	Tiers    []tierFile `json:"tiers"`
	Resource *struct {
		MaxMemoryPercent    *float64 `json:"max_memory_percent"`
		MaxCPUPercent       *float64 `json:"max_cpu_percent"`
		MaxGPUMemoryPercent *float64 `json:"max_gpu_memory_percent"`
	} `json:"resource"`
}

// Path returns the operator config path: SARINA_TIERS_FILE, else
// <home>/.sarina/tiers.json (home empty = the user's real home directory).
func Path(home string, getenv func(string) string) string {
	if getenv != nil {
		if p := strings.TrimSpace(getenv("SARINA_TIERS_FILE")); p != "" {
			return p
		}
	}
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	if home == "" {
		return ""
	}
	return filepath.Join(home, ".sarina", "tiers.json")
}

// Load reads the operator config. A missing file is the default (one local
// tier). A file that exists but is invalid returns an error AND the default
// config, so a caller that only warns still stays local-only: a broken file can
// never widen where data goes.
func Load(home string, getenv func(string) string) (Config, error) {
	p := Path(home, getenv)
	if p == "" {
		return Default(), nil
	}
	b, err := os.ReadFile(p)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return Default(), nil
		}
		return Default(), fmt.Errorf("cannot read %s: %w", p, err)
	}
	c, err := Parse(b)
	if err != nil {
		return Default(), fmt.Errorf("%s: %w", p, err)
	}
	c.Path = p
	return c, nil
}

// Parse validates a tiers.json document.
func Parse(data []byte) (Config, error) {
	var f configFile
	if err := json.Unmarshal(data, &f); err != nil {
		return Config{}, fmt.Errorf("invalid JSON: %w", err)
	}
	cfg := Config{Resource: Thresholds{MaxMemoryPercent: 90, MaxGPUMemoryPercent: 90}}
	if f.Resource != nil {
		if f.Resource.MaxMemoryPercent != nil {
			cfg.Resource.MaxMemoryPercent = *f.Resource.MaxMemoryPercent
		}
		if f.Resource.MaxCPUPercent != nil {
			cfg.Resource.MaxCPUPercent = *f.Resource.MaxCPUPercent
		}
		if f.Resource.MaxGPUMemoryPercent != nil {
			cfg.Resource.MaxGPUMemoryPercent = *f.Resource.MaxGPUMemoryPercent
		}
	}
	if len(f.Tiers) == 0 {
		return Default(), nil
	}
	seen := map[string]bool{}
	for i, tf := range f.Tiers {
		t := Tier{Name: strings.TrimSpace(tf.Name), Kind: strings.TrimSpace(tf.Kind), BaseURL: strings.TrimSpace(tf.BaseURL),
			Model: strings.TrimSpace(tf.Model), APIKeyEnv: strings.TrimSpace(tf.APIKeyEnv), MaxContext: tf.MaxContext,
			Wake: strings.TrimSpace(tf.Wake), WakeWait: tf.WakeWait, Enabled: true, Auto: true}
		if t.Kind == "" {
			t.Kind = KindOpenAICompat
		}
		if t.Kind != KindOpenAICompat && t.Kind != KindClaude && t.Kind != KindClaudeCLI {
			return Config{}, fmt.Errorf("tier %d: unknown kind %q (use %q, %q or %q)", i+1, t.Kind, KindOpenAICompat, KindClaude, KindClaudeCLI)
		}
		if t.Name == "" {
			t.Name = fmt.Sprintf("tier%d", i+1)
		}
		if seen[strings.ToLower(t.Name)] {
			return Config{}, fmt.Errorf("duplicate tier name %q", t.Name)
		}
		seen[strings.ToLower(t.Name)] = true
		if tf.Enabled != nil {
			t.Enabled = *tf.Enabled
		}
		if tf.Auto != nil {
			t.Auto = *tf.Auto
		}
		if t.MaxContext < 0 {
			return Config{}, fmt.Errorf("tier %q: max_context must not be negative", t.Name)
		}
		switch t.Kind {
		case KindClaude:
			if t.Model == "" {
				return Config{}, fmt.Errorf("tier %q: a claude tier needs a model", t.Name)
			}
			if t.APIKeyEnv == "" {
				return Config{}, fmt.Errorf("tier %q: a claude tier needs api_key_env (the NAME of the environment variable holding the key)", t.Name)
			}
		case KindClaudeCLI:
			if i == 0 {
				return Config{}, fmt.Errorf("tier %q: a claude_cli tier is a fallback and cannot be the first tier", t.Name)
			}
			t.CLIPath, t.TimeoutSeconds = tf.CLIPath, tf.TimeoutSeconds
			if t.CLIPath == "" {
				t.CLIPath = defaultCLIPath
			}
			if t.TimeoutSeconds < 0 {
				return Config{}, fmt.Errorf("tier %q: timeout_seconds must not be negative", t.Name)
			}
			if t.TimeoutSeconds == 0 {
				t.TimeoutSeconds = defaultCLITimeout
			}
			t.MaxEscalations = defaultMaxEscalation
			if tf.MaxEscalations != nil {
				t.MaxEscalations = *tf.MaxEscalations
			}
			t.Triggers = tf.Triggers
			if t.Triggers == nil {
				t.Triggers = []string{TriggerBackendError, TriggerResourceExhausted}
			}
			for _, tr := range t.Triggers {
				if tr != TriggerBackendError && tr != TriggerResourceExhausted {
					cfg.Warnings = append(cfg.Warnings, fmt.Sprintf("tier %q: trigger %q is decided by the agent loop, not the tier router, and is ignored here", t.Name, tr))
				}
			}
			t.ExtraArgs, t.ExtraDirs, t.AcceptEdits = tf.ExtraArgs, tf.ExtraDirs, tf.AcceptEdits
		default:
			if i > 0 && t.BaseURL == "" {
				return Config{}, fmt.Errorf("tier %q: base_url is required", t.Name)
			}
		}
		if looksLikeKey(t.APIKeyEnv) {
			return Config{}, fmt.Errorf("tier %q: api_key_env must be the NAME of an environment variable, not a key", t.Name)
		}
		// Safe defaults: the first tier is the device itself; every later tier
		// gets secrets redacted, and Claude also never receives captures.
		def := AllowData{All: true}
		if i > 0 {
			def = AllowData{Classes: []string{ClassNoSecrets}}
			if t.Kind == KindClaude || t.Kind == KindClaudeCLI {
				def.Classes = append(def.Classes, ClassNoCaptures)
			}
		}
		allow, warns, err := parseAllow(tf.AllowData, def)
		if err != nil {
			return Config{}, fmt.Errorf("tier %q: %w", t.Name, err)
		}
		t.AllowData = allow
		for _, w := range warns {
			cfg.Warnings = append(cfg.Warnings, fmt.Sprintf("tier %q: %s", t.Name, w))
		}
		cfg.Tiers = append(cfg.Tiers, t)
	}
	return cfg, nil
}

// looksLikeKey catches the mistake of pasting a key where a variable name belongs.
func looksLikeKey(s string) bool {
	if s == "" {
		return false
	}
	if strings.ContainsAny(s, " =:/.") || strings.HasPrefix(s, "sk-") {
		return true
	}
	for _, r := range s {
		if !(r == '_' || r >= 'A' && r <= 'Z' || r >= 'a' && r <= 'z' || r >= '0' && r <= '9') {
			return true
		}
	}
	return false
}
