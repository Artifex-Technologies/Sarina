package mcp

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// fakeServer is a stand-in MCP server both languages' tests run (sh, sed, head, tr), so the client is compared on the
// same process. It answers each request (a line starting {"id": with a "method") with the next line of <dir>/replies and
// logs it to <dir>/requests; any other line (notifications, answers to its own requests) goes to <dir>/notes. A reply
// line is parts separated by "|||", each printed as a line, after its directives: "@wait:S " sleeps S seconds,
// "@xline:N " prints a line of N 'x', "@text:ID:N" prints a result for ID whose text is N 'x', "@flood:N " prints N ping
// requests without reading, "@exit:C" writes "fatal: gone (C)" to stderr and exits with C, "@none" prints nothing. At
// start it writes <dir>/stderr (if any) to stderr and its directory, FAKE_VAR and FAKE_KEEP to <dir>/env; at the end of
// its input it exits, unless <dir>/stubborn exists (then it sleeps until killed).
const fakeServer = `#!/bin/sh
dir="$1"
[ -f "$dir/stderr" ] && cat "$dir/stderr" >&2
printf 'pwd=%s\nFAKE_VAR=%s\nFAKE_KEEP=%s\nargs=%s\n' "$(pwd)" "${FAKE_VAR-unset}" "${FAKE_KEEP-unset}" "$*" > "$dir/env"
n=0
while IFS= read -r line; do
  case "$line" in
    '{"id":'*'"method":'*) n=$((n+1)); printf '%s\n' "$line" >> "$dir/requests" ;;
    *) printf '%s\n' "$line" >> "$dir/notes"; continue ;;
  esac
  r=$(sed -n "${n}p" "$dir/replies")
  while :; do
    case "$r" in
      '@wait:'*) w=${r#@wait:}; sleep "${w%% *}"; r=${w#* } ;;
      '@xline:'*) w=${r#@xline:}; head -c "${w%% *}" /dev/zero | tr '\0' x; echo; r=${w#* } ;;
      '@flood:'*) w=${r#@flood:}; i=0; while [ $i -lt "${w%% *}" ]; do echo "{\"jsonrpc\":\"2.0\",\"id\":\"f$i\",\"method\":\"ping\"}"; i=$((i+1)); done; r=${w#* } ;;
      *) break ;;
    esac
  done
  case "$r" in
    '@text:'*) w=${r#@text:}; printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"' "${w%%:*}"; head -c "${w#*:}" /dev/zero | tr '\0' x; echo '"}]}}' ;;
    '@exit:'*) echo "fatal: gone (${r#@exit:})" >&2; exit "${r#@exit:}" ;;
    '@none') ;;
    *) printf '%s\n' "$r" | sed 's/|||/\n/g' ;;
  esac
done
[ -f "$dir/stubborn" ] && sleep 30
exit 0
`

// mcpOp is one client call in a scenario.
type mcpOp struct {
	Op       string         `json:"op"` // init, tools, resources, call, read, raw
	Name     string         `json:"name,omitempty"`
	Args     map[string]any `json:"args,omitempty"`
	URI      string         `json:"uri,omitempty"`
	Params   any            `json:"params,omitempty"`
	TimeMs   int            `json:"timeMs"`
	CancelMs int            `json:"cancelMs,omitempty"`
}

type mcpScenario struct {
	Dir      string            `json:"dir"`
	Replies  []string          `json:"replies"`
	Stderr   string            `json:"stderr,omitempty"`
	Stubborn bool              `json:"stubborn,omitempty"`
	Env      map[string]string `json:"env,omitempty"`
	Cwd      string            `json:"cwd,omitempty"`
	Ops      []mcpOp           `json:"ops"`
}

// short is a long text as its start, length and sha256.
func short(s string) any {
	if len(s) <= 4096 {
		return parity.Text(s)
	}
	h := sha256.Sum256([]byte(s))
	return []any{s[:100], len(s), hex.EncodeToString(h[:])}
}

