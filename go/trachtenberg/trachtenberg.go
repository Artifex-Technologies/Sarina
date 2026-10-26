// Package trachtenberg works the Trachtenberg speed system exactly, step by step, so that Sarina can teach it without an arithmetic
// slip: a small model explains well and multiplies badly. Every result is checked against the true product before it is shown.
//
// The rules are the system's own: the number is written with a zero in front and worked from right to left; at each digit a short
// rule uses the digit and its "neighbour" (the digit to its right), and the carry goes on to the next step. Multiplying by any number
// uses the "two-finger" method: each digit of the answer is the units digits of one set of pair products plus the tens digits of the
// next set, plus the carry.
package trachtenberg

import (
	"fmt"
	"math/big"
	"strings"
)

// Step is one move of a worked example: what was done at a position, the value it came to, the digit written and the carry.
type Step struct {
	Position int // 1 = the units digit of the answer
	Text     string
	Value    int
	Write    int
	Carry    int
}

// Work is a worked multiplication.
type Work struct {
	A, B   string
	Rule   string // what the rule is, in one sentence
	Steps  []Step
	Result string
}

// Rules are the single-number multipliers with a rule of their own.
var Rules = map[int]string{
	11: "Add each digit to its neighbour.",
	12: "Double each digit and add its neighbour.",
	6:  "Add to each digit half its neighbour; add 5 more if the digit is odd.",
	7:  "Double each digit and add half its neighbour; add 5 more if the digit is odd.",
	5:  "Take half the neighbour; add 5 if the digit is odd.",
	9:  "Rightmost digit: subtract it from 10. Middle digits: subtract from 9 and add the neighbour. Leftmost (the zero in front): the neighbour minus 1.",
	8:  "Rightmost digit: subtract it from 10 and double. Middle digits: subtract from 9, double, and add the neighbour. Leftmost (the zero in front): the neighbour minus 2.",
	4:  "Rightmost digit: subtract it from 10, add 5 if it is odd. Middle digits: subtract from 9, add half the neighbour, add 5 if odd. Leftmost (the zero in front): half the neighbour minus 1.",
	3:  "Rightmost digit: subtract it from 10 and double, add 5 if it is odd. Middle digits: subtract from 9, double, add half the neighbour, add 5 if odd. Leftmost (the zero in front): half the neighbour minus 2.",
}

func digits(s string) ([]int, error) {
	s = strings.NewReplacer(",", "", "_", "", " ", "").Replace(strings.TrimSpace(s))
	if s == "" || len(s) > 40 {
		return nil, fmt.Errorf("give a whole number of 1 to 40 digits")
	}
	out := make([]int, len(s))
	for i, r := range s {
		if r < '0' || r > '9' {
			return nil, fmt.Errorf("%q is not a whole number", s)
		}
		out[i] = int(r - '0')
	}
	return out, nil
}

func clean(s string) string {
	return strings.TrimLeft(strings.NewReplacer(",", "", "_", "", " ", "").Replace(strings.TrimSpace(s)), "0")
}

func odd(x int) bool { return x%2 == 1 }

