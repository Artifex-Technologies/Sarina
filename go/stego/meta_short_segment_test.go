package stego

import (
	"context"
	"strings"
	"testing"
)

// TestJPEGSegmentShorterThanItsLength: a segment whose length field is 0 or 1 (it counts its own two bytes, so the
// smallest real one is 2) made ParseJPEG slice backwards and panic, ending stego_inspect on a crafted JPEG.
func TestJPEGSegmentShorterThanItsLength(t *testing.T) {
	for _, n := range []byte{0, 1} {
		b := []byte{0xff, 0xd8, 0xff, 0xe0, 0x00, n, 'J', 'F', 'I', 'F', 0, 0xff, 0xd9}
		c := ParseJPEG(b)
		if len(c.Problems) != 1 || !strings.Contains(c.Problems[0], "under its own 2 bytes") {
			t.Fatalf("length %d: problems %q", n, c.Problems)
		}
		if out := Inspect(context.Background(), b, "x.jpg"); !strings.Contains(out, "under its own 2 bytes") {
			t.Fatalf("length %d: inspect said %q", n, out)
		}
	}
}
