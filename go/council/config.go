// Package council asks several models the same question and shows their answers
// side by side, so a person can see where they agree and where they do not.
//
// It is advice, never action. Members are sent text only (no tools, no
// permissions), their answers are untrusted text a person reads, and nothing
// here can run a command or change a file. The configuration is operator-owned
// (~/.sarina/council.json, or the file named by SARINA_COUNCIL_FILE) and is
// never read from a workspace. A member whose endpoint is not on this machine is
// "remote": it gets secrets and capture dumps stripped by default and is asked
// about once per run before anything is sent.
//
// The council is CLI-only. It must never become a model-callable tool: a tool
// that sends the conversation to third parties is an exfiltration path for a
// prompt-injected session. A test enforces that.
package council

import (
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

// Member kinds. Gemini and ChatGPT (OpenAI) are reached through their
// OpenAI-compatible endpoints, so they are KindOpenAICompat with that base_url.
const (
	KindOpenAICompat = "openai_compat"
	KindClaude       = "claude" // Anthropic Messages API; base_url may be left empty
)

const (
	maxMembers         = 8
	defaultMaxTokens   = 1024
	defaultTimeoutSecs = 90.0
	defaultLocalConc   = 1 // local members share one GPU: one at a time unless told otherwise
	defaultMaxRemote   = 3 // remote members asked per run
)

// Member is one model in the council.
type Member struct {
	Name      string
	Kind      string // KindOpenAICompat (default) or KindClaude
	BaseURL   string
	Model     string
	APIKeyEnv string // the NAME of an environment variable; the key itself is never stored
	AllowData tiers.AllowData
	Local     bool // derived from base_url (loopback), never set by hand
	Auto      bool // false: ask the operator once per run before first use
	Enabled   bool
}

// Config is the whole council setup.
type Config struct {
	Members        []Member
	Synthesizer    string // name of a local member; "" = none
	MaxTokens      int
	TimeoutSeconds float64
	LocalConc      int // local members asked at the same time (default 1)
	MaxRemote      int // remote members asked per run (default 3)
	Warnings       []string
	Path           string // where it was read from; empty when there is no file
}

type memberFile struct {
	Name      string          `json:"name"`
	Kind      string          `json:"kind"`
	BaseURL   string          `json:"base_url"`
	Model     string          `json:"model"`
	APIKeyEnv string          `json:"api_key_env"`
	AllowData json.RawMessage `json:"allow_data"`
	Enabled   *bool           `json:"enabled"`
	Auto      *bool           `json:"auto"`
}

type configFile struct {
	Members        []memberFile `json:"members"`
	Synthesizer    string       `json:"synthesizer"`
	MaxTokens      int          `json:"max_tokens"`
	TimeoutSeconds float64      `json:"timeout_seconds"`
	LocalConc      int          `json:"local_concurrency"`
	MaxRemote      int          `json:"max_remote_members"`
}

// Path returns the operator config path: SARINA_COUNCIL_FILE, else
// <home>/.sarina/council.json (home empty = the user's real home directory).
func Path(home string, getenv func(string) string) string {
	if getenv != nil {
		if p := strings.TrimSpace(getenv("SARINA_COUNCIL_FILE")); p != "" {
			return p
		}
	}
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	if home == "" {
		return ""
	}
	return filepath.Join(home, ".sarina", "council.json")
}

// Load reads the operator config. A missing file is an empty council. A file that
// exists but is invalid returns an error and an empty council, so a broken file
// can never widen where data goes.
func Load(home string, getenv func(string) string) (Config, error) {
	p := Path(home, getenv)
	if p == "" {
		return Config{}, nil
	}
	b, err := os.ReadFile(p)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return Config{}, nil
		}
		return Config{}, fmt.Errorf("cannot read %s: %w", p, err)
	}
	c, err := Parse(b)
	if err != nil {
		return Config{}, fmt.Errorf("%s: %w", p, err)
	}
	c.Path = p
	return c, nil
}

// IsLocalURL reports whether a base URL points at this machine (a loopback
// address or "localhost"). Anything else, or anything unparsable, is remote.
func IsLocalURL(raw string) bool {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Hostname() == "" {
		return false
	}
	h := strings.ToLower(u.Hostname())
	if h == "localhost" {
		return true
	}
	ip := net.ParseIP(h)
	return ip != nil && ip.IsLoopback()
}

