package quantum

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

func init() {
	tools.RegisterProvider(Tools)
	cli.Register("quantum", "simulate a small quantum circuit exactly: quantum [--shots N] QUBITS 'h 0; cx 0 1' (or the circuit on standard input)", command)
}

// Tools offers quantum_sim when SARINA_QUANTUM=on (off by default: its description costs every request a little context).
func Tools() []tools.Tool {
	if os.Getenv("SARINA_QUANTUM") != "on" {
		return nil
	}
	return []tools.Tool{{
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "quantum_sim",
			Description: fmt.Sprintf("Simulate a quantum circuit exactly (up to %d qubits) and get its final amplitudes and probabilities. Use it to check any claim about what a circuit does before you state it. Gates: x y z h s sdg t tdg rx(a) ry(a) rz(a) p(a) cx cy cz ch cp(a) swap ccx cswap; angles like pi/2. Qubit 0 is the rightmost bit.", MaxQubits),
			Parameters: map[string]any{"type": "object", "required": []string{"qubits", "circuit"}, "properties": map[string]any{
				"qubits":  map[string]any{"type": "integer", "description": "How many qubits (all start in |0>)."},
				"circuit": map[string]any{"type": "string", "description": "One gate per line or separated by ;, e.g. \"h 0; cx 0 1\"."},
				"shots":   map[string]any{"type": "integer", "description": "Optional: also measure this many times, like a real device."},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			n, _ := a["qubits"].(float64)
			shots, _ := a["shots"].(float64)
			circuit, _ := a["circuit"].(string)
			out, err := Simulate(int(n), circuit, int(shots), time.Now().UnixNano())
			if err != nil {
				return "", nil, err
			}
			return out + "\n[Exact simulation of an ideal, noise-free device. Real hardware adds noise.]", map[string]any{"action": "quantum_sim", "qubits": int(n)}, nil
		},
	}}
}

func command(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("quantum", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	shots := fs.Int("shots", 0, "also measure this many times")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if fs.NArg() < 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina quantum [--shots N] QUBITS 'CIRCUIT'")
		return 2
	}
	var n int
	if _, err := fmt.Sscan(fs.Arg(0), &n); err != nil {
		fmt.Fprintln(env.Stderr, "sarina quantum: the first argument is the number of qubits")
		return 2
	}
	circuit := strings.Join(fs.Args()[1:], " ")
	if circuit == "" && env.Stdin != nil {
		b, _ := io.ReadAll(io.LimitReader(env.Stdin, 64<<10))
		circuit = string(b)
	}
	out, err := Simulate(n, circuit, *shots, time.Now().UnixNano())
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina quantum:", err)
		return 2
	}
	fmt.Fprint(env.Stdout, out)
	return 0
}
