package chymaera

import (
	"bufio"
	"bytes"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/cli"
)

// serve starts a loopback console; token "" means no auth. It records requests.
func serve(t *testing.T, token string, captures []any) (port int, requests *[]map[string]any) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	reqs := &[]map[string]any{}
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				defer conn.Close()
				r := bufio.NewReader(conn)
				for {
					line, err := r.ReadString('\n')
					if err != nil {
						return
					}
					var req map[string]any
					json.Unmarshal([]byte(line), &req)
					*reqs = append(*reqs, req)
					resp := map[string]any{"ok": true, "id": req["id"]}
					switch {
					case token != "" && req["token"] != token:
						resp = map[string]any{"ok": false, "error": "unauthorized"}
					case req["cmd"] == "status":
						resp["version"], resp["serverRunning"], resp["eventCount"] = "1.0", true, 4
					case req["cmd"] == "capture.list":
						resp["captures"] = captures
					default:
						resp = map[string]any{"ok": false, "error": "unknown command"}
					}
					b, _ := json.Marshal(resp)
					conn.Write(append(b, '\n'))
				}
			}()
		}
	}()
	return ln.Addr().(*net.TCPAddr).Port, reqs
}

func run(args []string, stdin string, env map[string]string) (code int, out, errOut string) {
	var o, e bytes.Buffer
	code = cli.Main(args, cli.Env{
		Stdin: strings.NewReader(stdin), Stdout: &o, Stderr: &e,
		Getenv:     func(k string) string { return env[k] },
		Interrupts: make(chan os.Signal),
	})
	return code, o.String(), e.String()
}

func TestConsoleCommand(t *testing.T) {
	port, reqs := serve(t, "tok", []any{map[string]any{"type": "nfc", "name": "badge.nfc", "size": 512}})
	code, out, _ := run([]string{"chymaera-console", "--port", fmt.Sprint(port), "--captures", "3"}, "", map[string]string{TokenEnv: "tok"})
	want := "# Chymaera console\n\nversion: 1.0\nserverRunning: True\neventCount: 4\n\n## Captures (1)\n- [nfc] badge.nfc (512 bytes)\n"
	if code != 0 || out != want {
		t.Fatalf("code %d out:\n%s", code, out)
	}
	if len(*reqs) != 2 || (*reqs)[1]["limit"] != float64(3) || (*reqs)[0]["token"] != "tok" {
		t.Fatalf("requests: %v", *reqs)
	}
}

func TestConsoleCommandFailuresExitOne(t *testing.T) {
	port, _ := serve(t, "tok", nil)
	code, out, _ := run([]string{"chymaera-console", "--port", fmt.Sprint(port), "--token", "bad"}, "", nil)
	if code != 1 || strings.TrimSpace(out) != "status: unauthorized" {
		t.Fatalf("code %d out %q", code, out)
	}
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	closed := ln.Addr().(*net.TCPAddr).Port
	ln.Close()
	code, out, _ = run([]string{"chymaera-console", "--port", fmt.Sprint(closed)}, "", nil)
	if code != 1 || !strings.Contains(out, "no Chymaera console at") {
		t.Fatalf("code %d out %q", code, out)
	}
}

func TestAnalyzeCommand(t *testing.T) {
	meta := func(uid string) string { return `{"uid":"` + uid + `","frequency":"433920000"}` }
	port, _ := serve(t, "", []any{
		map[string]any{"type": "nfc", "name": "a.nfc", "meta": meta("AA")},
		map[string]any{"type": "nfc", "name": "b.nfc", "meta": `{"uid":"AA"}`},
	})
	code, out, errOut := run([]string{"chymaera-analyze", "--port", fmt.Sprint(port)}, "", nil)
	if code != 0 || errOut != "" {
		t.Fatalf("code %d err %q", code, errOut)
	}
	for _, want := range []string{"total captures: 2", "- AA: a.nfc, b.nfc", "- 433920000 Hz", "Semantic grouping unavailable:"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in\n%s", want, out)
		}
	}
	code, out, errOut = run([]string{"chymaera-analyze", "--port", fmt.Sprint(port), "--model-dir", "/m"}, "", nil)
	if code != 0 || !strings.HasPrefix(errOut, "semantic grouping unavailable: ") || !strings.Contains(out, "total captures: 2") {
		t.Fatalf("model-dir: code %d err %q out %q", code, errOut, out)
	}
}

func TestDeviceInfoCommand(t *testing.T) {
	healthy := "firmware_origin_fork: Chymaera\nfirmware_version: 1.4.3-CHY-0.1.0\nenclave_valid: true\nradio_alive: true\n"
	code, out, _ := run([]string{"chymaera-device-info"}, healthy, nil)
	if code != 0 || !strings.Contains(out, "No warnings.") {
		t.Fatalf("stdin: code %d\n%s", code, out)
	}
	path := filepath.Join(t.TempDir(), "info.txt")
	os.WriteFile(path, []byte(strings.Replace(healthy, "true", "false", 1)), 0o644)
	code, out, _ = run([]string{"chymaera-device-info", "--file", path}, "", nil)
	if code != 1 || !strings.Contains(out, "## Warnings") || !strings.Contains(out, "enclave_valid is 'false', expected 'true'") {
		t.Fatalf("file: code %d\n%s", code, out)
	}
	code, out, _ = run([]string{"chymaera-device-info", "--expected-commit", "deadbee"}, healthy+"firmware_commit: abc1234\n", nil)
	if code != 1 || !strings.Contains(out, "does not match expected 'deadbee'") {
		t.Fatalf("commit: code %d\n%s", code, out)
	}
	if code, _, _ = run([]string{"chymaera-device-info", "--file", filepath.Join(t.TempDir(), "nope")}, "", nil); code != 1 {
		t.Fatalf("missing file: code %d", code)
	}
}

func TestMergeReportCommandOutsideRepoExitsOne(t *testing.T) {
	code, out, _ := run([]string{"chymaera-merge-report", "--cwd", t.TempDir(), "--upstream-ref", "nope"}, "", nil)
	if code != 1 || !strings.HasPrefix(out, "git merge-base HEAD nope failed:") {
		t.Fatalf("code %d out %q", code, out)
	}
}
