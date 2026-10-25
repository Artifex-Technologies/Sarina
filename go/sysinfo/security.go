package sysinfo

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/heimdall"
	"github.com/Artifex-Technologies/Sarina/go/skills"
)

// The "security" topic is what Heimdall finds open right now (its GET /v1/audit): installed packages and kernels with known holes,
// and settings that could be safer, each with Heimdall's fix. The security-advisor skill starts from it ("prefetch: [security]").
func init() {
	skills.RegisterContext("security", func(string) string {
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		out, _ := Report(ctx, "security")
		return "What Heimdall reports right now (read-only, already looked up for you):\n\n" + out
	})
}

// heimdallBase is Heimdall's local API (the host of HEIMDALL_REVIEW_URL, else the default), loopback only.
func heimdallBase() (string, error) {
	host := "127.0.0.1:8930"
	if raw := heimdall.URL(os.Getenv); raw != "" {
		if u, err := url.Parse(raw); err == nil && u.Host != "" {
			host = u.Host
		}
	}
	h, _, err := net.SplitHostPort(host)
	if err != nil {
		return "", err
	}
	if ip := net.ParseIP(h); h != "localhost" && (ip == nil || !ip.IsLoopback()) {
		return "", fmt.Errorf("Heimdall's address %s is not on this computer", host)
	}
	return "http://" + host, nil
}

type auditReport struct {
	Packages []struct {
		Package, Installed, Fixed, Severity, Advisory, Type string
		CVEs                                                []string
	} `json:"packages"`
	Kernels []struct {
		Kernel   string `json:"kernel"`
		Running  bool   `json:"running"`
		Count    int    `json:"count"`
		FixAll   string `json:"fixed_all_in"`
		ListDate string `json:"list_date"`
		Worst    []struct {
			ID        string  `json:"id"`
			Score     float64 `json:"score"`
			FixedIn   string  `json:"fixed_in"`
			SameLine  bool    `json:"same_line"`
			Exploited bool    `json:"exploited"`
			Summary   string  `json:"summary"`
		} `json:"worst"`
	} `json:"kernels"`
	Hardening []struct {
		ID, Now, Why, Fix, Level string
	} `json:"hardening"`
	Missing []string `json:"missing"`
}

func securitySummary(ctx context.Context) string {
	base, err := heimdallBase()
	if err != nil {
		return "Heimdall could not be asked: " + err.Error()
	}
	req, _ := http.NewRequestWithContext(ctx, http.MethodGet, base+"/v1/audit", nil)
	resp, err := (&http.Client{Timeout: 10 * time.Second}).Do(req)
	if err != nil {
		return "Heimdall does not answer (is heimdall.service running?): " + err.Error()
	}
	defer resp.Body.Close()
	var r auditReport
	if resp.StatusCode != http.StatusOK || json.NewDecoder(io.LimitReader(resp.Body, 2<<20)).Decode(&r) != nil {
		return "Heimdall's answer could not be read (an older Heimdall without /v1/audit?)"
	}
	var b strings.Builder
	b.WriteString("### Packages with known vulnerabilities\n")
	if len(r.Packages) == 0 {
		b.WriteString("none\n")
	}
	for _, p := range r.Packages {
		fix := "no fixed version yet"
		if p.Fixed != "" {
			fix = "fixed in " + p.Fixed
		}
		fmt.Fprintf(&b, "- %s %s: severity %s (Arch's rating), kind: %s (%s, %s); %s\n", p.Package, p.Installed, strings.ToUpper(p.Severity), p.Type, p.Advisory, strings.Join(p.CVEs, " "), fix)
	}
	b.WriteString("\n### Kernels\n")
	for _, k := range r.Kernels {
		which := "installed, not running"
		if k.Running {
			which = "RUNNING"
		}
		if k.Count == 0 {
			fmt.Fprintf(&b, "- %s (%s): no known hole that an update fixes\n", k.Kernel, which)
			continue
		}
		same := 0
		for _, h := range k.Worst {
			if h.SameLine {
				same++
			}
		}
		fmt.Fprintf(&b, "- %s (%s): %d known holes that newer kernels fix; all fixed from %s. The worst %d:\n", k.Kernel, which, k.Count, k.FixAll, len(k.Worst))
		for _, h := range k.Worst {
			line := "only in newer kernel lines"
			if h.SameLine {
				line = "on its own line"
			}
			ex := ""
			if h.Exploited {
				ex = " EXPLOITED IN THE WILD"
			}
			fmt.Fprintf(&b, "  - %s score %.1f%s, fixed in %s (%s): %s\n", h.ID, h.Score, ex, h.FixedIn, line, h.Summary)
		}
	}
	if len(r.Kernels) > 0 {
		fmt.Fprintf(&b, "(kernel CVE list from %s)\n", r.Kernels[0].ListDate)
	}
	b.WriteString("\n### Settings that could be safer (Heimdall's fix for each)\n")
	if len(r.Hardening) == 0 {
		b.WriteString("none\n")
	}
	for _, g := range r.Hardening {
		fmt.Fprintf(&b, "- [%s] %s (%s)\n  why: %s\n  fix: %s\n", g.Level, g.Now, g.ID, g.Why, g.Fix)
	}
	if len(r.Missing) > 0 {
		b.WriteString("\nNot checked yet (not downloaded): " + strings.Join(r.Missing, ", ") + "\n")
	}
	return b.String()
}
