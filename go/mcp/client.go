package mcp

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// ProtocolVersion is what the client announces in initialize. The server's
// answer is not negotiated further; this client only uses the stable subset
// (tools and resources).
const ProtocolVersion = "2025-06-18"

const (
	// MaxLineBytes bounds one JSON-RPC line from a server; longer lines are dropped.
	MaxLineBytes = 8 << 20
	// stderrKeep is how much of a server's stderr is remembered for errors.
	stderrKeep = 2048
)

type rpcError struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data,omitempty"`
}

type rpcMessage struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      json.RawMessage `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *rpcError       `json:"error,omitempty"`
}

// Client is one connection to one server: JSON-RPC 2.0 messages, one per line.
// It is safe for concurrent use.
type Client struct {
	name string
	w    io.WriteCloser
	wmu  sync.Mutex

	mu      sync.Mutex
	pending map[int64]chan *rpcMessage
	next    int64

	closed chan struct{} // closed when the server's output ends
	cmd    *exec.Cmd     // nil for a pipe-backed client (tests)
	exited chan struct{} // closed once the process has been reaped
	once   sync.Once

	tailMu sync.Mutex
	tail   string
}

// newClient wraps a reader/writer pair and starts the read loop.
func newClient(name string, r io.Reader, w io.WriteCloser) *Client {
	c := &Client{name: name, w: w, pending: map[int64]chan *rpcMessage{}, closed: make(chan struct{}), exited: make(chan struct{})}
	go c.readLoop(r)
	return c
}

// NewPipeClient connects to a server over an existing pipe pair (no process).
// Close closes w and waits briefly for r to end. For tests and embedding.
func NewPipeClient(name string, r io.Reader, w io.WriteCloser) *Client {
	c := newClient(name, r, w)
	close(c.exited)
	return c
}

// Spawn starts def's command and connects to it. env is the complete child
// environment (already filtered); the definition's own env overlay is applied
// on top of it.
func Spawn(def ServerDef, env []string) (*Client, error) {
	cmd := exec.Command(def.Command, def.Args...)
	cmd.Dir = def.Cwd
	cmd.Env = overlayEnv(env, def.Env)
	tools.SetProcAttrs(cmd)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("cannot start MCP server %q (%s): %v", def.Name, def.Command, err)
	}
	c := newClient(def.Name, stdout, stdin)
	c.cmd = cmd
	errDone := make(chan struct{})
	go func() {
		defer close(errDone)
		buf := make([]byte, 1024)
		for {
			n, err := stderr.Read(buf)
			if n > 0 {
				c.addTail(string(buf[:n]))
			}
			if err != nil {
				return
			}
		}
	}()
	go func() {
		<-c.closed
		<-errDone
		_ = cmd.Wait()
		close(c.exited)
	}()
	return c, nil
}

func overlayEnv(base []string, extra map[string]string) []string {
	if len(extra) == 0 {
		return base
	}
	out := make([]string, 0, len(base)+len(extra))
	for _, kv := range base {
		k := kv
		if i := strings.IndexByte(kv, '='); i > 0 {
			k = kv[:i]
		}
		if _, over := extra[k]; !over {
			out = append(out, kv)
		}
	}
	for k, v := range extra {
		out = append(out, k+"="+v)
	}
	return out
}

func (c *Client) addTail(s string) {
	c.tailMu.Lock()
	defer c.tailMu.Unlock()
	c.tail += s
	if len(c.tail) > stderrKeep {
		c.tail = c.tail[len(c.tail)-stderrKeep:]
	}
}

// StderrTail is the end of the server's stderr, for error messages.
func (c *Client) StderrTail() string {
	c.tailMu.Lock()
	defer c.tailMu.Unlock()
	return strings.TrimSpace(c.tail)
}

// Alive reports whether the server's output is still open.
func (c *Client) Alive() bool {
	select {
	case <-c.closed:
		return false
	default:
		return true
	}
}

func (c *Client) readLoop(r io.Reader) {
	defer func() {
		close(c.closed)
		c.mu.Lock()
		for id, ch := range c.pending {
			delete(c.pending, id)
			close(ch)
		}
		c.mu.Unlock()
	}()
	br := bufio.NewReaderSize(r, 64<<10)
	var line []byte
	dropping := false
	for {
		chunk, more, err := br.ReadLine()
		if err != nil {
			return
		}
		if !dropping {
			line = append(line, chunk...)
			if len(line) > MaxLineBytes {
				dropping, line = true, nil
			}
		}
		if more {
			continue
		}
		if !dropping && len(line) > 0 {
			c.handle(line)
		}
		line, dropping = nil, false
	}
}

func (c *Client) handle(line []byte) {
	var m rpcMessage
	if json.Unmarshal(line, &m) != nil {
		return // not a JSON-RPC object: ignore, as servers may log to stdout
	}
	switch {
	case m.Method != "" && len(m.ID) > 0:
		// A request from the server. The client offers no capabilities, so the only
		// thing worth answering is ping. The answer is written from its own
		// goroutine: the read loop must never block on a write, or a server that
		// is itself blocked writing to us would deadlock both ends.
		id := json.RawMessage(append([]byte(nil), m.ID...))
		if m.Method == "ping" {
			go c.send(map[string]any{"jsonrpc": "2.0", "id": id, "result": map[string]any{}})
		} else {
			go c.send(map[string]any{"jsonrpc": "2.0", "id": id,
				"error": map[string]any{"code": -32601, "message": "method not supported by this client"}})
		}
	case m.Method != "":
		// notification: nothing to do
	case len(m.ID) > 0:
		id, ok := parseID(m.ID)
		if !ok {
			return
		}
		c.mu.Lock()
		ch := c.pending[id]
		delete(c.pending, id)
		c.mu.Unlock()
		if ch != nil {
			ch <- &m
		}
	}
}

func parseID(raw json.RawMessage) (int64, bool) {
	s := strings.Trim(string(raw), `" `)
	n, err := strconv.ParseInt(s, 10, 64)
	return n, err == nil
}

