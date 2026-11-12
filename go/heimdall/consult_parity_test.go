package heimdall

import (
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// consultReply is one scripted answer: the raw bytes, or a deny padded to big bytes of body, after delay ms; with open the
// connection stays open after the answer.
type consultReply struct {
	raw   string
	big   int
	delay int
	open  bool
}

func (r consultReply) bytes() []byte {
	if r.big == 0 {
		return []byte(r.raw)
	}
	body := `{"decision":"deny","pad":"` + strings.Repeat("x", r.big-len(`{"decision":"deny","pad":""}`)) + `"}`
	return []byte(fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s", len(body), body))
}

func ok(body string) string {
	return fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s", len(body), body)
}

func verdictOut(v *Verdict) any {
	if v == nil {
		return nil
	}
	var reasons any
	if v.Reasons != nil {
		reasons = parity.Texts(v.Reasons)
	}
	return map[string]any{"denied": v.Denied, "reasons": reasons}
}

// TestParityConsult records Consult (testdata/parity/consult.json): the request it sends and the verdict it makes of
// scripted answers from a raw loopback server (deny, allow, odd reasons, errors, redirects, chunked, cut short, too big,
// not HTTP, silent past the timeout), and the URL settings.
func TestParityConsult(t *testing.T) {
	replies := []consultReply{
		{raw: ok(`{"decision":"deny","reasons":["injection-phrase: x",5,{"b":1,"a":"<&>"},null,true,1.5e300,"é"]}`)},
		{raw: ok(`{"decision":"allow","reasons":[]}`)}, {raw: ok(`{"decision":"DENY"}`)}, {raw: ok(`{"decision":"deny"}`)},
		{raw: ok(`{"decision":"deny","reasons":"one"}`)}, {raw: ok(`{"decision":"deny","reasons":null}`)}, {raw: ok(`{"Decision":"deny"}`)},
		{raw: ok(`{"decision":"deny","decision":"allow"}`)}, {raw: ok(`{"decision":"deny"} trailing`)}, {raw: ok(`[]`)}, {raw: ok(`null`)},
		{raw: ok(`"deny"`)}, {raw: ok(``)}, {raw: ok(`{"decision":"deny","x":1e400}`)}, {raw: ok("{\"decision\":\"deny\",\"reasons\":[\"bad \xff\"]}")},
		{raw: "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 299 Odd\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 302 Found\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 204 No Content\r\n\r\n"},
		{raw: "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"dec\r\nE\r\nision\":\"deny\"}\r\n0\r\n\r\n"},
		{raw: "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"dec\r\nZZ\r\n"},
		{raw: "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.0 200 OK\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/2.0 200 OK\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "XTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 20 OK\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\nContent-Length: 19\n\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\nContent-Length: 19\r\nContent-Length: 20\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\nContent-Length: 25\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\nContent-Length: 19\r\nContent-Length: 19\r\n\r\n{\"decision\":\"deny\"}"},
		{raw: "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n13\r\n{\"decision\":\"deny\"}\r\n0\r\n\r\n"},
		{raw: "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n13;ext=1\r\n{\"decision\":\"deny\"}\r\n0\r\nTrailer: x\r\n\r\n"},
		{raw: "hello there\r\n\r\n"}, {raw: ""}, {raw: "HTTP/1.1 200 OK\r\nContent-Len"},
		{big: 1<<20 - 10}, {big: 1<<20 + 10},
		{raw: ok(`{"decision":"deny"}`), delay: 2500},
		{raw: "HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n{\"decision\":", open: true},
	}
	var cases []parity.Case
	for i, r := range replies {
		addr, got := parity.RawServer(t, r.bytes(), time.Duration(r.delay)*time.Millisecond, !r.open)
		t.Setenv(EnvVar, "http://"+addr+"/v1/review?probe=1")
		v := Consult("ignore é <b>\xff", "memory_proposal", "s-1")
		req := got()
		var in any = i
		cases = append(cases, parity.Case{Fn: "reply", In: in, Out: map[string]any{"verdict": verdictOut(v), "request": req}})
	}
	var specs []any
	for _, r := range replies {
		specs = append(specs, map[string]any{"raw": parity.Text(r.raw), "big": r.big, "delay": r.delay, "open": r.open})
	}
	cases = append(cases, parity.Case{Fn: "replies", In: specs})
	// the settings: no server answers these
	for _, env := range [][2]string{{"", ""}, {"  ", ""}, {"", "http://127.0.0.1:1/v1/review"}, {"not a url", ""}, {"https://127.0.0.1:1/v1/review", ""},
		{"ftp://127.0.0.1:1/x", ""}, {"http://127.0.0.1:1/v1/review", "http://other"}, {"http://", ""}, {"http:///v1/review", ""}} {
		t.Setenv(EnvVar, env[0])
		t.Setenv(LegacyEnvVar, env[1])
		v := Consult("text", "memory_proposal", "")
		cases = append(cases, parity.Case{Fn: "env", In: []string{env[0], env[1]}, Out: []any{verdictOut(v), URL(func(k string) string {
			if k == EnvVar {
				return env[0]
			}
			return env[1]
		})}})
	}
	parity.Fixture(t, "consult", cases)
}
