// Package quantum is a small exact simulator of quantum circuits, so that when Sarina teaches quantum computing she can check a claim
// ("this circuit gives 50/50") instead of trusting her memory. It keeps the whole state vector (complex amplitudes) in memory, up to
// MaxQubits qubits, and applies gates one by one: plain arithmetic, nothing random unless shots are asked for, no network, no files.
//
// Qubit order follows Qiskit: qubit 0 is the rightmost bit of a basis state, so |01> means qubit 0 is 1 and qubit 1 is 0.
package quantum

import (
	"fmt"
	"math"
	"math/cmplx"
	"math/rand"
	"sort"
	"strconv"
	"strings"
)

// MaxQubits bounds the state (2^12 amplitudes): plenty for teaching, never a burden on the computer.
const MaxQubits = 12

// MaxGates bounds a circuit.
const MaxGates = 500

// State is a register of n qubits.
type State struct {
	N   int
	Amp []complex128
}

// New is n qubits all in |0>.
func New(n int) (*State, error) {
	if n < 1 || n > MaxQubits {
		return nil, fmt.Errorf("between 1 and %d qubits", MaxQubits)
	}
	s := &State{N: n, Amp: make([]complex128, 1<<n)}
	s.Amp[0] = 1
	return s, nil
}

type mat2 [2][2]complex128

var (
	r2 = complex(1/math.Sqrt2, 0)
	// the one-qubit gates
	gates1 = map[string]mat2{
		"x":   {{0, 1}, {1, 0}},
		"y":   {{0, -1i}, {1i, 0}},
		"z":   {{1, 0}, {0, -1}},
		"h":   {{r2, r2}, {r2, -r2}},
		"s":   {{1, 0}, {0, 1i}},
		"sdg": {{1, 0}, {0, -1i}},
		"t":   {{1, 0}, {0, cmplx.Exp(1i * math.Pi / 4)}},
		"tdg": {{1, 0}, {0, cmplx.Exp(-1i * math.Pi / 4)}},
		"id":  {{1, 0}, {0, 1}},
	}
)

func rot(name string, th float64) mat2 {
	c, s := complex(math.Cos(th/2), 0), complex(math.Sin(th/2), 0)
	switch name {
	case "rx":
		return mat2{{c, -1i * s}, {-1i * s, c}}
	case "ry":
		return mat2{{c, -s}, {s, c}}
	case "rz":
		return mat2{{cmplx.Exp(complex(0, -th/2)), 0}, {0, cmplx.Exp(complex(0, th/2))}}
	default: // "p": phase
		return mat2{{1, 0}, {0, cmplx.Exp(complex(0, th))}}
	}
}

// apply1 applies a one-qubit gate to qubit q, only where every control qubit is 1.
func (s *State) apply1(m mat2, q int, controls ...int) {
	bit := 1 << q
	var cmask int
	for _, c := range controls {
		cmask |= 1 << c
	}
	for i := range s.Amp {
		if i&bit != 0 || i&cmask != cmask {
			continue
		}
		a0, a1 := s.Amp[i], s.Amp[i|bit]
		s.Amp[i] = m[0][0]*a0 + m[0][1]*a1
		s.Amp[i|bit] = m[1][0]*a0 + m[1][1]*a1
	}
}

func (s *State) swap(a, b int) {
	for i := range s.Amp {
		ba, bb := i>>a&1, i>>b&1
		if ba == 1 && bb == 0 {
			j := i&^(1<<a) | 1<<b
			s.Amp[i], s.Amp[j] = s.Amp[j], s.Amp[i]
		}
	}
}

// Step is one parsed instruction.
type Step struct {
	Gate   string
	Qubits []int
	Theta  float64
}

var arity = map[string]int{"x": 1, "y": 1, "z": 1, "h": 1, "s": 1, "sdg": 1, "t": 1, "tdg": 1, "id": 1, "rx": 1, "ry": 1, "rz": 1, "p": 1,
	"cx": 2, "cnot": 2, "cy": 2, "cz": 2, "ch": 2, "swap": 2, "cp": 2, "ccx": 3, "toffoli": 3, "cswap": 3}

