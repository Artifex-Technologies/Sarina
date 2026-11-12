package gui

import (
	"bufio"
	"context"
	"io"
	"net"
	"strings"
	"testing"
	"time"
)

// serveFor runs the console's real http.Server (httpServer) on a loopback port until the test ends.
func serveFor(t *testing.T, r *rig) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	hs := httpServer(r.srv.Handler())
	go func() { _ = hs.Serve(ln) }()
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		_ = hs.Shutdown(ctx)
		hs.Close()
	})
	return ln.Addr().String()
}

func dialFor(t *testing.T, addr string) net.Conn {
	t.Helper()
	c, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	return c
}

// A body that never fully arrives is answered once bodyReadTimeout passes, and the connection let go; before, the
// handler (or the drain after an answer) waited for ever.
func TestBodyThatNeverArrivesIsBounded(t *testing.T) {
	old := bodyReadTimeout
	bodyReadTimeout = 300 * time.Millisecond
	t.Cleanup(func() { bodyReadTimeout = old })
	r := newRig(t, nil)
	addr := serveFor(t, r)
	for _, tc := range []struct{ method, path, want string }{
		{"POST", "/api/state", "400 Bad Request"},
		{"POST", "/api/nowhere", "404 Not Found"},
		{"OPTIONS", "*", "400 Bad Request"},
	} {
		c := dialFor(t, addr)
		_, _ = io.WriteString(c, tc.method+" "+tc.path+" HTTP/1.1\r\nHost: 127.0.0.1\r\n"+tokenHeader+": "+r.srv.Token()+
			"\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"model\":")
		line, err := bufio.NewReader(c).ReadString('\n')
		if err != nil || !strings.Contains(line, tc.want) {
			t.Fatalf("%s %s: %q %v", tc.method, tc.path, line, err)
		}
		if rest, err := io.ReadAll(c); err != nil {
			t.Fatalf("%s: the connection was kept (%v) after %q", tc.path, err, rest)
		}
	}
}

// A kept-alive connection is closed after idleTimeout; before, it stayed open for ever. An event stream (a GET
// without a body) lives on past the body deadline.
func TestIdleConnectionIsClosedButAStreamLivesOn(t *testing.T) {
	oldIdle, oldBody := idleTimeout, bodyReadTimeout
	idleTimeout, bodyReadTimeout = 300*time.Millisecond, 300*time.Millisecond
	t.Cleanup(func() { idleTimeout, bodyReadTimeout = oldIdle, oldBody })
	r := newRig(t, nil)
	addr := serveFor(t, r)
	c := dialFor(t, addr)
	_, _ = io.WriteString(c, "GET /api/run HTTP/1.1\r\nHost: 127.0.0.1\r\n"+tokenHeader+": "+r.srv.Token()+"\r\n\r\n")
	all, err := io.ReadAll(c)
	if err != nil || !strings.Contains(string(all), "200 OK") {
		t.Fatalf("the idle connection stayed open: %v %q", err, all)
	}
	s := dialFor(t, addr)
	_ = s.SetDeadline(time.Now().Add(1500 * time.Millisecond))
	_, _ = io.WriteString(s, "GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\n"+tokenHeader+": "+r.srv.Token()+"\r\n\r\n")
	got, err := io.ReadAll(s)
	if ne, ok := err.(net.Error); !ok || !ne.Timeout() || !strings.Contains(string(got), "text/event-stream") {
		t.Fatalf("the event stream ended early: %v %q", err, got)
	}
}