func (c *Client) send(v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	c.wmu.Lock()
	defer c.wmu.Unlock()
	_, err = c.w.Write(append(b, '\n'))
	return err
}

// Call sends one request and waits for its answer. A JSON-RPC error becomes an
// error; a timeout (or cancelled ctx) sends notifications/cancelled and leaves
// the connection usable.
func (c *Client) Call(ctx context.Context, method string, params any, timeout time.Duration) (json.RawMessage, error) {
	c.mu.Lock()
	c.next++
	id := c.next
	ch := make(chan *rpcMessage, 1)
	c.pending[id] = ch
	c.mu.Unlock()
	forget := func() {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
	}
	req := map[string]any{"jsonrpc": "2.0", "id": id, "method": method}
	if params != nil {
		req["params"] = params
	}
	// The write runs on its own goroutine so a server that stopped reading its
	// stdin cannot make this call outlive its timeout.
	sent := make(chan error, 1)
	go func() { sent <- c.send(req) }()
	var timer <-chan time.Time
	if timeout > 0 {
		t := time.NewTimer(timeout)
		defer t.Stop()
		timer = t.C
	}
	for {
		select {
		case err := <-sent:
			sent = nil
			if err != nil {
				forget()
				return nil, c.failure("could not send %s: %v", method, err)
			}
		case m, ok := <-ch:
			if !ok {
				return nil, c.failure("server exited during %s", method)
			}
			if m.Error != nil {
				return nil, fmt.Errorf("MCP %s failed on server %q: %s (code %d)", method, c.name, m.Error.Message, m.Error.Code)
			}
			return m.Result, nil
		case <-timer:
			forget()
			c.cancel(id)
			return nil, c.failure("no answer to %s within %s", method, timeout)
		case <-ctx.Done():
			forget()
			c.cancel(id)
			return nil, ctx.Err()
		}
	}
}

