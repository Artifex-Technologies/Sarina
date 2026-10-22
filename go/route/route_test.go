package route

import "testing"

func TestPick(t *testing.T) {
	if got := pick([]Score{{"performance", 0.80}, {"crashes", 0.78}, {"disk", 0.70}}); len(got) != 2 || got[0] != "performance" || got[1] != "crashes" {
		t.Fatalf("%v", got)
	}
	if got := pick([]Score{{"chat", 0.85}, {"performance", 0.6}}); got != nil {
		t.Fatalf("small talk must route nowhere: %v", got)
	}
	if got := pick([]Score{{"network", 0.50}}); got != nil {
		t.Fatalf("below the floor nothing is offered: %v", got)
	}
	if got := pick([]Score{{"guides", 0.75}, {"chat", 0.74}}); len(got) != 1 || got[0] != "guides" {
		t.Fatalf("%v", got)
	}
}
