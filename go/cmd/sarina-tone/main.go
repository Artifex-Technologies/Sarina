// Command sarina-tone reads how a spoken question sounded and prints a short, private style hint for Sarina (or nothing). See package tone.
//
//	sarina-tone [--baseline FILE] [--json] RECORDING.wav   measure, compare with the usual, update the usual, note the mood, print the hint
//	sarina-tone --pattern                                  how the owner has sounded lately (one or two sentences, or nothing)
//	sarina-tone --forget                                   delete what has been learned about the voice, and the mood history
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/tone"
)

func main() {
	home, _ := os.UserHomeDir()
	def := filepath.Join(home, ".local", "state", "labyrinth", "tone-baseline.json")
	base := flag.String("baseline", def, "where the running averages are kept")
	asJSON := flag.Bool("json", false, "print the measurements and the verdict as JSON")
	hist := flag.String("history", filepath.Join(home, ".local", "state", "labyrinth", "mood-history.jsonl"), "when each question was asked and the mood it was heard as")
	forget := flag.Bool("forget", false, "delete the baseline and the mood history")
	noUpdate := flag.Bool("no-update", false, "measure and compare without learning from this recording")
	pattern := flag.Bool("pattern", false, "print how the owner has sounded lately, or nothing")
	flag.Parse()
	if *forget {
		for _, p := range []string{*base, *hist} {
			if err := os.Remove(p); err != nil && !os.IsNotExist(err) {
				fmt.Fprintln(os.Stderr, "sarina-tone:", err)
				os.Exit(1)
			}
		}
		fmt.Println("forgotten")
		return
	}
	if *pattern {
		if p := tone.Pattern(tone.ReadHistory(*hist), time.Now()); p != "" {
			fmt.Println(p)
		}
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
	mood := b.Mood(f)
	hint := tone.HintFor(mood)
	if !*noUpdate {
		b.Update(f)
		if err := b.Save(*base); err != nil {
			fmt.Fprintln(os.Stderr, "sarina-tone:", err)
		}
		if mood != "" {
			if err := tone.AppendHistory(*hist, tone.Entry{T: time.Now().Unix(), Mood: mood}); err != nil {
				fmt.Fprintln(os.Stderr, "sarina-tone:", err)
			}
		}
	}
	if *asJSON {
		out, _ := json.Marshal(map[string]any{"ok": true, "features": f, "samples": b.N, "mood": mood, "hint": hint})
		fmt.Println(string(out))
		return
	}
	if hint != "" {
		fmt.Println(hint)
	}
}
