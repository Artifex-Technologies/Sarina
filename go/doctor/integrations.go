package doctor

import (
	"context"
	"fmt"
	"net"
	"net/url"
	"os"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/limbo"
	"github.com/Artifex-Technologies/Sarina/go/heimdall"
)

const limboDocs = "Limbo docs/INTEGRATION-FAILURES.md"

// limboRemedies are the first checks the Limbo runbook gives for each code.
var limboRemedies = map[string]string{
	"virt_unavailable": "on the host run `sudo limbo doctor` and `systemctl status libvirtd virtqemud`",
	"profile_invalid":  "the operator fixes the profile file under /etc/limbo/validation/ (an invalid profile is silently absent from the list)",
	"profile_unknown":  "pick a name from limbo_profiles; the operator adds missing profiles",
	"image_missing":    "restore the base image at the path named in the profile",
	"image_mismatch":   "re-verify the image; update the profile sha256 only if the change is intended",
	"boot_failed":      "retry once; then `journalctl -u limbo` and `virsh -c qemu:///system list --all`",
	"boot_timeout":     "retry; check host load and that /dev/kvm exists",
	"host_resources":   "free disk or memory (`df -h /var/lib/limbo`, `free -m`) and retry",
	"busy":             "wait a minute and retry (two runs at a time)",
	"bad_request":      "the request was malformed; this is a client bug, report it",
	"run_unknown":      "start the run again",
}

var codeRE = regexp.MustCompile(`^([a-z][a-z_]+):`)

