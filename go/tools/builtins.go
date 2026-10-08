package tools

// DefaultTools is the core tool set every agent gets: files, search, shell,
// background processes and the to-do list. Each state-changing tool declares
// the permission it needs; the registry enforces it before the handler runs.
func DefaultTools() []Tool {
	ts := []Tool{ReadFile(), ListDir(), WriteFile(), EditFile(), GlobSearch(), GrepSearch(), Bash(), TaskOutput(), TaskStop()}
	ts = append(ts, TodoTools()...)
	return append(ts, PlanTools()...)
}
