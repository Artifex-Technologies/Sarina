package trust

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"math/big"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// paritySeed is the made-up owner key of the fixture (ed25519 is deterministic: the Rust side signs the same bytes the same).
var paritySeed = sha256.Sum256([]byte("sarina trust parity"))

// sigOp turns the signature of body into the variant a case asks for.
func sigOp(op string, body []byte) []byte {
	priv := ed25519.NewKeyFromSeed(paritySeed[:])
	sig := ed25519.Sign(priv, body)
	other := ed25519.NewKeyFromSeed(make([]byte, 32))
	switch op {
	case "flip":
		sig[5] ^= 1
	case "flip-s":
		sig[40] ^= 0x10
	case "s-plus-l": // the same signature with S+L: a second encoding Go refuses (S must be below L)
		sig = append(sig[:32:32], addL(sig[32:])...)
	case "other-key":
		sig = ed25519.Sign(other, body)
	case "short":
		sig = sig[:63]
	case "long":
		sig = append(sig, 0)
	case "other-body":
		sig = ed25519.Sign(priv, append(append([]byte(nil), body...), ' '))
	case "zero":
		sig = make([]byte, 64)
	}
	return sig
}

func sigTextOp(op, s string) string {
	switch op {
	case "crlf-end":
		return s + "\r\n"
	case "crlf-in":
		return s[:20] + "\r\n" + s[20:]
	}
	return "x" + s
}

// addL adds the group order L to the little-endian scalar s.
func addL(s []byte) []byte {
	l, _ := new(big.Int).SetString("7237005577332262213973186563042994240857116359379907606001950938285454250989", 10)
	be := make([]byte, len(s))
	for i := range s {
		be[len(s)-1-i] = s[i]
	}
	n := new(big.Int).Add(new(big.Int).SetBytes(be), l).FillBytes(make([]byte, 32))
	out := make([]byte, 32)
	for i := range n {
		out[31-i] = n[i]
	}
	return out
}