func (c *Client) cancel(id int64) {
	go func() { // best effort; never block the caller on a server that is not reading
		_ = c.Notify("notifications/cancelled", map[string]any{"requestId": id, "reason": "client gave up"})
	}()
}

func (c *Client) failure(format string, a ...any) error {
	msg := fmt.Sprintf(format, a...)
	if t := c.StderrTail(); t != "" {
		msg += " (server stderr: " + lastLines(t, 5) + ")"
	}
	return fmt.Errorf("MCP server %q: %s", c.name, msg)
}

func lastLines(s string, n int) string {
	lines := strings.Split(strings.TrimSpace(s), "\n")
	if len(lines) > n {
		lines = lines[len(lines)-n:]
	}
	return strings.Join(lines, " | ")
}

// Notify sends a notification (no answer expected).
func (c *Client) Notify(method string, params any) error {
	m := map[string]any{"jsonrpc": "2.0", "method": method}
	if params != nil {
		m["params"] = params
	}
	return c.send(m)
}

// Initialize performs the handshake.
func (c *Client) Initialize(ctx context.Context, timeout time.Duration) error {
	_, err := c.Call(ctx, "initialize", map[string]any{
		"protocolVersion": ProtocolVersion,
		"capabilities":    map[string]any{},
		"clientInfo":      map[string]any{"name": "sarina", "version": "0.1.0"},
	}, timeout)
	if err != nil {
		return err
	}
	return c.Notify("notifications/initialized", nil)
}

// Close shuts the connection down: close the server's stdin, give it a second
// to exit by itself, then kill the process tree.
func (c *Client) Close() {
	c.once.Do(func() {
		_ = c.w.Close()
		select {
		case <-c.closed: // the server saw the end of its input and left
		case <-time.After(time.Second):
			if c.cmd != nil && c.cmd.Process != nil {
				tools.KillTree(c.cmd.Process.Pid)
				_ = c.cmd.Process.Kill()
			}
		}
		select {
		case <-c.exited:
		case <-time.After(3 * time.Second):
		}
	})
}

var errNoResult = errors.New("empty result")

// ---- typed requests ----------------------------------------------------------

// ToolInfo is one tool a server offers.
type ToolInfo struct {
	Name        string
	Description string
	InputSchema map[string]any
}

// ResourceInfo is one resource a server offers.
type ResourceInfo struct {
	URI         string
	Name        string
	Description string
	MimeType    string
}

const maxPages = 10

// ListTools follows pagination (bounded).
func (c *Client) ListTools(ctx context.Context, timeout time.Duration) ([]ToolInfo, error) {
	var out []ToolInfo
	cursor := ""
	for page := 0; page < maxPages; page++ {
		var params any
		if cursor != "" {
			params = map[string]any{"cursor": cursor}
		}
		raw, err := c.Call(ctx, "tools/list", params, timeout)
		if err != nil {
			return out, err
		}
		var res struct {
			Tools []struct {
				Name        string         `json:"name"`
				Description string         `json:"description"`
				InputSchema map[string]any `json:"inputSchema"`
			} `json:"tools"`
			NextCursor string `json:"nextCursor"`
		}
		if json.Unmarshal(raw, &res) != nil {
			return out, fmt.Errorf("MCP server %q returned a tools/list result that is not an object", c.name)
		}
		for _, t := range res.Tools {
			if t.Name != "" {
				out = append(out, ToolInfo{t.Name, t.Description, t.InputSchema})
			}
		}
		if res.NextCursor == "" {
			break
		}
		cursor = res.NextCursor
	}
	return out, nil
}

