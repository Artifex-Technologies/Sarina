package tools

// Permission is what a tool needs from the operator before it may run.
type Permission int

const (
	// PermNone: a read-only tool.
	PermNone Permission = iota
	// PermWrite: the tool changes files or persistent state (--allow-write).
	PermWrite
	// PermShell: the tool runs or controls processes (--allow-shell).
	PermShell
)

func (p Permission) String() string {
	switch p {
	case PermWrite:
		return "write"
	case PermShell:
		return "shell"
	}
	return "none"
}

// checkPermission is the single gate in front of every handler.
func checkPermission(t Tool, tc *Context) error {
	switch t.Requires {
	case PermWrite:
		if !tc.Permissions.AllowFileWrite {
			return Denied("%s changes files or saved state, and file writing is disabled. The operator must re-run with --allow-write to enable it.", t.Spec.Name)
		}
	case PermShell:
		if !tc.Permissions.AllowShellCommands {
			return Denied("%s runs or controls processes, and shell commands are disabled. The operator must re-run with --allow-shell to enable them.", t.Spec.Name)
		}
	}
	return nil
}
