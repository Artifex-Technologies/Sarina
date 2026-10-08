package mcp

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// fakeOpts shapes the fake server a test talks to.
type fakeOpts struct {
	release  chan struct{} // the "slow" tool waits for this to close
	counter  atomic.Int64  // server-side state: the "count" tool increments it
	initSeen atomic.Int64
	mu       sync.Mutex
	received []string // methods the server saw
	noise    bool     // print junk lines and unrelated responses
	pages    bool     // tools/list in two pages
}

func (f *fakeOpts) saw(method string) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	for _, m := range f.received {
		if m == method {
			return true
		}
	}
	return false
}

func textResult(s string) map[string]any {
	return map[string]any{"content": []any{map[string]any{"type": "text", "text": s}}}
}

func toolList(names ...string) []any {
	var out []any
	for _, n := range names {
		out = append(out, map[string]any{"name": n, "description": "the " + n + " tool",
			"inputSchema": map[string]any{"type": "object", "properties": map[string]any{"text": map[string]any{"type": "string"}}}})
	}
	return out
}

// serveFake speaks the server side of MCP until r ends or a tool asks to crash.
func serveFake(r io.Reader, w io.Writer, f *fakeOpts) {
	var wmu sync.Mutex
	write := func(v any) {
		b, _ := json.Marshal(v)
		wmu.Lock()
		defer wmu.Unlock()
		_, _ = w.Write(append(b, '\n'))
	}
	reply := func(id json.RawMessage, result any) {
		write(map[string]any{"jsonrpc": "2.0", "id": json.RawMessage(id), "result": result})
	}
	fail := func(id json.RawMessage, code int, msg string) {
		write(map[string]any{"jsonrpc": "2.0", "id": json.RawMessage(id), "error": map[string]any{"code": code, "message": msg}})
	}
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 1<<20), 16<<20)
	for sc.Scan() {
		var m struct {
			ID     json.RawMessage `json:"id"`
			Method string          `json:"method"`
			Params json.RawMessage `json:"params"`
		}
		if json.Unmarshal(sc.Bytes(), &m) != nil {
			continue
		}
		f.mu.Lock()
		f.received = append(f.received, m.Method)
		f.mu.Unlock()
		if len(m.ID) == 0 {
			continue // a notification
		}
		switch m.Method {
		case "initialize":
			f.initSeen.Add(1)
			var p map[string]any
			_ = json.Unmarshal(m.Params, &p)
			info, _ := p["clientInfo"].(map[string]any)
			if f.noise {
				_, _ = w.Write([]byte("this is a log line, not JSON\n"))
				write(map[string]any{"jsonrpc": "2.0", "id": 9999, "result": map[string]any{}})
			}
			reply(m.ID, map[string]any{"protocolVersion": ProtocolVersion, "capabilities": map[string]any{"tools": map[string]any{}},
				"serverInfo": map[string]any{"name": "fake", "version": "1"}, "seenClient": info["name"]})
		case "ping":
			reply(m.ID, map[string]any{})
		case "tools/list":
			var p struct{ Cursor string }
			_ = json.Unmarshal(m.Params, &p)
			switch {
			case f.pages && p.Cursor == "":
				reply(m.ID, map[string]any{"tools": toolList("echo", "big"), "nextCursor": "p2"})
			case f.pages:
				reply(m.ID, map[string]any{"tools": toolList("count", "fail")})
			default:
				reply(m.ID, map[string]any{"tools": toolList("echo", "big", "fail", "slow", "count", "crash", "env", "rich", "pid")})
			}
		case "tools/call":
			var p struct {
				Name      string         `json:"name"`
				Arguments map[string]any `json:"arguments"`
			}
			_ = json.Unmarshal(m.Params, &p)
			switch p.Name {
			case "echo":
				reply(m.ID, textResult(fmt.Sprintf("echo: %v", p.Arguments["text"])))
			case "big":
				reply(m.ID, textResult("START"+strings.Repeat("x", 30000)+"END"))
			case "fail":
				res := textResult("boom")
				res["isError"] = true
				reply(m.ID, res)
			case "slow":
				go func(id json.RawMessage) {
					<-f.release
					reply(id, textResult("slow done"))
				}(m.ID)
			case "count":
				reply(m.ID, textResult(fmt.Sprint(f.counter.Add(1))))
			case "crash":
				return // the process "dies" mid-request
			case "env":
				name, _ := p.Arguments["name"].(string)
				if v, ok := os.LookupEnv(name); ok {
					reply(m.ID, textResult("present:"+v))
				} else {
					reply(m.ID, textResult("absent"))
				}
			case "pid":
				reply(m.ID, textResult(fmt.Sprint(os.Getpid())))
			case "rich":
				reply(m.ID, map[string]any{
					"content": []any{
						map[string]any{"type": "text", "text": "plain"},
						map[string]any{"type": "image", "data": "QUJD", "mimeType": "image/png"},
						map[string]any{"type": "resource", "resource": map[string]any{"uri": "file:///a", "text": "inline"}},
					},
					"structuredContent": map[string]any{"k": 1},
				})
			default:
				fail(m.ID, -32602, "unknown tool "+p.Name)
			}
		case "resources/list":
			reply(m.ID, map[string]any{"resources": []any{
				map[string]any{"uri": "mem://notes", "name": "notes", "description": "operator notes", "mimeType": "text/plain"},
				map[string]any{"uri": "mem://other", "name": "other"},
			}})
		case "resources/read":
			var p struct{ URI string }
			_ = json.Unmarshal(m.Params, &p)
			if p.URI == "mem://notes" {
				reply(m.ID, map[string]any{"contents": []any{map[string]any{"uri": p.URI, "text": "remember the milk"},
					map[string]any{"uri": p.URI, "blob": "QUJDRA==", "mimeType": "application/octet-stream"}}})
			} else {
				fail(m.ID, -32002, "resource not found")
			}
		default:
			fail(m.ID, -32601, "method not found")
		}
	}
}

