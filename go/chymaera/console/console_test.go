package console

import (
	"bufio"
	"encoding/json"
	"errors"
	"net"
	"strings"
	"sync"
	"testing"
	"time"
)

// fakeConsole is a real loopback server speaking the app's NDJSON protocol.
type fakeConsole struct {
	ln       net.Listener
	handler  func(req map[string]any) map[string]any // nil result = hang up
	token    string
	rawReply string
	mu       sync.Mutex
	requests []map[string]any
}

func newFake(t *testing.T, f *fakeConsole) *fakeConsole {
	t.Helper()
	if f.handler == nil {
		f.handler = f.defaultHandler
	}
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	f.ln = ln
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go f.serve(conn)
		}
	}()
	return f
}

func (f *fakeConsole) port() int { return f.ln.Addr().(*net.TCPAddr).Port }

func (f *fakeConsole) all() []map[string]any {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]map[string]any(nil), f.requests...)
}

func (f *fakeConsole) defaultHandler(req map[string]any) map[string]any {
	switch req["cmd"] {
	case "ping":
		return map[string]any{"ok": true, "pong": true, "version": "1.2.3"}
	case "status":
		return map[string]any{"ok": true, "serverRunning": true, "serverPort": f.port(), "sessionActive": false, "eventCount": 4}
	case "help":
		return map[string]any{"ok": true, "commands": []any{"ping", "status"}}
	case "log.tail":
		return map[string]any{"ok": true, "entries": []any{map[string]any{"message": "hello", "severity": "info"}}}
	case "capture.list":
		return map[string]any{"ok": true, "captures": []any{map[string]any{"id": 1, "type": "nfc", "name": "badge.nfc", "size": 512}}}
	case "session.start":
		return map[string]any{"ok": true, "sessionId": 42}
	case "log.add", "session.end":
		return map[string]any{"ok": true}
	}
	return map[string]any{"ok": false, "error": "unknown command"}
}

func (f *fakeConsole) serve(conn net.Conn) {
	defer conn.Close()
	r := bufio.NewReader(conn)
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			return
		}
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var req map[string]any
		if json.Unmarshal([]byte(line), &req) != nil {
			return
		}
		f.mu.Lock()
		f.requests = append(f.requests, req)
		f.mu.Unlock()
		if f.rawReply != "" {
			conn.Write([]byte(f.rawReply + "\n"))
			return
		}
		var resp map[string]any
		if f.token != "" && req["token"] != f.token {
			resp = map[string]any{"ok": false, "error": "unauthorized"}
		} else {
			resp = f.handler(req)
		}
		if resp == nil {
			return
		}
		if id, ok := req["id"]; ok {
			if _, has := resp["id"]; !has {
				resp["id"] = id
			}
		}
		b, _ := json.Marshal(resp)
		conn.Write(append(b, '\n'))
	}
}

func client(f *fakeConsole) *Client {
	c := New()
	c.Port = f.port()
	c.Timeout = 5 * time.Second
	return c
}

func TestPingStatusHelpLogCaptures(t *testing.T) {
	f := newFake(t, &fakeConsole{})
	c := client(f)
	if err := c.Connect(); err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	p, err := c.Ping()
	if err != nil || p["version"] != "1.2.3" {
		t.Fatalf("ping: %v %v", p, err)
	}
	s, err := c.Status()
	if err != nil || s["serverRunning"] != true {
		t.Fatalf("status: %v %v", s, err)
	}
	h, err := c.Help()
	if err != nil || len(h) != 2 || h[0] != "ping" {
		t.Fatalf("help: %v %v", h, err)
	}
	l, err := c.LogTail(50)
	if err != nil || l[0]["message"] != "hello" {
		t.Fatalf("log: %v %v", l, err)
	}
	cl, err := c.CaptureList(7)
	if err != nil || cl[0]["name"] != "badge.nfc" {
		t.Fatalf("captures: %v %v", cl, err)
	}
	if got := f.all()[len(f.all())-1]["limit"]; got != float64(7) {
		t.Fatalf("limit forwarded as %v", got)
	}
	id, err := c.SessionStart("engagement-1", "")
	if err != nil || id.(json.Number).String() != "42" {
		t.Fatalf("session id: %v %v", id, err)
	}
}

func TestRequestIDsIncrementAndTokenOmittedWhenUnset(t *testing.T) {
	f := newFake(t, &fakeConsole{})
	c := client(f)
	c.Connect()
	defer c.Close()
	c.Ping()
	c.Ping()
	reqs := f.all()
	if reqs[0]["id"] != float64(1) || reqs[1]["id"] != float64(2) {
		t.Fatalf("ids: %v", reqs)
	}
	if _, has := reqs[1]["token"]; has {
		t.Fatal("token sent although unset")
	}
}

