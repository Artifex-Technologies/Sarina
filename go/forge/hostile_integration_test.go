package forge

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// TestHostileAgentsInLimbo runs four hostile agents in real Limbo guests (network, the host's files and root, an output flood, a fork flood).
// It needs Limbo with the agent-run profile and the agent socket: SARINA_FORGE_IT=1 SARINA_LIMBO_SOCKET=/run/limbo/agent.sock go test ./forge -run Hostile -v
// (run as a member of the limbo-agent group). Passed 2026-10-22: nothing reached the network or the host, the flood was cut at 64 KiB and the
// fork flood hit the guest's limits and was killed at its timeout.
func TestHostileAgentsInLimbo(t *testing.T) {
	if os.Getenv("SARINA_FORGE_IT") != "1" {
		t.Skip("integration test: set SARINA_FORGE_IT=1 (needs Limbo)")
	}
	d := t.TempDir()
	s := &Store{Dir: filepath.Join(d, "agents"), StateDir: filepath.Join(d, "state")}
	cases := []struct {
		name, cmd string
		files     map[string]string
		timeout   int
	}{
		{"net", "python3 main.py", map[string]string{"main.py": "import socket,urllib.request\nfor h in [('1.1.1.1',53),('archlinux.org',443)]:\n    try:\n        socket.create_connection(h,timeout=4); print('REACHED',h)\n    except Exception as e: print('blocked',h,type(e).__name__)\ntry:\n    urllib.request.urlopen('http://example.com',timeout=4); print('REACHED web')\nexcept Exception as e: print('web blocked',type(e).__name__)\n"}, 60},
		{"homes", "sh main.sh", map[string]string{"main.sh": "ls -la /home 2>&1; ls /home/zlothra 2>&1 | head -3; cat /etc/shadow 2>&1 | head -1; sudo -n true 2>&1 | head -1; id\n"}, 60},
		{"flood", "sh main.sh", map[string]string{"main.sh": "yes 'flood flood flood flood' | head -c 5000000; echo; echo END\n"}, 60},
		{"forks", "sh main.sh", map[string]string{"main.sh": "i=0; while [ $i -lt 100000 ]; do sleep 1000 & i=$((i+1)); done; echo spawned $i\n"}, 60},
	}
	for _, c := range cases {
		_, err := s.Create(Manifest{Name: c.name, Purpose: "hostile test " + c.name, Language: "shell", Command: c.cmd, TimeoutSeconds: c.timeout}, c.files)
		if err != nil {
			t.Fatal(err)
		}
		out, err := s.Run(limboRunner{}, c.name, "agent-run")
		t.Logf("=== %s err=%v\n%s", c.name, err, tail(out, 900))
		if strings.Contains(out, "REACHED") {
			t.Errorf("%s: the guest reached the network", c.name)
		}
		if strings.Contains(out, "zlothra") && c.name == "homes" && !strings.Contains(out, "No such file") {
			t.Errorf("homes: the host home is visible")
		}
	}
}

func tail(s string, n int) string {
	if len(s) > n {
		return "…" + s[len(s)-n:]
	}
	return s
}
