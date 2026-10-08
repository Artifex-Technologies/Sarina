package agent

import "github.com/Artifex-Technologies/Sarina/go/session"

// TierTracker is the agent's side of compute tiers (the tiers package's Router
// satisfies it). BeginTurn runs at the start of every Run/Resume (de-escalating,
// and giving the tracker a way to show events to the operator); Record runs
// when the turn ends and returns what to store in the session file under
// plugin_state["compute_tiers"], or nil for nothing. The tracker only decides
// where the model runs: tools and permissions are never touched.
type TierTracker interface {
	BeginTurn(emit func(map[string]any))
	Record(prev any) map[string]any
}

// tierStateKey is the plugin_state key for the tier record.
const tierStateKey = "compute_tiers"

func (a *Agent) beginTierTurn(r *run) {
	if a.opts.Tiers != nil {
		a.opts.Tiers.BeginTurn(r.emit)
	}
}

func (a *Agent) recordTiers(s *session.Session) {
	if a.opts.Tiers == nil {
		return
	}
	if rec := a.opts.Tiers.Record(s.PluginState[tierStateKey]); rec != nil {
		if s.PluginState == nil {
			s.PluginState = map[string]any{}
		}
		s.PluginState[tierStateKey] = rec
	}
}
