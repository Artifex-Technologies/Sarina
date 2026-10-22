package route

import (
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/vault"
)

func init() {
	cli.Register("route", "what to look up before answering a spoken question: route [--scores] QUESTION (prints topics, comma-separated, or nothing)", routeCommand)
}

func routeCommand(env *cli.Env, args []string) int {
	scores := false
	if len(args) > 0 && args[0] == "--scores" {
		scores, args = true, args[1:]
	}
	q := strings.TrimSpace(strings.Join(args, " "))
	if q == "" {
		fmt.Fprintln(env.Stderr, "usage: sarina route [--scores] QUESTION")
		return 2
	}
	emb := vault.DefaultEmbedder()
	if scores {
		s, err := Scores(q, emb)
		if err != nil {
			fmt.Fprintln(env.Stderr, err)
			return 1
		}
		for _, x := range s {
			fmt.Fprintf(env.Stdout, "%.3f %s\n", x.Value, x.Topic)
		}
		return 0
	}
	topics, err := Route(q, emb)
	if err != nil {
		fmt.Fprintln(env.Stderr, err)
		return 1
	}
	fmt.Fprintln(env.Stdout, strings.Join(topics, ","))
	return 0
}
