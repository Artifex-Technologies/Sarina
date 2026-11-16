package cryptotools

import (
	"context"
	"crypto/ed25519"
	"crypto/hmac"
	"crypto/sha256"
	"crypto/sha512"
	"encoding/base64"
	"encoding/binary"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityModern records (testdata/parity/modern.json) crypto_modern on what needs no crypto/x509: SSH public keys,
// JWTs (claims, expiry, alg none, missing signatures, HMAC verified or not, unknown algorithms), private keys refused,
// PGP blocks with gpg missing and with a stand-in gpg (its script is in the fixture). RS/ES/PS and certificates: next part.
func TestParityModern(t *testing.T) {
	enc := base64.RawURLEncoding.EncodeToString
	std := base64.StdEncoding.EncodeToString
	sshBlob := func(parts ...[]byte) string {
		var b []byte
		for _, p := range parts {
			b = binary.BigEndian.AppendUint32(b, uint32(len(p)))
			b = append(b, p...)
		}
		return std(b)
	}
	pub := ed25519.NewKeyFromSeed(make([]byte, 32)).Public().(ed25519.PublicKey)
	rsaN := append([]byte{0}, make([]byte, 256)...)
	rsaN[1] = 0x45
	jwt := func(hdr, body string, sign func(string) string) string {
		s := enc([]byte(hdr)) + "." + enc([]byte(body))
		return s + "." + sign(s)
	}
	mac := func(secret string, size int) func(string) string {
		return func(s string) string {
			var m []byte
			switch size {
			case 256:
				x := hmac.New(sha256.New, []byte(secret))
				x.Write([]byte(s))
				m = x.Sum(nil)
			case 384:
				x := hmac.New(sha512.New384, []byte(secret))
				x.Write([]byte(s))
				m = x.Sum(nil)
			default:
				x := hmac.New(sha512.New, []byte(secret))
				x.Write([]byte(s))
				m = x.Sum(nil)
			}
			return enc(m)
		}
	}
	none := func(string) string { return "" }
	junk := func(string) string { return "c2lnbmF0dXJl" }
	body := `{"sub":"1234567890","name":"<John> & Doe","admin":true,"iat":1516239022,"exp":1700000100,"nbf":1.5e9,"scopes":["a","b"],"n":null,"x":1e21}`
	type in struct{ input, key string }
	inputs := []in{{"", ""}, {"  ", ""},
		{"ssh-ed25519 " + sshBlob([]byte("ssh-ed25519"), pub) + " user@host extra words", ""},
		{"ssh-rsa " + sshBlob([]byte("ssh-rsa"), []byte{1, 0, 1}, rsaN), ""}, {"ssh-rsa " + sshBlob([]byte("ssh-rsa"), []byte{1, 0, 1}), ""},
		{"ssh-dss " + sshBlob([]byte("ssh-dss"), []byte{1}), "c"}, {"ecdsa-sha2-nistp256 " + sshBlob([]byte("ecdsa-sha2-nistp256"), []byte{4, 1, 2}), ""},
		{"ssh-rsa " + sshBlob([]byte("ssh-ed25519"), pub), ""}, {"ssh-ed25519 AAAA====", ""}, {"ssh-ed25519 AAAAC3Nz", ""}, {"ssh-rsa AAAA", ""},
		{jwt(`{"alg":"HS256","typ":"JWT"}`, body, mac("secret", 256)), ""}, {jwt(`{"alg":"HS256","typ":"JWT"}`, body, mac("secret", 256)), "secret"},
		{jwt(`{"alg":"HS256"}`, body, mac("secret", 256)), "wrong"}, {jwt(`{"alg":"HS384"}`, body, mac("k", 384)), "k"},
		{jwt(`{"alg":"hs512"}`, body, mac("k", 512)), "k"}, {jwt(`{"alg":"none"}`, body, none), ""}, {jwt(`{"alg":"NONE"}`, `{}`, junk), "k"},
		{jwt(`{"alg":"HS256"}`, body, none), "k"}, {jwt(`{"alg":"HS999"}`, body, junk), "k"}, {jwt(`{"alg":"XX256"}`, body, junk), "k"},
		{jwt(`{"alg":"H"}`, body, junk), "k"}, {jwt(`{"typ":"JWT"}`, body, junk), "k"}, {jwt(`{"alg":"RS256"}`, body, junk), ""},
		{jwt(`{"alg":"HS256"}`, `not json`, junk), ""}, {jwt(`null`, `null`, junk), ""}, {jwt(`[1]`, body, junk), ""},
		{jwt(`{"alg":"HS256"}`, `{"exp":1699999999}`, junk), ""}, {jwt(`{"alg":"HS256"}`, `{"exp":1700000000}`, junk), ""},
		{jwt(`{"alg":"HS256"}`, `{"exp":-62135596800,"iat":253402300800,"nbf":"soon"}`, junk), ""},
		{jwt(`{"alg":"HS256","alg":"none"}`, `{"a":1,"a":2}`, junk), ""}, {jwt(`{"alg":"HÉ256"}`, body, junk), "k"},
		{"eyJhbGciOiJIUzI1NiJ9!.eyJ.x", ""}, {"abcdefgh.ij.kl", ""}, {"abcdefgh.eyJhIjoxfQ.", ""},
		{"-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----", ""}, {"my PRIVATE KEY-----", ""},
		{"-----BEGIN PGP MESSAGE-----\n\nhQEMA\n-----END PGP MESSAGE-----", ""}, {"-----BEGIN PGP", ""}}
	gpg := "#!/bin/sh\ncat >/dev/null\ncase \"$PGP_MODE\" in\n err) echo \"gpg: no valid OpenPGP data found.\" >&2;;\n mid) i=0; while [ $i -lt 260 ]; do printf 'é:packet %d\\n' $i; i=$((i+1)); done;;\n long) i=0; while [ $i -lt 400 ]; do printf 'é:packet %d\\n' $i; i=$((i+1)); done;;\n *) echo \"# off=0 ctb=85 tag=1 hlen=3 plen=268\"; echo \":pubkey enc packet: version 3, algo 1, keyid 0123456789ABCDEF\";;\nesac\n"
	run := func(path, mode string) []any {
		t.Setenv("PATH", path)
		t.Setenv("PGP_MODE", mode)
		var out []any
		for _, c := range inputs {
			if mode != "" && !strings.Contains(c.input, "PGP") {
				continue
			}
			got, err := Modern(context.Background(), c.input, c.key, time.Unix(1700000000, 0))
			out = append(out, []any{c.input, c.key, mode, parity.Text(got), parity.Err(err)})
		}
		return out
	}
	cases := []parity.Case{{Fn: "nogpg", Out: run(t.TempDir(), "")}}
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "gpg"), []byte(gpg), 0o755); err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"ok", "err", "mid", "long"} {
		cases = append(cases, parity.Case{Fn: "gpg", In: gpg, Out: run(dir+":/usr/bin:/bin", mode)})
	}
	parity.Fixture(t, "modern", cases)
}
