package tools

import (
	"fmt"
	"strings"
	"testing"
)

func TestACutReadListsWhatItLeftOut(t *testing.T) {
	var b strings.Builder
	for i := 1; i <= 600; i++ {
		switch i {
		case 300:
			b.WriteString("pub fn protected(pkg: &str) -> bool {\n")
		case 305:
			b.WriteString("fn protected_with(pkg: &str, os_list: &str) -> bool {\n")
		default:
			fmt.Fprintf(&b, "    let x%d = %d; // filler line\n", i, i)
		}
	}
	out := truncateFile(b.String(), 4000)
	if !strings.Contains(out, "omitted") || !strings.Contains(out, "300: pub fn protected(pkg: &str) -> bool {") || !strings.Contains(out, "305: fn protected_with(") {
		t.Fatalf("the omitted definitions are not listed:\n%s", out[1500:3200])
	}
	if strings.Contains(truncateFile("short", 4000), "defined in") {
		t.Fatal("a whole read has no outline")
	}
}
