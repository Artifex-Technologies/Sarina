package doctor

import (
	"context"
	"fmt"
	"os"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/tiers"
)

var idUnsafe = regexp.MustCompile(`[^a-z0-9_-]+`)

func (r *runner) tiers(ctx context.Context) {
	path := tiers.Path(r.home, r.env)
	cfg, err := tiers.Load(r.home, r.env)
	if err != nil {
		r.add("tiers_config", Warn, "tiers config is invalid, so Sarina stays on the local backend only: "+err.Error(),
			"fix the named file (tiers.json): JSON syntax, tier kinds, api_key_env must be a variable NAME")
		return
	}
	if _, statErr := os.Stat(path); statErr != nil {
		r.add("tiers_config", Pass, "tiers.json absent: a single local tier (the OPENAI_* backend)", "")
		return
	}
	names := make([]string, 0, len(cfg.Tiers))
	for _, t := range cfg.Tiers {
		names = append(names, t.Name)
	}
	finding := fmt.Sprintf("tiers.json present and valid: %d tier(s): %s", len(cfg.Tiers), strings.Join(names, ", "))
	if len(cfg.Warnings) > 0 {
		r.add("tiers_config", Warn, finding+"; "+strings.Join(cfg.Warnings, "; "), "review the warnings in tiers.json")
	} else {
		r.add("tiers_config", Pass, finding, "")
	}
	router, err := tiers.New(tiers.Options{Config: cfg, Base: config.ModelFromEnv(r.env), Getenv: r.env,
		HTTPClient: r.hc, HealthTimeout: ShortTimeout})
	if err != nil {
		return
	}
	for _, s := range router.Status(ctx) {
		id := "tier_" + idUnsafe.ReplaceAllString(strings.ToLower(s.Name), "_")
		where := s.Kind
		if s.BaseURL != "" {
			where += " at " + hostOf(s.BaseURL)
		}
		if s.APIKeyEnv != "" {
			where += fmt.Sprintf(", key variable %s %s", s.APIKeyEnv, map[bool]string{true: "set", false: "unset"}[s.KeySet])
		}
		switch {
		case !s.Enabled:
			r.add(id, Skip, where+": disabled in tiers.json", "")
		case s.Healthy && s.Kind == tiers.KindClaudeCLI:
			r.add(id, Pass, fmt.Sprintf("%s: claude executable %q found", where, s.CLIPath), "")
		case s.Healthy:
			r.add(id, Pass, where+": healthy", "")
		case s.Kind == tiers.KindClaudeCLI:
			r.add(id, Warn, fmt.Sprintf("%s: claude executable %q was not found on PATH", where, s.CLIPath),
				"install the claude CLI or set cli_path in tiers.json; until then this tier is skipped")
		default:
			r.add(id, Warn, where+": "+s.Why, "start that machine or server, or disable the tier; Sarina falls through to the next tier meanwhile")
		}
	}
}