func (r *runner) limbo() {
	sock := strings.TrimSpace(r.env(limbo.EnvVar))
	if sock == "" {
		r.add("limbo", Skip, limbo.EnvVar+" is unset: Limbo validation tools are inactive", "")
		return
	}
	// Same rule as the client: only the agent socket, never admin or control.
	if base := sock[strings.LastIndexAny(sock, `/\`)+1:]; base != "agent.sock" {
		r.add("limbo", Fail, fmt.Sprintf("%s names %q, not an agent.sock; Sarina refuses any other socket", limbo.EnvVar, base),
			"set it to /run/limbo/agent.sock ("+limboDocs+" section A)")
		return
	}
	st, err := os.Stat(sock)
	switch {
	case err != nil:
		r.add("limbo", Fail, "Limbo socket "+sock+" cannot be used ("+shortErr(err)+")",
			"start it: `sudo systemctl start limbo`, then `ls -l /run/limbo/` and `journalctl -u limbo` ("+limboDocs+" section A)")
		return
	case st.Mode()&os.ModeSocket == 0:
		r.add("limbo", Fail, sock+" exists but is not a unix socket",
			"point "+limbo.EnvVar+" at the agent socket, normally /run/limbo/agent.sock ("+limboDocs+" section A)")
		return
	case runtime.GOOS != "windows" && st.Mode().Perm()&0o002 != 0: // Windows reports fake permission bits
		r.add("limbo", Fail, fmt.Sprintf("%s is world-writable (mode %04o): any local user could submit commands", sock, st.Mode().Perm()),
			"the packaged socket is root:limbo-agent mode 0660; restart `limbo serve` with the packaged unit ("+limboDocs+" section A)")
		return
	}
	c := &limbo.Client{Path: sock, Timeout: ShortTimeout}
	listing, err := c.DescribeProfilesAndSkipped()
	profiles := listing.Profiles
	if err != nil {
		msg := err.Error()
		if m := codeRE.FindStringSubmatch(msg); m != nil {
			if rem, ok := limboRemedies[m[1]]; ok {
				r.add("limbo", Fail, "Limbo answered with error code "+m[1]+": "+truncate(msg, 200), rem+" ("+limboDocs+")")
				return
			}
			r.add("limbo", Fail, "Limbo answered with an error: "+truncate(msg, 200), "look the code up in "+limboDocs)
			return
		}
		rem := "check `systemctl status limbo` and `journalctl -u limbo -n 50` (" + limboDocs + " section A)"
		if strings.Contains(msg, "permission denied") {
			rem = "add Sarina's user to the limbo-agent group (`sudo usermod -aG limbo-agent <user>`) and start a new login or service process (" + limboDocs + " section A)"
		} else if strings.Contains(msg, "refused") {
			rem = "stale socket from a crashed process: `sudo systemctl restart limbo` (" + limboDocs + " section A)"
		}
		r.add("limbo", Fail, "socket exists but GET /v1/profiles failed: "+truncate(msg, 200), rem)
		return
	}
	extra := ""
	if runtime.GOOS != "windows" && st.Mode().Perm()&0o007 != 0 {
		extra = fmt.Sprintf(" (note: mode %04o gives other users access; the packaged mode is 0660)", st.Mode().Perm())
	}
	if len(listing.Skipped) > 0 {
		shown := listing.Skipped
		if len(shown) > 5 {
			shown = shown[:5]
		}
		var items []string
		for _, s := range shown {
			name := s.Name
			if name == "" {
				name = "(unnamed)"
			}
			items = append(items, name+" ["+s.Code+"]")
		}
		more := ""
		if len(listing.Skipped) > len(shown) {
			more = fmt.Sprintf(" and %d more", len(listing.Skipped)-len(shown))
		}
		rem := truncate(shown[0].Remedy, 200)
		if rem == "" {
			rem = "fix the profile files under /etc/limbo/validation/"
		}
		r.add("limbo", Warn, fmt.Sprintf("Limbo answers with %d usable profile(s) but skipped %d invalid: %s%s%s",
			len(profiles), len(listing.Skipped), strings.Join(items, ", "), more, extra),
			"operator remedy: "+rem+" ("+limboDocs+")")
		return
	}
	if len(profiles) == 0 {
		r.add("limbo", Warn, "Limbo answers but lists 0 validation profiles"+extra,
			"a profile that is malformed or not disposable is silently absent: check /etc/limbo/validation/ and `journalctl -u limbo` ("+limboDocs+" known gaps)")
		return
	}
	r.add("limbo", Pass, fmt.Sprintf("agent socket answers; %d profile(s)%s", len(profiles), extra), "")
}

func (r *runner) heimdall(ctx context.Context) {
	const docs = "Heimdall docs/INTEGRATION-FAILURES.md"
	raw := heimdall.URL(r.env)
	if raw == "" {
		r.add("heimdall", Skip, heimdall.EnvVar+" is unset: no external governance review; memory writes rely on Sarina's own in-process scan (fail-open by design)", "")
		return
	}
	u, err := url.Parse(raw)
	if err != nil || u.Host == "" || (u.Scheme != "http" && u.Scheme != "https") {
		r.add("heimdall", Warn, heimdall.EnvVar+" is set ("+secretState(raw)+") but is not a usable http(s) URL; the review is skipped (fail-open)",
			"set it to Heimdall's review URL, e.g. http://127.0.0.1:8930/v1/review ("+docs+")")
		return
	}
	where := u.Scheme + "://" + u.Host
	// Heimdall's cheap health endpoint is GET /healthz on the same address; no memory text is sent.
	status, _, err := r.get(ctx, where+"/healthz", nil)
	switch {
	case err != nil:
		r.add("heimdall", Warn, "Heimdall at "+where+" is not reachable; reviews are skipped (fail-open: memory writes still pass Sarina's own scan)",
			"start heimdall or correct "+heimdall.EnvVar+"; `journalctl -u heimdall -n 50` ("+docs+", section 'Sarina consulting Heimdall')")
	case status < 200 || status > 299:
		r.add("heimdall", Warn, fmt.Sprintf("Heimdall at %s answered HTTP %d on /healthz; the address may not be Heimdall's API", where, status),
			"check "+heimdall.EnvVar+" matches Heimdall's api_addr, default 127.0.0.1:8930 ("+docs+")")
	case !strings.HasSuffix(strings.TrimRight(u.Path, "/"), "/v1/review"):
		r.add("heimdall", Warn, "Heimdall at "+where+" is healthy, but the URL path is not /v1/review, so consultations will fail open",
			"set "+heimdall.EnvVar+" to http://HOST:PORT/v1/review ("+docs+")")
	default:
		r.add("heimdall", Pass, "Heimdall at "+where+" answers /healthz; governance review is active (a 'deny' verdict blocks the write)", "")
	}
}

func shortErr(err error) string {
	if os.IsNotExist(err) {
		return "no such file"
	}
	if os.IsPermission(err) {
		return "permission denied"
	}
	return truncate(err.Error(), 120)
}

func truncate(s string, n int) string {
	s = oneLine(s)
	if r := []rune(s); len(r) > n {
		return string(r[:n]) + "..."
	}
	return s
}

func (r *runner) ports() {
	ports := r.o.Ports
	if ports == nil {
		ports = map[string]int{"service": 8899, "gui": 8765}
	}
	names := make([]string, 0, len(ports))
	for n := range ports {
		names = append(names, n)
	}
	sort.Strings(names)
	var parts []string
	for _, n := range names {
		addr := fmt.Sprintf("127.0.0.1:%d", ports[n])
		state := "free"
		if c, err := net.DialTimeout("tcp", addr, 300*time.Millisecond); err == nil {
			c.Close()
			state = "in use"
		}
		parts = append(parts, fmt.Sprintf("%s %d %s", n, ports[n], state))
	}
	r.add("ports", Pass, strings.Join(parts, "; ")+" (informational: 'in use' is normal while that server runs)", "")
}
