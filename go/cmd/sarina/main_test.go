package main

import "testing"

func TestAliasCommand(t *testing.T) {
	for argv0, want := range map[string]string{
		"sarina-service": "service", "/usr/bin/sarina-service": "service", "/opt/Sarina-GUI.exe": "gui",
		"sarina-gui": "gui", "sarina": "", "/usr/bin/sarina": "", "sarina.exe": "",
	} {
		if got := aliasCommand(argv0); got != want {
			t.Errorf("%q: got %q, want %q", argv0, got, want)
		}
	}
}
