// Package helpers runs the optional Python helper (python/sarina_helpers): the
// original embedding and voice code that needs onnxruntime, faster-whisper and
// Piper, none of which Go can use. See python/sarina_helpers/PROTOCOL.md.
//
// The Go build never imports Python code: it locates an interpreter, starts
// `python -m sarina_helpers ...`, and talks to it over pipes.
package helpers

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

// Environment variables.
const (
	PythonEnv = "SARINA_HELPERS_PYTHON" // interpreter to use (default: python3, then python, on PATH)
	PathEnv   = "SARINA_HELPERS_PATH"   // directory holding the sarina_helpers package (added to PYTHONPATH)
)

// ProtocolVersion is the JSON-lines protocol version this build speaks.
const ProtocolVersion = 1

// MaxAnswerBytes bounds one answer line read from the helper (64 MiB). Its answers are small (a label, groups of capture
// names, a few vectors); the bound is there so a broken or hostile helper cannot make Sarina read without end. A longer
// line stops the helper.
const MaxAnswerBytes = 64 << 20

// stderrKeep is how much of the helper's error output is kept (the end of it; only its last line is ever shown).
const stderrKeep = 64 << 10

// InstallHint tells the operator how to get the helper.
const InstallHint = "pip install python/sarina_helpers[ml,voice] (or set " + PythonEnv + " / " + PathEnv + ")"

// Python finds the interpreter.
func Python() (string, error) {
	if p := os.Getenv(PythonEnv); p != "" {
		return p, nil
	}
	for _, name := range []string{"python3", "python"} {
		if p, err := exec.LookPath(name); err == nil {
			return p, nil
		}
	}
	return "", fmt.Errorf("no Python interpreter found (set %s)", PythonEnv)
}

// Command builds `python -P -m sarina_helpers args...` with PYTHONPATH adjusted.
func Command(args ...string) (*exec.Cmd, error) {
	py, err := Python()
	if err != nil {
		return nil, err
	}
	// -P: the current folder is not put on the import path, so a sarina_helpers/ folder planted where this runs is not used
	cmd := exec.Command(py, append([]string{"-P", "-m", "sarina_helpers"}, args...)...)
	cmd.Env = os.Environ()
	if dir := os.Getenv(PathEnv); dir != "" {
		pp := dir
		if old := os.Getenv("PYTHONPATH"); old != "" {
			pp = dir + string(os.PathListSeparator) + old
		}
		cmd.Env = append(cmd.Env, "PYTHONPATH="+pp)
	}
	return cmd, nil
}

// Available reports whether the helper package can be run, with the reason if not.
// The helper prints its own usage line (and exits 2) when run without arguments,
// which is how "importable" is told apart from "interpreter ran but no module".
func Available() error {
	cmd, err := Command()
	if err != nil {
		return err
	}
	out := &tail{max: stderrKeep}
	cmd.Stdout, cmd.Stderr = out, out
	_ = cmd.Run()
	if strings.Contains(out.String(), "usage: python -m sarina_helpers") {
		return nil
	}
	return fmt.Errorf("the Python helper is not importable: %s", lastLine(out.String()))
}

func lastLine(s string) string {
	s = strings.TrimSpace(s)
	if s == "" {
		return "no output"
	}
	lines := strings.Split(s, "\n")
	return strings.TrimSpace(lines[len(lines)-1])
}

// Passthrough runs `python -m sarina_helpers args...` wired to the given streams
// and returns its exit code. If the helper cannot run, it prints why and returns 1.
func Passthrough(stdin io.Reader, stdout, stderr io.Writer, args ...string) int {
	if err := Available(); err != nil {
		fmt.Fprintf(stderr, "sarina: %v\n  install: %s\n", err, InstallHint)
		return 1
	}
	cmd, err := Command(args...)
	if err != nil {
		fmt.Fprintf(stderr, "sarina: %v\n", err)
		return 1
	}
	cmd.Stdin, cmd.Stdout, cmd.Stderr = stdin, stdout, stderr
	if err := cmd.Run(); err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode()
		}
		fmt.Fprintf(stderr, "sarina: %v\n", err)
		return 1
	}
	return 0
}

// Server is a running `sarina_helpers serve --model-dir D` process.
type Server struct {
	cmd     *exec.Cmd
	in      io.WriteCloser
	out     *bufio.Reader
	stderr  *tail
	maxLine int           // answer line limit; 0 = MaxAnswerBytes (tests set it lower)
	done    chan struct{} // closed when the helper has exited and its output has been copied
	waitErr error
	broken  error // once the helper is gone (or stopped), every later call answers this
	mu      sync.Mutex
	// Prototypes is the helper's category-prototype fingerprint (from ping).
	Prototypes string
}

