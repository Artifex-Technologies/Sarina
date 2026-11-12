package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/heimdall"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// tierEnv are values for SARINA_ACCESS_TIER.
var tierEnv = []string{"", "0", "1", "2", "3", "4", "9", "-1", " 2 ", "+2", "02", "2.0", "x", "99999999999999999999",
	"\t3\n", "٣", "0x2", "-0", " 2"}

// TestParityAccessTier records AccessTierFromEnv against Heimdall's grants file (heimdall.LoadGrants and TierCap): every tier
// in sarina.env against the cap missing, damaged, lower, higher, in a file that is a link, not the owner's, writable by
// others, in a folder others can write, unreadable, a folder (testdata/parity/accesstier.json). {DIR} is the file's folder.
func TestParityAccessTier(t *testing.T) {
	oldP, oldO := heimdall.GrantsPath, heimdall.GrantsOwner
	defer func() { heimdall.GrantsPath, heimdall.GrantsOwner = oldP, oldO }()
	me := uint32(os.Getuid())
	type gcase struct {
		body    *string
		mode    os.FileMode
		dirMode os.FileMode
		special string // "link", "other-owner", "folder", "under-file"
	}
	str := func(s string) *string { return &s }
	var gcs []gcase
	gcs = append(gcs, gcase{}) // no file: the gate is off
	for _, b := range []string{`{"access_tier": 1}`, `{"access_tier":0}`, `{"access_tier":2}`, `{"access_tier":3}`, `{"access_tier":4}`,
		`{"access_tier":-1}`, `{"access_tier":null}`, `{}`, `null`, `{"access_tier":"2"}`, `{"access_tier":2.5}`, `{"access_tier":2.0}`,
		`{"access_tier":1e0}`, `{"access_tier":-0}`, `{"access_tier":99999999999999999999}`, `{"access_tier":9223372036854775807}`,
		`{"access_tier":-9223372036854775808}`, `{"access_tier":true}`, `{"access_tier":[2]}`, `{"Access_Tier":2}`, `{"ACCESS_TIER":3}`,
		`{"access-tier":3}`, `{"accesstier":3}`, `{"access_tier":1,"access_tier":3}`, `{"access_tier":3,"access_tier":null}`,
		`{"access_tier":3,"ACCESS_TIER":1}`, `{"access_tier":2,"escalations":[]}`, `{"access_tier":2,"escalations":null}`,
		`{"access_tier":2,"escalations":[{"tier":"claude","session_id":"","until":"2026-11-12T10:00:00Z"}]}`,
		`{"access_tier":2,"escalations":[{"tier":"claude","until":"2026-11-12T10:00:00.123456789+01:00"}]}`,
		`{"access_tier":2,"escalations":[{"until":"2026-11-12 10:00:00"}]}`, `{"access_tier":2,"escalations":[{"until":"2026-11-12T10:00:00,5Z"}]}`,
		`{"access_tier":2,"escalations":[{"until":"2026-02-30T10:00:00Z"}]}`, `{"access_tier":2,"escalations":[{"until":"2026-11-12T24:00:00Z"}]}`,
		`{"access_tier":2,"escalations":[{"until":"2026-11-12T10:00:00+24:00"}]}`, `{"access_tier":2,"escalations":[{"until":"2026-11-12T10:00:00z"}]}`,
		`{"access_tier":2,"escalations":[{"until":5}]}`, `{"access_tier":2,"escalations":[{"until":null}]}`, `{"access_tier":2,"escalations":[{"tier":1}]}`,
		`{"access_tier":2,"escalations":[5]}`, `{"access_tier":2,"escalations":{}}`, `{"access_tier":2,"escalations":[{"tier":"a"},{"tier":[]}]}`,
		`{"access_tier":2,"x":1e400}`, `{"access_tier":2} x`, `{"access_tier":2,}`, `[]`, `"2"`, `2`, ``, " \n", "{\"access_tier\":2,\"escalations\":[{\"tier\":\"\xff\"}]}",
		`{"escalations":[{"tier":"x"}],"access_tier":"x"}`, `{"access_tier":"x","escalations":[{"until":"bad"}]}`} {
		gcs = append(gcs, gcase{body: str(b), mode: 0o644, dirMode: 0o755})
	}
	one := `{"access_tier": 2}`
	gcs = append(gcs, gcase{body: &one, mode: 0o600, dirMode: 0o700}, gcase{body: &one, mode: 0o664, dirMode: 0o755},
		gcase{body: &one, mode: 0o646, dirMode: 0o755}, gcase{body: &one, mode: 0o644, dirMode: 0o775}, gcase{body: &one, mode: 0o644, dirMode: 0o757},
		gcase{body: &one, mode: 0o644, dirMode: 0o1777}, gcase{body: &one, mode: 0o000, dirMode: 0o755}, gcase{body: &one, mode: 0o4644, dirMode: 0o755},
		gcase{body: &one, mode: 0o644, dirMode: 0o755, special: "link"}, gcase{body: &one, mode: 0o644, dirMode: 0o755, special: "other-owner"},
		gcase{mode: 0o755, dirMode: 0o755, special: "folder"}, gcase{mode: 0o644, dirMode: 0o755, special: "under-file"})
	var cases []parity.Case
	for _, g := range gcs {
		dir := filepath.Join(t.TempDir(), "heimdall")
		os.Mkdir(dir, 0o755)
		p := filepath.Join(dir, "grants.json")
		heimdall.GrantsOwner = me
		in := map[string]any{}
		if g.body != nil {
			os.WriteFile(p, []byte(*g.body), 0o600)
			os.Chmod(p, g.mode)
			in["body"], in["mode"] = parity.Text(*g.body), int(g.mode.Perm())
			if g.mode&os.ModeSetuid != 0 || g.mode == 0o4644 {
				os.Chmod(p, os.FileMode(0o644)|os.ModeSetuid)
				in["mode"] = 0o4644
			}
		}
		if g.dirMode != 0 {
			os.Chmod(dir, g.dirMode&0o777|map[bool]os.FileMode{true: os.ModeSticky}[g.dirMode&0o1000 != 0])
			in["dirMode"] = int(g.dirMode)
		}
		switch g.special {
		case "link":
			os.Rename(p, p+".real")
			os.Symlink(p+".real", p)
		case "other-owner":
			heimdall.GrantsOwner = me + 1
		case "folder":
			os.Mkdir(p, 0o755)
		case "under-file":
			os.WriteFile(filepath.Join(dir, "file"), nil, 0o644)
			p = filepath.Join(dir, "file", "grants.json")
		}
		if g.special != "" {
			in["special"] = g.special
		}
		heimdall.GrantsPath = p
		gr := heimdall.LoadGrants()
		var caps, tiers []int
		for own := 0; own <= 4; own++ {
			caps = append(caps, gr.TierCap(own))
		}
		for _, v := range tierEnv {
			tiers = append(tiers, AccessTierFromEnv(func(k string) string {
				if k == "SARINA_ACCESS_TIER" {
					return v
				}
				return ""
			}))
		}
		os.Chmod(p, 0o644)
		os.Chmod(dir, 0o755)
		cases = append(cases, parity.Case{Fn: "grants", In: in, Out: map[string]any{"on": gr.On,
			"problem": parity.Text(strings.ReplaceAll(gr.Problem, dir, "{DIR}")), "cap": caps, "tiers": tiers}})
	}
	cases = append(cases, parity.Case{Fn: "env", In: parity.Texts(tierEnv)})
	// a time.Time read from JSON (the grants' "until"): the instant, its offset, or Go's error
	times := []string{"2026-11-12T10:00:00Z", "2026-11-12T10:00:00+24:00", "2026-11-12T10:00:00+24:59", "2026-11-12T10:00:00+25:00",
		"2026-11-12T10:00:00+23:60", "2026-11-12T10:00:00+23:61", "2026-11-12T10:00:00+23:99", "2026-11-12T10:00:00-24:00", "2026-11-12T10:00:00+99:00",
		"2026-11-12T10:00:00x25:00", "2026-11-12T10:00:00+2x:99", "2026-11-12T10:00:00+25:x0", "2026-11-12T10:00:00+0100", "2026-11-12T10:00:00+01",
		"2026-11-12T10:00:00.Z", "2026-11-12T10:00:00.1234567891Z", "2026-11-12T10:00:00.000000001Z", "2026-11-12T10:00:00,5Z", "2026-11-12T10:00:00,Z",
		"2026-11-12t10:00:00Z", "2026-11-12T10:00:60Z", "2026-11-12T10:60:00Z", "2026-13-12T10:00:00Z", "2026-00-12T10:00:00Z", "2026-11-00T10:00:00Z",
		"2026-11-45T25:00:00Z", "2026-11-31T10:00:00Z", "2026-11-31T10:00:00Zx", "2026-11-99T10:99:00Z", "0000-01-01T00:00:00Z", "99999-01-01T00:00:00Z",
		"2026-1-12T10:00:00Z", "2026-11-1T10:00:00Z", "2026-11-12T1:00:00Z", "2026-11-12T1:0:00Z", "2026-11-12T10:00:0Z", "2026-11-12T10:00:00Zx",
		"2026-11-12T10:00:00", "", "2026", "x", "2026-11-12T10:00:00 Z", "+2026-11-12T10:00:00Z", "-2026-11-12T10:00:00Z", "20a6-11-12T10:00:00Z",
		"2026-11-12T10:00:00.5+01:00", "2026-11-12T10:00:00+01:00x", "2024-02-29T00:00:00Z", "2023-02-29T00:00:00Z", "2000-02-29T23:59:59.999999999-12:00",
		"1900-02-29T00:00:00Z", "2026-11-12T10:00:00−01:00", "２026-11-12T10:00:00Z", "2026-11-12T10:00:00+1:00", "2026-11-12T10:00:00+01:0",
		"2026-11-12T10:00:00\"Z", "2026-11-12T10:00:00\\\\Z", `2026-11-12T10:00:00\u0001`, `2026-11-12T10:00:00�`, `\ud800`, `2026-11-12T10:00:00Z\t`,
		"2026-11-12T24:00:00Z", "2026-11-12T23:59:59.9999999999999999+00:00", "9999-12-31T23:59:59Z", "0001-01-01T00:00:00Z"}
	for _, s := range times {
		var tm time.Time
		err := json.Unmarshal([]byte(`"`+s+`"`), &tm)
		var out any = parity.Err(err)
		if err == nil {
			_, off := tm.Zone()
			out = []any{strconv.FormatInt(tm.Unix(), 10), tm.Nanosecond(), off}
		}
		cases = append(cases, parity.Case{Fn: "time", In: s, Out: out})
	}
	parity.Fixture(t, "accesstier", cases)
}
