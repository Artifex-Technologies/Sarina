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
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"sync"
)

// Environment variables.
const (
	PythonEnv = "SARINA_HELPERS_PYTHON" // interpreter to use (default: python3, then python, on PATH)
	PathEnv   = "SARINA_HELPERS_PATH"   // directory holding the sarina_helpers package (added to PYTHONPATH)
)

// ProtocolVersion is the JSON-lines protocol version this build speaks.
const ProtocolVersion = 1

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

// Command builds `python -m sarina_helpers args...` with PYTHONPATH adjusted.
func Command(args ...string) (*exec.Cmd, error) {
	py, err := Python()
	if err != nil {
		return nil, err
	}
	cmd := exec.Command(py, append([]string{"-m", "sarina_helpers"}, args...)...)
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
	var out bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &out
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
	cmd    *exec.Cmd
	in     io.WriteCloser
	out    *bufio.Reader
	stderr *bytes.Buffer
	mu     sync.Mutex
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
	cmd, err := Command("serve", "--model-dir", modelDir)
	if err != nil {
		return nil, err
	}
	s := &Server{cmd: cmd, stderr: &bytes.Buffer{}}
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
	b, _ := json.Marshal(req)
	if _, err := s.in.Write(append(b, '\n')); err != nil {
		return nil, s.dead(err)
	}
	line, err := s.out.ReadBytes('\n')
	if err != nil && len(bytes.TrimSpace(line)) == 0 {
		return nil, s.dead(err)
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
	return s.cmd.Wait()
}
