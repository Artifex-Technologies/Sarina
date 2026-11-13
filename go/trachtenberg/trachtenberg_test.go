package trachtenberg

import (
	"fmt"
	"math/rand"
	"strings"
	"testing"
)

func TestEveryRuleGivesTheTrueProduct(t *testing.T) {
	rng := rand.New(rand.NewSource(1))
	for m := range Rules {
		for i := 0; i < 2000; i++ {
			n := rng.Int63n(1_000_000_000)
			if i < 200 {
				n = int64(i) // the small numbers and the ones ending in zeros, where a rule's ends meet
			}
			if _, err := ByRule(fmt.Sprint(n), m); err != nil {
				t.Fatalf("%d × %d: %v", n, m, err)
			}
		}
	}
}

func TestTheGeneralMethodGivesTheTrueProduct(t *testing.T) {
	rng := rand.New(rand.NewSource(2))
	for i := 0; i < 3000; i++ {
		a, b := rng.Int63n(1_000_000), rng.Int63n(100_000)
		if _, err := General(fmt.Sprint(a), fmt.Sprint(b)); err != nil {
			t.Fatalf("%d × %d: %v", a, b, err)
		}
	}
}

func TestTheStepsReadAsTheSystemTeachesThem(t *testing.T) {
	w, err := ByRule("3,452", 11)
	if err != nil || w.Result != "37972" {
		t.Fatalf("%v %s", err, w.Result)
	}
	if !strings.Contains(w.Steps[0].Text, "digit 2: 2 + neighbour 0 = 2: write 2") {
		t.Fatalf("first step: %q", w.Steps[0].Text)
	}
	w, _ = ByRule("2468", 7)
	if w.Result != "17276" || !strings.Contains(Report(w), "Nines check") {
		t.Fatalf("%s\n%s", w.Result, Report(w))
	}
	w, _ = General("23", "47")
	if w.Result != "1081" || !strings.Contains(w.Steps[1].Text, "tens of 3×7 = 2") {
		t.Fatalf("%s %q", w.Result, w.Steps[1].Text)
	}
}

func TestAWrongAnswerIsPlaced(t *testing.T) {
	w, _ := ByRule("324", 12)
	g := Grade(w, "3,788")
	if !strings.Contains(g, "it is 3888") || !strings.Contains(g, "place 3") {
		t.Fatalf("%s", g)
	}
	if !strings.Contains(Grade(w, "3888"), "is right") {
		t.Fatal("a right answer")
	}
}

// A multiplier of up to 40 digits whose value wrapped around the int in atoi to a rule number was worked by that rule:
// 12 × 1844674407370955161600000000000000000009 came out as 12 × 9 = 108 (the check compared it with 12 × 9 too).
func TestHugeMultiplierIsNotARule(t *testing.T) {
	big := "1844674407370955161600000000000000000009" // 9 + 2^64 * 10^20
	for _, ab := range [][2]string{{"12", big}, {big, "12"}} {
		w, err := WorkOut(ab[0], ab[1])
		if err != nil {
			t.Fatal(err)
		}
		if w.Result != "22136092888451461939200000000000000000108" || strings.Contains(Report(w), "× 9 by") {
			t.Fatalf("%s × %s worked out as %s", ab[0], ab[1], w.Result)
		}
	}
}
