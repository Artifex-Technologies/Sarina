package forge

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
)

// audit appends one JSON line (time, action, agent, hash, outcome, detail). Code is never written here, only its hash and a short note.
func (s *Store) audit(action, name, hash, outcome, detail string) {
	if s.StateDir == "" {
		return
	}
	_ = os.MkdirAll(s.StateDir, 0o700)
	line, _ := json.Marshal(map[string]string{
		"time": s.now().UTC().Format("2006-01-02T15:04:05Z"), "action": action, "agent": cleanText(name, 40), "hash": hash,
		"outcome": outcome, "detail": cleanText(detail, 120),
	})
	f, err := openAuditLog(s.StateDir, os.O_APPEND|os.O_CREATE|os.O_WRONLY)
	if err != nil {
		return
	}
	defer f.Close()
	f.Write(append(line, '\n'))
}

// openAuditLog opens the audit log only as a regular file of its own: a link at its name is not followed (the log is
// not written, or counted, wherever it leads) and a named pipe is not waited on.
func openAuditLog(dir string, flag int) (*os.File, error) {
	p := filepath.Join(dir, "forge-audit.jsonl")
	f, err := os.OpenFile(p, flag|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0o600)
	if err != nil {
		return nil, err
	}
	if st, err := f.Stat(); err != nil || !st.Mode().IsRegular() {
		f.Close()
		return nil, fmt.Errorf("%s is not a regular file", p)
	}
	return f, nil
}
