package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
)

// DefaultAnthropicURL is the public Messages API host.
const DefaultAnthropicURL = "https://api.anthropic.com"

const anthropicVersion = "2023-06-01"

// defaultAnthropicMaxTokens is used because the Messages API requires max_tokens.
const defaultAnthropicMaxTokens = 8192

// AnthropicClient talks to the Anthropic Messages API and presents the same
// Complete method as Client, translating the OpenAI-shaped Request (system
// messages, tool_calls, tool results) to Messages blocks and the reply back.
// The API key lives only in Config.APIKey (which is never serialised) and is
// scrubbed from every error message.
type AnthropicClient struct {
	Config     config.ModelConfig
	HTTPClient *http.Client
	RetryDelay time.Duration
}

// NewAnthropicClient returns a client for cfg.
func NewAnthropicClient(cfg config.ModelConfig) *AnthropicClient {
	return &AnthropicClient{Config: cfg}
}

func (c *AnthropicClient) base() string {
	b := strings.TrimRight(c.Config.BaseURL, "/")
	if b == "" {
		b = DefaultAnthropicURL
	}
	return b
}

func (c *AnthropicClient) endpoint() string {
	b := c.base()
	if strings.HasSuffix(b, "/v1") {
		return b + "/messages"
	}
	return b + "/v1/messages"
}

// ModelsEndpoint is the cheap, free URL a health check can GET.
func (c *AnthropicClient) ModelsEndpoint() string {
	b := c.base()
	if strings.HasSuffix(b, "/v1") {
		return b + "/models"
	}
	return b + "/v1/models"
}

// SetHeaders adds the authentication and version headers.
func (c *AnthropicClient) SetHeaders(h http.Header) {
	h.Set("x-api-key", c.Config.APIKey)
	h.Set("anthropic-version", anthropicVersion)
}

func (c *AnthropicClient) scrub(s string) string {
	if k := c.Config.APIKey; k != "" {
		s = strings.ReplaceAll(s, k, "[redacted-key]")
	}
	return s
}

var anthropicIDBad = regexp.MustCompile(`[^A-Za-z0-9_-]`)

// anthropicID maps any tool-call id to the character set the API accepts. The
// same function is applied to a call and its result, so pairs stay matched.
func anthropicID(id string) string {
	id = anthropicIDBad.ReplaceAllString(id, "_")
	if id == "" {
		id = "call"
	}
	return id
}

type anthropicMessage struct {
	Role    string           `json:"role"`
	Content []map[string]any `json:"content"`
}

type anthropicRequest struct {
	Model       string             `json:"model"`
	MaxTokens   int                `json:"max_tokens"`
	System      string             `json:"system,omitempty"`
	Messages    []anthropicMessage `json:"messages"`
	Temperature *float64           `json:"temperature,omitempty"`
	Tools       []map[string]any   `json:"tools,omitempty"`
	ToolChoice  map[string]any     `json:"tool_choice,omitempty"`
}

