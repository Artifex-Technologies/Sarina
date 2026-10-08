package main

import (
	"encoding/json"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/agent"
	"github.com/Artifex-Technologies/Sarina/go/agents"
	"github.com/Artifex-Technologies/Sarina/go/config"
)

// Size budgets for the serialized tool definitions the model receives on every
// request. They are what the tool descriptions cost a small context window; a
// test fails 10% above them so the definitions cannot quietly grow back.
const (
	budgetAllPermissions = 11100
	budgetReadOnly       = 4300
	budgetSlack          = 1.10
)

func toolDefinitionSize(t *testing.T, perms config.Permissions) (int, int) {
	t.Helper()
	dir := t.TempDir()
	rt := config.DefaultRuntime(dir)
	rt.Permissions = perms
	a, err := agent.New(agent.Options{
		Model: config.ModelConfig{Model: "test", BaseURL: "http://127.0.0.1:1/v1"}, Runtime: rt, Home: dir,
		Subagents: agents.Load(agents.Options{Home: dir}),
	})
	if err != nil {
		t.Fatal(err)
	}
	b, err := json.Marshal(a.Registry().Specs())
	if err != nil {
		t.Fatal(err)
	}
	return len(b), a.ToolCount()
}

func TestToolDefinitionFootprint(t *testing.T) {
	for _, c := range []struct {
		name   string
		perms  config.Permissions
		budget int
	}{
		{"all permissions", config.Permissions{AllowFileWrite: true, AllowShellCommands: true}, budgetAllPermissions},
		{"read only", config.Permissions{}, budgetReadOnly},
	} {
		size, n := toolDefinitionSize(t, c.perms)
		t.Logf("%s: %d tools, %d JSON chars (target %d)", c.name, n, size, c.budget)
		if limit := int(float64(c.budget) * budgetSlack); size > limit {
			t.Errorf("%s: tool definitions are %d chars, over the limit of %d (target %d); shorten descriptions instead of raising the budget",
				c.name, size, limit, c.budget)
		}
	}
}
