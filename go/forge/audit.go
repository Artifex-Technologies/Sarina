package forge

import (
	"encoding/json"
	"os"
	"path/filepath"
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
	f, err := os.OpenFile(filepath.Join(s.StateDir, "forge-audit.jsonl"), os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o600)
	if err != nil {
		return
	}
	defer f.Close()
	f.Write(append(line, '\n'))
}