// BuildBody translates req into a Messages API body.
func (c *AnthropicClient) BuildBody(req Request) ([]byte, error) {
	var system []string
	var msgs []anthropicMessage
	push := func(role string, blocks ...map[string]any) {
		if len(blocks) == 0 {
			return
		}
		if n := len(msgs); n > 0 && msgs[n-1].Role == role {
			msgs[n-1].Content = append(msgs[n-1].Content, blocks...)
			return
		}
		msgs = append(msgs, anthropicMessage{Role: role, Content: blocks})
	}
	text := func(s string) map[string]any { return map[string]any{"type": "text", "text": s} }
	for _, m := range req.Messages {
		switch m.Role {
		case "system":
			if strings.TrimSpace(m.Content) != "" {
				system = append(system, m.Content)
			}
		case "assistant":
			var blocks []map[string]any
			if strings.TrimSpace(m.Content) != "" {
				blocks = append(blocks, text(m.Content))
			}
			for _, tc := range m.ToolCalls {
				input := map[string]any{}
				if err := json.Unmarshal([]byte(tc.Function.Arguments), &input); err != nil || input == nil {
					input = map[string]any{}
				}
				blocks = append(blocks, map[string]any{"type": "tool_use", "id": anthropicID(tc.ID),
					"name": tc.Function.Name, "input": input})
			}
			push("assistant", blocks...)
		case "tool":
			content := m.Content
			if strings.TrimSpace(content) == "" {
				content = "(no output)"
			}
			push("user", map[string]any{"type": "tool_result", "tool_use_id": anthropicID(m.ToolCallID), "content": content})
		default: // user and anything unknown
			if strings.TrimSpace(m.Content) != "" {
				push("user", text(m.Content))
			}
		}
	}
	if len(msgs) == 0 || msgs[0].Role != "user" {
		msgs = append([]anthropicMessage{{Role: "user", Content: []map[string]any{text("(continuing the conversation)")}}}, msgs...)
	}
	if req.Schema != nil {
		raw, _ := json.Marshal(req.Schema.Schema)
		system = append(system, fmt.Sprintf("Reply with a single JSON document and nothing else. It must conform to this JSON Schema (named %s): %s",
			req.Schema.Name, raw))
	}
	body := anthropicRequest{Model: c.Config.Model, MaxTokens: req.MaxTokens, Messages: msgs,
		System: strings.Join(system, "\n\n")}
	if body.MaxTokens <= 0 {
		body.MaxTokens = defaultAnthropicMaxTokens
	}
	if c.Config.Temperature > 0 {
		t := c.Config.Temperature
		body.Temperature = &t
	}
	for _, t := range req.Tools {
		params := t.Parameters
		if params == nil {
			params = map[string]any{"type": "object", "properties": map[string]any{}}
		}
		body.Tools = append(body.Tools, map[string]any{"name": t.Name, "description": t.Description, "input_schema": params})
	}
	if len(body.Tools) > 0 {
		body.ToolChoice = map[string]any{"type": "auto"}
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(body); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// Complete performs one model call. Streaming is not used on the wire; with
// req.Stream set the finished reply is replayed to on as a short event sequence
// so callers see the same shape as from Client. One retry for connection
// failures, 429 and 5xx; cancellation returns ctx.Err(); anything else is a
// *BackendError whose text never contains the API key.
func (c *AnthropicClient) Complete(ctx context.Context, req Request, on func(StreamEvent)) (*Turn, error) {
	body, err := c.BuildBody(req)
	if err != nil {
		return nil, &BackendError{Message: "cannot encode request: " + err.Error()}
	}
	var lastErr error
	for attempt := 0; attempt < 2; attempt++ {
		turn, retryable, err := c.once(ctx, body)
		if err == nil {
			if req.Stream && on != nil {
				replay(turn, on)
			}
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

func replay(t *Turn, on func(StreamEvent)) {
	on(StreamEvent{Type: "message_start"})
	if t.Content != "" {
		on(StreamEvent{Type: "content_delta", Delta: t.Content})
	}
	if t.UsageReported {
		u := t.Usage
		on(StreamEvent{Type: "usage", Usage: &u})
	}
	on(StreamEvent{Type: "message_stop", FinishReason: t.FinishReason})
}

func (c *AnthropicClient) once(ctx context.Context, body []byte) (*Turn, bool, error) {
	timeout := time.Duration(c.Config.TimeoutSeconds * float64(time.Second))
	if timeout <= 0 {
		timeout = time.Duration(config.DefaultTimeoutSeconds * float64(time.Second))
	}
	rctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	fail := func(err error, retryable bool) (*Turn, bool, error) {
		if ctx.Err() != nil {
			return nil, false, ctx.Err()
		}
		if errors.Is(rctx.Err(), context.DeadlineExceeded) {
			return nil, false, &BackendError{Message: fmt.Sprintf("request to %s timed out after %s", c.endpoint(), timeout)}
		}
		var be *BackendError
		if errors.As(err, &be) {
			return nil, retryable, err
		}
		return nil, retryable, &BackendError{Message: c.scrub(fmt.Sprintf("cannot reach backend at %s: %v", c.base(), err))}
	}
	httpReq, err := http.NewRequestWithContext(rctx, http.MethodPost, c.endpoint(), bytes.NewReader(body))
	if err != nil {
		return nil, false, &BackendError{Message: c.scrub("bad backend URL: " + err.Error())}
	}
	c.SetHeaders(httpReq.Header)
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
			Message: c.scrub(fmt.Sprintf("backend returned HTTP %d: %s", resp.StatusCode, strings.ToValidUTF8(string(b), "�")))}
		return fail(be, resp.StatusCode >= 500 || resp.StatusCode == 429)
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, 256<<20))
	if err != nil {
		return fail(err, false)
	}
	turn, err := parseAnthropic(b)
	if err != nil {
		return nil, false, err
	}
	return turn, false, nil
}

func parseAnthropic(b []byte) (*Turn, error) {
	var root map[string]any
	if err := json.Unmarshal(b, &root); err != nil || root == nil {
		return nil, &BackendError{Message: "backend returned invalid JSON"}
	}
	if t, _ := root["type"].(string); t == "error" {
		return nil, &BackendError{Message: "backend error: " + errorText(root["error"])}
	}
	blocks, ok := root["content"].([]any)
	if !ok {
		return nil, &BackendError{Message: "backend response had no content"}
	}
	t := &Turn{Raw: root}
	var text, thinking strings.Builder
	for _, item := range blocks {
		blk, _ := item.(map[string]any)
		switch blk["type"] {
		case "text":
			s, _ := blk["text"].(string)
			text.WriteString(s)
		case "thinking":
			s, _ := blk["thinking"].(string)
			thinking.WriteString(s)
		case "tool_use":
			name, _ := blk["name"].(string)
			if name == "" {
				return nil, &BackendError{Message: "tool call missing function name"}
			}
			id, _ := blk["id"].(string)
			input, _ := blk["input"].(map[string]any)
			if input == nil {
				input = map[string]any{}
			}
			t.ToolCalls = append(t.ToolCalls, ToolCall{ID: id, Name: name, Arguments: input,
				RawArguments: string(compactASCII(input))})
		}
	}
	t.Content, t.Reasoning = text.String(), thinking.String()
	if sr, _ := root["stop_reason"].(string); sr != "" {
		t.HasFinishReason = true
		switch sr {
		case "end_turn", "stop_sequence", "pause_turn":
			t.FinishReason = "stop"
		case "tool_use":
			t.FinishReason = "tool_calls"
		case "max_tokens":
			t.FinishReason = "length"
		case "refusal":
			t.FinishReason = "content_filter"
		default:
			t.FinishReason = sr
		}
	}
	t.Usage = UsageFromProvider(root["usage"])
	t.UsageReported = !t.Usage.IsZero()
	return t, nil
}
