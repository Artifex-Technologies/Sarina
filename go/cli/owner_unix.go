//go:build unix

package cli

import (
	"os"
	"syscall"
)

// ownedByOther reports whether the file belongs to an account other than this one (and which).
func ownedByOther(path string) (int, bool) {
	fi, err := os.Stat(path)
	if err != nil {
		return 0, false
	}
	st, ok := fi.Sys().(*syscall.Stat_t)
	if !ok {
		return 0, false
	}
	return int(st.Uid), int(st.Uid) != os.Geteuid()
}