// ByRule multiplies a by one of the rule numbers (3 4 5 6 7 8 9 11 12), digit by digit.
func ByRule(a string, m int) (Work, error) {
	rule, ok := Rules[m]
	if !ok {
		return Work{}, fmt.Errorf("there is no single rule for %d (rules: 3 4 5 6 7 8 9 11 12); use the general method", m)
	}
	d, err := digits(a)
	if err != nil {
		return Work{}, err
	}
	D := append([]int{0}, d...) // the zero in front
	w := Work{A: clean(a), B: fmt.Sprint(m), Rule: rule}
	carry := 0
	var out []int
	last := len(D) - 1
	for k := last; k >= 0; k-- {
		x, n := D[k], 0
		if k < last {
			n = D[k+1]
		}
		var v int
		var how string
		switch m {
		case 11:
			v, how = x+n, fmt.Sprintf("%d + neighbour %d", x, n)
		case 12:
			v, how = 2*x+n, fmt.Sprintf("double %d = %d, + neighbour %d", x, 2*x, n)
		case 6, 7, 5:
			base := x
			how = fmt.Sprint(x)
			if m == 7 {
				base, how = 2*x, fmt.Sprintf("double %d = %d", x, 2*x)
			}
			if m == 5 {
				base, how = 0, ""
			}
			v = base + n/2
			if how == "" {
				how = fmt.Sprintf("half of neighbour %d = %d", n, n/2)
			} else {
				how += fmt.Sprintf(", + half of neighbour %d = %d", n, n/2)
			}
			if odd(x) {
				v += 5
				how += fmt.Sprintf(", + 5 because %d is odd", x)
			}
		case 9, 8, 4, 3:
			double := m == 8 || m == 3
			half := m == 4 || m == 3
			switch {
			case k == last:
				v, how = 10-x, fmt.Sprintf("10 - %d = %d", x, 10-x)
			case k == 0:
				sub := map[int]int{9: 1, 8: 2, 4: 1, 3: 2}[m]
				if half {
					v, how = n/2-sub, fmt.Sprintf("half of neighbour %d = %d, - %d", n, n/2, sub)
				} else {
					v, how = n-sub, fmt.Sprintf("neighbour %d - %d", n, sub)
				}
			default:
				v, how = 9-x, fmt.Sprintf("9 - %d = %d", x, 9-x)
			}
			if k != 0 {
				if double {
					v *= 2
					how += fmt.Sprintf(", doubled = %d", v)
				}
				if k != last {
					if half {
						v += n / 2
						how += fmt.Sprintf(", + half of neighbour %d = %d", n, n/2)
					} else {
						v += n
						how += fmt.Sprintf(", + neighbour %d", n)
					}
				}
				if (m == 4 || m == 3) && odd(x) {
					v += 5
					how += fmt.Sprintf(", + 5 because %d is odd", x)
				}
			}
		}
		total := v + carry
		if carry != 0 {
			how += fmt.Sprintf(", + carry %d", carry)
		}
		write := ((total % 10) + 10) % 10
		next := (total - write) / 10
		what := fmt.Sprintf("digit %d", x)
		if k == 0 {
			what = "the zero in front"
		}
		shown := fmt.Sprintf("%s = %d", how, total)
		if strings.HasSuffix(how, fmt.Sprintf("= %d", total)) {
			shown = how // "10 - 3 = 7", not "10 - 3 = 7 = 7"
		}
		w.Steps = append(w.Steps, Step{Position: last - k + 1, Text: fmt.Sprintf("%s: %s: write %d, carry %d", what, shown, write, next), Value: total, Write: write, Carry: next})
		out = append([]int{write}, out...)
		carry = next
	}
	for carry > 0 {
		out = append([]int{carry % 10}, out...)
		w.Steps = append(w.Steps, Step{Position: len(out), Text: fmt.Sprintf("the carry %d left over: write it in front", carry%10), Write: carry % 10})
		carry /= 10
	}
	w.Result = join(out)
	return w, check(w)
}

// General multiplies any two numbers by the two-finger method: answer digit k = units digits of the pair products that land on k,
// + tens digits of the pair products that land one place to the right, + the carry.
func General(a, b string) (Work, error) {
	x, err := digits(a)
	if err != nil {
		return Work{}, err
	}
	y, err := digits(b)
	if err != nil {
		return Work{}, err
	}
	rev := func(s []int) []int {
		r := make([]int, len(s))
		for i, v := range s {
			r[len(s)-1-i] = v
		}
		return r
	}
	ax, by := rev(x), rev(y) // index 0 = units
	w := Work{A: clean(a), B: clean(b), Rule: "Each digit of the answer is the units digits of the pairs that land on that place, plus the tens digits of the pairs that land one place to the right, plus the carry."}
	pairs := func(p int) [][2]int {
		var out [][2]int
		for i := range ax {
			if j := p - i; j >= 0 && j < len(by) {
				out = append(out, [2]int{i, j})
			}
		}
		return out
	}
	carry := 0
	var out []int
	for p := 0; p < len(ax)+len(by); p++ {
		var parts []string
		v := 0
		for _, pr := range pairs(p) {
			prod := ax[pr[0]] * by[pr[1]]
			v += prod % 10
			parts = append(parts, fmt.Sprintf("units of %d×%d = %d", ax[pr[0]], by[pr[1]], prod%10))
		}
		for _, pr := range pairs(p - 1) {
			prod := ax[pr[0]] * by[pr[1]]
			v += prod / 10
			parts = append(parts, fmt.Sprintf("tens of %d×%d = %d", ax[pr[0]], by[pr[1]], prod/10))
		}
		if len(parts) == 0 && carry == 0 {
			continue
		}
		if carry != 0 {
			parts = append(parts, fmt.Sprintf("carry %d", carry))
		}
		total := v + carry
		w.Steps = append(w.Steps, Step{Position: p + 1, Text: fmt.Sprintf("place %d: %s = %d: write %d, carry %d", p+1, strings.Join(parts, " + "), total, total%10, total/10), Value: total, Write: total % 10, Carry: total / 10})
		out = append([]int{total % 10}, out...)
		carry = total / 10
	}
	for carry > 0 {
		out = append([]int{carry % 10}, out...)
		carry /= 10
	}
	w.Result = join(out)
	return w, check(w)
}

