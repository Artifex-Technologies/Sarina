package llm

import (
	"context"
	"io"
	"net/http"
	"strings"
	"testing"
)

func withBounds(t *testing.T, response int64, line int) {
	t.Helper()
	oldR, oldL := maxResponseBytes, maxStreamLine
	maxResponseBytes, maxStreamLine = response, line
	t.Cleanup(func() { maxResponseBytes, maxStreamLine = oldR, oldL })
}

// An answer past the bound is refused with its own error, streamed or not, and a streamed line past its bound too; an
// answer within the bounds still reads.
func TestModelAnswersAreBounded(t *testing.T) {
	ok := `{"choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}`
	withBounds(t, int64(len(ok)), 1<<20)
	s := serve(t, func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, ok) })
	if turn, err := newTestClient(s.URL).Complete(context.Background(), Request{}, nil); err != nil || turn.Content != "hi" {
		t.Fatalf("an answer of exactly the bound must read: %v %v", turn, err)
	}
	big := serve(t, func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, ok+" ") })
	if _, err := newTestClient(big.URL).Complete(context.Background(), Request{}, nil); err == nil || !strings.Contains(err.Error(), "larger than") {
		t.Fatalf("err %v", err)
	}

	event := `data: {"choices":[{"delta":{"content":"x"}}]}` + "\n\n"
	withBounds(t, int64(len(event)*3+len("data: [DONE]\n\n")), 1<<20)
	if _, _, err := completeStream(t, strings.Repeat(event, 3)+"data: [DONE]\n\n"); err != nil {
		t.Fatalf("a stream of exactly the bound must read: %v", err)
	}
	if _, _, err := completeStream(t, strings.Repeat(event, 4)+"data: [DONE]\n\n"); err == nil || !strings.Contains(err.Error(), "streamed answer is larger than") {
		t.Fatalf("err %v", err)
	}

	withBounds(t, 1<<20, 100)
	if _, _, err := completeStream(t, "data: "+strings.Repeat("x", 200)+"\n\n"); err == nil || !strings.Contains(err.Error(), "longer than 100 bytes") {
		t.Fatalf("err %v", err)
	}
	if maxResponseBytes != 1<<20 {
		t.Fatal("restore")
	}
}

// The real bounds leave room for any real answer.
func TestModelBoundsAreGenerous(t *testing.T) {
	if maxResponseBytes < 64<<20 || maxStreamLine < 1<<20 {
		t.Fatal("too tight")
	}
}
