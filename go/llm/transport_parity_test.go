package llm

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"fmt"
	"net"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

type S = parity.Step

func okJSON(body string) []S {
	return []S{{Send: fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s", len(body), body)}}
}

func statusJSON(code int, body string) []S {
	return []S{{Send: fmt.Sprintf("HTTP/1.1 %d X\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s", code, len(body), body)}}
}

const sseHead = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"

func chunk(s string) string { return fmt.Sprintf("%x\r\n%s\r\n", len(s), s) }

func sseEvent(content string) string {
	return chunk(`data: {"choices":[{"delta":{"content":"` + content + `"}}]}` + "\n\n")
}

const sseEnd = "0\r\n\r\n"

var answerOK = `{"choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}`

// transportCase is one call of Complete against a scripted server: the server's answers per connection and the
// client's settings. The Rust test reads all of it from the fixture.
type transportCase struct {
	name      string
	script    [][]S
	tls       string // "", "trusted", "untrusted" (another authority), "wrongip", "dnsonly"
	base      string // "" = the server's; "refused" = a port nobody listens on; else as given
	timeout   float64
	fallbacks []string
	stream    bool
	tools     bool
	maxTokens int
	cancelMs  int
	ctxMs     int
	retryMs   int
	maxResp   int64
	apiKey    string
}

func transportCases() []transportCase {
	streamOK := []S{{Send: sseHead}, {Send: sseEvent("Hel")}, {Send: sseEvent("lo")},
		{Send: chunk(`data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}` + "\n\n")},
		{Send: chunk("data: [DONE]\n\n")}, {Send: sseEnd}}
	slowStream := []S{{Send: sseHead}}
	for i := 0; i < 5; i++ {
		slowStream = append(slowStream, S{Wait: 250}, S{Send: sseEvent(fmt.Sprint(i))})
	}
	slowStream = append(slowStream, S{Send: sseEnd})
	stalled := []S{{Send: sseHead}, {Send: sseEvent("a")}, {Wait: 1500}, {Send: sseEvent("b")}, {Send: sseEnd}}
	bigErr := []S{{Send: "HTTP/1.1 500 X\r\nContent-Length: 70000\r\n\r\n\xff\xfe"}, {Fill: 69998}}
	think := `{"choices":[{"message":{"content":"<think>plan</think>Reading.<tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}</tool_call>"}}]}`
	return []transportCase{
		{name: "ok", script: [][]S{okJSON(answerOK)}},
		{name: "okStream", script: [][]S{streamOK}, stream: true},
		{name: "normalized", script: [][]S{okJSON(think)}, tools: true},
		{name: "retry503", script: [][]S{statusJSON(503, `{"error":"busy"}`), okJSON(answerOK)}},
		{name: "retry429Twice", script: [][]S{statusJSON(429, `slow down`), statusJSON(429, `slow down`)}},
		{name: "fallbackAfter5xx", script: [][]S{statusJSON(503, "a"), statusJSON(503, "b"), okJSON(answerOK)}, fallbacks: []string{"m2", "m3"}},
		{name: "fallback404", script: [][]S{statusJSON(404, `{"error":"model not found"}`), okJSON(answerOK)}, fallbacks: []string{"m2"}},
		{name: "fallbackAllFail", script: [][]S{statusJSON(502, "a"), statusJSON(502, "b"), statusJSON(500, "c"), statusJSON(500, "d")}, fallbacks: []string{"m2"}},
		{name: "fallbackStopsOn400", script: [][]S{statusJSON(503, "a"), statusJSON(503, "b"), statusJSON(400, "bad")}, fallbacks: []string{"m2", "m3"}},
		{name: "noRetry400", script: [][]S{statusJSON(400, `{"error":{"message":"bad request"}}`)}, fallbacks: []string{"m2"}},
		{name: "teapotNotUTF8", script: [][]S{statusJSON(418, "\xff\xfe bad \xc3 é \xe2\x82")}},
		{name: "errorBodyCut", script: [][]S{bigErr, bigErr}},
		{name: "eofThenOK", script: [][]S{{}, okJSON(answerOK)}},
		{name: "eofTwice", script: [][]S{{}, {}}},
		{name: "garbageTwice", script: [][]S{{{Send: "SSH-2.0-x\r\n\r\n"}}, {{Send: "SSH-2.0-x\r\n\r\n"}}}},
		{name: "refused", base: "refused"},
		{name: "noScheme", base: ""},
		{name: "ftpScheme", base: "ftp://x/v1"},
		{name: "badKey", apiKey: "k\r\nX-Injected: 1", script: [][]S{okJSON(answerOK)}},
		{name: "informational", script: [][]S{{{Send: "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 102 Processing\r\n\r\n"}, {Send: okJSON(answerOK)[0].Send}}}},
		{name: "timeout", script: [][]S{{{Wait: 1500}, {Send: okJSON(answerOK)[0].Send}}}, timeout: 0.3},
		{name: "timeoutFallback", script: [][]S{{{Wait: 1500}, {Send: okJSON(answerOK)[0].Send}}, okJSON(answerOK)}, timeout: 0.3, fallbacks: []string{"m2"}},
		{name: "maxTokensExtends", script: [][]S{{{Wait: 600}, {Send: okJSON(answerOK)[0].Send}}}, timeout: 0.3, maxTokens: 40},
		{name: "maxTokensStillTimesOut", script: [][]S{{{Wait: 3000}, {Send: okJSON(answerOK)[0].Send}}}, timeout: 0.3, maxTokens: 40},
		{name: "bodyTimeout", script: [][]S{{{Send: "HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n{\"cho"}, {Wait: 1500}}}, timeout: 0.3},
		{name: "streamIdleReset", script: [][]S{slowStream}, stream: true, timeout: 1.0},
		{name: "streamStalled", script: [][]S{stalled}, stream: true, timeout: 0.4, maxTokens: 400},
		{name: "streamHeadTimeout", script: [][]S{{{Wait: 1500}, {Send: sseHead}}}, stream: true, timeout: 0.3},
		{name: "cancel", script: [][]S{{{Hold: true}}}, cancelMs: 250},
		{name: "cancelStream", script: [][]S{{{Send: sseHead}, {Send: sseEvent("a")}, {Hold: true}}}, stream: true, cancelMs: 250},
		{name: "cancelInRetryWait", script: [][]S{statusJSON(503, "a")}, cancelMs: 600, retryMs: 3000},
		{name: "ctxDeadline", script: [][]S{{{Hold: true}}}, ctxMs: 400},
		{name: "bodyCutShort", script: [][]S{{{Send: "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"choices\":[]}"}}}},
		{name: "streamCutShort", script: [][]S{{{Send: sseHead}, {Send: sseEvent("a")}, {Send: "40\r\ndata: {\"cho"}}}, stream: true},
		{name: "streamBadChunk", script: [][]S{{{Send: sseHead}, {Send: sseEvent("a")}, {Send: "zz\r\n"}}}, stream: true},
		{name: "tooLarge", script: [][]S{okJSON(answerOK)}, maxResp: 64},
		{name: "streamTooLarge", script: [][]S{streamOK}, stream: true, maxResp: 100},
		{name: "notJSON", script: [][]S{okJSON("not json")}},
		{name: "closeDelimited", script: [][]S{{{Send: "HTTP/1.1 200 OK\r\n\r\n" + answerOK}}}},
		{name: "tlsOK", script: [][]S{okJSON(answerOK)}, tls: "trusted"},
		{name: "tlsStream", script: [][]S{streamOK}, tls: "trusted", stream: true},
		{name: "tlsUntrusted", script: [][]S{okJSON(answerOK), okJSON(answerOK)}, tls: "untrusted"},
		{name: "tlsWrongIP", script: [][]S{okJSON(answerOK), okJSON(answerOK)}, tls: "wrongip"},
		{name: "tlsDNSOnly", script: [][]S{okJSON(answerOK), okJSON(answerOK)}, tls: "dnsonly"},
	}
}

// errShort is errOut with the server's address as {ADDR} and a long message as its start, length and sha256.
func errShort(err error, addr string) any {
	if err == nil {
		return nil
	}
	out := errOut(err).([]any)
	msg, _ := out[0].(string)
	if addr != "" {
		msg = strings.ReplaceAll(msg, addr, "{ADDR}")
	}
	if len(msg) > 4096 {
		h := sha256.Sum256([]byte(msg))
		return []any{msg[:200], len(msg), hex.EncodeToString(h[:]), out[1]}
	}
	out[0] = parity.Text(msg)
	return out
}

// TestParityOpenAIComplete records Complete (testdata/parity/transport.json) against scripted servers: the requests
// each connection carried, the turn (with the model that answered) or the error, and the events handed on. TLS cases use
// an authority made in the test; nothing leaves the machine.
func TestParityOpenAIComplete(t *testing.T) {
	if testing.Short() {
		t.Skip("timed cases")
	}
	ca := parity.NewTestCA(t, "Sarina parity test CA")
	var cases []parity.Case
	for _, tc := range transportCases() {
		var srvCfg *tls.Config
		clientCfg := &tls.Config{RootCAs: ca.Pool}
		scheme := "http"
		switch tc.tls {
		case "trusted":
			srvCfg, scheme = ca.Server(t, []string{"127.0.0.1"}, nil), "https"
		case "untrusted":
			srvCfg, scheme = ca.Server(t, []string{"127.0.0.1"}, nil), "https"
			clientCfg = &tls.Config{RootCAs: parity.NewTestCA(t, "Another test CA").Pool}
		case "wrongip":
			srvCfg, scheme = ca.Server(t, []string{"127.0.0.2"}, nil), "https"
		case "dnsonly":
			srvCfg, scheme = ca.Server(t, nil, []string{"localhost"}), "https"
		}
		addr, seen := "", func() []map[string]any { return nil }
		base := tc.base
		switch {
		case tc.base == "refused":
			ln, _ := net.Listen("tcp", "127.0.0.1:0")
			addr = ln.Addr().String()
			ln.Close()
			base = "http://" + addr + "/v1"
		case tc.script != nil:
			addr, seen = parity.RawSeq(t, srvCfg, tc.script)
			base = scheme + "://" + addr + "/v1/"
		}
		key := tc.apiKey
		if key == "" {
			key = "test-key"
		}
		c := &Client{Config: config.ModelConfig{Model: "m1", BaseURL: base, APIKey: key, Temperature: 0.2, TimeoutSeconds: tc.timeout,
			Fallbacks: tc.fallbacks}, RetryDelay: time.Duration(max(tc.retryMs, 30)) * time.Millisecond,
			HTTPClient: &http.Client{Transport: &http.Transport{DisableKeepAlives: true, TLSClientConfig: clientCfg}}}
		req := Request{Messages: []Message{{Role: "system", Content: "be brief"}, {Role: "user", Content: "hi <there>"}}, Stream: tc.stream,
			MaxTokens: tc.maxTokens}
		if tc.tools {
			req.Tools = []ToolSpec{{Name: "read_file", Description: "Reads a file"}}
		}
		ctx, cancel := context.Background(), context.CancelFunc(func() {})
		if tc.ctxMs > 0 {
			ctx, cancel = context.WithTimeout(ctx, time.Duration(tc.ctxMs)*time.Millisecond)
		} else if tc.cancelMs > 0 {
			ctx, cancel = context.WithCancel(ctx)
			time.AfterFunc(time.Duration(tc.cancelMs)*time.Millisecond, cancel)
		}
		oldR := maxResponseBytes
		if tc.maxResp > 0 {
			maxResponseBytes = tc.maxResp
		}
		var evs []any
		turn, err := c.Complete(ctx, req, func(e StreamEvent) {
			evs = append(evs, []any{e.Type, parity.Text(e.Delta), e.FinishReason})
		})
		maxResponseBytes = oldR
		cancel()
		requests := parity.Settle(seen) // a refused handshake is seen by the server a moment after the client gives up
		var tf any
		if turn != nil {
			m := turnFields(turn).(map[string]any)
			m["model"] = turn.Model
			tf = m
		}
		in := map[string]any{"name": tc.name, "script": parity.Steps(tc.script), "tls": tc.tls, "base": tc.base,
			"timeout": tc.timeout, "fallbacks": tc.fallbacks, "stream": tc.stream, "tools": tc.tools, "maxTokens": tc.maxTokens,
			"cancelMs": tc.cancelMs, "ctxMs": tc.ctxMs, "retryMs": tc.retryMs, "maxResp": tc.maxResp, "apiKey": tc.apiKey}
		cases = append(cases, parity.Case{Fn: "complete", In: in, Out: map[string]any{"turn": tf, "err": errShort(err, addr), "events": evs,
			"requests": requests}})
	}
	cases = append(cases, parity.Case{Fn: "defaults", Out: []any{int64(750 * time.Millisecond), config.DefaultTimeoutSeconds}})
	parity.Fixture(t, "transport", cases)
}
