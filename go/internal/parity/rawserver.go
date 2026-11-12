package parity

import (
	"bufio"
	"io"
	"net"
	"net/textproto"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// RawServer listens on loopback for one connection, reads one HTTP request and answers it with the response bytes
// exactly as given (after delay; with close false the connection is then held open until the test ends). It returns the
// address and a function giving what the request carried: method, request URI, Content-Type and body, so a client's
// requests can be compared across languages without a Go server's own answers in between.
func RawServer(t *testing.T, response []byte, delay time.Duration, close bool) (string, func() map[string]any) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	got := make(chan map[string]any, 1)
	done := make(chan struct{})
	t.Cleanup(func() { close2(done); ln.Close() })
	go func() {
		c, err := ln.Accept()
		if err != nil {
			got <- nil
			return
		}
		defer c.Close()
		c.SetDeadline(time.Now().Add(10 * time.Second))
		r := bufio.NewReader(c)
		tp := textproto.NewReader(r)
		line, _ := tp.ReadLine()
		hdr, _ := tp.ReadMIMEHeader()
		n, _ := strconv.Atoi(hdr.Get("Content-Length"))
		body := make([]byte, min(n, 1<<20))
		io.ReadFull(r, body)
		parts := strings.SplitN(line, " ", 3)
		for len(parts) < 3 {
			parts = append(parts, "")
		}
		got <- map[string]any{"method": parts[0], "uri": parts[1], "contentType": hdr.Get("Content-Type"), "body": Text(string(body))}
		time.Sleep(delay)
		c.Write(response)
		if !close {
			<-done
		}
	}()
	return ln.Addr().String(), func() map[string]any {
		select {
		case g := <-got:
			return g
		case <-time.After(5 * time.Second):
			return nil
		}
	}
}

func close2(c chan struct{}) {
	select {
	case <-c:
	default:
		close(c)
	}
}

// RawUnixServer listens on the unix socket path for one connection per response, in order: each request is read and
// answered with the next response's bytes, then the connection is closed. It returns a function giving the requests
// seen so far (method, request URI, Content-Type, body). A response "@big:N" is a 200 with N spaces of body.
func RawUnixServer(t *testing.T, path string, responses []string) func() []map[string]any {
	t.Helper()
	ln, err := net.Listen("unix", path)
	if err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	var seen []map[string]any
	t.Cleanup(func() { ln.Close() })
	go func() {
		for _, resp := range responses {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			c.SetDeadline(time.Now().Add(10 * time.Second))
			r := bufio.NewReader(c)
			tp := textproto.NewReader(r)
			line, _ := tp.ReadLine()
			hdr, _ := tp.ReadMIMEHeader()
			n, _ := strconv.Atoi(hdr.Get("Content-Length"))
			body := make([]byte, min(n, 1<<20))
			io.ReadFull(r, body)
			parts := strings.SplitN(line, " ", 3)
			for len(parts) < 3 {
				parts = append(parts, "")
			}
			mu.Lock()
			seen = append(seen, map[string]any{"method": parts[0], "uri": parts[1], "contentType": hdr.Get("Content-Type"), "body": Text(string(body))})
			mu.Unlock()
			if rest, ok := strings.CutPrefix(resp, "@big:"); ok {
				n, _ := strconv.Atoi(rest)
				resp = "HTTP/1.1 200 OK\r\nContent-Length: " + rest + "\r\n\r\n" + strings.Repeat(" ", n)
			}
			c.Write([]byte(resp))
			c.Close()
		}
	}()
	return func() []map[string]any {
		mu.Lock()
		defer mu.Unlock()
		return append([]map[string]any(nil), seen...)
	}
}