func join(d []int) string {
	var b strings.Builder
	for _, v := range d {
		b.WriteByte(byte('0' + v))
	}
	s := strings.TrimLeft(b.String(), "0")
	if s == "" {
		return "0"
	}
	return s
}

// check refuses a worked example whose answer is not the true product (it never happens; it guards the teaching).
func check(w Work) error {
	a, _ := new(big.Int).SetString(orZero(w.A), 10)
	b, _ := new(big.Int).SetString(orZero(w.B), 10)
	if want := new(big.Int).Mul(a, b).String(); want != w.Result {
		return fmt.Errorf("internal check failed: %s × %s worked out as %s, the true product is %s", w.A, w.B, w.Result, want)
	}
	return nil
}

func orZero(s string) string {
	if s == "" {
		return "0"
	}
	return s
}

// DigitSum is the digit sum taken down to one digit (the "nines" check of the system).
func DigitSum(s string) int {
	d, err := digits(s)
	if err != nil {
		return -1
	}
	t := 0
	for _, v := range d {
		t += v
	}
	for t > 9 {
		n := 0
		for t > 0 {
			n, t = n+t%10, t/10
		}
		t = n
	}
	if t == 9 {
		return 0
	}
	return t
}

// Grade compares a student's answer with the true product and says where it first goes wrong (counting places from the right),
// and what the nines check shows about it.
func Grade(w Work, answer string) string {
	got := clean(answer)
	if got == "" {
		got = "0"
	}
	if got == w.Result {
		return fmt.Sprintf("The answer %s is right.", got)
	}
	place := 0
	for i := 1; i <= max(len(got), len(w.Result)); i++ {
		g, r := byte('0'), byte('0')
		if i <= len(got) {
			g = got[len(got)-i]
		}
		if i <= len(w.Result) {
			r = w.Result[len(w.Result)-i]
		}
		if g != r {
			place = i
			break
		}
	}
	nines := ""
	ca, cb, cg := DigitSum(w.A), DigitSum(w.B), DigitSum(got)
	if ca >= 0 && cb >= 0 && cg >= 0 {
		want := (ca * cb) % 9
		if want == cg {
			nines = fmt.Sprintf(" The nines check would not have caught it (digit sums %d × %d → %d, and the answer's digit sum is also %d): it misses some slips, such as swapped digits.", ca, cb, want, cg)
		} else {
			nines = fmt.Sprintf(" The nines check shows it too: digit sums %d × %d → %d, but the answer's digit sum is %d.", ca, cb, want, cg)
		}
	}
	return fmt.Sprintf("The answer %s is not right: it is %s. The first wrong digit is place %d from the right (the step for place %d).%s", got, w.Result, place, place, nines)
}

// Report is a worked example as text: the rule, each step, the answer, and the nines check.
func Report(w Work) string {
	var b strings.Builder
	fmt.Fprintf(&b, "%s × %s by the Trachtenberg method.\nRule: %s\n", w.A, w.B, w.Rule)
	if _, single := Rules[atoi(w.B)]; single && len(w.B) <= 2 {
		fmt.Fprintf(&b, "Write %s with a zero in front (0%s) and work from right to left; the neighbour is the digit to the right.\n", w.A, w.A)
	}
	for i, s := range w.Steps {
		fmt.Fprintf(&b, "Step %d (%s)\n", i+1, s.Text)
	}
	ca, cb, cr := DigitSum(w.A), DigitSum(w.B), DigitSum(w.Result)
	fmt.Fprintf(&b, "Answer: %s.\nNines check (add the digits until one is left; 9 counts as 0): %d × %d = %d → %d; the answer's digit sum is %d: it agrees.", w.Result, ca, cb, ca*cb, (ca*cb)%9, cr)
	return b.String()
}

func atoi(s string) int {
	n := 0
	for _, r := range s {
		if r < '0' || r > '9' {
			return -1
		}
		n = n*10 + int(r-'0')
	}
	return n
}

// Work out a × b the way a learner would: the single rule when b (or a) is a rule number, otherwise the general method.
func WorkOut(a, b string) (Work, error) {
	if m := atoi(clean(b)); m >= 0 {
		if _, ok := Rules[m]; ok {
			return ByRule(a, m)
		}
	}
	if m := atoi(clean(a)); m >= 0 {
		if _, ok := Rules[m]; ok {
			w, err := ByRule(b, m)
			return w, err
		}
	}
	return General(a, b)
}
