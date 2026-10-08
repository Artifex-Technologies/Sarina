package mcp

import (
	"context"
	"io"
	"strings"
	"testing"
	"time"
)

const wait = 5 * time.Second

func ready(t *testing.T, f *fakeOpts) *Client {
	t.Helper()
	c := startPipeFake(t, f)
	if err := c.Initialize(t.Context(), wait); err != nil {
		t.Fatal(err)
	}
	return c
}

func TestHandshakeListAndCallOverPipes(t *testing.T) {
	f := &fakeOpts{}
	c := ready(t, f)
	if f.initSeen.Load() != 1 {
		t.Fatal("initialize not sent")
	}
	// the initialized notification is sent after the handshake; give the server a moment to read it
	deadline := time.Now().Add(wait)
	for !f.saw("notifications/initialized") && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	if !f.saw("notifications/initialized") {
		t.Fatal("notifications/initialized was not sent")
	}
	list, err := c.ListTools(t.Context(), wait)
	if err != nil || len(list) != 9 || list[0].Name != "echo" {
		t.Fatalf("tools = %v, %v", list, err)
	}
	res, err := c.CallTool(t.Context(), "echo", map[string]any{"text": "hi"}, wait)
	if err != nil || res.Text != "echo: hi" || res.IsError {
		t.Fatalf("call = %+v, %v", res, err)
	}
}

func TestToolListPaginationIsFollowed(t *testing.T) {
	c := ready(t, &fakeOpts{pages: true})
	list, err := c.ListTools(t.Context(), wait)
	if err != nil || len(list) != 4 {
		t.Fatalf("tools = %v, %v", list, err)
	}
}

func TestIsErrorIsReportedNotHidden(t *testing.T) {
	c := ready(t, &fakeOpts{})
	res, err := c.CallTool(t.Context(), "fail", nil, wait)
	if err != nil || !res.IsError || res.Text != "boom" {
		t.Fatalf("%+v %v", res, err)
	}
}

func TestRichContentIsRenderedWithoutInliningBinary(t *testing.T) {
	c := ready(t, &fakeOpts{})
	res, err := c.CallTool(t.Context(), "rich", nil, wait)
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"plain", "[image content omitted: image/png, 4 base64 characters]", "inline", `"k": 1`} {
		if !strings.Contains(res.Text, want) {
			t.Errorf("result lacks %q:\n%s", want, res.Text)
		}
	}
	if strings.Contains(res.Text, "QUJD") {
		t.Fatal("binary data was inlined")
	}
}

func TestJSONRPCErrorBecomesAnError(t *testing.T) {
	c := ready(t, &fakeOpts{})
	_, err := c.CallTool(t.Context(), "no_such", nil, wait)
	if err == nil || !strings.Contains(err.Error(), "unknown tool no_such") {
		t.Fatalf("err = %v", err)
	}
	_, err = c.Call(t.Context(), "bogus/method", nil, wait)
	if err == nil || !strings.Contains(err.Error(), "method not found") {
		t.Fatalf("err = %v", err)
	}
}

func TestTimeoutCancelsTheRequestAndKeepsTheConnectionUsable(t *testing.T) {
	f := &fakeOpts{}
	c := ready(t, f)
	start := time.Now()
	_, err := c.CallTool(t.Context(), "slow", nil, 150*time.Millisecond)
	if err == nil || !strings.Contains(err.Error(), "no answer") {
		t.Fatalf("err = %v", err)
	}
	if time.Since(start) > 3*time.Second {
		t.Fatal("the timeout was not honoured")
	}
	deadline := time.Now().Add(wait)
	for !f.saw("notifications/cancelled") && time.Now().Before(deadline) {
		time.Sleep(5 * time.Millisecond)
	}
	if !f.saw("notifications/cancelled") {
		t.Fatal("the server was not told the request was abandoned")
	}
	// a late answer to the abandoned request is ignored and the next call works
	close(f.release)
	res, err := c.CallTool(t.Context(), "echo", map[string]any{"text": "again"}, wait)
	if err != nil || res.Text != "echo: again" {
		t.Fatalf("after a timeout: %+v %v", res, err)
	}
}

func TestCancelledContextAbandonsTheCall(t *testing.T) {
	f := &fakeOpts{}
	c := ready(t, f)
	ctx, cancel := context.WithCancel(t.Context())
	go func() { time.Sleep(50 * time.Millisecond); cancel() }()
	_, err := c.CallTool(ctx, "slow", nil, wait)
	if err != context.Canceled {
		t.Fatalf("err = %v", err)
	}
}