// TestParityTrust records Go's base64 reading, ed25519.Verify on edge cases, and whole trust folders (mode, key, signed list,
// signature variants) asked about files on a made-up tree (testdata/parity/trust.json). {ROOT} stands for the tree's folder.
func TestParityTrust(t *testing.T) {
	var cases []parity.Case
	for _, s := range []string{"", "aGk=", "aGk", "aG k=", "aG\r\nk=", "aGk=\n", "\naGk=", "aGk=aGk=", "a-_=", "aGl=", "aG==", "a===",
		"====", "aGk==", "aGVsbG8=", "aGVsbG8", "aGVsbG8h", "aGVs\nbG8h", "aGVsbG8h\r\n", "aGVs bG8h", "aGVsbG8h=", "aG=k", "aGk=\r",
		"YQ==", "YR==", "YQ=", "YQ", "Y", "QUJD\tREVG", "QUJDREVG\x00", "+/+/", "-_-_", "QUJD\xffREVG", "Zm9v\n\n\nYmFy"} {
		b, err := base64.StdEncoding.DecodeString(s)
		out := hex.EncodeToString(b)
		if err != nil {
			out = "error"
		}
		cases = append(cases, parity.Case{Fn: "base64", In: parity.Text(s), Out: out})
	}

	pub := ed25519.NewKeyFromSeed(paritySeed[:]).Public().(ed25519.PublicKey)
	msg := []byte(`{"files":{}}`)
	identity := append([]byte{1}, make([]byte, 31)...)
	pHigh, _ := hex.DecodeString("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f") // p itself: y = p (non-canonical 0)
	pPlus1, _ := hex.DecodeString("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
	notOnCurve, _ := hex.DecodeString("0200000000000000000000000000000000000000000000000000000000000000")
	negZero := append([]byte{1}, make([]byte, 31)...)
	negZero[31] = 0x80
	type vec struct{ pub, msg, sig []byte }
	vecs := []vec{}
	for _, op := range []string{"", "flip", "flip-s", "s-plus-l", "other-key", "short", "long", "other-body", "zero"} {
		vecs = append(vecs, vec{pub, msg, sigOp(op, msg)})
	}
	// a small-order key with the matching signature (R = identity, S = 0), non-canonical and invalid keys
	idSig := append(append([]byte(nil), identity...), make([]byte, 32)...)
	for _, k := range [][]byte{identity, pHigh, pPlus1, notOnCurve, negZero, make([]byte, 32), pub[:31], append(append([]byte(nil), pub...), 0)} {
		vecs = append(vecs, vec{k, msg, idSig}, vec{k, []byte("other"), idSig}, vec{k, msg, sigOp("", msg)})
	}
	// R non-canonical: identity's y written as p+1
	vecs = append(vecs, vec{identity, msg, append(append([]byte(nil), pPlus1...), make([]byte, 32)...)})
	for _, v := range vecs {
		ok := len(v.pub) == ed25519.PublicKeySize && ed25519.Verify(ed25519.PublicKey(v.pub), v.msg, v.sig)
		cases = append(cases, parity.Case{Fn: "verify", In: []string{hex.EncodeToString(v.pub), hex.EncodeToString(v.msg), hex.EncodeToString(v.sig)}, Out: ok})
	}

	root := t.TempDir()
	files := map[string]string{"files/a.txt": "alpha\n", "files/b.txt": "beta\n", "files/empty": "", "files/sub/c.md": "# c\n"}
	for name, body := range files {
		p := filepath.Join(root, name)
		os.MkdirAll(filepath.Dir(p), 0o755)
		os.WriteFile(p, []byte(body), 0o644)
	}
	os.MkdirAll(filepath.Join(root, "files", "dir"), 0o755)
	os.Symlink(filepath.Join(root, "files", "a.txt"), filepath.Join(root, "files", "link"))
	sum := func(s string) string { h := sha256.Sum256([]byte(s)); return hex.EncodeToString(h[:]) }
	good := `{"version":1,"files":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `","{ROOT}/files/b.txt":"` + sum("BETA\n") + `","{ROOT}/files/empty":"` + sum("") +
		`","{ROOT}/files/missing":"` + sum("x") + `","{ROOT}/files/dir":"` + sum("") + `","{ROOT}/files/link":"` + sum("alpha\n") + `","/usr/share/x":"00"}}`
	pubText := base64.StdEncoding.EncodeToString(pub)
	queries := []string{"{ROOT}/files/a.txt", "{ROOT}/files/b.txt", "{ROOT}/files/empty", "{ROOT}/files/missing", "{ROOT}/files/dir", "{ROOT}/files/link",
		"{ROOT}/files/sub/c.md", "{ROOT}/files/../files/a.txt", "{ROOT}/files//a.txt", "{ROOT}/files/a.txt/", "/usr/share/x", "/usr/bin/sarina", "/usr",
		"/usr/", "/usrx/a", "relative/file", "", ".", "/", "{ROOT}/FILES/A.TXT", "{ROOT}/files/A.txt"}
	str := func(s string) *string { return &s }
	type dirCase struct {
		mode, pub, manifest *string
		sig                 string // "" none, else a sigOp ("ok" the plain signature)
		sigText             string // a change to the signature's text (sigTextOp)
	}
	dcs := []dirCase{
		{mode: nil},
		{mode: str("off\n")},
		{mode: str("watch")},
		{mode: str("Enforce")},
		{mode: str("enforce\n"), pub: &pubText, manifest: &good, sig: "ok"},
		{mode: str("  enforce \r\n"), pub: &pubText, manifest: &good, sig: "ok"},
		{mode: str("enforce"), pub: &pubText, manifest: &good, sig: "ok"},
		{mode: str("enforce"), pub: str(pubText + "\n"), manifest: &good, sig: "ok", sigText: "crlf-end"},
		{mode: str("enforce"), pub: str(pubText[:10] + "\n" + pubText[10:]), manifest: &good, sig: "ok", sigText: "crlf-in"},
		{mode: str("enforce"), pub: str(pubText[:10] + " " + pubText[10:]), manifest: &good, sig: "ok"},
		{mode: str("enforce"), pub: str(strings.TrimRight(pubText, "=")), manifest: &good, sig: "ok"},
		{mode: str("enforce"), pub: str(base64.StdEncoding.EncodeToString(pub[:31])), manifest: &good, sig: "ok"},
		{mode: str("enforce"), pub: nil, manifest: &good, sig: "ok"},
		{mode: str("enforce"), pub: &pubText, manifest: nil, sig: "ok"},
		{mode: str("enforce"), pub: &pubText, manifest: &good, sig: ""},
		{mode: str("enforce"), pub: &pubText, manifest: &good, sig: "ok", sigText: "x-before"},
	}
	for _, op := range []string{"flip", "flip-s", "s-plus-l", "other-key", "short", "long", "other-body", "zero"} {
		dcs = append(dcs, dirCase{mode: str("enforce"), pub: &pubText, manifest: &good, sig: op})
	}
	for _, m := range []string{`{"files":null}`, `{}`, `null`, `{"Files":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `"}}`,
		`{"FILES":{"{ROOT}/files/a.txt":"x"},"files":{"{ROOT}/files/b.txt":"` + sum("beta\n") + `"}}`,
		`{"files":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `"},"files":null,"files":{"{ROOT}/files/empty":"` + sum("") + `"}}`,
		`{"files":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `","{ROOT}/files/a.txt":"00"}}`,
		`{"files":{"{ROOT}/files/a.txt":"` + strings.ToUpper(sum("alpha\n")) + `"}}`,
		`{"files":{"{ROOT}/files/a.txt":null}}`, `{"files":{"{ROOT}/files/empty":null}}`,
		`{"files":{"{ROOT}/files/a.txt":5}}`, `{"files":[]}`, `{"files":"x"}`, `[]`, `"x"`, `{"files":{}} x`, `{"files":{"a":"b"},}`, ``,
		`{"fileſ":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `"}}`, `{"fi_les":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `"}}`,
		`{"version":"one","files":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `"}}`, "{\"files\":{\"{ROOT}/files/a.txt\xff\":\"00\"}}",
		`{"files":{"{ROOT}/files/a.txt":"` + sum("alpha\n") + `"},"x":1e400}`} {
		m := m
		dcs = append(dcs, dirCase{mode: str("enforce"), pub: &pubText, manifest: &m, sig: "ok"})
	}
	wd, _ := os.Getwd()
	for _, dc := range dcs {
		dir := filepath.Join(t.TempDir(), "trust")
		os.MkdirAll(dir, 0o755)
		in := map[string]any{}
		if dc.mode != nil {
			os.WriteFile(filepath.Join(dir, "mode"), []byte(*dc.mode), 0o644)
			in["mode"] = *dc.mode
		}
		if dc.pub != nil {
			os.WriteFile(filepath.Join(dir, "owner.pub"), []byte(*dc.pub), 0o644)
			in["pub"] = *dc.pub
		}
		var body []byte
		if dc.manifest != nil {
			body = []byte(strings.ReplaceAll(*dc.manifest, "{ROOT}", root))
			os.WriteFile(filepath.Join(dir, "manifest.json"), body, 0o644)
			in["manifest"] = parity.Text(*dc.manifest)
		}
		if dc.sig != "" {
			s := base64.StdEncoding.EncodeToString(sigOp(dc.sig, body))
			in["sig"] = dc.sig
			if dc.sigText != "" {
				s = sigTextOp(dc.sigText, s)
				in["sigText"] = dc.sigText
			}
			os.WriteFile(filepath.Join(dir, "manifest.sig"), []byte(s), 0o644)
		}
		c := New(dir)
		var out []string
		for _, q := range queries {
			why := c.Refuse(strings.ReplaceAll(q, "{ROOT}", root))
			why = strings.ReplaceAll(why, root, "{ROOT}")
			out = append(out, strings.ReplaceAll(why, wd, "{CWD}"))
		}
		cases = append(cases, parity.Case{Fn: "checker", In: in, Out: map[string]any{"enforcing": c.Enforcing(), "refuse": out}})
	}
	cases = append(cases, parity.Case{Fn: "queries", In: queries})

	// Heimdall's lock
	for _, lock := range []*string{nil, str(`{"reason":"tampering seen"}`), str(`{}`), str(`garbage`), str(`{"Reason":"by key case"}`),
		str(`{"reason":5}`), str(`{"reason":"x","reason":""}`), str(`null`), str(``), str(`{"reason":"a"} {`), str(`{"REASON":"b","reason":"c"}`),
		str(`[]`), str(`{"reason":"x","reason":5}`), str(`{"reason":"y","other":1e400}`), str(`{"reason":"z","Reason":[1]}`), str("{\"reason\":\"bad \xff byte\"}"), str(`{"reason":"é\n"}`), str(`{"reason":null}`)} {
		p := filepath.Join(t.TempDir(), "contained")
		if lock != nil {
			os.WriteFile(p, []byte(*lock), 0o600)
		}
		t.Setenv(EnvContained, p)
		on, why := Contained()
		var in any
		if lock != nil {
			in = parity.Text(*lock)
		}
		cases = append(cases, parity.Case{Fn: "contained", In: in, Out: []any{on, parity.Text(why)}})
	}
	p := t.TempDir() // a folder where the lock should be
	t.Setenv(EnvContained, p)
	on, why := Contained()
	cases = append(cases, parity.Case{Fn: "contained", In: "folder", Out: []any{on, why}})
	parity.Fixture(t, "trust", cases)
}
