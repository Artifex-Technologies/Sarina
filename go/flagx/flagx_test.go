package flagx

import (
	"flag"
	"io"
	"reflect"
	"testing"
)

func newFS() (*flag.FlagSet, *bool, *string) {
	fs := flag.NewFlagSet("t", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	return fs, fs.Bool("w", false, ""), fs.String("cwd", ".", "")
}

func TestFlagsAnywhere(t *testing.T) {
	fs, w, cwd := newFS()
	pos, err := Parse(fs, []string{"do", "a thing", "-w", "--cwd", "/x", "more"})
	if err != nil {
		t.Fatal(err)
	}
	if !*w || *cwd != "/x" {
		t.Fatalf("flags after positionals were not parsed: w=%v cwd=%q", *w, *cwd)
	}
	if !reflect.DeepEqual(pos, []string{"do", "a thing", "more"}) {
		t.Fatalf("positionals = %q", pos)
	}
}

func TestFlagsFirstStillWork(t *testing.T) {
	fs, w, _ := newFS()
	pos, err := Parse(fs, []string{"-w", "prompt"})
	if err != nil || !*w || !reflect.DeepEqual(pos, []string{"prompt"}) {
		t.Fatalf("pos=%q w=%v err=%v", pos, *w, err)
	}
}

func TestTerminatorEndsFlagParsing(t *testing.T) {
	fs, w, _ := newFS()
	pos, err := Parse(fs, []string{"a", "--", "-w", "--cwd"})
	if err != nil {
		t.Fatal(err)
	}
	if *w {
		t.Fatal("a flag after -- must stay positional")
	}
	if !reflect.DeepEqual(pos, []string{"a", "-w", "--cwd"}) {
		t.Fatalf("positionals = %q", pos)
	}
}

func TestNoArgsAndLoneDash(t *testing.T) {
	fs, _, _ := newFS()
	pos, err := Parse(fs, nil)
	if err != nil || len(pos) != 0 {
		t.Fatalf("pos=%q err=%v", pos, err)
	}
	fs, _, _ = newFS()
	pos, err = Parse(fs, []string{"-", "x"})
	if err != nil || !reflect.DeepEqual(pos, []string{"-", "x"}) {
		t.Fatalf("a lone dash is positional: pos=%q err=%v", pos, err)
	}
}

func TestUnknownFlagAndHelp(t *testing.T) {
	fs, _, _ := newFS()
	if _, err := Parse(fs, []string{"p", "--nope"}); err == nil {
		t.Fatal("an unknown flag after a positional must be an error, not prompt text")
	}
	fs, _, _ = newFS()
	if _, err := Parse(fs, []string{"p", "-h"}); err != flag.ErrHelp {
		t.Fatalf("want ErrHelp, got %v", err)
	}
}
