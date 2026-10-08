package tools

import (
	"bytes"
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"time"
)

// secretEnvMarkers are substrings that mark an environment variable as a
// secret. Matching is case-insensitive and by substring, so it also drops
// harmless names such as GIT_AUTHOR_NAME and SSH_AUTH_SOCK; that is intended.
var secretEnvMarkers = []string{"SECRET", "TOKEN", "PASSWORD", "PRIVATE_KEY", "API_KEY", "CREDENTIAL", "AUTH"}

func isSecretEnvName(name string) bool {
	up := strings.ToUpper(name)
	for _, m := range secretEnvMarkers {
		if strings.Contains(up, m) {
			return true
		}
	}
	return false
}

// childEnv builds the environment a model-run process starts with, in three
// layers, later ones overriding: (1) the parent environment without any
// secret-looking variable, (2) the operator's session variables, (3)
// tc.ExtraEnv. Layers 2 and 3 are deliberate operator choices and are not
// filtered. No value is ever logged or returned in a result.
func childEnv(tc *Context) []string {
	environ := os.Environ
	if tc != nil && tc.Environ != nil {
		environ = tc.Environ
	}
	key := func(k string) string {
		if runtime.GOOS == "windows" {
			return strings.ToUpper(k)
		}
		return k
	}
	vals := map[string]string{}
	names := map[string]string{}
	set := func(k, v string) {
		vals[key(k)] = v
		names[key(k)] = k
	}
	for _, kv := range environ() {
		i := strings.IndexByte(kv, '=')
		if i <= 0 {
			// Windows keeps entries like "=C:=C:\dir"; a leading '=' is part of the name.
			if i == 0 {
				if j := strings.IndexByte(kv[1:], '='); j > 0 {
					i = j + 1
				} else {
					continue
				}
			} else {
				continue
			}
		}
		if isSecretEnvName(kv[:i]) {
			continue
		}
		set(kv[:i], kv[i+1:])
	}
	if tc != nil {
		for k, v := range tc.SessionEnv {
			set(k, v)
		}
		for k, v := range tc.ExtraEnv {
			set(k, v)
		}
	}
	out := make([]string, 0, len(vals))
	for k, v := range vals {
		out = append(out, names[k]+"="+v)
	}
	sort.Strings(out)
	return out
}

// shellPath finds bash. On Windows it prefers Git for Windows' bash over the
// WSL launcher that sits in System32.
func shellPath() (string, error) {
	if runtime.GOOS == "windows" {
		var roots []string
		for _, v := range []string{"ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"} {
			if p := os.Getenv(v); p != "" {
				roots = append(roots, filepath.Join(p, "Git", "bin", "bash.exe"))
			}
		}
		if p := os.Getenv("LocalAppData"); p != "" {
			roots = append(roots, filepath.Join(p, "Programs", "Git", "bin", "bash.exe"))
		}
		for _, r := range roots {
			if st, err := os.Stat(r); err == nil && !st.IsDir() {
				return r, nil
			}
		}
	}
	if p, err := exec.LookPath("bash"); err == nil {
		if runtime.GOOS != "windows" || !strings.Contains(strings.ToLower(p), `\system32\`) {
			return p, nil
		}
	}
	if runtime.GOOS != "windows" {
		if p, err := exec.LookPath("sh"); err == nil {
			return p, nil
		}
	}
	return "", errors.New("bash is not available on this machine, so shell commands cannot run")
}

// capBuffer keeps the most recent output up to a limit.
type capBuffer struct {
	mu      sync.Mutex
	data    []byte
	dropped int
}

const bufferCap = 2 << 20

func (b *capBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.data = append(b.data, p...)
	if len(b.data) > bufferCap {
		cut := len(b.data) - bufferCap/2
		b.dropped += cut
		b.data = append([]byte(nil), b.data[cut:]...)
	}
	return len(p), nil
}

func (b *capBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	s := strings.ToValidUTF8(string(b.data), "�")
	if b.dropped > 0 {
		s = "[earlier output dropped]\n" + s
	}
	return s
}

// lineEmitter turns a byte stream into per-line emit calls.
type lineEmitter struct {
	stream string
	emit   Emit
	mu     *sync.Mutex
	part   []byte
}

func (l *lineEmitter) Write(p []byte) (int, error) {
	l.part = append(l.part, p...)
	for {
		i := bytes.IndexByte(l.part, '\n')
		if i < 0 {
			break
		}
		l.send(string(l.part[:i+1]))
		l.part = l.part[i+1:]
	}
	return len(p), nil
}

func (l *lineEmitter) flush() {
	if len(l.part) > 0 {
		l.send(string(l.part))
		l.part = nil
	}
}

func (l *lineEmitter) send(s string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.emit(l.stream, strings.ToValidUTF8(s, "�"))
}

type shellResult struct {
	stdout, stderr string
	exitCode       int
	timedOut       bool
	interrupted    bool
}

func newShellCmd(command string, tc *Context) (*exec.Cmd, error) {
	shell, err := shellPath()
	if err != nil {
		return nil, err
	}
	cmd := exec.Command(shell, "-c", command)
	cmd.Dir = tc.Root
	cmd.Env = childEnv(tc)
	setProcAttrs(cmd)
	cmd.WaitDelay = 2 * time.Second
	return cmd, nil
}

// runForeground runs command to completion, killing the whole process tree on
// timeout or cancellation. emit, if not nil, receives output lines as they
// arrive.
func runForeground(ctx context.Context, command string, tc *Context, timeout time.Duration, emit Emit) (shellResult, error) {
	if timeout <= 0 {
		timeout = 30 * time.Second
	}
	cmd, err := newShellCmd(command, tc)
	if err != nil {
		return shellResult{}, err
	}
	var out, errb capBuffer
	var outW, errW io.Writer = &out, &errb
	var emitters []*lineEmitter
	if emit != nil {
		mu := &sync.Mutex{}
		oe := &lineEmitter{stream: "stdout", emit: emit, mu: mu}
		ee := &lineEmitter{stream: "stderr", emit: emit, mu: mu}
		emitters = []*lineEmitter{oe, ee}
		outW, errW = io.MultiWriter(&out, oe), io.MultiWriter(&errb, ee)
	}
	cmd.Stdout, cmd.Stderr = outW, errW

	if err := cmd.Start(); err != nil {
		return shellResult{}, err
	}
	pid := cmd.Process.Pid
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()

	timer := time.NewTimer(timeout)
	defer timer.Stop()
	res := shellResult{}
	var waitErr error
	select {
	case waitErr = <-done:
	case <-timer.C:
		res.timedOut = true
		hardKill(pid)
		waitErr = <-done
	case <-ctx.Done():
		res.interrupted = true
		hardKill(pid)
		waitErr = <-done
	}
	_ = waitErr
	reapGroup(pid)
	for _, e := range emitters {
		e.flush()
	}
	res.stdout, res.stderr = out.String(), errb.String()
	res.exitCode = -1
	if cmd.ProcessState != nil {
		res.exitCode = cmd.ProcessState.ExitCode()
	}
	return res, nil
}
