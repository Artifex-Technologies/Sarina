package quantum

import (
	"fmt"
	"math"
	"math/cmplx"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityQuantum records (testdata/parity/quantum.json) Go's math.Sin, Cos and Sincos and cmplx.Exp of a pure
// imaginary number bit for bit (the gates' entries come from them), the numbers as the report writes them, the qubit count
// as the command reads it (fmt.Sscan), and whole simulations with fixed seeds: every gate, angles, errors, shots.
func TestParityQuantum(t *testing.T) {
	var cases []parity.Case
	bits := func(x float64) string { return fmt.Sprintf("%016x", math.Float64bits(x)) }
	xs := []float64{0, math.Copysign(0, -1), 1e-300, 5e-324, 1e-9, 0.1, 0.5, 0.7853981633974483, 1, math.Pi / 8, math.Pi / 4, math.Pi / 2,
		math.Pi, 3 * math.Pi / 2, 2 * math.Pi, 3, 4, 5, 6, 7, 10, 100, 1000, 12345.678, 1e6, 1e8, 1 << 29, (1 << 29) - 0.5,
		(1 << 29) + 1, 1e10, 1e15, 1e16, 1e17, 1e20, 1e22, 1e100, 1e200, 1e300, math.MaxFloat64, math.Inf(1), math.Inf(-1), math.NaN(),
		-0.1, -1, -math.Pi / 2, -math.Pi, -1e10, -1e300, 0.6299605249474366, 2.5e-8, math.Pi / 3, math.Pi / 6, 22, 355, 1.5707963267948966e10}
	for i := 1; i < 200; i++ {
		xs = append(xs, float64(i)*0.37-30, math.Ldexp(float64(i)*0.9137, i*5-300))
	}
	var trig []any
	for _, x := range xs {
		s, c := math.Sincos(x)
		e := cmplx.Exp(complex(0, x))
		trig = append(trig, []string{bits(x), bits(math.Sin(x)), bits(math.Cos(x)), bits(s), bits(c), bits(real(e)), bits(imag(e))})
	}
	cases = append(cases, parity.Case{Fn: "trig", In: len(xs), Out: trig})

	var nums []any
	for _, x := range []float64{0, math.Copysign(0, -1), 4e-10, -4e-10, 5e-10, -5e-10, 1e-5, -1e-5, 0.03125, 0.00005, 0.00015, 0.5,
		0.70710678118654757, -0.70710678118654757, 1, 0.99999, 0.999951, 0.99995, 1.00005, 123456.78905, 1e20, 1e300, math.MaxFloat64,
		math.Inf(1), math.Inf(-1), math.NaN(), 0.12345, 0.12355, 2.5e-5, 7.5e-5} {
		nums = append(nums, []string{bits(x), num(x), cnum(complex(x, 0)), cnum(complex(0, x)), cnum(complex(x, x)), cnum(complex(x, -x)),
			cnum(complex(-x, 0.5)), cnum(complex(0.5, x))})
	}
	cases = append(cases, parity.Case{Fn: "num", Out: nums})

	var scans []any
	for _, s := range []string{"2", " 3", "\t\n 4", "+2", "-1", "0", "02", "09", "0x3", "0X3", "0b11", "0o7", "0_1", "1_0", "_1", "1__0",
		"0x", "0b", "+", "-", "", "  ", "2abc", "abc", "3 4", "1e2", "2.5", "9223372036854775807", "9223372036854775808", "-9223372036854775808",
		"99999999999999999999", " 2", "　2", "１", "0x_3", "0_x3", "+0x2", "-0b1", "0012", " 2"} {
		var n int
		_, err := fmt.Sscan(s, &n)
		scans = append(scans, []any{s, n, err != nil})
	}
	cases = append(cases, parity.Case{Fn: "sscan", Out: scans})

	type sim struct {
		n       int
		circuit string
		shots   int
		seed    int64
	}
	sims := []sim{{1, "", 0, 1}, {1, "h 0", 0, 1}, {2, "h 0; cx 0 1", 0, 1}, {2, "h 0; cx 0 1", 1000, 1}, {2, "h 0; cx 0 1", 1000, 42},
		{3, "h 0; h 1; h 2", 4096, 7}, {3, "h 0\ncx 0 1\ncx 1 2", 0, 1}, {1, "x 0", 0, 1}, {1, "y 0", 0, 1}, {1, "z 0", 0, 1},
		{1, "h 0; s 0", 0, 1}, {1, "h 0; sdg 0", 0, 1}, {1, "h 0; t 0", 0, 1}, {1, "h 0; tdg 0", 0, 1}, {1, "id 0", 0, 1},
		{1, "rx(pi/2) 0", 0, 1}, {1, "ry(pi/3) 0", 0, 1}, {1, "h 0; rz(pi/4) 0", 0, 1}, {1, "h 0; p(3*pi/2) 0", 0, 1},
		{1, "rx(0.5) 0; ry(-1.25) 0; rz(2pi) 0", 0, 1}, {2, "x 0; cy 0 1", 0, 1}, {2, "h 0; h 1; cz 0 1", 0, 1}, {2, "x 0; ch 0 1", 0, 1},
		{2, "h 0; h 1; cp(pi/2) 0 1", 0, 1}, {2, "x 0; swap 0 1", 0, 1}, {3, "x 0; x 1; ccx 0 1 2", 0, 1}, {3, "x 0; x 1; toffoli 0 1 2", 0, 1},
		{3, "x 0; x 1; cswap 0 1 2", 0, 1}, {2, "x 0; cnot 0 1", 0, 1}, {2, "H 0; CX 0 1", 0, 1},
		{2, "h q[0]; cx q[0], q[1]  # Qiskit-style\nmeasure all", 0, 1}, {2, "barrier q; h 0; MEASURE; Measure All", 0, 1},
		{1, "h 0; ]", 0, 1}, {1, ",", 0, 1}, {1, "q[]; x 0", 0, 1}, {1, "h 0 # x 0", 0, 1}, {1, "#only", 0, 1},
		{2, "h 5", 0, 1}, {2, "cx 0 0", 0, 1}, {2, "frob 0", 0, 1}, {2, "rx 0", 0, 1}, {2, "rz(pi/ 0", 0, 1}, {2, "cx 0", 0, 1},
		{2, "rz(banana) 0", 0, 1}, {2, "rz(pi/0) 0", 0, 1}, {2, "rz(-0) 0", 0, 1}, {2, "rz(pi/-0) 0", 0, 1}, {2, "rz() 0", 0, 1},
		{2, "rz(inf) 0", 0, 1}, {2, "h 0; rz(nan) 0", 0, 1}, {2, "h 0; rx(1e400) 0", 0, 1}, {2, "h 0; rx(1e300) 0", 0, 1},
		{2, "h 0; rx(0x1p-2) 0", 0, 1}, {2, "h 0; rx(1_0) 0", 0, 1}, {2, "h 0; rx(2**pi) 0", 0, 1}, {2, "h 0; rx(pi*2) 0", 0, 1},
		{2, "h 0; rx(--pi) 0", 0, 1}, {2, "h 0; rx(-pi/-2) 0", 0, 1}, {2, "h 0; rx(pi/pi) 0", 0, 1}, {2, "h(pi) 0", 0, 1},
		{2, "( 0", 0, 1}, {2, "rz( pi ) 0", 0, 1}, {2, "x +1", 0, 1}, {2, "x -0", 0, 1}, {2, "x 01", 0, 1}, {2, "x 1.0", 0, 1},
		{2, "x \xff", 0, 1}, {2, "\xffx 0", 0, 1}, {2, "x 0\xff", 0, 1}, {2, "İ 0", 0, 1}, {2, "x 0", 0, 1}, {2, "ſwap 0 1", 0, 1},
		{2, "meaſure", 0, 1}, {2, "BARRIER", 0, 1}, {0, "h 0", 0, 1}, {13, "h 0", 0, 1}, {-1, "", 0, 1},
		{12, "h 0; h 1; h 2; h 3; h 4; h 5; h 6; h 7; h 8; h 9; h 10; h 11", 0, 1}, {7, "h 0; h 1; h 2; h 3; h 4; h 5; h 6", 300, 3},
		{2, "h 0; cx 0 1", 200000, 5}, {2, "h 0; cx 0 1", -5, 5}, {1, "h 0", 10, -9}, {4, "h 0; cx 0 1; ry(pi/7) 2; cp(pi/5) 2 3; h 3", 50, 11},
		{1, strings.Repeat("x 0;", 500), 0, 1}, {1, strings.Repeat("x 0;", 501), 0, 1}, {1, strings.Repeat("h 0;", 499) + "t 0", 0, 1},
		{3, "rx(pi/3) 0; ry(pi/5) 1; rz(pi/7) 2; cx 0 1; cx 1 2; cp(1.1) 0 2; cswap 2 0 1; ccx 2 1 0", 100, 8}}
	var out []any
	for _, s := range sims {
		r, err := Simulate(s.n, s.circuit, s.shots, s.seed)
		out = append(out, []any{s.n, parity.Text(s.circuit), s.shots, s.seed, r, parity.Err(err)})
	}
	cases = append(cases, parity.Case{Fn: "simulate", Out: out})

	// the final states bit for bit (the report rounds to four places)
	var states []any
	for _, s := range sims {
		st, err := New(s.n)
		if err != nil {
			continue
		}
		steps, err := Parse(s.circuit, s.n)
		if err != nil {
			continue
		}
		st.Run(steps)
		var amp []string
		for _, a := range st.Amp[:min(len(st.Amp), 64)] {
			amp = append(amp, bits(real(a))+bits(imag(a)))
		}
		states = append(states, []any{s.n, parity.Text(s.circuit), amp})
	}
	cases = append(cases, parity.Case{Fn: "state", Out: states})
	parity.Fixture(t, "quantum", cases)
}
