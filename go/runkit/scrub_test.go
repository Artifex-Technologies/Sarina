package runkit

import "testing"

// A secret-named key in JSON (tool-call arguments in a transcript the service and the GUI send out) is scrubbed, inside
// its quotes. Before 2026-11-12 the key=value rule never matched it (the key's closing quote stands before the colon), so
// `{"password":"hunter2hunter2"}` left the process as it was.
func TestScrubberJSONKeys(t *testing.T) {
	scrub := Scrubber()
	for in, want := range map[string]string{
		`{"api_key":"sk-12345678"}`:                    `{"api_key":"[REDACTED]"}`,
		`{"path":"a", "Password" : "hunter2hunter2"}`:  `{"path":"a", "Password" : "[REDACTED]"}`,
		`{"my_token":"ab\"cd\\ef"}`:                    `{"my_token":"[REDACTED]"}`,
		`{"secret":"short"}`:                           `{"secret":"short"}`,
		`{"tokenizer":"bpe-tokens"}`:                   `{"tokenizer":"[REDACTED]"}`,
		`{"name":"api_key","value":"not-a-key-value"}`: `{"name":"api_key","value":"not-a-key-value"}`,
	} {
		if got := scrub(in); got != want {
			t.Errorf("%s\n got %s\nwant %s", in, got, want)
		}
	}
}
