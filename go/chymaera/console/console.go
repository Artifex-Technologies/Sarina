// Package console is the client for the Chymaera_Zero_App Sarina control API.
//
// The wire format is owned by that repo (docs/SARINA_API.md): newline-delimited
// JSON over loopback TCP, one request object per line, one response object per
// line. This package implements the client half exactly as specified and does
// not extend it; new commands get proposed on that repo's board first.
//
// The server is off by default and must be started from the app. A refused
// connection normally means nobody turned it on.
package console

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"strconv"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/chymaera/internal/pyfmt"
)

// Defaults: the loopback port the app binds (registered in docs/ECOSYSTEM.md).
const (
	DefaultPort    = 44700
	DefaultHost    = "127.0.0.1"
	DefaultTimeout = 5 * time.Second
)

// Error means the console answered {"ok": false, "error": ...}.
type Error struct {
	Message string
	Command string
}

func (e *Error) Error() string { return e.Command + ": " + e.Message }

// UnavailableError means the console could not be reached at all, or spoke
// something other than the protocol.
type UnavailableError struct{ Msg string }

func (e *UnavailableError) Error() string { return e.Msg }

// Client is one connection to the app's control API. Use Connect/Close to reuse
// a single socket across calls; otherwise each Request opens and closes its own.
type Client struct {
	Host    string
	Port    int
	Token   string
	Timeout time.Duration

	conn   net.Conn
	reader *bufio.Reader
	nextID int
}

// New returns a client with the default host, port and timeout.
func New() *Client {
	return &Client{Host: DefaultHost, Port: DefaultPort, Timeout: DefaultTimeout}
}

func (c *Client) addr() string { return net.JoinHostPort(c.Host, strconv.Itoa(c.Port)) }

// Connect opens the connection if it is not already open.
func (c *Client) Connect() error {
	if c.conn != nil {
		return nil
	}
	timeout := c.Timeout
	if timeout <= 0 {
		timeout = DefaultTimeout
	}
	conn, err := net.DialTimeout("tcp", c.addr(), timeout)
	if err != nil {
		return &UnavailableError{fmt.Sprintf("no Chymaera console at %s:%d (%v) - the app must be running with its Sarina server started", c.Host, c.Port, err)}
	}
	c.conn = conn
	c.reader = bufio.NewReader(conn)
	return nil
}

// Close drops the connection (safe to call when closed).
func (c *Client) Close() {
	if c.conn != nil {
		c.conn.Close()
		c.conn, c.reader = nil, nil
	}
}

// Request sends one command and returns its response payload. Params whose
// value is nil are omitted. It returns *Error when the console reports failure
// and *UnavailableError when the connection drops or is not there.
func (c *Client) Request(cmd string, params map[string]any) (map[string]any, error) {
	owns := c.conn == nil
	if err := c.Connect(); err != nil {
		return nil, err
	}
	if c.nextID == 0 {
		c.nextID = 1
	}
	id := c.nextID
	c.nextID++
	payload := map[string]any{"cmd": cmd, "id": id}
	for k, v := range params {
		if v != nil {
			payload[k] = v
		}
	}
	if c.Token != "" {
		payload["token"] = c.Token
	}
	data, err := json.Marshal(payload)
	if err != nil {
		c.Close()
		return nil, err
	}
	timeout := c.Timeout
	if timeout <= 0 {
		timeout = DefaultTimeout
	}
	c.conn.SetDeadline(time.Now().Add(timeout))
	var line string
	if _, err = c.conn.Write(append(data, '\n')); err == nil {
		line, err = c.reader.ReadString('\n')
		if err == io.EOF {
			err = nil // EOF with no data is reported below as "closed"
		}
	}
	if err != nil {
		c.Close()
		return nil, &UnavailableError{fmt.Sprintf("%s: connection failed (%v)", cmd, err)}
	}
	if line == "" {
		c.Close()
		return nil, &UnavailableError{fmt.Sprintf("%s: console closed the connection without replying", cmd)}
	}
	dec := json.NewDecoder(strings.NewReader(line))
	dec.UseNumber()
	var v any
	derr := dec.Decode(&v)
	if derr == nil {
		var extra any
		if e2 := dec.Decode(&extra); e2 != io.EOF {
			derr = fmt.Errorf("extra data after JSON value")
		}
	}
	if derr != nil {
		c.Close()
		return nil, &UnavailableError{fmt.Sprintf("%s: console sent malformed JSON (%v)", cmd, derr)}
	}
	resp, ok := v.(map[string]any)
	if !ok {
		c.Close()
		return nil, &UnavailableError{fmt.Sprintf("%s: expected a JSON object, got %s", cmd, pyfmt.TypeName(v))}
	}
	if owns {
		c.Close()
	}
	if !pyfmt.Truthy(resp["ok"]) {
		msg := "unknown error"
		if e, present := resp["error"]; present {
			msg = pyfmt.Str(e)
		}
		return nil, &Error{Message: msg, Command: cmd}
	}
	return resp, nil
}

