package session

import (
	"fmt"
	"io"
	"os"
	"syscall"
)

// MaxFileBytes bounds a session file read: far above any real session (tool output is cut at 12,000 characters a call
// and history is compacted), so a larger file is a planted or damaged one, and a sparse file of gigabytes costs nothing
// on disk.
const MaxFileBytes = 64 << 20

// ReadFile reads a session file (Load, MigrateDir, session search): see ReadRegular.
func ReadFile(path string) ([]byte, error) { return ReadRegular(path, MaxFileBytes) }

// ReadRegular reads a regular file of at most max bytes. It is opened without blocking, so a named pipe or a device
// planted under a session file's name is refused at once instead of waiting forever for a writer; a link is followed.
func ReadRegular(path string, max int64) ([]byte, error) {
	f, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NONBLOCK, 0)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return nil, err
	}
	if !st.Mode().IsRegular() {
		return nil, fmt.Errorf("%s is not a regular file", path)
	}
	b, err := io.ReadAll(io.LimitReader(f, max+1))
	if err != nil {
		return nil, err
	}
	if int64(len(b)) > max {
		return nil, fmt.Errorf("%s is larger than %d bytes; not read", path, max)
	}
	return b, nil
}
