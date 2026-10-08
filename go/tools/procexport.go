package tools

import "os/exec"

// SetProcAttrs puts a child in its own process group (Windows: a new group),
// so KillTree can stop everything it started. For other packages that spawn
// operator-configured helper processes (MCP servers).
func SetProcAttrs(cmd *exec.Cmd) { setProcAttrs(cmd) }

// KillTree force-kills a process and its descendants.
func KillTree(pid int) { hardKill(pid) }

// ChildEnv is the environment a model-visible child process starts with: the
// parent's without secret-looking names, then tc's operator layers. A nil tc
// means "just the filtered parent environment".
func ChildEnv(tc *Context) []string { return childEnv(tc) }
