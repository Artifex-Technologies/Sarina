package limbo

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

func httpReply(status int, body string) string {
	return fmt.Sprintf("HTTP/1.1 %d X\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s", status, len(body), body)
}

func resultOut(r RunResult) any {
	var exit any
	if r.ExitCode != nil {
		exit = *r.ExitCode
	}
	return map[string]any{"id": r.RunID, "profile": r.Profile, "status": r.Status, "exit": exit, "stdout": parity.Text(r.Stdout),
		"stderr": parity.Text(r.Stderr), "truncated": r.Truncated, "error": parity.Text(r.Error), "code": r.Code, "remedy": r.Remedy,
		"infra": r.InfrastructureFailure(), "ok": r.Succeeded(), "text": parity.Text(r.AsText())}
}

// limboScenario is one call against a scripted Limbo: the answers in order and what is asked.
type limboScenario struct {
	call    string // list, describe, validate, bundle
	replies []string
	profile string
	command string
	bundle  string
	timeout int
}

// TestParityLimbo records the Limbo client (testdata/parity/limbo.json): profiles by name and with details, the skipped
// list Limbo reports, a validation run (start, polling, the result, its text for the model), bundles, Limbo's errors,
// damaged and over-long answers, and the socket rules, against a scripted unix socket. {SOCK} is the socket's path.
func TestParityLimbo(t *testing.T) {
	long := strings.Repeat("y", 70)
	skipped := `[{"name":"bad-one","code":"profile_invalid","reason":"line 3:\n  unknown key\u0007","remedy":"fix\tthe file"},{"name":5,"code":"x","reason":"r","remedy":"m"},` +
		`{"name":"","code":"no_image","reason":"","remedy":""},{"name":"n","code":"","reason":"r","remedy":"m"},{"name":"` + long + `","code":"` + long + `","reason":"` +
		strings.Repeat("z ", 150) + `","remedy":"\u202eevil\u202c"}` + strings.Repeat(`,{"name":"m","code":"c","reason":"","remedy":""}`, 25) + `]`
	details := `{"profiles":["arch-update"],"details":[{"name":"arch-update","network":"allowlist","sudo":true,"max_seconds":900},` +
		`{"name":"web","network":"internet-only","sudo":false,"max_seconds":1.5},{"name":"x","network":"weird","max_seconds":-3},{"name":"Bad"},5,` +
		`{"name":"lan","network":"none","sudo":"yes","max_seconds":60}],"skipped":` + skipped + `}`
	stdout := "line one\u001b[31m red\u0000\r\nbidi \u202ehidden\u202c and \u200bzero\u0085" + strings.Repeat("é", 17000)
	done := httpReply(200, `{"id":"r0123abcd","profile":"arch-update","status":"done","exit_code":0,"stdout":`+jsonString(stdout)+`,"stderr":"warn","truncated":true}`)
	scen := []limboScenario{
		{call: "list", replies: []string{httpReply(200, `{"profiles":["arch-update","Bad","x",5,"a-very-long-name-that-is-way-too-long-for-the-rule"]}`)}},
		{call: "describe", replies: []string{httpReply(200, details)}},
		{call: "describe", replies: []string{httpReply(200, `{"profiles":["arch-update","web","Nope"]}`)}},
		{call: "list", replies: []string{httpReply(404, `{"error":"no such route"}`)}},
		{call: "list", replies: []string{httpReply(500, `{"error":""}`)}},
		{call: "list", replies: []string{httpReply(503, `[]`)}},
		{call: "list", replies: []string{httpReply(200, `not json`)}},
		{call: "list", replies: []string{httpReply(200, ``)}},
		{call: "describe", replies: []string{httpReply(200, `[]`)}},
		{call: "list", replies: []string{"@big:1048577"}},
		{call: "list", replies: []string{"@big:1048576"}},
		{call: "list", replies: []string{"garbage\r\n\r\n"}},
		{call: "validate", profile: "arch-update", command: "pacman -Syu --noconfirm", timeout: 600,
			replies: []string{httpReply(200, `{"id":"r0123abcd"}`), httpReply(200, `{"id":"r0123abcd","status":"running"}`), done}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(200, `{"id":"R0123ABCD"}`)}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(200, `{"id":"r0123abcd"}`),
			httpReply(200, `{"id":"r0123abcd","profile":"arch-update","status":"failed","error":"guest did not boot","code":"guest_boot_failed","remedy":"check the image"}`)}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(200, `{"id":"r0123abcd"}`),
			httpReply(200, `{"id":5,"profile":true,"status":"timeout","exit_code":1.5,"stdout":{"a":[1,"b"]},"remedy":"raise the limit"}`)}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(200, `{"id":"r0123abcd"}`),
			httpReply(200, `{"id":"r0123abcd","status":"done","exit_code":2,"code":"output_capped","remedy":"less output","error":"cap"}`)}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(200, `{"id":"r0123abcd"}`),
			httpReply(200, `{"profile":"arch-update"}`)}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(200, `{"id":"r0123abcd"}`), httpReply(500, `{"error":"lost the run"}`)}},
		{call: "validate", profile: "arch-update", command: "x", timeout: 1, replies: []string{httpReply(429, `{"error":"too many runs"}`)}},
		{call: "validate", profile: "Arch", command: "x"}, {call: "validate", profile: "arch-update", command: "  \t"},
		{call: "bundle", profile: "forge", command: "python3 main.py", bundle: "tar\x00bytes\xff", timeout: 30,
			replies: []string{httpReply(200, `{"id":"rdeadbeef"}`), httpReply(200, `{"id":"rdeadbeef","status":"done","exit_code":0}`)}},
		{call: "bundle", profile: "forge", command: "x"},
	}
	var cases []parity.Case
	for i, sc := range scen {
		dir, err := os.MkdirTemp("", "lmb")
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { os.RemoveAll(dir) })
		sock := filepath.Join(dir, "agent.sock")
		got := parity.RawUnixServer(t, sock, sc.replies)
		c := &Client{Path: sock, PollInterval: time.Millisecond}
		out := map[string]any{}
		errText := func(err error) string { return strings.ReplaceAll(parity.Err(err), sock, "{SOCK}") }
		switch sc.call {
		case "list":
			names, err := c.ListProfiles()
			out["names"], out["error"] = names, errText(err)
		case "describe":
			l, err := c.DescribeProfilesAndSkipped()
			var infos []string
			for _, p := range l.Profiles {
				infos = append(infos, p.AsText())
			}
			out["profiles"], out["skipped"], out["error"] = infos, l.SkippedLines(), errText(err)
		case "validate", "bundle":
			var r RunResult
			var err error
			if sc.call == "bundle" {
				r, err = c.ValidateBundle(sc.profile, sc.command, []byte(sc.bundle), sc.timeout)
			} else {
				r, err = c.Validate(sc.profile, sc.command, sc.timeout)
			}
			out["result"], out["error"] = resultOut(r), errText(err)
		}
		out["requests"] = got()
		cases = append(cases, parity.Case{Fn: "call", In: map[string]any{"i": i, "call": sc.call, "replies": parity.Texts(sc.replies), "profile": sc.profile,
			"command": sc.command, "bundle": parity.Text(sc.bundle), "timeout": sc.timeout}, Out: out})
	}
	// the socket rules
	for _, p := range []string{"", "/run/limbo/admin.sock", "/run/limbo/control.sock", "/run/limbo/agent.sock.evil", `C:\x\admin.sock`, `C:\x\agent.sock`,
		"/nonexistent-sarina-parity/agent.sock", "agent.sock"} {
		t.Setenv(EnvVar, "")
		_, err := (&Client{Path: p}).ListProfiles()
		cases = append(cases, parity.Case{Fn: "socket", In: p, Out: parity.Err(err)})
	}
	parity.Fixture(t, "limbo", cases)
}

func jsonString(s string) string {
	b := &strings.Builder{}
	b.WriteByte('"')
	for _, r := range s {
		switch {
		case r == '"' || r == '\\':
			b.WriteString(`\` + string(r))
		case r < 0x20 || (r >= 0x7f && r <= 0x9f) || (r >= 0x2000 && r <= 0x206f):
			fmt.Fprintf(b, `\u%04x`, r)
		default:
			b.WriteRune(r)
		}
	}
	b.WriteByte('"')
	return b.String()
}