// Parse reads a circuit: one instruction per line or separated by ';', like "h 0", "cx 0 1", "rz(pi/2) 1", "ccx 0 1 2".
// Angles may use pi: "pi/4", "3*pi/2", "-pi", "0.5".
func Parse(text string, n int) ([]Step, error) {
	var out []Step
	for _, raw := range strings.FieldsFunc(text, func(r rune) bool { return r == ';' || r == '\n' }) {
		line := strings.TrimSpace(strings.SplitN(raw, "#", 2)[0])
		if line == "" || strings.EqualFold(line, "measure") || strings.EqualFold(line, "measure all") || strings.HasPrefix(strings.ToLower(line), "barrier") {
			continue
		}
		f := strings.Fields(strings.NewReplacer(",", " ", "q[", "", "[", " ", "]", "").Replace(line))
		name := strings.ToLower(f[0])
		st := Step{}
		if i := strings.Index(name, "("); i >= 0 {
			if !strings.HasSuffix(name, ")") {
				return nil, fmt.Errorf("%q: the angle needs a closing bracket", line)
			}
			th, err := angle(name[i+1 : len(name)-1])
			if err != nil {
				return nil, fmt.Errorf("%q: %v", line, err)
			}
			st.Theta, name = th, name[:i]
		} else if name == "rx" || name == "ry" || name == "rz" || name == "p" || name == "cp" {
			return nil, fmt.Errorf("%q: %s needs an angle, like %s(pi/2) 0", line, name, name)
		}
		k, ok := arity[name]
		if !ok {
			return nil, fmt.Errorf("unknown gate %q (known: x y z h s sdg t tdg rx ry rz p cx cy cz ch cp swap ccx cswap)", name)
		}
		st.Gate = name
		if len(f)-1 != k {
			return nil, fmt.Errorf("%q: %s acts on %d qubit(s)", line, name, k)
		}
		seen := map[int]bool{}
		for _, a := range f[1:] {
			q, err := strconv.Atoi(a)
			if err != nil || q < 0 || q >= n {
				return nil, fmt.Errorf("%q: qubit %q is not between 0 and %d", line, a, n-1)
			}
			if seen[q] {
				return nil, fmt.Errorf("%q: the same qubit twice", line)
			}
			seen[q] = true
			st.Qubits = append(st.Qubits, q)
		}
		out = append(out, st)
		if len(out) > MaxGates {
			return nil, fmt.Errorf("at most %d gates", MaxGates)
		}
	}
	return out, nil
}

func angle(s string) (float64, error) {
	s = strings.ReplaceAll(strings.ToLower(strings.TrimSpace(s)), " ", "")
	sign := 1.0
	if strings.HasPrefix(s, "-") {
		sign, s = -1, s[1:]
	}
	num, den := s, "1"
	if i := strings.Index(s, "/"); i >= 0 {
		num, den = s[:i], s[i+1:]
	}
	val := func(x string) (float64, error) {
		mul := 1.0
		if strings.HasSuffix(x, "pi") {
			mul, x = math.Pi, strings.TrimSuffix(strings.TrimSuffix(x, "pi"), "*")
			if x == "" {
				return mul, nil
			}
		}
		v, err := strconv.ParseFloat(x, 64)
		return v * mul, err
	}
	a, err := val(num)
	if err != nil {
		return 0, fmt.Errorf("angle %q not understood", s)
	}
	b, err := val(den)
	if err != nil || b == 0 {
		return 0, fmt.Errorf("angle %q not understood", s)
	}
	return sign * a / b, nil
}

