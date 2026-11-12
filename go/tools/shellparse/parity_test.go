package shellparse

import (
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

type pWord struct {
	T any  `json:"t"`
	Q bool `json:"q,omitempty"`
	P bool `json:"p,omitempty"`
}

type pRedirect struct {
	Op     string `json:"op"`
	Target pWord  `json:"target"`
	Writes bool   `json:"writes,omitempty"`
}

type pCommand struct {
	Assigns   []pWord     `json:"assigns,omitempty"`
	Words     []pWord     `json:"words,omitempty"`
	Redirects []pRedirect `json:"redirects,omitempty"`
	Heredocs  []any       `json:"heredocs,omitempty"`
	Pipe      int         `json:"pipe,omitempty"`
	Func      any         `json:"func,omitempty"`
	Data      bool        `json:"data,omitempty"`
}

func pw(w Word) pWord { return pWord{T: parity.Text(w.Text), Q: w.Quoted, P: w.ProcSub} }

func pws(ws []Word) []pWord {
	var out []pWord
	for _, w := range ws {
		out = append(out, pw(w))
	}
	return out
}

// TestParityShellparse records Parse on the shell corpus (testdata/parity/shellparse.json; docs/RUST_PORT.md).
func TestParityShellparse(t *testing.T) {
	var cases []parity.Case
	for _, src := range parity.ShellCorpus(t) {
		s, err := Parse(src)
		out := map[string]any{}
		if err != nil {
			out["err"] = err.Error()
		} else {
			cmds := []pCommand{}
			for _, c := range s.Commands {
				pc := pCommand{Assigns: pws(c.Assigns), Words: pws(c.Words), Pipe: c.PipeID, Data: c.Data}
				for _, r := range c.Redirects {
					pc.Redirects = append(pc.Redirects, pRedirect{Op: r.Op, Target: pw(r.Target), Writes: r.Writes()})
				}
				if len(c.Heredocs) > 0 {
					pc.Heredocs = parity.Texts(c.Heredocs)
				}
				if c.FuncName != "" {
					pc.Func = parity.Text(c.FuncName)
				}
				cmds = append(cmds, pc)
			}
			out["commands"] = cmds
		}
		cases = append(cases, parity.Case{Fn: "Parse", In: parity.Text(src), Out: out})
	}
	parity.Fixture(t, "shellparse", cases)
}
