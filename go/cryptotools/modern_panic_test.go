package cryptotools

import (
	"context"
	"encoding/base64"
	"strings"
	"testing"
	"time"
)

// crypto_modern never panics on what a model or a file hands it: a "PRIVATE KEY-----" or "-----BEGIN PGP" without a whole
// PEM header, and a JWT whose alg is shorter than two characters checked with a key, each indexed past the end of a
// slice (the tool answered "failed internally"; `sarina crypto modern` ended with a Go trace).
func TestModernDoesNotPanicOnBrokenHeaders(t *testing.T) {
	t.Setenv("PATH", t.TempDir()) // no gpg: the PGP answer from the header alone
	enc := base64.RawURLEncoding.EncodeToString
	payload := enc([]byte(`{"sub":"x"}`))
	for _, c := range []struct{ input, key, want string }{
		{"see the PRIVATE KEY-----", "", "PRIVATE key"},
		{"-----BEGIN PGP", "", "PGP"},
		{"-----BEGIN PGP MESSAGE", "", "PGP"},
		{enc([]byte(`{"alg":"H"}`)) + "." + payload + ".c2ln", "secret", "not one I verify"},
		{enc([]byte(`{"alg":""}`)) + "." + payload + ".c2ln", "secret", "not one I verify"},
		{enc([]byte(`{"typ":"JWT"}`)) + "." + payload + ".c2ln", "secret", "not one I verify"},
	} {
		func() {
			defer func() {
				if p := recover(); p != nil {
					t.Errorf("%q panicked: %v", c.input, p)
				}
			}()
			out, err := Modern(context.Background(), c.input, c.key, time.Unix(0, 0))
			if err != nil || !strings.Contains(out, c.want) {
				t.Errorf("%q: %v %q", c.input, err, out)
			}
		}()
	}
}
