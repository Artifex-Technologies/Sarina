package forge

import (
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"
)

// Found while porting the forge to Rust: answers that depended on Go's map order, and the state folder's files (the
// approvals and the audit log) read and written through whatever was planted at their names.

var newRE = regexp.MustCompile(`\.new-\d+`)

func done(t *testing.T, what string, fn func()) {
	t.Helper()
	ch := make(chan struct{})
	go func() { fn(); close(ch) }()
	select {
	case <-ch:
	case <-time.After(10 * time.Second):
		t.Fatalf("%s waited on a named pipe", what)
	}
}

func fifo(t *testing.T, p string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(p), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(p, 0o600); err != nil {
		t.Skip(err)
	}
}

// With two bad files, which one the refusal named depended on Go's map order: the same call answered differently.
func TestRefusalsNameTheSameFileEveryTime(t *testing.T) {
	m, _ := good()
	files := map[string]string{"b/../x.py": "1", "a bad name.py": "2", "c.py": "3", "d.py": "4\x00"}
	first := Validate(m, files)
	s := store(t)
	private := map[string]string{"a.py": "open('/home/x/.ssh/id_rsa')", "b.py": "open('/home/x/.gnupg/pubring.kbx')", "c.py": "x"}
	_, pfirst := s.Create(m, private)
	for i := 0; i < 60; i++ {
		if err := Validate(m, files); err == nil || first == nil || err.Error() != first.Error() {
			t.Fatalf("Validate: %v, then %v", first, err)
		}
		if _, err := s.Create(m, private); err == nil || pfirst == nil || err.Error() != pfirst.Error() {
			t.Fatalf("Create: %v, then %v", pfirst, err)
		}
	}
	// a file and a folder of the same name: which error came first depended on the order the files were written in
	clash := map[string]string{"a": "x", "a/b": "y", "c": "z"}
	_, cfirst := s.Create(m, clash)
	for i := 0; i < 60; i++ {
		if _, err := s.Create(m, clash); err == nil || cfirst == nil || newRE.ReplaceAllString(err.Error(), "") != newRE.ReplaceAllString(cfirst.Error(), "") {
			t.Fatalf("Create with a clash: %v, then %v", cfirst, err)
		}
	}
	if !strings.Contains(first.Error(), `"a bad name.py"`) || !strings.Contains(pfirst.Error(), ".ssh") {
		t.Fatalf("not the first file in name order: %v / %v", first, pfirst)
	}
}

func TestApprovalsFileThatIsNullOrAPipe(t *testing.T) {
	s := store(t)
	m, files := good()
	h, err := s.Create(m, files)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(s.StateDir, "approvals.json"), []byte("null"), 0o600); err != nil {
		t.Fatal(err)
	}
	func() {
		defer func() {
			if r := recover(); r != nil {
				t.Fatalf("Approve panicked on an approvals file of null: %v", r)
			}
		}()
		if err := s.Approve("tally", h); err != nil || !s.Approved("tally") {
			t.Fatalf("approve: %v", err)
		}
	}()
	os.Remove(filepath.Join(s.StateDir, "approvals.json"))
	fifo(t, filepath.Join(s.StateDir, "approvals.json"))
	done(t, "Approved", func() {
		if s.Approved("tally") {
			t.Error("a named pipe counted as an approval")
		}
	})
}

// The audit log: a named pipe at its name held every create, run and approval for ever; a link had the log written
// wherever it led, and the hourly run limit counted the file it named. Now the log must be a regular file of its own,
// and runs are refused while it is not.
func TestAuditLogThatIsAPipeOrALink(t *testing.T) {
	for _, kind := range []string{"pipe", "link"} {
		t.Run(kind, func(t *testing.T) {
			s := store(t)
			outside := filepath.Join(t.TempDir(), "elsewhere.log")
			if err := os.WriteFile(outside, nil, 0o600); err != nil {
				t.Fatal(err)
			}
			log := filepath.Join(s.StateDir, "forge-audit.jsonl")
			if kind == "pipe" {
				fifo(t, log)
			} else {
				if err := os.MkdirAll(s.StateDir, 0o700); err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink(outside, log); err != nil {
					t.Fatal(err)
				}
			}
			m, files := good()
			done(t, "Create", func() {
				if _, err := s.Create(m, files); err != nil {
					t.Error(err)
				}
			})
			r := &fakeRunner{text: "ran"}
			var runErr error
			done(t, "Run", func() { _, runErr = s.Run(r, "tally", "agent-run") })
			if runErr == nil || r.calls != 0 || !strings.Contains(runErr.Error(), "audit log") {
				t.Fatalf("run: %v, %d calls", runErr, r.calls)
			}
			if b, _ := os.ReadFile(outside); len(b) != 0 {
				t.Fatalf("the audit was written through a link: %q", b)
			}
		})
	}
}

// Load read every file of an agent whole before Validate found it too large: a sparse file of gigabytes planted in an
// agent's folder was read into memory. Now no file is read past the bundle's limit.
func TestLoadDoesNotReadHugeFiles(t *testing.T) {
	s := store(t)
	m, files := good()
	if _, err := s.Create(m, files); err != nil {
		t.Fatal(err)
	}
	f, err := os.Create(filepath.Join(s.Dir, "tally", "huge.py"))
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(1 << 30); err != nil {
		t.Skip(err)
	}
	f.Close()
	var before, after runtimeMem
	before.read()
	if _, _, err := s.Load("tally"); err == nil {
		t.Fatal("a huge file loaded")
	}
	after.read()
	if after.total-before.total > 64<<20 {
		t.Fatalf("Load allocated %d MiB", (after.total-before.total)>>20)
	}
}

type runtimeMem struct{ total uint64 }

func (m *runtimeMem) read() {
	var s runtime.MemStats
	runtime.ReadMemStats(&s)
	m.total = s.TotalAlloc
}
