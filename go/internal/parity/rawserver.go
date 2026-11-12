package parity

import (
	"bufio"
	"io"
	"net"
	"net/textproto"
	"strconv"
	"strings"
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
