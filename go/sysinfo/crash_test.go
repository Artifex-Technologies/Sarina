package sysinfo

import (
	"strings"
	"testing"
)

func TestTheNewestCrashIsNamedAndClassed(t *testing.T) {
	list := "TIME PID UID GID SIG COREFILE EXE SIZE\n" +
		"Sun 2026-10-25 08:45:26 MDT   25721 1000 1000 SIGSEGV present      /tmp/sarina-evals/crashy/crashy   20.4K\n" +
		"Sat 2026-10-24 23:53:35 MDT  822546 1000 1000 SIGSEGV present      /home/u/.local/share/Steam/steamapps/common/Game/game.exe 1.2M\n"
	got := newestCrash(list)
	if !strings.Contains(got, "crashy") || !strings.Contains(got, "not a game") {
		t.Fatal(got)
	}
	if got := newestCrash("Sat 2026-10-24 23:53:35 MDT  822546 1000 1000 SIGSEGV present /home/u/.local/share/Steam/steamapps/common/Game/game.exe 1.2M"); !strings.Contains(got, "is a game") {
		t.Fatal(got)
	}
	if !strings.Contains(newestCrash("No coredumps found."), "No crash") {
		t.Fatal("none")
	}
}
