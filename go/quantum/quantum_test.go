package quantum

import (
	"math"
	"strings"
	"testing"
)

func probs(t *testing.T, n int, c string) map[string]float64 {
	t.Helper()
	s, err := New(n)
	if err != nil {
		t.Fatal(err)
	}
	steps, err := Parse(c, n)
	if err != nil {
		t.Fatal(err)
	}
	s.Run(steps)
	return s.Probabilities()
}

func near(a, b float64) bool { return math.Abs(a-b) < 1e-9 }

func TestTextbookCircuits(t *testing.T) {
	// a Bell pair: only 00 and 11, half each
	p := probs(t, 2, "h 0; cx 0 1")
	if len(p) != 2 || !near(p["|00>"], 0.5) || !near(p["|11>"], 0.5) {
		t.Errorf("Bell: %v", p)
	}
	// interference: H then H returns to |0>
	if p := probs(t, 1, "h 0\nh 0"); len(p) != 1 || !near(p["|0>"], 1) {
		t.Errorf("HH: %v", p)
	}
	// phase kickback: H Z H is X
	if p := probs(t, 1, "h 0; z 0; h 0"); !near(p["|1>"], 1) {
		t.Errorf("HZH: %v", p)
	}
	// qubit order: x on qubit 0 is the rightmost bit
	if p := probs(t, 3, "x 0"); !near(p["|001>"], 1) {
		t.Errorf("order: %v", p)
	}
	// GHZ on 3 qubits
	if p := probs(t, 3, "h 0; cx 0 1; cx 1 2"); !near(p["|000>"], 0.5) || !near(p["|111>"], 0.5) {
		t.Errorf("GHZ: %v", p)
	}
	// Toffoli flips the target only when both controls are 1
	if p := probs(t, 3, "x 0; x 1; ccx 0 1 2"); !near(p["|111>"], 1) {
		t.Errorf("Toffoli: %v", p)
	}
	// swap, and ry(pi/2) gives an even split, rx(pi) flips
	if p := probs(t, 2, "x 0; swap 0 1"); !near(p["|10>"], 1) {
		t.Errorf("swap: %v", p)
	}
	if p := probs(t, 1, "ry(pi/2) 0"); !near(p["|0>"], 0.5) {
		t.Errorf("ry: %v", p)
	}
	if p := probs(t, 1, "rx(pi) 0"); !near(p["|1>"], 1) {
		t.Errorf("rx: %v", p)
	}
	// Grover on 2 qubits, marked |11>: one iteration finds it with certainty
	g := "h 0; h 1; cz 0 1; h 0; h 1; x 0; x 1; cz 0 1; x 0; x 1; h 0; h 1"
	if p := probs(t, 2, g); !near(p["|11>"], 1) {
		t.Errorf("Grover: %v", p)
	}
	// Deutsch-Jozsa, balanced f(x)=x: the input qubit reads 1
	if p := probs(t, 2, "x 1; h 0; h 1; cx 0 1; h 0"); !near(p["|11>"]+p["|01>"], 1) {
		t.Errorf("Deutsch: %v", p)
	}
}

func TestReportAndShots(t *testing.T) {
	out, err := Simulate(2, "h 0; cx 0 1", 1000, 1)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out, "|00>  amplitude 0.7071  probability 0.5000") || !strings.Contains(out, "q0=0.5000") || !strings.Contains(out, "Measured 1000 times") {
		t.Errorf("report:\n%s", out)
	}
	out, _ = Simulate(1, "h 0; s 0", 0, 1)
	if !strings.Contains(out, "0.7071i") {
		t.Errorf("a complex amplitude:\n%s", out)
	}
}

func TestBadInputIsExplained(t *testing.T) {
	for _, c := range []string{"h 5", "cx 0 0", "frob 0", "rx 0", "rz(pi/ 0", "cx 0", "rz(banana) 0"} {
		if _, err := Simulate(2, c, 0, 1); err == nil {
			t.Errorf("%q accepted", c)
		}
	}
	if _, err := Simulate(13, "h 0", 0, 1); err == nil {
		t.Error("13 qubits accepted")
	}
	if _, err := Simulate(2, "h q[0]; cx q[0], q[1]  # Qiskit-style\nmeasure all", 0, 1); err != nil {
		t.Errorf("Qiskit-like spelling: %v", err)
	}
	if a, _ := angle("-3*pi/2"); !near(a, -3*math.Pi/2) {
		t.Errorf("angle %v", a)
	}
}
