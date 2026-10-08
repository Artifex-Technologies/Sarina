// Package doctor is a strictly read-only checkup of a Sarina installation and
// of the ecosystem pieces it talks to (model backend, sessions, compute tiers,
// operator extension files, Limbo, Chymaera Sentry). It exists so a person, or
// Sarina's own agent, can find out WHY something is broken from one report.
//
// Rules every check follows: nothing is modified, every network call has a
// short timeout, and no token, key or environment value is ever put in a
// finding (only "set"/"unset" and lengths; URLs are reduced to scheme and host).
package doctor

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"
)

// Status of one check.
type Status string

// The four statuses. Only FAIL makes the exit code non-zero.
const (
	Pass Status = "PASS"
	Warn Status = "WARN"
	Fail Status = "FAIL"
	Skip Status = "SKIP"
)

// Check is one row of the report. Remedy is empty for PASS and SKIP.
type Check struct {
	ID      string `json:"id"`
	Status  Status `json:"status"`
	Finding string `json:"finding"`
	Remedy  string `json:"remedy"`
}

// Options says what to inspect. Zero values mean the real environment.
type Options struct {
	Cwd        string              // workspace directory (default ".")
	SessionDir string              // default <cwd>/.port_sessions/agent
	Home       string              // default the user's home directory
	Getenv     func(string) string // default os.Getenv
	HTTPClient *http.Client        // default: a client with ShortTimeout
	// SystemSkillsDir overrides the root-owned skills directory (tests); empty = the default.
	SystemSkillsDir string
	// Ports to look at for the informational port check; nil = the service and GUI defaults.
	Ports map[string]int
}

// ShortTimeout bounds every network call the doctor makes.
const ShortTimeout = 3 * time.Second

type runner struct {
	o    Options
	home string
	env  func(string) string
	hc   *http.Client
	out  []Check
}

func (r *runner) add(id string, st Status, finding, remedy string) {
	if st == Pass || st == Skip {
		remedy = ""
	}
	r.out = append(r.out, Check{ID: id, Status: st, Finding: finding, Remedy: remedy})
}

// Run executes every check, in a fixed order, and returns the report.
func Run(ctx context.Context, o Options) []Check {
	r := &runner{o: o}
	if o.Cwd == "" {
		r.o.Cwd = "."
	}
	if o.Getenv == nil {
		r.env = os.Getenv
	} else {
		r.env = o.Getenv
	}
	r.home = o.Home
	if r.home == "" {
		r.home, _ = os.UserHomeDir()
	}
	r.hc = o.HTTPClient
	if r.hc == nil {
		r.hc = &http.Client{Timeout: ShortTimeout}
	}
	r.backend(ctx)
	r.sessions()
	r.tiers(ctx)
	r.extensions()
	r.limbo()
	r.sentry(ctx)
	r.ports()
	return r.out
}

// HasFailure reports whether any check failed.
func HasFailure(cs []Check) bool {
	for _, c := range cs {
		if c.Status == Fail {
			return true
		}
	}
	return false
}

// JSON renders the stable machine-readable form: an array of {id,status,finding,remedy}.
func JSON(cs []Check) string {
	if cs == nil {
		cs = []Check{}
	}
	b, _ := json.MarshalIndent(cs, "", "  ")
	return string(b)
}

// Text renders a compact aligned table; a remedy follows its row on its own line.
func Text(cs []Check) string {
	w := 0
	for _, c := range cs {
		w = max(w, len(c.ID))
	}
	var b strings.Builder
	for _, c := range cs {
		fmt.Fprintf(&b, "%-4s  %-*s  %s\n", c.Status, w, c.ID, oneLine(c.Finding))
		if c.Remedy != "" {
			fmt.Fprintf(&b, "      %-*s  -> %s\n", w, "", oneLine(c.Remedy))
		}
	}
	n := map[Status]int{}
	for _, c := range cs {
		n[c.Status]++
	}
	fmt.Fprintf(&b, "\n%d pass, %d warn, %d fail, %d skip\n", n[Pass], n[Warn], n[Fail], n[Skip])
	return b.String()
}

func oneLine(s string) string {
	return strings.Join(strings.Fields(s), " ")
}

// hostOf reduces a URL to scheme://host[:port]: no userinfo, path or query, so
// an embedded token never reaches a report.
func hostOf(raw string) string {
	u, err := url.Parse(strings.TrimSpace(raw))
	if err != nil || u.Host == "" {
		return "(unparseable URL)"
	}
	return u.Scheme + "://" + u.Host
}

// secretState describes a secret without revealing it.
func secretState(v string) string {
	if v == "" {
		return "unset"
	}
	return fmt.Sprintf("set, %d chars", len(v))
}

func (r *runner) get(ctx context.Context, target string, header http.Header) (int, []byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if err != nil {
		return 0, nil, err
	}
	for k, v := range header {
		req.Header[k] = v
	}
	resp, err := r.hc.Do(req)
	if err != nil {
		return 0, nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	return resp.StatusCode, body, nil
}
