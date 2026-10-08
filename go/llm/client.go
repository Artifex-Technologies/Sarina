package llm

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync/atomic"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// Client talks to an OpenAI-compatible /chat/completions endpoint.
type Client struct {
	Config     config.ModelConfig
	HTTPClient *http.Client  // optional
	RetryDelay time.Duration // delay before the single retry; 0 = 750ms
}

// NewClient returns a client for cfg.
func NewClient(cfg config.ModelConfig) *Client { return &Client{Config: cfg} }

func (c *Client) endpoint() string {
	return strings.TrimRight(c.Config.BaseURL, "/") + "/chat/completions"
}

type streamOptions struct {
	IncludeUsage bool `json:"include_usage"`
}

type chatRequest struct {
	Model          string         `json:"model"`
	Messages       []Message      `json:"messages"`
	Temperature    float64        `json:"temperature"`
	Stream         bool           `json:"stream"`
	StreamOptions  *streamOptions `json:"stream_options,omitempty"`
	Tools          []ToolSpec     `json:"tools,omitempty"`
	ToolChoice     string         `json:"tool_choice,omitempty"`
	ResponseFormat any            `json:"response_format,omitempty"`
	MaxTokens      int            `json:"max_tokens,omitempty"`
}

// BuildBody builds the JSON request body. tools and tool_choice are omitted
// when there are no tools.
func (c *Client) BuildBody(req Request) ([]byte, error) {
	msgs := req.Messages
	if msgs == nil {
		msgs = []Message{}
	}
	body := chatRequest{
		Model:       c.Config.Model,
		Messages:    msgs,
		Temperature: c.Config.Temperature,
		Stream:      req.Stream,
		MaxTokens:   req.MaxTokens,
	}
	if req.Stream {
		body.StreamOptions = &streamOptions{IncludeUsage: true}
	}
	if len(req.Tools) > 0 {
		body.Tools = req.Tools
		body.ToolChoice = "auto"
	}
	if req.Schema != nil {
		body.ResponseFormat = map[string]any{
			"type": "json_schema",
			"json_schema": map[string]any{
				"name": req.Schema.Name, "schema": req.Schema.Schema, "strict": req.Schema.Strict,
			},
		}
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(body); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// Complete performs one model call. When req.Stream is set, on (if non-nil)
// receives each stream event as it arrives. Cancellation of ctx is returned as
// ctx.Err(); every other failure is a *BackendError. One retry is made for
// connection failures, 429 and 5xx before any response data was consumed.
func (c *Client) Complete(ctx context.Context, req Request, on func(StreamEvent)) (*Turn, error) {
	body, err := c.BuildBody(req)
	if err != nil {
		return nil, &BackendError{Message: "cannot encode request: " + err.Error()}
	}
	var lastErr error
	for attempt := 0; attempt < 2; attempt++ {
		turn, retryable, err := c.once(ctx, body, req, on)
		if err == nil {
			Normalize(turn, toolNames(req.Tools))
			return turn, nil
		}
		lastErr = err
		if !retryable || ctx.Err() != nil {
			break
		}
		delay := c.RetryDelay
		if delay == 0 {
			delay = 750 * time.Millisecond
		}
		select {
		case <-time.After(delay):
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
	return nil, lastErr
}

func toolNames(specs []ToolSpec) []string {
	names := make([]string, 0, len(specs))
	for _, s := range specs {
		names = append(names, s.Name)
	}
	return names
}

func (c *Client) once(ctx context.Context, body []byte, req Request, on func(StreamEvent)) (*Turn, bool, error) {
	timeout := time.Duration(c.Config.TimeoutSeconds * float64(time.Second))
	if timeout <= 0 {
		timeout = time.Duration(config.DefaultTimeoutSeconds * float64(time.Second))
	}
	rctx, cancel := context.WithCancel(ctx)
	defer cancel()
	var timedOut atomic.Bool
	timer := time.AfterFunc(timeout, func() { timedOut.Store(true); cancel() })
	defer timer.Stop()

	fail := func(err error, retryable bool) (*Turn, bool, error) {
		if ctx.Err() != nil {
			return nil, false, ctx.Err()
		}
		if timedOut.Load() {
			return nil, false, &BackendError{Message: fmt.Sprintf("request to %s timed out after %s", c.endpoint(), timeout)}
		}
		var be *BackendError
		if errors.As(err, &be) {
			return nil, retryable, err
		}
		return nil, retryable, &BackendError{Message: fmt.Sprintf("cannot reach backend at %s: %v", c.Config.BaseURL, err)}
	}

	httpReq, err := http.NewRequestWithContext(rctx, http.MethodPost, c.endpoint(), bytes.NewReader(body))
	if err != nil {
		return nil, false, &BackendError{Message: "bad backend URL: " + err.Error()}
	}
	httpReq.Header.Set("Authorization", "Bearer "+c.Config.APIKey)
	httpReq.Header.Set("Content-Type", "application/json")
	hc := c.HTTPClient
	if hc == nil {
		hc = http.DefaultClient
	}
	resp, err := hc.Do(httpReq)
	if err != nil {
		return fail(err, true)
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 64<<10))
		be := &BackendError{Status: resp.StatusCode,
			Message: fmt.Sprintf("backend returned HTTP %d: %s", resp.StatusCode, strings.ToValidUTF8(string(b), "�"))}
		return fail(be, resp.StatusCode >= 500 || resp.StatusCode == 429)
	}
	if req.Stream {
		turn, err := readStream(resp.Body, func() { timer.Reset(timeout) }, on)
		if err != nil {
			return fail(err, false)
		}
		return turn, false, nil
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, 256<<20))
	if err != nil {
		return fail(err, false)
	}
	turn, err := parseResponse(b)
	if err != nil {
		return nil, false, err
	}
	return turn, false, nil
}

// ---- non-streaming --------------------------------------------------------

func errorText(v any) string {
	switch e := v.(type) {
	case string:
		return e
	case map[string]any:
		if m, ok := e["message"].(string); ok {
			return m
		}
	}
	b, _ := json.Marshal(v)
	return string(b)
}

func flattenContent(v any) string {
	switch c := v.(type) {
	case nil:
		return ""
	case string:
		return c
	case []any:
		var b strings.Builder
		for _, item := range c {
			switch it := item.(type) {
			case string:
				b.WriteString(it)
			case map[string]any:
				if s, ok := it["text"].(string); ok {
					b.WriteString(s)
				} else {
					b.Write(compactASCII(it))
				}
			default:
				b.Write(compactASCII(it))
			}
		}
		return b.String()
	default:
		return fmt.Sprint(c)
	}
}

func parseResponse(b []byte) (*Turn, error) {
	var root any
	if err := json.Unmarshal(b, &root); err != nil {
		return nil, &BackendError{Message: "backend returned invalid JSON: " + err.Error()}
	}
	obj, ok := root.(map[string]any)
	if !ok {
		return nil, &BackendError{Message: "backend returned a malformed JSON payload (not an object)"}
	}
	choices, _ := obj["choices"].([]any)
	if len(choices) == 0 {
		if e, has := obj["error"]; has {
			return nil, &BackendError{Message: "backend error: " + errorText(e)}
		}
		return nil, &BackendError{Message: "backend response contained no choices"}
	}
	choice, ok := choices[0].(map[string]any)
	if !ok {
		return nil, &BackendError{Message: "backend response had a malformed choice"}
	}
	msg, ok := choice["message"].(map[string]any)
	if !ok {
		return nil, &BackendError{Message: "backend response had no assistant message"}
	}
	t := &Turn{Content: flattenContent(msg["content"]), Raw: msg}
	for _, key := range []string{"reasoning_content", "reasoning"} {
		if s, ok := msg[key].(string); ok && s != "" {
			t.Reasoning = s
			break
		}
	}
	switch f := choice["finish_reason"].(type) {
	case nil:
	case string:
		t.FinishReason, t.HasFinishReason = f, true
	default:
		t.FinishReason, t.HasFinishReason = fmt.Sprint(f), true
	}
	t.Usage = UsageFromProvider(obj["usage"])
	t.UsageReported = !t.Usage.IsZero()

	if list, ok := msg["tool_calls"].([]any); ok {
		for _, item := range list {
			m, ok := item.(map[string]any)
			if !ok {
				return nil, &BackendError{Message: "backend returned a malformed tool call"}
			}
			fn, _ := m["function"].(map[string]any)
			if fn == nil {
				fn = map[string]any{}
			}
			name, _ := fn["name"].(string)
			if name == "" {
				return nil, &BackendError{Message: "tool call missing function name"}
			}
			id, _ := m["id"].(string)
			t.ToolCalls = append(t.ToolCalls, makeCall(id, name, fn["arguments"]))
		}
	} else if fc, ok := msg["function_call"].(map[string]any); ok {
		name, _ := fc["name"].(string)
		if name == "" {
			return nil, &BackendError{Message: "tool call missing function name"}
		}
		t.ToolCalls = append(t.ToolCalls, makeCall("call_0", name, fc["arguments"]))
	}
	return t, nil
}

func makeCall(id, name string, args any) ToolCall {
	c := ToolCall{ID: id, Name: name, Arguments: map[string]any{}}
	switch a := args.(type) {
	case nil:
	case map[string]any:
		c.Arguments = a
	case string:
		c.RawArguments = a
		parsed, trunc, err := ParseArguments(a)
		if err != "" {
			c.ParseError = err
		} else {
			c.Arguments, c.truncated = parsed, trunc
		}
	default:
		c.ParseError = "arguments must be a JSON object"
	}
	return c
}

// ---- streaming ------------------------------------------------------------

type fragCall struct {
	id, name string
	args     strings.Builder
}

type streamAcc struct {
	content, reasoning strings.Builder
	slots              []*fragCall
	byIndex            map[int]int
	finish             string
	hasFinish          bool
	usage              Usage
	payloads           int
}

func (a *streamAcc) merge(index int, id, name, args string) {
	pos, exists := a.byIndex[index]
	if exists && id != "" && a.slots[pos].id != "" && a.slots[pos].id != id {
		exists = false // some servers reuse index 0 for every parallel call
	}
	if !exists {
		a.slots = append(a.slots, &fragCall{})
		pos = len(a.slots) - 1
		a.byIndex[index] = pos
	}
	s := a.slots[pos]
	if id != "" {
		s.id = id
	}
	switch {
	case name == "":
	case s.name == "" || strings.HasPrefix(name, s.name):
		s.name = name
	case name == s.name:
	default:
		s.name += name
	}
	s.args.WriteString(args)
}

func readStream(r io.Reader, touch func(), on func(StreamEvent)) (*Turn, error) {
	emit := func(e StreamEvent) {
		if on != nil {
			on(e)
		}
	}
	acc := &streamAcc{byIndex: map[int]int{}}
	emit(StreamEvent{Type: "message_start"})
	br := bufio.NewReaderSize(r, 64<<10)
	var data []string
	done := false

	flush := func() error {
		if len(data) == 0 {
			return nil
		}
		payload := strings.TrimSpace(strings.Join(data, "\n"))
		data = nil
		if payload == "[DONE]" {
			done = true
			return nil
		}
		var v any
		if err := json.Unmarshal([]byte(payload), &v); err != nil {
			return &BackendError{Message: "invalid JSON in streaming response"}
		}
		m, ok := v.(map[string]any)
		if !ok {
			return &BackendError{Message: "invalid JSON in streaming response (not an object)"}
		}
		return acc.process(m, emit)
	}

	for !done {
		line, err := br.ReadString('\n')
		if line != "" {
			touch()
			line = strings.TrimRight(line, "\r\n")
			switch {
			case line == "":
				if ferr := flush(); ferr != nil {
					return nil, ferr
				}
			case strings.HasPrefix(line, "data:"):
				data = append(data, strings.TrimPrefix(line, "data:"))
			}
		}
		if err != nil {
			if err == io.EOF {
				break
			}
			return nil, err
		}
	}
	if err := flush(); err != nil {
		return nil, err
	}
	if acc.payloads == 0 {
		return nil, &BackendError{Message: "backend returned an empty stream"}
	}

	t := &Turn{
		Content: acc.content.String(), Reasoning: acc.reasoning.String(),
		FinishReason: acc.finish, HasFinishReason: acc.hasFinish,
		Usage: acc.usage, UsageReported: !acc.usage.IsZero(),
	}
	for _, s := range acc.slots {
		if s.name == "" {
			return nil, &BackendError{Message: "tool call missing function name"}
		}
		t.ToolCalls = append(t.ToolCalls, makeCall(s.id, s.name, s.args.String()))
	}
	return t, nil
}

func (a *streamAcc) process(m map[string]any, emit func(StreamEvent)) error {
	a.payloads++
	if _, hasChoices := m["choices"]; !hasChoices {
		if e, has := m["error"]; has {
			return &BackendError{Message: "backend error: " + errorText(e)}
		}
	}
	if u := UsageFromProvider(m["usage"]); !u.IsZero() {
		a.usage = u // servers repeat cumulative usage; keep the last, do not sum
		uc := u
		emit(StreamEvent{Type: "usage", Usage: &uc})
	}
	choices, _ := m["choices"].([]any)
	for _, ch := range choices {
		cm, ok := ch.(map[string]any)
		if !ok {
			continue
		}
		if d, ok := cm["delta"].(map[string]any); ok {
			if s, ok := d["content"].(string); ok && s != "" {
				a.content.WriteString(s)
				emit(StreamEvent{Type: "content_delta", Delta: s})
			}
			for _, key := range []string{"reasoning_content", "reasoning"} {
				if s, ok := d[key].(string); ok && s != "" {
					a.reasoning.WriteString(s)
					break
				}
			}
			if tcs, ok := d["tool_calls"].([]any); ok {
				for _, tc := range tcs {
					tm, ok := tc.(map[string]any)
					if !ok {
						continue
					}
					idx, _ := config.AsInt(tm["index"])
					if _, isBool := tm["index"].(bool); isBool {
						idx = 0
					}
					id, _ := tm["id"].(string)
					fn, _ := tm["function"].(map[string]any)
					name, _ := fn["name"].(string)
					args, _ := fn["arguments"].(string)
					a.merge(idx, id, name, args)
					emit(StreamEvent{Type: "tool_call_delta", ToolCallIndex: idx, ToolCallID: id, ToolName: name, ArgumentsDelta: args})
				}
			}
		}
		switch f := cm["finish_reason"].(type) {
		case nil:
		case string:
			a.finish, a.hasFinish = f, true
			emit(StreamEvent{Type: "message_stop", FinishReason: f})
		default:
			s := fmt.Sprint(f)
			a.finish, a.hasFinish = s, true
			emit(StreamEvent{Type: "message_stop", FinishReason: s})
		}
	}
	return nil
}