// Run applies the steps.
func (s *State) Run(steps []Step) {
	for _, st := range steps {
		q := st.Qubits
		switch st.Gate {
		case "rx", "ry", "rz", "p":
			s.apply1(rot(st.Gate, st.Theta), q[0])
		case "cx", "cnot":
			s.apply1(gates1["x"], q[1], q[0])
		case "cy":
			s.apply1(gates1["y"], q[1], q[0])
		case "cz":
			s.apply1(gates1["z"], q[1], q[0])
		case "ch":
			s.apply1(gates1["h"], q[1], q[0])
		case "cp":
			s.apply1(rot("p", st.Theta), q[1], q[0])
		case "swap":
			s.swap(q[0], q[1])
		case "ccx", "toffoli":
			s.apply1(gates1["x"], q[2], q[0], q[1])
		case "cswap":
			s.apply1(gates1["x"], q[1], q[0], q[2])
			s.apply1(gates1["x"], q[2], q[0], q[1])
			s.apply1(gates1["x"], q[1], q[0], q[2])
		default:
			s.apply1(gates1[st.Gate], q[0])
		}
	}
}

func (s *State) ket(i int) string {
	return "|" + fmt.Sprintf("%0*b", s.N, i) + ">"
}

func num(x float64) string {
	if math.Abs(x) < 5e-10 {
		x = 0
	}
	return strconv.FormatFloat(x, 'f', 4, 64)
}

func cnum(c complex128) string {
	re, im := real(c), imag(c)
	switch {
	case math.Abs(im) < 5e-10:
		return num(re)
	case math.Abs(re) < 5e-10:
		return num(im) + "i"
	case im < 0:
		return num(re) + " - " + num(-im) + "i"
	}
	return num(re) + " + " + num(im) + "i"
}

// Probabilities of each basis state (only those above zero), largest first.
func (s *State) Probabilities() map[string]float64 {
	out := map[string]float64{}
	for i, a := range s.Amp {
		if p := real(a)*real(a) + imag(a)*imag(a); p > 1e-12 {
			out[s.ket(i)] = p
		}
	}
	return out
}

// Report describes the final state: amplitudes, probabilities, each qubit's chance of reading 1, and (with shots > 0) a sample of
// measurement outcomes.
func (s *State) Report(shots int, seed int64) string {
	var b strings.Builder
	type row struct {
		i int
		p float64
	}
	var rows []row
	for i, a := range s.Amp {
		if p := real(a)*real(a) + imag(a)*imag(a); p > 1e-12 {
			rows = append(rows, row{i, p})
		}
	}
	sort.SliceStable(rows, func(x, y int) bool { return rows[x].p > rows[y].p })
	fmt.Fprintf(&b, "%d qubit(s); qubit 0 is the rightmost bit. Final state, exact:\n", s.N)
	for k, r := range rows {
		if k == 64 {
			fmt.Fprintf(&b, "... and %d more basis states\n", len(rows)-64)
			break
		}
		fmt.Fprintf(&b, "  %s  amplitude %s  probability %s\n", s.ket(r.i), cnum(s.Amp[r.i]), num(r.p))
	}
	b.WriteString("Chance each qubit reads 1:")
	for q := 0; q < s.N; q++ {
		var p float64
		for i, a := range s.Amp {
			if i>>q&1 == 1 {
				p += real(a)*real(a) + imag(a)*imag(a)
			}
		}
		fmt.Fprintf(&b, " q%d=%s", q, num(p))
	}
	b.WriteString("\n")
	if shots > 0 {
		if shots > 100000 {
			shots = 100000
		}
		rng := rand.New(rand.NewSource(seed))
		counts := map[int]int{}
		for k := 0; k < shots; k++ {
			x, acc := rng.Float64(), 0.0
			for _, r := range rows {
				acc += r.p
				if x < acc {
					counts[r.i]++
					break
				}
			}
		}
		fmt.Fprintf(&b, "Measured %d times (random, like a real device):", shots)
		for _, r := range rows {
			if counts[r.i] > 0 {
				fmt.Fprintf(&b, " %s:%d", s.ket(r.i), counts[r.i])
			}
		}
		b.WriteString("\n")
	}
	return b.String()
}

// Simulate parses and runs a circuit and reports.
func Simulate(qubits int, circuit string, shots int, seed int64) (string, error) {
	s, err := New(qubits)
	if err != nil {
		return "", err
	}
	steps, err := Parse(circuit, qubits)
	if err != nil {
		return "", err
	}
	s.Run(steps)
	return s.Report(shots, seed), nil
}
