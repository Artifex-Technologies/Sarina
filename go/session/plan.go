package session

// Plan modes.
const (
	PlanNormal = "normal"
	PlanActive = "plan"
)

// planKey is where plan state lives inside the session file: the existing
// plugin_state object, so no top-level key is added and older readers (which
// keep plugin_state as an opaque map) are unaffected. A session in normal mode
// with nothing pending carries no entry at all, so its file is unchanged.
const planKey = "plan_mode"

// PlanState is the plan-mode state of a session.
type PlanState struct {
	Mode    string // PlanNormal or PlanActive
	Pending string // a submitted plan awaiting the operator's decision
	Note    string // the last approved plan, kept as a session note
}

// Active reports whether plan mode is on.
func (p PlanState) Active() bool { return p.Mode == PlanActive }

// Plan reads the plan state; a missing or malformed entry means normal mode.
func (s *Session) Plan() PlanState {
	m, ok := s.PluginState[planKey].(map[string]any)
	if !ok {
		return PlanState{Mode: PlanNormal}
	}
	str := func(k string) string { v, _ := m[k].(string); return v }
	p := PlanState{Mode: PlanNormal, Pending: str("pending"), Note: str("note")}
	if str("mode") == PlanActive {
		p.Mode = PlanActive
	}
	return p
}

// SetPlan stores the plan state.
func (s *Session) SetPlan(p PlanState) {
	if p.Mode != PlanActive && p.Pending == "" && p.Note == "" {
		delete(s.PluginState, planKey)
		return
	}
	if s.PluginState == nil {
		s.PluginState = map[string]any{}
	}
	mode := PlanNormal
	if p.Mode == PlanActive {
		mode = PlanActive
	}
	m := map[string]any{"mode": mode}
	if p.Pending != "" {
		m["pending"] = p.Pending
	}
	if p.Note != "" {
		m["note"] = p.Note
	}
	s.PluginState[planKey] = m
}