// Parse validates a council.json document.
func Parse(data []byte) (Config, error) {
	var f configFile
	if err := json.Unmarshal(data, &f); err != nil {
		return Config{}, fmt.Errorf("invalid JSON: %w", err)
	}
	if len(f.Members) > maxMembers {
		return Config{}, fmt.Errorf("too many members (%d, at most %d)", len(f.Members), maxMembers)
	}
	cfg := Config{MaxTokens: f.MaxTokens, TimeoutSeconds: f.TimeoutSeconds, LocalConc: f.LocalConc, MaxRemote: f.MaxRemote}
	if cfg.MaxTokens < 0 || cfg.TimeoutSeconds < 0 || cfg.LocalConc < 0 || cfg.MaxRemote < 0 {
		return Config{}, errors.New("max_tokens, timeout_seconds, local_concurrency and max_remote_members must not be negative")
	}
	if cfg.LocalConc == 0 {
		cfg.LocalConc = defaultLocalConc
	}
	if cfg.MaxRemote == 0 {
		cfg.MaxRemote = defaultMaxRemote
	}
	if cfg.MaxTokens == 0 {
		cfg.MaxTokens = defaultMaxTokens
	}
	if cfg.TimeoutSeconds == 0 {
		cfg.TimeoutSeconds = defaultTimeoutSecs
	}
	seen := map[string]bool{}
	for i, mf := range f.Members {
		m := Member{Name: strings.TrimSpace(mf.Name), Kind: strings.TrimSpace(mf.Kind), BaseURL: strings.TrimSpace(mf.BaseURL),
			Model: strings.TrimSpace(mf.Model), APIKeyEnv: strings.TrimSpace(mf.APIKeyEnv), Enabled: true}
		if m.Name == "" {
			m.Name = fmt.Sprintf("member%d", i+1)
		}
		if seen[strings.ToLower(m.Name)] {
			return Config{}, fmt.Errorf("duplicate member name %q", m.Name)
		}
		seen[strings.ToLower(m.Name)] = true
		if m.Kind == "" {
			m.Kind = KindOpenAICompat
		}
		switch m.Kind {
		case KindOpenAICompat:
			if m.BaseURL == "" || m.Model == "" {
				return Config{}, fmt.Errorf("member %q: base_url and model are required", m.Name)
			}
		case KindClaude:
			if m.Model == "" || m.APIKeyEnv == "" {
				return Config{}, fmt.Errorf("member %q: a claude member needs model and api_key_env (the NAME of the variable holding the key)", m.Name)
			}
		default:
			return Config{}, fmt.Errorf("member %q: unknown kind %q (use %q or %q)", m.Name, m.Kind, KindOpenAICompat, KindClaude)
		}
		if m.BaseURL != "" {
			if u, err := url.Parse(m.BaseURL); err != nil || u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") {
				return Config{}, fmt.Errorf("member %q: base_url must be an http(s) URL", m.Name)
			}
		}
		if tiers.LooksLikeKey(m.APIKeyEnv) {
			return Config{}, fmt.Errorf("member %q: api_key_env must be the NAME of an environment variable, not a key", m.Name)
		}
		m.Local = IsLocalURL(m.BaseURL)
		def := tiers.AllowData{All: true}
		m.Auto = true
		if !m.Local {
			def = tiers.AllowData{Classes: []string{tiers.ClassNoSecrets, tiers.ClassNoCaptures}}
			m.Auto = false
		}
		allow, warns, err := tiers.ParseAllowData(mf.AllowData, def)
		if err != nil {
			return Config{}, fmt.Errorf("member %q: %w", m.Name, err)
		}
		m.AllowData = allow
		for _, w := range warns {
			cfg.Warnings = append(cfg.Warnings, fmt.Sprintf("member %q: %s", m.Name, w))
		}
		if !m.Local && allow.All {
			cfg.Warnings = append(cfg.Warnings, fmt.Sprintf("member %q is remote and allow_data is \"all\": secrets and captures will be sent to it", m.Name))
		}
		if mf.Enabled != nil {
			m.Enabled = *mf.Enabled
		}
		if mf.Auto != nil {
			m.Auto = *mf.Auto
		}
		cfg.Members = append(cfg.Members, m)
	}
	switch name := strings.TrimSpace(f.Synthesizer); {
	case name != "":
		found := false
		for _, m := range cfg.Members {
			if strings.EqualFold(m.Name, name) {
				if !m.Local {
					return Config{}, fmt.Errorf("synthesizer %q is remote; the synthesizer reads every member's answer, so it must be a local member", m.Name)
				}
				cfg.Synthesizer, found = m.Name, true
			}
		}
		if !found {
			return Config{}, fmt.Errorf("synthesizer %q is not a member", name)
		}
	default:
		for _, m := range cfg.Members {
			if m.Local && m.Enabled {
				cfg.Synthesizer = m.Name
				break
			}
		}
	}
	return cfg, nil
}
