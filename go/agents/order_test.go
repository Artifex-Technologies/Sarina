package agents

import "testing"

// With both shell and project_context wrong, which one the refusal named followed Go's map order (CI failed on it on
// 2026-11-17: the fixture had seen both). Now shell is checked first, every time.
func TestBadSwitchesAreNamedInOrder(t *testing.T) {
	doc := []byte("---\ndescription: d\nshell: maybe\nproject_context: perhaps\n---\nb")
	for i := 0; i < 500; i++ {
		_, err := Parse(doc, "fallback")
		if err == nil || err.Error() != `shell must be true or false, got "maybe"` {
			t.Fatalf("run %d: %v", i, err)
		}
	}
}
