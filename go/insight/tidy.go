package insight

import (
	"bufio"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/vault"
)

func init() {
	cli.Register("vault-tidy", "drop old small talk from the talk log (greetings, thanks, status checks) and her own voice heard back; shows the lines and asks first: vault-tidy [--path VAULT] [--older-than DAYS] [--yes] [--dry-run]", tidyCommand)
}

// What is never dropped from the talk log, however old: a laugh, the owner's name, something they like or want, a decision, a plan,
// people and pets, or anything they said at length.
var keepRx = regexp.MustCompile(`(?i)\b(haha|hahaha|lol|lmao|rofl|funny|hilarious|cracked me up|matt|matthew|zlothra|i (really )?(like|love|hate|prefer|want|wish|enjoy|miss)|` +
	`my (favou?rite|cat|dog|pet|wife|husband|partner|friend|brother|sister|mom|mum|dad|son|daughter|kid|boss|job|work|project|book|game)|` +
	`(decided|let'?s go with|i'?ll go with|we'?ll|plan(ning)? to|going to|remember that|don'?t forget|call me|my name)\b|😂|🤣|😆)`)

// What is small talk: a short greeting, thanks, an acknowledgement, or the question "how's it going" and its kin, with a short answer
// from her (or a status line, "Your network is stable").
var smallTalkRx = regexp.MustCompile(`(?i)^(hey|hi|hello|yo|good (morning|afternoon|evening|night)|thanks?( you)?( so much| a lot)?|thank you|ok(ay)?|sure|yes|no|yep|nope|alright|cool|nice|sounds good|got it|` +
	`bye|goodbye|see you( later)?|take care|what time is it|how are (you|things)( doing| looking| going)?|how'?s it going|how is it going|how'?s (the )?(system|computer|everything)( doing| looking)?|` +
	`(just )?(wanted to )?(check(ing)? (in|and see)|see) how things (were|are) (looking|going|doing)|(is )?everything (ok|okay|fine|good)|you there|are you there|testing)\W*(sarina\W*)?$`)

// Leading fillers and the trailing words a greeting or a check-in carries ("Yeah, thank you too", "...how things were looking for you").
var fillerRx = regexp.MustCompile(`^((yeah|yes|yep|ok|okay|oh|well|alright|so|um|uh|hey|hi|hello|sarina|please)[,!.]?\s+)+`)
var trailerRx = regexp.MustCompile(`(\s+(too|again|today|there|though|for you|with you|sarina|please|now))*[.!?, ]*$`)

var statusAnswerRx = regexp.MustCompile(`(?i)^(your (network|audio|system|computer)|everything('s| is) (looking good|fine|stable)|all (good|fine)|no (hiccups|issues|problems))`)

// SmallTalk says whether an exchange is small talk that can go once it is old: short on both sides, nothing to keep in it.
func SmallTalk(l Line) bool {
	said := strings.TrimSpace(l.Said)
	if keepRx.MatchString(said) {
		return false
	}
	s := fillerRx.ReplaceAllString(strings.ToLower(strings.TrimSpace(said)), "")
	s = strings.TrimSpace(trailerRx.ReplaceAllString(s, ""))
	words := len(strings.Fields(s))
	if words > 12 {
		return false
	}
	if smallTalkRx.MatchString(s) || smallTalkRx.MatchString(said) {
		return true
	}
	return words <= 6 && statusAnswerRx.MatchString(l.Me)
}

// Tidy says which lines of a talk file would go: echoes of her own voice (any age) and small talk older than `olderThan`.
// Each kept or dropped line comes back with the reason it was dropped.
func Tidy(text string, now time.Time, olderThan time.Duration) (kept []string, dropped []string, why []string) {
	var seen []Line
	for _, raw := range strings.Split(text, "\n") {
		m := talkRx.FindStringSubmatch(strings.TrimSpace(raw))
		if m == nil {
			kept = append(kept, raw)
			continue
		}
		t, _ := time.ParseInLocation("2006-01-02 15:04", m[1], time.Local)
		l := Line{When: t, Said: m[2], Me: m[3]}
		switch {
		case IsEcho(l.Said, seen):
			dropped, why = append(dropped, raw), append(why, "my own voice heard back")
		case now.Sub(t) > olderThan && SmallTalk(l):
			dropped, why = append(dropped, raw), append(why, "small talk")
		default:
			kept = append(kept, raw)
		}
		seen = append(seen, l)
	}
	return kept, dropped, why
}

func tidyCommand(env *cli.Env, args []string) int {
	fs := flag.NewFlagSet("vault-tidy", flag.ContinueOnError)
	fs.SetOutput(env.Stderr)
	path := fs.String("path", env.Getenv(vault.EnvVar), "the vault folder (default $"+vault.EnvVar+")")
	days := fs.Int("older-than", 30, "small talk older than this many days may go (her own voice heard back goes at any age)")
	yes := fs.Bool("yes", false, "do not ask")
	dry := fs.Bool("dry-run", false, "show the lines; change nothing")
	if err := fs.Parse(args); err != nil {
		return 2
	}
	if *path == "" {
		fmt.Fprintln(env.Stderr, "sarina vault-tidy: no vault: give --path or set "+vault.EnvVar)
		return 2
	}
	v, err := vault.Open(*path)
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina vault-tidy:", err)
		return 1
	}
	files, _ := filepath.Glob(filepath.Join(v.Root, "Talks", "20*.md"))
	sort.Strings(files)
	type change struct {
		file string
		text string
	}
	var changes []change
	total := 0
	for _, f := range files {
		b, err := os.ReadFile(f)
		if err != nil {
			continue
		}
		kept, dropped, why := Tidy(string(b), time.Now(), time.Duration(*days)*24*time.Hour)
		if len(dropped) == 0 {
			continue
		}
		rel, _ := filepath.Rel(v.Root, f)
		fmt.Fprintf(env.Stdout, "%s: %d line(s) would go\n", rel, len(dropped))
		for i, d := range dropped {
			line := strings.TrimSpace(d)
			if r := []rune(line); len(r) > 140 {
				line = string(r[:140]) + "..."
			}
			fmt.Fprintf(env.Stdout, "  - (%s) %s\n", why[i], line)
		}
		total += len(dropped)
		changes = append(changes, change{f, strings.Join(kept, "\n")})
	}
	if total == 0 {
		fmt.Fprintln(env.Stdout, "nothing to tidy")
		return 0
	}
	if *dry {
		return 0
	}
	if !*yes {
		fmt.Fprintf(env.Stdout, "Remove these %d line(s)? The vault is in git, so this can be undone. [y/N] ", total)
		in := env.Stdin
		if in == nil {
			in = os.Stdin
		}
		answer, _ := bufio.NewReader(in).ReadString('\n')
		if a := strings.ToLower(strings.TrimSpace(answer)); a != "y" && a != "yes" {
			fmt.Fprintln(env.Stdout, "nothing changed")
			return 0
		}
	}
	for _, c := range changes {
		if err := os.WriteFile(c.file, []byte(c.text), 0o644); err != nil {
			fmt.Fprintln(env.Stderr, "sarina vault-tidy:", err)
			return 1
		}
	}
	v.Commit(fmt.Sprintf("talk log tidied: %d line(s) of small talk or my own voice removed", total))
	fmt.Fprintf(env.Stdout, "removed %d line(s)\n", total)
	return 0
}
