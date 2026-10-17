// Command sarina-tone reads how a spoken question sounded and prints a short, private style hint for Sarina (or nothing). See package tone.
//
//	sarina-tone [--baseline FILE] [--json] RECORDING.wav   measure, compare with the usual, update the usual, print the hint
//	sarina-tone --baseline FILE --forget                   delete what has been learned about the voice
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"

	"github.com/Artifex-Technologies/Sarina/go/tone"
)

func main() {
	home, _ := os.UserHomeDir()
	def := filepath.Join(home, ".local", "state", "labyrinth", "tone-baseline.json")
	base := flag.String("baseline", def, "where the running averages are kept")
	asJSON := flag.Bool("json", false, "print the measurements and the verdict as JSON")
	forget := flag.Bool("forget", false, "delete the baseline")
	noUpdate := flag.Bool("no-update", false, "measure and compare without learning from this recording")
	flag.Parse()
	if *forget {
		if err := os.Remove(*base); err != nil && !os.IsNotExist(err) {
			fmt.Fprintln(os.Stderr, "sarina-tone:", err)
			os.Exit(1)
		}
		fmt.Println("forgotten")
		return
	}
	if flag.NArg() != 1 {
		fmt.Fprintln(os.Stderr, "usage: sarina-tone [--baseline FILE] [--json] RECORDING.wav")
		os.Exit(2)
	}
	x, rate, err := tone.ReadWAV(flag.Arg(0))
	if err != nil {
		fmt.Fprintln(os.Stderr, "sarina-tone:", err)
		os.Exit(1)
	}
	f, ok := tone.Analyze(x, rate)
	if !ok {
		if *asJSON {
			fmt.Println(`{"ok":false}`)
		}
		return // too little speech: nothing to say, nothing learned
	}
	b := tone.Load(*base)
	hint := b.Hint(f)
	if !*noUpdate {
		b.Update(f)
		if err := b.Save(*base); err != nil {
			fmt.Fprintln(os.Stderr, "sarina-tone:", err)
		}
	}
	if *asJSON {
		out, _ := json.Marshal(map[string]any{"ok": true, "features": f, "samples": b.N, "hint": hint})
		fmt.Println(string(out))
		return
	}
	if hint != "" {
		fmt.Println(hint)
	}
}