func TestOptionalParamsOmittedOrSent(t *testing.T) {
	f := newFake(t, &fakeConsole{})
	c := client(f)
	c.SessionStart("run", "")
	if _, has := f.all()[0]["notes"]; has {
		t.Fatal("empty notes must be omitted")
	}
	c.SessionStart("run", "context")
	if f.all()[1]["notes"] != "context" {
		t.Fatalf("notes: %v", f.all()[1])
	}
}

func TestSingleShotRequestOpensAndClosesItsOwnConnection(t *testing.T) {
	f := newFake(t, &fakeConsole{})
	c := client(f)
	if _, err := c.Ping(); err != nil {
		t.Fatal(err)
	}
	if c.conn != nil {
		t.Fatal("connection left open")
	}
}

func TestErrorResponse(t *testing.T) {
	f := newFake(t, &fakeConsole{})
	c := client(f)
	_, err := c.Request("nope", nil)
	var ce *Error
	if !errors.As(err, &ce) || ce.Message != "unknown command" || ce.Command != "nope" {
		t.Fatalf("err = %v", err)
	}
	if err.Error() != "nope: unknown command" {
		t.Fatalf("text = %q", err.Error())
	}
}

func TestToken(t *testing.T) {
	f := newFake(t, &fakeConsole{token: "secret"})
	c := client(f)
	c.Token = "secret"
	if p, err := c.Ping(); err != nil || p["pong"] != true {
		t.Fatalf("good token: %v %v", p, err)
	}
	c.Token = "wrong"
	_, err := c.Ping()
	var ce *Error
	if !errors.As(err, &ce) || ce.Message != "unauthorized" {
		t.Fatalf("wrong token: %v", err)
	}
}

func TestUnavailable(t *testing.T) {
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	port := ln.Addr().(*net.TCPAddr).Port
	ln.Close()
	c := New()
	c.Port, c.Timeout = port, 2*time.Second
	_, err := c.Ping()
	var ue *UnavailableError
	if !errors.As(err, &ue) || !strings.Contains(err.Error(), "no Chymaera console at 127.0.0.1:") {
		t.Fatalf("err = %v", err)
	}

	hangup := newFake(t, &fakeConsole{handler: func(map[string]any) map[string]any { return nil }})
	if _, err := client(hangup).Ping(); !errors.As(err, &ue) || !strings.Contains(err.Error(), "closed the connection without replying") {
		t.Fatalf("hangup: %v", err)
	}
	bad := newFake(t, &fakeConsole{rawReply: "this is not json"})
	if _, err := client(bad).Ping(); !errors.As(err, &ue) || !strings.Contains(err.Error(), "malformed JSON") {
		t.Fatalf("malformed: %v", err)
	}
	list := newFake(t, &fakeConsole{rawReply: `["a list, not an object"]`})
	if _, err := client(list).Ping(); !errors.As(err, &ue) || !strings.Contains(err.Error(), "expected a JSON object, got list") {
		t.Fatalf("non-object: %v", err)
	}
}

func TestReplyTimeoutIsUnavailable(t *testing.T) {
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	defer ln.Close()
	go func() {
		conn, err := ln.Accept()
		if err == nil {
			time.Sleep(2 * time.Second)
			conn.Close()
		}
	}()
	c := New()
	c.Port, c.Timeout = ln.Addr().(*net.TCPAddr).Port, 200*time.Millisecond
	_, err := c.Ping()
	var ue *UnavailableError
	if !errors.As(err, &ue) || !strings.Contains(err.Error(), "connection failed") {
		t.Fatalf("err = %v", err)
	}
}

func TestRenderStatus(t *testing.T) {
	text := RenderStatus(
		map[string]any{"version": "1.0", "serverRunning": true, "eventCount": json.Number("2")},
		[]map[string]any{{"type": "nfc", "name": "badge.nfc", "size": json.Number("512")}},
	)
	want := "# Chymaera console\n\nversion: 1.0\nserverRunning: True\neventCount: 2\n\n## Captures (1)\n- [nfc] badge.nfc (512 bytes)"
	if text != want {
		t.Fatalf("got:\n%s", text)
	}
	if !strings.Contains(RenderStatus(map[string]any{"version": "1.0"}, nil), "No captures recorded.") {
		t.Fatal("no-captures text missing")
	}
	if got := RenderStatus(nil, []map[string]any{{}}); !strings.Contains(got, "- [?] ? (? bytes)") {
		t.Fatalf("defaults: %s", got)
	}
}
