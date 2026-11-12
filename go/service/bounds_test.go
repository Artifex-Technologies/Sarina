package service

import (
	"bufio"
	"context"
	"io"
	"net"
	"strings"
	"testing"
	"time"
)

// serveFor runs the real HTTP server (Serve) on a loopback port until the test ends.
func serveFor(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { _ = Serve(ctx, New(Config{}), ln); close(done) }()
	t.Cleanup(func() { cancel(); <-done })
	return ln.Addr().String()
}

// A body that never fully arrives is answered once bodyReadTimeout passes; before, the handler waited for ever. Also
// when the handler never reads the body (an unknown path): the server's drain after the answer is bounded too.
func TestBodyThatNeverArrivesIsBounded(t *testing.T) {
	old := bodyReadTimeout
	bodyReadTimeout = 300 * time.Millisecond
	t.Cleanup(func() { bodyReadTimeout = old })
	addr := serveFor(t)
	for _, tc := range []struct{ path, want string }{
		{"/v1/sessions", "400 Bad Request"},
		{"/nowhere", "404 Not Found"},
	} {
		c, err := net.Dial("tcp", addr)
		if err != nil {
			t.Fatal(err)
		}
		_ = c.SetDeadline(time.Now().Add(5 * time.Second))
		_, _ = io.WriteString(c, "POST "+tc.path+" HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 100\r\n\r\n{\"cwd\":")
		line, err := bufio.NewReader(c).ReadString('\n')
		if err != nil || !strings.Contains(line, tc.want) {
			t.Fatalf("%s: %q %v", tc.path, line, err)
		}
		// and the connection is let go: the read below ends with EOF, not the test's deadline
		rest, err := io.ReadAll(c)
		if err != nil {
			t.Fatalf("%s: the connection was kept (%v) after %q", tc.path, err, rest)
		}
		c.Close()
	}
}

// A kept-alive connection is closed after idleTimeout; before, it stayed open for ever.
func TestIdleConnectionIsClosed(t *testing.T) {
	old := idleTimeout
	idleTimeout = 300 * time.Millisecond
	t.Cleanup(func() { idleTimeout = old })
	addr := serveFor(t)
	c, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	_, _ = io.WriteString(c, "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
	all, err := io.ReadAll(c)
	if err != nil || !strings.Contains(string(all), "200 OK") {
		t.Fatalf("the idle connection stayed open: %v %q", err, all)
	}
}