// startPipeFake connects a client to an in-process fake server over pipes.
func startPipeFake(t *testing.T, f *fakeOpts) *Client {
	t.Helper()
	if f.release == nil {
		f.release = make(chan struct{})
	}
	c2sR, c2sW := io.Pipe()
	s2cR, s2cW := io.Pipe()
	go func() {
		serveFake(c2sR, s2cW, f)
		c2sR.Close() // a dead process no longer reads: later writes fail, as with a real pipe
		s2cW.Close()
	}()
	c := NewPipeClient("fake", s2cR, c2sW)
	t.Cleanup(func() {
		select {
		case <-f.release:
		default:
			close(f.release)
		}
		c.Close()
	})
	return c
}

// TestHelperProcess is the fake server as a real child process: the tests
// re-run the test binary with MCP_FAKE_SERVER=1.
func TestHelperProcess(t *testing.T) {
	if os.Getenv("MCP_FAKE_SERVER") != "1" {
		return
	}
	if marker := os.Getenv("MCP_MARKER"); marker != "" {
		_ = os.WriteFile(marker, []byte("started"), 0o644)
	}
	serveFake(os.Stdin, os.Stdout, &fakeOpts{release: make(chan struct{})})
	if os.Getenv("MCP_STUBBORN") == "1" {
		time.Sleep(time.Minute) // ignores the end of stdin, so only a kill stops it
	}
	os.Exit(0)
}

// helperDef is a ServerDef that runs TestHelperProcess.
func helperDef(name string, env map[string]string) ServerDef {
	e := map[string]string{"MCP_FAKE_SERVER": "1"}
	for k, v := range env {
		e[k] = v
	}
	return ServerDef{Name: name, Command: os.Args[0], Args: []string{"-test.run=^TestHelperProcess$"}, Env: e}
}
