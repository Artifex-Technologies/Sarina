package reverse

import (
	"context"
	"fmt"
	"net"
	"os/exec"
	"regexp"
	"sort"
	"strings"
	"time"
)

// The network map (an idea from Eleven's "network map" playbook, no code taken): which addresses a program carries inside it.
// The strings are read in the sandbox; the addresses are picked out here as plain text. Nothing is run, no name is looked up
// (that would contact the network), and an IP address is named only from Heimdall's offline database (heimdalld origin).

var (
	urlRx   = regexp.MustCompile(`(?i)\b(?:https?|wss?|ftp|tcp|udp)://[A-Za-z0-9.\-\[\]:]+(?:/[^\s"'<>\\]{0,200})?`)
	ipv4Rx  = regexp.MustCompile(`\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)(?::\d{1,5})?\b`)
	ipv6Rx  = regexp.MustCompile(`(?:[0-9a-fA-F]{0,4}:){2,7}[0-9a-fA-F]{1,4}`) // candidates; the address parser decides
	onionRx = regexp.MustCompile(`\b[a-z2-7]{16,56}\.onion\b`)
	// host names: only well-known endings, so file names (libc.so.6, main.go, setup.py) are not taken for hosts
	hostRx = regexp.MustCompile(`(?i)\b(?:[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?\.)+(?:com|net|org|io|info|biz|gov|edu|mil|ru|cn|de|uk|fr|nl|jp|kr|br|in|us|eu|ca|au|it|es|pl|ua|ir|kp|tr|vn|xyz|top|online|site|club|app|dev|cloud|tk|ml|ga|cf|gq|pw|cc|me|tv|ws|su|link|live|icu|shop|store|tech|space|website|fun)\b`)
)

// NetMap is what was found, each list sorted and without repeats.
type NetMap struct {
	URLs, Hosts, IPs, Onions []string
}

// FindAddresses picks addresses out of a program's strings.
func FindAddresses(text string) NetMap {
	set := func(xs []string, keep func(string) bool) []string {
		m := map[string]bool{}
		for _, x := range xs {
			x = strings.TrimRight(x, ".,;:)")
			if keep == nil || keep(x) {
				m[x] = true
			}
		}
		out := make([]string, 0, len(m))
		for x := range m {
			out = append(out, x)
		}
		sort.Strings(out)
		if len(out) > 60 {
			out = append(out[:60], fmt.Sprintf("... and %d more", len(out)-60))
		}
		return out
	}
	var nm NetMap
	nm.URLs = set(urlRx.FindAllString(text, 2000), nil)
	nm.Onions = set(onionRx.FindAllString(text, 500), nil)
	nm.IPs = set(append(ipv4Rx.FindAllString(text, 2000), ipv6Rx.FindAllString(text, 2000)...), func(s string) bool {
		h := s
		if hh, _, err := net.SplitHostPort(s); err == nil {
			h = hh
		}
		ip := net.ParseIP(h)
		// an IPv6 candidate must parse and be at least "a:b::c" long; "aa:bb" and time stamps like 12:30:45 do not parse
		return ip != nil && !ip.IsUnspecified() && !ip.IsLoopback() && (ip.To4() != nil || len(h) >= 6)
	})
	nm.Hosts = set(hostRx.FindAllString(text, 2000), func(s string) bool {
		return !strings.HasPrefix(strings.ToLower(s), "www.w3.org") && !strings.HasSuffix(strings.ToLower(s), ".gnu.org") // standards and licence text in every program
	})
	return nm
}

// origins names public addresses from Heimdall's offline database, when heimdalld is installed.
func origins(ctx context.Context, ips []string) map[string]string {
	out := map[string]string{}
	var plain []string
	for _, s := range ips {
		h := s
		if hh, _, err := net.SplitHostPort(s); err == nil {
			h = hh
		}
		if net.ParseIP(h) != nil {
			plain = append(plain, h)
		}
	}
	bin, err := exec.LookPath("heimdalld")
	if err != nil || len(plain) == 0 {
		return out
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	b, _ := exec.CommandContext(ctx, bin, append([]string{"origin"}, plain...)...).Output()
	for _, line := range strings.Split(string(b), "\n") {
		if ip, o, ok := strings.Cut(line, "  "); ok {
			out[ip] = strings.TrimSpace(o)
		}
	}
	return out
}

// Report is the network map as text for her.
func (nm NetMap) Report(ctx context.Context, file string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "Network map of %s (from its strings only: nothing was run and no name was looked up).\n", file)
	if len(nm.URLs)+len(nm.Hosts)+len(nm.IPs)+len(nm.Onions) == 0 {
		b.WriteString("No web addresses, host names or IP addresses were found in its strings. It may build them at run time or keep them encrypted; that cannot be seen this way.\n")
		return b.String()
	}
	list := func(title string, xs []string, note func(string) string) {
		if len(xs) == 0 {
			return
		}
		fmt.Fprintf(&b, "\n%s (%d):\n", title, len(xs))
		for _, x := range xs {
			if n := note(x); n != "" {
				fmt.Fprintf(&b, "- %s  (%s)\n", x, n)
			} else {
				fmt.Fprintf(&b, "- %s\n", x)
			}
		}
	}
	org := origins(ctx, nm.IPs)
	none := func(string) string { return "" }
	list("Web addresses", nm.URLs, none)
	list("Host names", nm.Hosts, none)
	list("IP addresses", nm.IPs, func(s string) string {
		h := s
		if hh, _, err := net.SplitHostPort(s); err == nil {
			h = hh
		}
		return org[h]
	})
	list("Tor addresses", nm.Onions, func(string) string { return "a hidden service: unusual for ordinary software" })
	b.WriteString("\nAn address in a program's strings is not proof it is contacted (documentation, certificates and libraries carry addresses too); a dotted number may be a version. To know whether one is used, search for it (search) and decompile the function that refers to it (xrefs, then decompile).\n")
	return b.String()
}
