//go:build unix

package doctor

import "syscall"

// writableDir asks the kernel whether this process may write the directory,
// without creating anything.
func writableDir(dir string) bool { return syscall.Access(dir, 2) == nil }