func objects(v any) []map[string]any {
	list, _ := v.([]any)
	out := make([]map[string]any, 0, len(list))
	for _, e := range list {
		if m, ok := e.(map[string]any); ok {
			out = append(out, m)
		}
	}
	return out
}

// Commands: exactly those in the app's SARINA_API.md.

func (c *Client) Ping() (map[string]any, error)   { return c.Request("ping", nil) }
func (c *Client) Status() (map[string]any, error) { return c.Request("status", nil) }

func (c *Client) Help() ([]any, error) {
	r, err := c.Request("help", nil)
	if err != nil {
		return nil, err
	}
	l, _ := r["commands"].([]any)
	return l, nil
}

func (c *Client) LogTail(limit int) ([]map[string]any, error) {
	r, err := c.Request("log.tail", map[string]any{"limit": limit})
	if err != nil {
		return nil, err
	}
	return objects(r["entries"]), nil
}

// LogAdd appends a log line; an empty severity is omitted.
func (c *Client) LogAdd(message, severity string) error {
	p := map[string]any{"message": message}
	if severity != "" {
		p["severity"] = severity
	}
	_, err := c.Request("log.add", p)
	return err
}

// SessionStart returns the sessionId the console reports (nil if absent).
// An empty notes string is omitted.
func (c *Client) SessionStart(name, notes string) (any, error) {
	p := map[string]any{"name": name}
	if notes != "" {
		p["notes"] = notes
	}
	r, err := c.Request("session.start", p)
	if err != nil {
		return nil, err
	}
	return r["sessionId"], nil
}

func (c *Client) SessionEnd() error {
	_, err := c.Request("session.end", nil)
	return err
}

func (c *Client) CaptureList(limit int) ([]map[string]any, error) {
	r, err := c.Request("capture.list", map[string]any{"limit": limit})
	if err != nil {
		return nil, err
	}
	return objects(r["captures"]), nil
}

func (c *Client) CaptureImport(path string) (any, error) {
	r, err := c.Request("capture.import", map[string]any{"path": path})
	if err != nil {
		return nil, err
	}
	return r["summary"], nil
}

// RenderStatus renders a console status summary for humans.
func RenderStatus(status map[string]any, captures []map[string]any) string {
	lines := []string{"# Chymaera console", ""}
	for _, key := range []string{"version", "serverRunning", "serverPort", "sessionActive",
		"sessionName", "datastorePath", "eventCount"} {
		if v, ok := status[key]; ok {
			lines = append(lines, key+": "+pyfmt.Str(v))
		}
	}
	lines = append(lines, "")
	if len(captures) == 0 {
		lines = append(lines, "No captures recorded.")
		return strings.Join(lines, "\n")
	}
	lines = append(lines, fmt.Sprintf("## Captures (%d)", len(captures)))
	for _, c := range captures {
		get := func(k string) string {
			if v, ok := c[k]; ok {
				return pyfmt.Str(v)
			}
			return "?"
		}
		lines = append(lines, fmt.Sprintf("- [%s] %s (%s bytes)", get("type"), get("name"), get("size")))
	}
	return strings.Join(lines, "\n")
}
