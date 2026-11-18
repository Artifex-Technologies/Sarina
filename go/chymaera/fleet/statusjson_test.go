package fleet

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func decode(t *testing.T, text string) map[string]any {
	t.Helper()
	var doc map[string]any
	if err := json.Unmarshal([]byte(text), &doc); err != nil {
		t.Fatalf("not JSON: %v\n%s", err, text)
	}
	return doc
}

func TestStatusJSONCarriesTheSanitisedView(t *testing.T) {
	statuses := []AgentStatus{
		parse(t, "claude", map[string]any{"name": "\x1b]0;pwned\x07Cla‮ude", "usageStatusText": "\x1b[2Jok"}),
		parse(t, "codex", map[string]any{"limits": []any{map[string]any{"label": "w", "percent": "lots"}}}),
	}
	text, err := StatusJSON(statuses, SelfStatus{ID: "sarina", Name: "Sarina", Running: true, LastActive: "2026-09-26T17:59:00Z"}, false)
	if err != nil {
		t.Fatal(err)
	}
	if strings.ContainsAny(text, "\x1b\x07") || strings.Contains(text, `\u001b`) || strings.Contains(text, `‮`) {
		t.Fatalf("an escape or bidi character reached the JSON: %s", text)
	}
	doc := decode(t, text)
	agents := doc["agents"].([]any)
	if len(agents) != 2 {
		t.Fatal(text)
	}
	a := agents[0].(map[string]any)
	if a["id"] != "claude" || a["name"] != "]0;pwnedClaude" || a["statusText"] != "[2Jok" || a["stale"] != false || a["ready"] != true {
		t.Fatalf("%v", a)
	}
	l := a["limits"].([]any)[1].(map[string]any)
	if l["label"] != "Weekly (7-day)" || l["percentUsed"] != float64(55) || l["resetsAt"] != "2026-09-30T00:00:00Z" {
		t.Fatalf("%v", l)
	}
	if p, ok := agents[1].(map[string]any)["limits"].([]any)[0].(map[string]any)["percentUsed"]; !ok || p != nil {
		t.Fatalf("an unknown percent is null, not 0: %v", p)
	}
	self := doc["self"].(map[string]any)
	if self["id"] != "sarina" || self["running"] != true || self["lastActive"] != "2026-09-26T17:59:00Z" {
		t.Fatalf("%v", self)
	}
	if _, ok := doc["mostHeadroom"]; ok {
		t.Fatal("mostHeadroom only with --suggest")
	}
}

func TestStatusJSONEmptyIsAnEmptyListAndSuggestNamesAnID(t *testing.T) {
	text, _ := StatusJSON(nil, SelfStatus{ID: "sarina", Name: "Sarina"}, true)
	doc := decode(t, text)
	if a, ok := doc["agents"].([]any); !ok || len(a) != 0 || doc["mostHeadroom"] != "" {
		t.Fatalf("%s", text)
	}
	text, _ = StatusJSON([]AgentStatus{suggestStatus(t, "codex", fp(20), nil), parse(t, "zed", map[string]any{"limits": nil})}, SelfStatus{}, true)
	doc = decode(t, text)
	if doc["mostHeadroom"] != "codex" {
		t.Fatalf("%s", text)
	}
	if l, ok := doc["agents"].([]any)[1].(map[string]any)["limits"].([]any); !ok || len(l) != 0 {
		t.Fatalf("no limits is [], not null: %s", text)
	}
}

func TestReadSelfTakesTheNewestSessionFolderAndFindsTheService(t *testing.T) {
	a, b := t.TempDir(), t.TempDir()
	old, recent := time.Date(2026, 9, 1, 0, 0, 0, 0, time.UTC), time.Date(2026, 9, 26, 17, 59, 0, 0, time.UTC)
	if err := os.Chtimes(a, old, old); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(b, recent, recent); err != nil {
		t.Fatal(err)
	}
	proc := t.TempDir()
	put := func(pid, cmdline string) {
		t.Helper()
		if err := os.MkdirAll(filepath.Join(proc, pid), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(proc, pid, "cmdline"), []byte(cmdline), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	put("12", "/usr/bin/bash\x00-c\x00sarina-service")
	put("self", "/usr/bin/sarina-service\x00")
	got := ReadSelf([]string{a, filepath.Join(a, "missing"), b}, proc)
	if got.ID != "sarina" || got.Name != "Sarina" || got.LastActive != "2026-09-26T17:59:00Z" {
		t.Fatalf("%+v", got)
	}
	if got.Running {
		t.Fatal("an argument naming the service, or a folder that is not a process id, is not the service")
	}
	put("40", "/usr/bin/sarina-service\x00--host\x00127.0.0.1\x00")
	if !ReadSelf(nil, proc).Running {
		t.Fatal("the service's own process was not found")
	}
	if s := ReadSelf([]string{filepath.Join(a, "missing")}, filepath.Join(proc, "none")); s.LastActive != "" || s.Running {
		t.Fatalf("%+v", s)
	}
}

func TestReadSelfNeverOpensAPlantedPipe(t *testing.T) {
	proc := t.TempDir()
	if err := os.MkdirAll(filepath.Join(proc, "7"), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(filepath.Join(proc, "7", "cmdline"), 0o644); err != nil {
		t.Skip("no named pipes here:", err)
	}
	done := make(chan bool)
	go func() { done <- serviceRunning(proc) }()
	select {
	case r := <-done:
		if r {
			t.Fatal("a pipe is not the service")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a pipe at a cmdline held it")
	}
}

func TestFleetStatusCommandJSON(t *testing.T) {
	d := t.TempDir()
	put(t, d, "codex.json", rec("codex", map[string]any{"updatedAt": time.Now().UTC().Format(time.RFC3339)}))
	saved := procRoot
	procRoot = t.TempDir()
	defer func() { procRoot = saved }()
	stdout, _, code := runCLI(t, nil, "fleet-status", "--usage-dir", d, "--json", "--suggest")
	if code != 0 || strings.Count(stdout, "\n") != 1 {
		t.Fatalf("%d %q", code, stdout)
	}
	doc := decode(t, stdout)
	if doc["mostHeadroom"] != "codex" || doc["agents"].([]any)[0].(map[string]any)["stale"] != false {
		t.Fatalf("%s", stdout)
	}
	if self := doc["self"].(map[string]any); self["id"] != "sarina" || self["running"] != false {
		t.Fatalf("%s", stdout)
	}
}