type response struct {
	OK         bool        `json:"ok"`
	Error      string      `json:"error"`
	Protocol   int         `json:"protocol"`
	Prototypes string      `json:"prototypes"`
	Label      *string     `json:"label"`
	Score      float64     `json:"score"`
	Groups     [][]string  `json:"groups"`
	Vectors    [][]float64 `json:"vectors"`
}

// StartServer launches the embedding server and pings it. The error says why
// the helper is unavailable (not installed, extra missing, bad model directory).
func StartServer(modelDir string) (*Server, error) {
	return startServer(modelDir, 0)
}

func startServer(modelDir string, maxLine int) (*Server, error) {
	cmd, err := Command("serve", "--model-dir", modelDir)
	if err != nil {
		return nil, err
	}
	s := &Server{cmd: cmd, stderr: &tail{max: stderrKeep}, maxLine: maxLine}
	cmd.Stderr = s.stderr
	if s.in, err = cmd.StdinPipe(); err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	s.out = bufio.NewReaderSize(stdout, 1<<20)
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	s.done = make(chan struct{})
	go func() { s.waitErr = cmd.Wait(); close(s.done) }()
	r, err := s.call(map[string]any{"op": "ping"})
	if err != nil {
		s.Close()
		return nil, err
	}
	if r.Protocol != ProtocolVersion {
		s.Close()
		return nil, fmt.Errorf("helper speaks protocol %d, this build expects %d", r.Protocol, ProtocolVersion)
	}
	s.Prototypes = r.Prototypes
	return s, nil
}

func (s *Server) call(req map[string]any) (*response, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.broken != nil {
		return nil, s.broken
	}
	b, _ := json.Marshal(req)
	if _, err := s.in.Write(append(b, '\n')); err != nil {
		s.broken = s.dead(err)
		return nil, s.broken
	}
	max := s.maxLine
	if max <= 0 {
		max = MaxAnswerBytes
	}
	line, err := readLine(s.out, max)
	if err == errTooLong {
		// the stream is mid-line now: the helper cannot be talked to any more
		s.cmd.Process.Kill()
		s.broken = fmt.Errorf("helper answer is longer than %d bytes; the helper was stopped", max)
		return nil, s.broken
	}
	if err != nil && len(bytes.TrimSpace(line)) == 0 {
		s.broken = s.dead(err)
		return nil, s.broken
	}
	var r response
	if err := json.Unmarshal(line, &r); err != nil {
		return nil, fmt.Errorf("bad helper response: %v", err)
	}
	if !r.OK {
		return nil, fmt.Errorf("%s", r.Error)
	}
	return &r, nil
}

func (s *Server) dead(err error) error {
	// the reason is the helper's last line of error output: wait (briefly) until it has exited and that output is copied,
	// or the line may not be there yet
	select {
	case <-s.done:
	case <-time.After(2 * time.Second):
	}
	if msg := lastLine(s.stderr.String()); msg != "no output" {
		return fmt.Errorf("helper exited: %s", msg)
	}
	return fmt.Errorf("helper exited: %v", err)
}

// Classify returns the nearest prototype label (";"-joined categories, "" if none) and its score.
func (s *Server) Classify(text string) (string, float64, error) {
	r, err := s.call(map[string]any{"op": "classify", "text": text})
	if err != nil {
		return "", 0, err
	}
	if r.Label == nil {
		return "", r.Score, nil
	}
	return *r.Label, r.Score, nil
}

// Group asks for semantic groups of captures (records as capture.list returns them).
func (s *Server) Group(captures []map[string]any, threshold float64) ([][]string, error) {
	r, err := s.call(map[string]any{"op": "group", "captures": captures, "threshold": threshold})
	if err != nil {
		return nil, err
	}
	return r.Groups, nil
}

// Close ends the helper (closing stdin makes it exit).
func (s *Server) Close() error {
	s.in.Close()
	<-s.done
	return s.waitErr
}

var errTooLong = errors.New("line too long")

// readLine reads up to and including the next newline, refusing a line longer than max bytes.
func readLine(r *bufio.Reader, max int) ([]byte, error) {
	var line []byte
	for {
		part, err := r.ReadSlice('\n')
		if len(line)+len(part) > max {
			return nil, errTooLong
		}
		line = append(line, part...)
		if err != bufio.ErrBufferFull {
			return line, err
		}
	}
}

// tail keeps the last max bytes written to it (the helper's error output); safe for the copying goroutine and readers.
type tail struct {
	mu  sync.Mutex
	b   []byte
	max int
}

func (t *tail) Write(p []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.b = append(t.b, p...)
	if len(t.b) > t.max {
		t.b = append([]byte(nil), t.b[len(t.b)-t.max:]...)
	}
	return len(p), nil
}

func (t *tail) String() string {
	t.mu.Lock()
	defer t.mu.Unlock()
	return string(t.b)
}