func TestNoiseOnStdoutAndUnrelatedResponsesAreIgnored(t *testing.T) {
	c := ready(t, &fakeOpts{noise: true})
	res, err := c.CallTool(t.Context(), "echo", map[string]any{"text": "x"}, wait)
	if err != nil || res.Text != "echo: x" {
		t.Fatalf("%+v %v", res, err)
	}
}

func TestServerRequestsAreAnsweredNotLeftHanging(t *testing.T) {
	// A hand-rolled server that sends a ping and an unsupported request first,
	// then checks the client answered both before replying to initialize.
	c2sR, c2sW := io.Pipe()
	s2cR, s2cW := io.Pipe()
	got := make(chan string, 4)
	go func() {
		buf := make([]byte, 4096)
		write := func(s string) { _, _ = s2cW.Write([]byte(s + "\n")) }
		n, _ := c2sR.Read(buf) // initialize
		_ = n
		write(`{"jsonrpc":"2.0","id":"p1","method":"ping"}`)
		write(`{"jsonrpc":"2.0","id":"r1","method":"roots/list"}`)
		for i := 0; i < 2; i++ {
			n, _ = c2sR.Read(buf)
			got <- string(buf[:n])
		}
		write(`{"jsonrpc":"2.0","id":1,"result":{}}`)
		_, _ = c2sR.Read(buf)
	}()
	c := NewPipeClient("hand", s2cR, c2sW)
	defer c.Close()
	if err := c.Initialize(t.Context(), wait); err != nil {
		t.Fatal(err)
	}
	both := <-got + <-got // the two answers may arrive in either order
	for _, want := range []string{`"id":"p1"`, `"result":{}`, `"id":"r1"`, "-32601"} {
		if !strings.Contains(both, want) {
			t.Fatalf("answers lack %s: %s", want, both)
		}
	}
}

func TestServerExitFailsPendingCallsFast(t *testing.T) {
	c := ready(t, &fakeOpts{})
	start := time.Now()
	_, err := c.CallTool(t.Context(), "crash", nil, 30*time.Second)
	if err == nil || !strings.Contains(err.Error(), "exited") {
		t.Fatalf("err = %v", err)
	}
	if time.Since(start) > 3*time.Second {
		t.Fatal("waited for the timeout although the server was gone")
	}
	if c.Alive() {
		t.Fatal("Alive after exit")
	}
	if _, err := c.CallTool(t.Context(), "echo", nil, wait); err == nil {
		t.Fatal("a call on a dead connection must fail")
	}
}

func TestOversizedLinesAreDroppedNotBuffered(t *testing.T) {
	c2sR, c2sW := io.Pipe()
	s2cR, s2cW := io.Pipe()
	go func() {
		buf := make([]byte, 4096)
		_, _ = c2sR.Read(buf)
		junk := strings.Repeat("a", MaxLineBytes+1024)
		_, _ = s2cW.Write([]byte(junk + "\n"))
		_, _ = s2cW.Write([]byte(`{"jsonrpc":"2.0","id":1,"result":{}}` + "\n"))
		_, _ = c2sR.Read(buf)
	}()
	c := NewPipeClient("big", s2cR, c2sW)
	defer c.Close()
	if err := c.Initialize(t.Context(), wait); err != nil {
		t.Fatalf("a huge junk line should be skipped: %v", err)
	}
}

func TestResourcesListAndRead(t *testing.T) {
	c := ready(t, &fakeOpts{})
	rs, err := c.ListResources(t.Context(), wait)
	if err != nil || len(rs) != 2 || rs[0].URI != "mem://notes" || rs[0].MimeType != "text/plain" {
		t.Fatalf("%v %v", rs, err)
	}
	text, err := c.ReadResource(t.Context(), "mem://notes", wait)
	if err != nil || !strings.Contains(text, "remember the milk") || strings.Contains(text, "QUJD") {
		t.Fatalf("%q %v", text, err)
	}
	if _, err := c.ReadResource(t.Context(), "mem://missing", wait); err == nil {
		t.Fatal("expected a not-found error")
	}
}

func TestCloseIsIdempotentAndPrompt(t *testing.T) {
	c := ready(t, &fakeOpts{})
	start := time.Now()
	c.Close()
	c.Close()
	if time.Since(start) > 2*time.Second {
		t.Fatal("Close was slow")
	}
}
