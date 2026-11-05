package heimdall

// The grant gate (Heimdall's docs/GRANTS.md): Sarina may ask for more than she has, a hand-off to a model off this computer
// or a higher access tier, and never give it to herself. The owner answers with `sudo heimdalld grant ...`, which writes
// /etc/heimdall/grants.json; only root can write that file. Sarina reads the file directly, never an HTTP answer: while
// Heimdall is stopped another program could take its port and answer "approved".
//
// The gate is on while the file exists. While it is off, nothing here changes behaviour (the operator's own setting and the
// [y/N] question decide, as before). The two callers are the only places Sarina raises what she may do:
// config.AccessTierFromEnv (default permissions) and tiers.Router.attempt (every hand-off to another tier). Keep it that way.

import (
	"bytes"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// DefaultGrantsPath is Heimdall's decisions file.
const DefaultGrantsPath = "/etc/heimdall/grants.json"

// GrantsPath is the file read; a variable for tests. Test binaries never read the real one (testing.Testing), so a gate
// switched on on the developer's machine does not change other packages' test results.
var GrantsPath = DefaultGrantsPath

// GrantsOwner is the account that must own the file and its folder (root). A variable for tests.
var GrantsOwner uint32 = 0

// Grants is what the owner decided, as far as Sarina needs it.
type Grants struct {
	On          bool // the file exists: the gate decides
	accessTier  *int
	escalations []grantEscalation
	Problem     string // why the file was not trusted (then the gate is on and allows nothing)
}

type grantEscalation struct {
	Tier      string    `json:"tier"`
	SessionID string    `json:"session_id"`
	Until     time.Time `json:"until"`
}

// LoadGrants reads the decisions file. A missing file is the gate off. A file that is not root's, or that others could
// write, or that cannot be read, turns the gate on with nothing allowed: a planted or damaged file must not grant anything.
func LoadGrants() Grants {
	if testing.Testing() && GrantsPath == DefaultGrantsPath {
		return Grants{}
	}
	fi, err := os.Lstat(GrantsPath)
	if errors.Is(err, os.ErrNotExist) {
		return Grants{}
	}
	g := Grants{On: true}
	if err != nil {
		g.Problem = err.Error()
		return g
	}
	if why := untrusted(fi); why != "" {
		g.Problem = GrantsPath + " " + why
		return g
	}
	if dfi, err := os.Stat(filepath.Dir(GrantsPath)); err != nil || untrusted(dfi) != "" {
		g.Problem = filepath.Dir(GrantsPath) + " is not root's alone"
		return g
	}
	b, err := os.ReadFile(GrantsPath)
	var d struct {
		AccessTier  *int              `json:"access_tier"`
		Escalations []grantEscalation `json:"escalations"`
	}
	if err == nil {
		err = json.Unmarshal(b, &d)
	}
	if err != nil {
		g.Problem = err.Error()
		return g
	}
	g.accessTier, g.escalations = d.AccessTier, d.Escalations
	return g
}

func untrusted(fi os.FileInfo) string {
	if fi.Mode()&os.ModeSymlink != 0 {
		return "is a link"
	}
	st, ok := fi.Sys().(*syscall.Stat_t)
	if !ok || st.Uid != GrantsOwner {
		return "is not root's"
	}
	if fi.Mode().Perm()&0o022 != 0 {
		return "can be written by others"
	}
	return ""
}

// TierCap returns the access tier Sarina may use: her own setting while the gate is off, else the lower of it and the
// owner's cap (no cap in the file: 0, read-only).
func (g Grants) TierCap(own int) int {
	if !g.On {
		return own
	}
	if g.accessTier == nil {
		return 0
	}
	return min(own, max(*g.accessTier, 0))
}

// EscalationAllowed reports whether the owner approved a hand-off to tier (for this session, or for any) that has not
// expired.
func (g Grants) EscalationAllowed(tier, sessionID string, now time.Time) bool {
	for _, e := range g.escalations {
		if e.Tier == tier && (e.SessionID == "" || e.SessionID == sessionID) && now.Before(e.Until) {
			return true
		}
	}
	return false
}

// RequestGrant files a request with Heimdall (POST /v1/requests on the host of HEIMDALL_REVIEW_URL, else 127.0.0.1:8930)
// and returns the command that answers it. Filing asks; it never allows anything.
func RequestGrant(kind, detail, sessionID, reason string) (string, error) {
	base := "http://127.0.0.1:8930"
	if u, err := url.Parse(URL(os.Getenv)); err == nil && u.Host != "" {
		base = u.Scheme + "://" + u.Host
	}
	payload, _ := json.Marshal(map[string]string{"kind": kind, "detail": detail, "session_id": sessionID, "reason": reason})
	req, err := http.NewRequest(http.MethodPost, base+"/v1/requests", bytes.NewReader(payload))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	resp, err := (&http.Client{Timeout: Timeout}).Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(io.LimitReader(resp.Body, 64*1024))
	if resp.StatusCode != http.StatusOK {
		return "", errors.New("Heimdall answered " + resp.Status + ": " + strings.TrimSpace(string(raw)))
	}
	var out struct {
		Approve string `json:"approve"`
	}
	if json.Unmarshal(raw, &out) != nil || out.Approve == "" {
		return "", errors.New("Heimdall's answer could not be read")
	}
	return out.Approve, nil
}
