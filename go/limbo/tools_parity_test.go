package limbo

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// TestParityLimboTools records the limbo_profiles and limbo_validate tools (testdata/parity/limbotools.json) through
// the registry against a scripted agent socket: the listing and its empty forms, the timeout clamped (10..900, read
// leniently), blank arguments, an infrastructure failure reported as an error with its metadata, Limbo's errors, the
// shell gate, a missing socket. {SOCK} is the socket's path.
func TestParityLimboTools(t *testing.T) {
	start := httpReply(200, `{"id":"r0123abcd"}`)
	done := func(extra string) string {
		return httpReply(200, `{"id":"r0123abcd","profile":"arch-update","status":"done","exit_code":0,"stdout":"5.15.0\n"`+extra+`}`)
	}
	details := `{"profiles":["arch-update","web"],"details":[{"name":"arch-update","network":"allowlist","sudo":true,"max_seconds":900},` +
		`{"name":"web","network":"internet-only","sudo":false,"max_seconds":120}],"skipped":[{"name":"bad","code":"profile_invalid","reason":"line 3","remedy":"fix it"}]}`
	type sc struct {
		tool, args string
		replies    []string
		noShell    bool
		noSocket   bool
	}
	scen := []sc{
		{tool: "limbo_profiles", args: `{}`, replies: []string{httpReply(200, details)}},
		{tool: "limbo_profiles", args: `{}`, replies: []string{httpReply(200, `{"profiles":[],"skipped":[{"name":"x","code":"no_image","reason":"r","remedy":"m"},{"name":"y","code":"c","reason":"","remedy":""}]}`)}},
		{tool: "limbo_profiles", args: `{}`, replies: []string{httpReply(200, `{"profiles":[]}`)}},
		{tool: "limbo_profiles", args: `{}`, replies: []string{httpReply(500, `{"error":"limbo is down"}`)}},
		{tool: "limbo_profiles", args: `{}`, noSocket: true},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"uname -r","timeout_seconds":5}`, replies: []string{start, done("")}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"uname -r","timeout_seconds":1000}`, replies: []string{start, done(`,"stderr":"warn"`)}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"uname -r","timeout_seconds":"30"}`, replies: []string{start, done("")}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"uname -r","timeout_seconds":2.5}`, replies: []string{start, done("")}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"uname -r","timeout_seconds":true}`, replies: []string{start, done("")}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"uname -r"}`, replies: []string{start, done(`,"truncated":true`)}},
		{tool: "limbo_validate", args: `{"profile":"  ","command":"x"}`},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":""}`},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":5}`},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"x"}`, replies: []string{start,
			httpReply(200, `{"id":"r0123abcd","profile":"arch-update","status":"failed","error":"guest did not boot","code":"guest_boot_failed","remedy":"check the image"}`)}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"x"}`, replies: []string{start,
			httpReply(200, `{"id":"r0123abcd","profile":"arch-update","status":"done","exit_code":3,"stderr":"E: failed"}`)}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"x"}`, replies: []string{start,
			httpReply(200, `{"id":"r0123abcd","status":"timeout","remedy":"raise the limit"}`)}},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"x"}`, replies: []string{httpReply(429, `{"error":"too many runs"}`)}},
		{tool: "limbo_validate", args: `{"profile":"Arch Update","command":"x"}`},
		{tool: "limbo_validate", args: `{"profile":"arch-update","command":"x"}`, noShell: true},
	}
	var cases []parity.Case
	reg := tools.NewRegistry(Tools()...)
	var specs []any
	for _, sp := range reg.Specs() {
		b, _ := json.Marshal(sp.Parameters)
		specs = append(specs, []any{sp.Name, sp.Description, string(b)})
	}
	cases = append(cases, parity.Case{Fn: "specs", Out: specs})
	for i, s := range scen {
		dir, err := os.MkdirTemp("", "lmt")
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { os.RemoveAll(dir) })
		sock := filepath.Join(dir, "agent.sock")
		var got func() []map[string]any
		if !s.noSocket {
			got = parity.RawUnixServer(t, sock, s.replies)
			t.Setenv(EnvVar, sock)
		} else {
			t.Setenv(EnvVar, "")
		}
		var args map[string]any
		if err := json.Unmarshal([]byte(s.args), &args); err != nil {
			t.Fatal(err)
		}
		tc := &tools.Context{Permissions: config.Permissions{AllowShellCommands: !s.noShell}}
		r := reg.Execute(context.Background(), s.tool, args, tc)
		md, _ := json.Marshal(r.Metadata)
		out := []any{r.OK, strings.ReplaceAll(r.Content, sock, "{SOCK}"), string(md)}
		var reqs any
		if got != nil {
			reqs = got()
		}
		cases = append(cases, parity.Case{Fn: "call", In: []any{i, s.tool, s.args, s.replies, s.noShell, s.noSocket}, Out: []any{out, reqs}})
	}
	parity.Fixture(t, "limbotools", cases)
}
