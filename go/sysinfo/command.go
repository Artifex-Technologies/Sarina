package sysinfo

import (
	"context"
	"fmt"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

func init() {
	cli.Register("crash-detail", "the latest crash of one program (read-only): crash-detail PROGRAM", crashCommand)
	cli.Register("system-status", "print what the computer reports about one topic (read-only): system-status performance|network|disk|logs|services|hardware|crashes|boot|audio|kernel|gaming", statusCommand)
}

func statusCommand(env *cli.Env, args []string) int {
	if len(args) != 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina system-status TOPIC ("+strings.Join(names(), ", ")+")")
		return 2
	}
	out, err := Report(context.Background(), args[0])
	if err != nil {
		fmt.Fprintln(env.Stderr, err)
		return 2
	}
	fmt.Fprintln(env.Stdout, out)
	return 0
}

func crashCommand(env *cli.Env, args []string) int {
	if len(args) != 1 {
		fmt.Fprintln(env.Stderr, "usage: sarina crash-detail PROGRAM")
		return 2
	}
	out, err := CrashDetail(context.Background(), args[0])
	if err != nil {
		fmt.Fprintln(env.Stderr, err)
		return 2
	}
	fmt.Fprintln(env.Stdout, out)
	return 0
}