func mcpScenarios() []mcpScenario {
	r := func(id int, result string) string {
		return fmt.Sprintf(`{"jsonrpc":"2.0","id":%d,"result":%s}`, id, result)
	}
	init1 := r(1, `{"protocolVersion":"2025-06-18","capabilities":{}}`)
	t := 3000
	// the exactly-at-the-bound result line: {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"..."}]}}
	frame := len(`{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":""}]}}`)
	return []mcpScenario{
		{Dir: "main", Replies: []string{init1,
			r(2, `{"tools":[{"name":"echo","description":"Echo <it> & more","inputSchema":{"type":"object","properties":{"x":{"type":"string"}}}},{"name":"","description":"nameless"}],"nextCursor":"p2"}`),
			r(3, `{"tools":[{"name":"slow"},{"name":"bad","inputSchema":5}],"nextCursor":""}`),
			r(4, `{"resources":[{"uri":"file:///a","name":"a","description":"the a","mimeType":"text/plain"},{"name":"no uri"}],"nextCursor":"r2"}`),
			r(5, `{"resources":[{"uri":"file:///b"}]}`),
			r(6, `{"content":[{"type":"text","text":"hello"},{"type":"image","data":"QUJD","mimeType":"image/png"},{"type":"resource","resource":{"uri":"file:///b","blob":"AAAA"}},`+
				`{"type":"resource","resource":{"uri":"file:///c","text":"inline"}},{"type":"resource_link","uri":"file:///d"},{"type":"audio","data":""},{"data":"QQ=="},{"weird":[1,"<"]},`+
				`{"type":5,"mimeType":7,"blob":"x"}],"structuredContent":{"b":1,"a":[true,null,"é <"]}}`),
			r(7, `{"contents":[{"uri":"file:///a","text":"body"},{"uri":"x","blob":"Zm9v","mimeType":"application/octet-stream"}]}`),
			r(8, `{"content":[],"isError":true}`),
			`{"jsonrpc":"2.0","id":9,"error":{"code":-32000,"message":"nope <x>","data":{"k":1}}}`,
			`{"jsonrpc":"2.0","id":10}`,
			`{"jsonrpc":"2.0","id":11,"result": { "a" : [1, 2.50, "éé"] } }`,
			r(12, `[1]`), r(13, `{"content":[{"type":"text","text":"  "}]}`), r(14, `{"contents":5}`), r(15, `{"tools":5}`),
			r(16, `{"content":[],"structuredContent":{"x":1e400}}`),
		}, Ops: []mcpOp{{Op: "init", TimeMs: t}, {Op: "tools", TimeMs: t}, {Op: "resources", TimeMs: t},
			{Op: "call", Name: "echo", Args: map[string]any{"x": "<b> & é"}, TimeMs: t}, {Op: "read", URI: "file:///a", TimeMs: t},
			{Op: "call", Name: "failing", TimeMs: t}, {Op: "raw", Name: "custom/x", Params: map[string]any{"a": []any{1, "<"}}, TimeMs: t},
			{Op: "raw", Name: "custom/none", TimeMs: t}, {Op: "raw", Name: "custom/spaced", TimeMs: t},
			{Op: "call", Name: "array", TimeMs: t}, {Op: "call", Name: "blank", TimeMs: t}, {Op: "read", URI: "x", TimeMs: t},
			{Op: "tools", TimeMs: t}, {Op: "call", Name: "huge", TimeMs: t}}},
		{Dir: "noise", Replies: []string{
			`not json|||{"jsonrpc":"2.0","method":"notifications/message","params":{}}|||{"jsonrpc":"2.0","id":99,"result":{}}|||` +
				`{"jsonrpc":"2.0","id":"s1","method":"ping"}|||{"jsonrpc":"2.0","id":7,"method":"sampling/createMessage","params":{}}|||` +
				`{"jsonrpc":"2.0","method":5,"id":1,"result":{}}|||{"jsonrpc":"2.0","id":"1","result":{"ok":true}}`,
			`{"jsonrpc":"2.0","ID":2,"Result":{"case":1}}`,
			`{"jsonrpc":"2.0","id":3,"result":1,"result":2}`,
			`{"jsonrpc":"2.0","id":{"x":"<a>"},"method":"ping"}|||{"jsonrpc":"2.0","id": "s 2" ,"method":"ping"}|||{"jsonrpc":"2.0","id":null,"method":"ping"}|||{"jsonrpc":"2.0","id":"+4","result":{"plus":1}}`,
			`{"jsonrpc":"2.0","id":5,"error":{"code":1.5,"message":"x"}}|||{"jsonrpc":"2.0","id":5,"error":null,"result":"second"}`,
			`{"jsonrpc":"2.0","id":" 6","error":{"message":"no code"}}`,
			`{"jsonrpc":"2.0","id":7 ,"result":"tail"}  `,
		}, Ops: []mcpOp{{Op: "init", TimeMs: t}, {Op: "raw", Name: "a", TimeMs: t}, {Op: "raw", Name: "b", TimeMs: t}, {Op: "raw", Name: "c", TimeMs: t},
			{Op: "raw", Name: "d", TimeMs: t}, {Op: "raw", Name: "e", TimeMs: t}, {Op: "raw", Name: "f", TimeMs: t}}},
		{Dir: "timeout", Replies: []string{init1, `@wait:1 ` + r(2, `{"content":[{"type":"text","text":"late"}]}`), r(3, `{"content":[{"type":"text","text":"on time"}]}`)},
			Ops: []mcpOp{{Op: "init", TimeMs: t}, {Op: "call", Name: "slow", TimeMs: 300}, {Op: "call", Name: "echo", TimeMs: t}}},
		{Dir: "cancel", Replies: []string{init1, `@wait:1 ` + r(2, `{}`), r(3, `{"after":true}`)},
			Ops: []mcpOp{{Op: "init", TimeMs: t}, {Op: "raw", Name: "slow", TimeMs: t, CancelMs: 200}, {Op: "raw", Name: "next", TimeMs: t}}},
		{Dir: "dies", Stderr: "starting up\nwarning: one\n", Replies: []string{init1, `@exit:3`},
			Ops: []mcpOp{{Op: "init", TimeMs: t}, {Op: "call", Name: "x", TimeMs: t}}},
		{Dir: "tail", Stderr: strings.Repeat("a line of the server's log that goes on for a while\n", 60), Replies: []string{`@exit:1`},
			Ops: []mcpOp{{Op: "init", TimeMs: t}}},
		{Dir: "lines", Replies: []string{init1, `@xline:9437184 ` + r(2, `{"after":"long"}`), fmt.Sprintf("@text:3:%d", MaxLineBytes-frame),
			fmt.Sprintf("@text:4:%d", MaxLineBytes-frame+1)},
			Ops: []mcpOp{{Op: "init", TimeMs: t}, {Op: "raw", Name: "a", TimeMs: t}, {Op: "call", Name: "exact", TimeMs: t}, {Op: "call", Name: "over", TimeMs: 1000}}},
		{Dir: "flood", Replies: []string{`@flood:5000 ` + init1, r(2, `{"still":"here"}`)},
			Ops: []mcpOp{{Op: "init", TimeMs: 10000}, {Op: "raw", Name: "a", TimeMs: 10000}}},
		{Dir: "env", Env: map[string]string{"FAKE_VAR": "over"}, Cwd: "work", Replies: []string{init1}, Ops: []mcpOp{{Op: "init", TimeMs: t}}},
		{Dir: "stubborn", Stubborn: true, Replies: []string{init1}, Ops: []mcpOp{{Op: "init", TimeMs: t}}},
	}
}

