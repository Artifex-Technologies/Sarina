package flagx

import (
	"bytes"
	"flag"
	"math/rand"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

var parityArgs = []string{
	"-w", "--w", "-w=true", "-w=false", "-w=0", "-w=T", "-w=x", "-w=", "-W", "-cwd", "--cwd", "--cwd=/x", "-cwd=", "-cwd=a=b", "-n", "-n=5",
	"--n=-3", "0x1f", "0o17", "0b101", "017", "1_000", "_1", "1__0", "-1", "+7", "9223372036854775807", "9223372036854775808",
	"-9223372036854775808", "-9223372036854775809", "99999999999999999999", "x", "--", "-", "---x", "-=x", "--=x", "-h", "-help", "--help",
	"-h=1", "-nope", "a", "b c", "", "-n=", "-b", "-b=false", "--b=f", "é", "-é", "-n=0X_FF", "-n=08", "-n=0x", "-n= 1",
}

func paritySet(out *bytes.Buffer) (*flag.FlagSet, *bool, *string, *int, *bool) {
	fs := flag.NewFlagSet("t", flag.ContinueOnError)
	fs.SetOutput(out)
	w := fs.Bool("w", false, "allow writing")
	cwd := fs.String("cwd", ".", "the working folder")
	n := fs.Int("n", 0, "how many\nat most")
	b := fs.Bool("b", true, "")
	return fs, w, cwd, n, b
}

// TestParityFlagx records Parse (and so Go's flag package: syntax, values, error texts, the usage written on an error) on
// the flagx tests' arguments and generated argument lists (testdata/parity/flagx.json; docs/RUST_PORT.md).
func TestParityFlagx(t *testing.T) {
	lists := [][]string{nil, {"do", "a thing", "-w", "--cwd", "/x", "more"}, {"-w", "prompt"}, {"a", "--", "-w", "--cwd"}, {"-", "x"},
		{"p", "--nope"}, {"p", "-h"}, {"--", "--"}, {"a", "--", "--"}, {"--", "a", "--"}, {"-w", "--"}, {"-n", "5", "--", "-n"}, {"-cwd"}}
	rng := rand.New(rand.NewSource(11))
	for i := 0; i < 1500; i++ {
		var l []string
		for j := rng.Intn(6); j > 0; j-- {
			l = append(l, parityArgs[rng.Intn(len(parityArgs))])
		}
		lists = append(lists, l)
	}
	var cases []parity.Case
	for _, args := range lists {
		var out bytes.Buffer
		fs, w, cwd, n, b := paritySet(&out)
		pos, err := Parse(fs, args)
		cases = append(cases, parity.Case{Fn: "Parse", In: args, Out: map[string]any{
			"positional": pos, "err": parity.Err(err), "output": out.String(), "w": *w, "cwd": *cwd, "n": *n, "b": *b,
		}})
	}
	parity.Fixture(t, "flagx", cases)
}
