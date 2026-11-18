package console

import (
	"errors"
	"net"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// Found by the Rust port (2026-11-18): a named pipe planted at Argus App's token file held every chymaera command for
// ever, and a reply line was read whole however long it was (a console sending gigabytes in its five seconds took that
// much memory).

func TestDefaultTokenSkipsAPlantedPipe(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(filepath.Join(dir, "argus"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(filepath.Join(dir, "argus", "token"), 0o600); err != nil {
		t.Skip("no named pipes here:", err)
	}
	t.Setenv("XDG_RUNTIME_DIR", dir)
	done := make(chan string)
	go func() { done <- DefaultToken() }()
	select {
	case tok := <-done:
		if tok != "" {
			t.Fatalf("a pipe gave the token %q", tok)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a named pipe at the token file held it")
	}
}

func TestAnOverlongReplyIsRefused(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		buf := make([]byte, 4096)
		conn.Read(buf)
		conn.Write([]byte(`{"ok":true,"pad":"` + strings.Repeat("x", MaxReplyBytes) + `"}` + "\n"))
	}()
	c := New()
	c.Port, c.Timeout = ln.Addr().(*net.TCPAddr).Port, 5*time.Second
	_, err = c.Status()
	var ue *UnavailableError
	if !errors.As(err, &ue) || !strings.Contains(err.Error(), "longer than") {
		t.Fatalf("err = %v", err)
	}
}
