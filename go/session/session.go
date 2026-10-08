package session

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// KV is one ordered context entry.
type KV struct{ Key, Value string }

// OrderedMap is a string map that keeps insertion order across a JSON round trip.
type OrderedMap []KV

// Set replaces or appends a key.
func (o *OrderedMap) Set(k, v string) {
	for i := range *o {
		if (*o)[i].Key == k {
			(*o)[i].Value = v
			return
		}
	}
	*o = append(*o, KV{k, v})
}

// Get returns a value.
func (o OrderedMap) Get(k string) (string, bool) {
	for _, kv := range o {
		if kv.Key == k {
			return kv.Value, true
		}
	}
	return "", false
}

// MarshalJSON writes an object in insertion order.
func (o OrderedMap) MarshalJSON() ([]byte, error) {
	var b bytes.Buffer
	b.WriteByte('{')
	for i, kv := range o {
		if i > 0 {
			b.WriteByte(',')
		}
		k, _ := marshalRaw(kv.Key)
		v, _ := marshalRaw(kv.Value)
		b.Write(k)
		b.WriteByte(':')
		b.Write(v)
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

// UnmarshalJSON reads an object preserving order; non-objects load empty.
func (o *OrderedMap) UnmarshalJSON(data []byte) error {
	*o = nil
	dec := json.NewDecoder(bytes.NewReader(data))
	tok, err := dec.Token()
	if err != nil || tok != json.Delim('{') {
		return nil
	}
	for dec.More() {
		kt, err := dec.Token()
		if err != nil {
			return nil
		}
		key, _ := kt.(string)
		var v any
		if err := dec.Decode(&v); err != nil {
			return nil
		}
		s, ok := v.(string)
		if !ok {
			s = fmt.Sprint(v)
		}
		*o = append(*o, KV{key, s})
	}
	return nil
}

// BudgetState is the persisted counter set.
type BudgetState struct {
	ModelCalls     int `json:"model_calls"`
	SessionTurns   int `json:"session_turns"`
	ToolCalls      int `json:"tool_calls"`
	DelegatedTasks int `json:"delegated_tasks"`
	Escalations    int `json:"escalations"`
}

// Session is one conversation and everything saved with it.
type Session struct {
	ID                  string
	Model               config.ModelConfig
	Runtime             config.RuntimeConfig
	SystemPromptParts   []string
	UserContext         OrderedMap
	SystemContext       OrderedMap
	Messages            []Message
	Turns               int
	ToolCalls           int
	Usage               llm.Usage
	TotalCostUSD        float64
	FileHistory         []map[string]any
	BudgetState         BudgetState
	PluginState         map[string]any
	ScratchpadDirectory string // "" serialises as null
}

type fileJSON struct {
	SessionID           string               `json:"session_id"`
	ModelConfig         config.ModelConfig   `json:"model_config"`
	RuntimeConfig       config.RuntimeConfig `json:"runtime_config"`
	SystemPromptParts   []string             `json:"system_prompt_parts"`
	UserContext         OrderedMap           `json:"user_context"`
	SystemContext       OrderedMap           `json:"system_context"`
	Messages            []Message            `json:"messages"`
	Turns               int                  `json:"turns"`
	ToolCalls           int                  `json:"tool_calls"`
	Usage               llm.Usage            `json:"usage"`
	TotalCostUSD        float64              `json:"total_cost_usd"`
	FileHistory         []map[string]any     `json:"file_history"`
	BudgetState         BudgetState          `json:"budget_state"`
	PluginState         map[string]any       `json:"plugin_state"`
	ScratchpadDirectory *string              `json:"scratchpad_directory"`
}

// NewID returns a 32-character lowercase hex UUID4 without dashes.
func NewID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return hex.EncodeToString(b[:])
}

// NextMessageID returns "<prefix>_<index of the next message>".
func (s *Session) NextMessageID(prefix string) string {
	return fmt.Sprintf("%s_%d", prefix, len(s.Messages))
}

// Append adds a message and returns its index.
func (s *Session) Append(m Message) int {
	if m.State == "" {
		m.State = StateFinal
	}
	s.Messages = append(s.Messages, m)
	return len(s.Messages) - 1
}

// Marshal renders the session file. The API key is never included.
func (s *Session) Marshal() ([]byte, error) {
	f := fileJSON{
		SessionID: s.ID, ModelConfig: s.Model, RuntimeConfig: s.Runtime.Normalized(),
		SystemPromptParts: s.SystemPromptParts, UserContext: s.UserContext, SystemContext: s.SystemContext,
		Messages: s.Messages, Turns: s.Turns, ToolCalls: s.ToolCalls, Usage: s.Usage,
		TotalCostUSD: s.TotalCostUSD, FileHistory: s.FileHistory, BudgetState: s.BudgetState,
		PluginState: s.PluginState,
	}
	if f.SystemPromptParts == nil {
		f.SystemPromptParts = []string{}
	}
	if f.Messages == nil {
		f.Messages = []Message{}
	}
	if f.FileHistory == nil {
		f.FileHistory = []map[string]any{}
	}
	if f.PluginState == nil {
		f.PluginState = map[string]any{}
	}
	if s.ScratchpadDirectory != "" {
		sd := s.ScratchpadDirectory
		f.ScratchpadDirectory = &sd
	}
	return EncodeJSON(f)
}

// Path returns the file path for this session under dir.
func Path(dir, id string) string { return filepath.Join(dir, id+".json") }

// Save writes the session atomically (temp file + rename, mode 0600) to
// <dir>/<id>.json and returns the path.
func (s *Session) Save(dir string) (string, error) {
	data, err := s.Marshal()
	if err != nil {
		return "", err
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	dest := Path(dir, s.ID)
	if err := writeAtomic(dest, s.ID, data); err != nil {
		return "", err
	}
	return dest, nil
}

// writeAtomic writes data to dest through a temp file in the same directory.
func writeAtomic(dest, id string, data []byte) error {
	tmp, err := os.CreateTemp(filepath.Dir(dest), id+".*.tmp")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	cleanup := func() { _ = os.Remove(tmpName) }
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		cleanup()
		return err
	}
	if err := tmp.Close(); err != nil {
		cleanup()
		return err
	}
	_ = os.Chmod(tmpName, 0o600)
	if err := os.Rename(tmpName, dest); err != nil {
		cleanup()
		return err
	}
	return nil
}

// LoadByID loads <dir>/<id>.json.
func LoadByID(dir, id string) (*Session, error) { return Load(Path(dir, id)) }

// Load reads a session file, tolerating the optional/legacy fields described
// in the format spec. A present api_key is ignored.
func Load(path string) (*Session, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return Parse(b)
}

// Parse decodes a session file's bytes. Retired tool names in the history are
// rewritten to their current equivalents in memory (see MigrateRetiredTools).
func Parse(b []byte) (*Session, error) {
	s, err := parse(b)
	if err != nil {
		return nil, err
	}
	s.MigrateRetiredTools()
	return s, nil
}

// parse decodes a session file's bytes without migrating anything.
func parse(b []byte) (*Session, error) {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(b, &raw); err != nil {
		return nil, fmt.Errorf("session file is not a JSON object: %w", err)
	}
	for _, key := range []string{"session_id", "model_config", "runtime_config", "system_prompt_parts",
		"user_context", "system_context", "messages", "turns", "tool_calls"} {
		if _, ok := raw[key]; !ok {
			return nil, fmt.Errorf("session file is missing required key %q", key)
		}
	}
	generic := func(key string) any {
		var v any
		if r, ok := raw[key]; ok {
			_ = json.Unmarshal(r, &v)
		}
		return v
	}
	s := &Session{}
	var ok bool
	if id, isStr := generic("session_id").(string); isStr && id != "" {
		s.ID = id
	} else {
		return nil, errors.New("session_id must be a non-empty string")
	}
	if s.Model, ok = config.ModelFromMap(config.AsMap(generic("model_config"))); !ok {
		return nil, errors.New("model_config.model is required")
	}
	if s.Runtime, ok = config.RuntimeFromMap(config.AsMap(generic("runtime_config"))); !ok {
		return nil, errors.New("runtime_config.cwd is required")
	}
	if parts, isList := generic("system_prompt_parts").([]any); isList {
		for _, p := range parts {
			if str, isStr := p.(string); isStr {
				s.SystemPromptParts = append(s.SystemPromptParts, str)
			}
		}
	} else {
		return nil, errors.New("system_prompt_parts must be a list")
	}
	_ = s.UserContext.UnmarshalJSON(raw["user_context"])
	_ = s.SystemContext.UnmarshalJSON(raw["system_context"])
	msgs, isList := generic("messages").([]any)
	if !isList {
		return nil, errors.New("messages must be a list")
	}
	for _, m := range msgs {
		if mm, isMap := m.(map[string]any); isMap {
			s.Messages = append(s.Messages, MessageFromMap(mm))
		}
	}
	s.Turns, _ = config.AsInt(generic("turns"))
	s.ToolCalls, _ = config.AsInt(generic("tool_calls"))
	s.Usage = llm.UsageFromProvider(generic("usage"))
	s.TotalCostUSD, _ = config.AsFloat(generic("total_cost_usd"))
	if fh, isList := generic("file_history").([]any); isList {
		for _, e := range fh {
			if em, isMap := e.(map[string]any); isMap {
				s.FileHistory = append(s.FileHistory, em)
			}
		}
	}
	bs := config.AsMap(generic("budget_state"))
	geti := func(k string) int { n, _ := config.AsInt(bs[k]); return n }
	s.BudgetState = BudgetState{geti("model_calls"), geti("session_turns"), geti("tool_calls"),
		geti("delegated_tasks"), geti("escalations")}
	if ps, isMap := generic("plugin_state").(map[string]any); isMap {
		s.PluginState = ps
	}
	if sd, isStr := generic("scratchpad_directory").(string); isStr {
		s.ScratchpadDirectory = sd
	}
	return s, nil
}