// ListResources follows pagination (bounded).
func (c *Client) ListResources(ctx context.Context, timeout time.Duration) ([]ResourceInfo, error) {
	var out []ResourceInfo
	cursor := ""
	for page := 0; page < maxPages; page++ {
		var params any
		if cursor != "" {
			params = map[string]any{"cursor": cursor}
		}
		raw, err := c.Call(ctx, "resources/list", params, timeout)
		if err != nil {
			return out, err
		}
		var res struct {
			Resources []struct {
				URI         string `json:"uri"`
				Name        string `json:"name"`
				Description string `json:"description"`
				MimeType    string `json:"mimeType"`
			} `json:"resources"`
			NextCursor string `json:"nextCursor"`
		}
		if json.Unmarshal(raw, &res) != nil {
			return out, fmt.Errorf("MCP server %q returned a resources/list result that is not an object", c.name)
		}
		for _, r := range res.Resources {
			if r.URI != "" {
				out = append(out, ResourceInfo{r.URI, r.Name, r.Description, r.MimeType})
			}
		}
		if res.NextCursor == "" {
			break
		}
		cursor = res.NextCursor
	}
	return out, nil
}

// CallResult is a tools/call answer, flattened to text.
type CallResult struct {
	Text    string
	IsError bool
}

// CallTool runs a tool and renders its content as text.
func (c *Client) CallTool(ctx context.Context, name string, args map[string]any, timeout time.Duration) (CallResult, error) {
	if args == nil {
		args = map[string]any{}
	}
	raw, err := c.Call(ctx, "tools/call", map[string]any{"name": name, "arguments": args}, timeout)
	if err != nil {
		return CallResult{}, err
	}
	var res struct {
		Content           []map[string]any `json:"content"`
		StructuredContent any              `json:"structuredContent"`
		IsError           bool             `json:"isError"`
	}
	if json.Unmarshal(raw, &res) != nil {
		return CallResult{}, fmt.Errorf("MCP server %q returned a tools/call result that is not an object", c.name)
	}
	var parts []string
	for _, item := range res.Content {
		parts = append(parts, renderContent(item))
	}
	if res.StructuredContent != nil {
		if b, err := json.MarshalIndent(res.StructuredContent, "", "  "); err == nil {
			parts = append(parts, string(b))
		}
	}
	text := strings.Join(parts, "\n\n")
	if strings.TrimSpace(text) == "" {
		text = "(the tool returned no content)"
	}
	return CallResult{Text: text, IsError: res.IsError}, nil
}

// ReadResource returns a resource's contents as text.
func (c *Client) ReadResource(ctx context.Context, uri string, timeout time.Duration) (string, error) {
	raw, err := c.Call(ctx, "resources/read", map[string]any{"uri": uri}, timeout)
	if err != nil {
		return "", err
	}
	var res struct {
		Contents []map[string]any `json:"contents"`
	}
	if json.Unmarshal(raw, &res) != nil {
		return "", errNoResult
	}
	var parts []string
	for _, item := range res.Contents {
		parts = append(parts, renderContent(item))
	}
	return strings.Join(parts, "\n\n"), nil
}

// renderContent turns one content item into text. Binary payloads are not
// inlined: the model gets a placeholder with the type and size.
func renderContent(item map[string]any) string {
	if s, ok := item["text"].(string); ok {
		return s
	}
	typ, _ := item["type"].(string)
	mime, _ := item["mimeType"].(string)
	if blob, ok := item["blob"].(string); ok {
		return fmt.Sprintf("[binary content omitted: %s, %d base64 characters]", orDefault(mime, "unknown type"), len(blob))
	}
	if data, ok := item["data"].(string); ok {
		return fmt.Sprintf("[%s content omitted: %s, %d base64 characters]", orDefault(typ, "binary"), orDefault(mime, "unknown type"), len(data))
	}
	if res, ok := item["resource"].(map[string]any); ok {
		return renderContent(res)
	}
	if uri, ok := item["uri"].(string); ok {
		return "[resource " + uri + "]"
	}
	b, _ := json.Marshal(item)
	return string(b)
}

func orDefault(s, d string) string {
	if s == "" {
		return d
	}
	return s
}