// TestParityMCPClient records the stdio client (testdata/parity/mcp.json) against the shell stand-in: the handshake,
// tool and resource listings with pages, tool calls and resource reads rendered as text, JSON-RPC errors, the server's
// own requests answered, noise ignored, a timeout or cancel that leaves the connection usable, a dying server's reason,
// the line bound, a flood of requests, the child's environment and folder, Close; and Spawn's errors. {DIR} is the test
// folder.
func TestParityMCPClient(t *testing.T) {
	if testing.Short() {
		t.Skip("timed cases")
	}
	root := t.TempDir()
	server := filepath.Join(root, "fake-mcp")
	os.WriteFile(server, []byte(fakeServer), 0o755)
	cwd, _ := os.Getwd()
	clean := func(s string) string {
		// the test's own folder (a child without its own folder starts there) differs between the languages' tests
		return strings.ReplaceAll(strings.ReplaceAll(s, root, "{DIR}"), "pwd="+cwd+"\n", "pwd={CWD}\n")
	}
	var cases []parity.Case
	base := []string{"FAKE_VAR=base", "FAKE_KEEP=kept", "PATH=/usr/bin:/bin"}
	for _, sc := range mcpScenarios() {
		dir := filepath.Join(root, sc.Dir)
		os.MkdirAll(dir, 0o755)
		os.WriteFile(filepath.Join(dir, "replies"), []byte(strings.Join(sc.Replies, "\n")+"\n"), 0o644)
		if sc.Stderr != "" {
			os.WriteFile(filepath.Join(dir, "stderr"), []byte(sc.Stderr), 0o644)
		}
		if sc.Stubborn {
			os.WriteFile(filepath.Join(dir, "stubborn"), nil, 0o644)
		}
		def := ServerDef{Name: "fake " + sc.Dir, Command: server, Args: []string{dir}, Env: sc.Env}
		if sc.Cwd != "" {
			def.Cwd = filepath.Join(dir, sc.Cwd)
			os.MkdirAll(def.Cwd, 0o755)
		}
		c, err := Spawn(def, base)
		if err != nil {
			t.Fatal(err)
		}
		var results []any
		for _, op := range sc.Ops {
			ctx, cancel := context.Background(), context.CancelFunc(func() {})
			if op.CancelMs > 0 {
				ctx, cancel = context.WithCancel(ctx)
				time.AfterFunc(time.Duration(op.CancelMs)*time.Millisecond, cancel)
			}
			d := time.Duration(op.TimeMs) * time.Millisecond
			var out any
			switch op.Op {
			case "init":
				err = c.Initialize(ctx, d)
				out = nil
			case "tools":
				var list []ToolInfo
				list, err = c.ListTools(ctx, d)
				var o []any
				for _, x := range list {
					var s any
					if x.InputSchema != nil {
						s = parity.Typed(x.InputSchema)
					}
					o = append(o, []any{x.Name, x.Description, s})
				}
				out = o
			case "resources":
				var list []ResourceInfo
				list, err = c.ListResources(ctx, d)
				var o []any
				for _, x := range list {
					o = append(o, []string{x.URI, x.Name, x.Description, x.MimeType})
				}
				out = o
			case "call":
				var res CallResult
				res, err = c.CallTool(ctx, op.Name, op.Args, d)
				out = []any{short(res.Text), res.IsError}
			case "read":
				var s string
				s, err = c.ReadResource(ctx, op.URI, d)
				out = short(s)
			case "raw":
				raw, e := c.Call(ctx, op.Name, op.Params, d)
				err = e
				if raw == nil {
					out = nil
				} else {
					out = short(string(raw))
				}
			}
			cancel()
			results = append(results, []any{out, clean(parity.Err(err))})
		}
		alive := c.Alive()
		start := time.Now()
		c.Close()
		prompt := time.Since(start) < 3*time.Second
		read := func(name string) string { b, _ := os.ReadFile(filepath.Join(dir, name)); return clean(string(b)) }
		notes := strings.Split(strings.TrimSpace(read("notes")), "\n")
		sort.Strings(notes)
		out := map[string]any{"results": results, "alive": alive, "prompt": prompt, "tail": parity.Text(c.StderrTail()),
			"requests": short(read("requests")), "notes": notes, "env": read("env"), "dropped": c.dropped.Load() > 0}
		if sc.Dir == "flood" {
			// how many answers the server's full pipe took before the rest were dropped depends on the machine: only
			// that some were dropped, and that the notes are answers to the flood, is compared
			flood := 0
			for _, n := range notes {
				if strings.HasPrefix(n, `{"id":"f`) && strings.HasSuffix(n, `,"jsonrpc":"2.0","result":{}}`) {
					flood++
				}
			}
			out["notes"] = flood > 0 && flood < 5000
		}
		cases = append(cases, parity.Case{Fn: "client", In: sc, Out: out})
	}
	// Spawn's errors
	os.WriteFile(filepath.Join(root, "noexec"), []byte("#!/bin/sh\n"), 0o644)
	t.Setenv("PATH", filepath.Join(root, "bin")+":/usr/bin:/bin")
	for _, def := range []ServerDef{{Name: "a", Command: "no-such-mcp-server"}, {Name: "b", Command: filepath.Join(root, "missing")},
		{Name: "c", Command: filepath.Join(root, "noexec")}, {Name: "d", Command: server, Cwd: filepath.Join(root, "no-such-dir")},
		{Name: "e", Command: root}} {
		_, err := Spawn(def, base)
		cases = append(cases, parity.Case{Fn: "spawn", In: []string{def.Name, clean(def.Command), clean(def.Cwd)}, Out: clean(parity.Err(err))})
	}
	// overlayEnv
	for _, extra := range []map[string]string{nil, {"FAKE_VAR": "x"}, {"NEW": "1"}} {
		out := overlayEnv([]string{"A=1", "FAKE_VAR=base", "NOEQUALS", "=weird", "A=2"}, extra)
		cases = append(cases, parity.Case{Fn: "overlay", In: extra, Out: out})
	}
	cases = append(cases, parity.Case{Fn: "script", In: fakeServer, Out: []any{MaxLineBytes, stderrKeep, maxReplies, ProtocolVersion}})
	parity.Fixture(t, "mcp", cases)
}
