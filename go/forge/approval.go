package forge

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
)

func (s *Store) approvalsPath() string { return filepath.Join(s.StateDir, "approvals.json") }

func (s *Store) readApprovals() map[string]string {
	m := map[string]string{}
	if b, err := os.ReadFile(s.approvalsPath()); err == nil {
		_ = json.Unmarshal(b, &m)
	}
	return m
}

func (s *Store) writeApprovals(m map[string]string) error {
	if err := s.checkPaths(); err != nil {
		return err
	}
	b, _ := json.MarshalIndent(m, "", "  ")
	tmp, err := os.CreateTemp(s.StateDir, ".appr-")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.Write(b); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), s.approvalsPath())
}

// Approve records the owner's approval of one exact hash of an agent. It is only for the owner's own command: no tool offered to the model calls it.
// The hash must be what the agent is right now, so an edit made after the owner looked cannot ride on the approval.
func (s *Store) Approve(name, hash string) error {
	cur, err := s.Hash(name)
	if err != nil {
		return err
	}
	if cur != hash {
		s.audit("approve", name, hash, "refused", "the agent changed since it was shown")
		return errors.New("the agent has changed since you looked at it: look again")
	}
	m := s.readApprovals()
	m[name] = hash
	if err := s.writeApprovals(m); err != nil {
		return err
	}
	s.audit("approve", name, hash, "ok", "")
	return nil
}

// Approved says whether the agent as it is now is the one the owner approved.
func (s *Store) Approved(name string) bool {
	cur, err := s.Hash(name)
	if err != nil {
		return false
	}
	return s.readApprovals()[name] == cur
}

func (s *Store) dropApproval(name string) error {
	m := s.readApprovals()
	if _, ok := m[name]; !ok {
		return nil
	}
	delete(m, name)
	return s.writeApprovals(m)
}
