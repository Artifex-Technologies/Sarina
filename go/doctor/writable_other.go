//go:build !unix

package doctor

import "os"

// writableDir is a permission-bit check: without a portable access(2) it cannot
// see ACLs, but it never creates anything.
func writableDir(dir string) bool {
	st, err := os.Stat(dir)
	return err == nil && st.IsDir() && st.Mode().Perm()&0o200 != 0
}
