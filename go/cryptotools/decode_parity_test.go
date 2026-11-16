package cryptotools

import (
	"encoding/base64"
	"encoding/hex"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityDecode records (testdata/parity/decode.json) crypto_decode: which encodings a string looks like, each
// decoder on good and broken input with Go's own error texts, the automatic layered decoding and its description.
func TestParityDecode(t *testing.T) {
	var cases []parity.Case
	b64 := base64.StdEncoding.EncodeToString
	hello := "Hello, World! This is a secret message."
	inputs := []string{"", "  ", hello, "48656c6c6f", "48 65 6c 6c 6f", "48:65:6C:6c:6F", "4865 6c6c 6f", "486", "zz", "4g", "0x48",
		b64([]byte(hello)), "SGVsbG8", "SGVsbG8=", "SGVsbG8==", "SGVs bG8=", "SGVsbG8===", "SG=sbG8=", "S===", "SGVsbG8_LQ", "-_-_", "SGV\nsbG8=",
		"SGVsbA==", "SGVsbB==", "aGVsbG8gd29ybGQ", "JBSWY3DP", "JBSWY3DPEB3W64TMMQ======", "JBSWY3DPEB3W64TMMQ", "jbswy3dp", "JBSWY3D=",
		"JBSWY3DPE======", "JB======", "J=======", "JBSWY===", "JBSWY3DPEHPK3PXP", "1", "1111", "StV1DL6CwTryKyV", "0OIl", "3yZe7d",
		"<~87cURD]i,\"Ebo80~>", "<~87cURD]i,\"Ebo8~>", "<~z~>", "<~ab~>", "<~a~>", "<~vvvv~>", "87cURD", "01001000 01101001", "0100100001101001",
		"0100100", "01001000 0110100x", "72 101 108 108 111", "72,101,108,108,111", "72 101 256", "72 -1", "1 2", "Hello%20World%21",
		"a+b%2Bc", "%zz", "abc%4", "%e2%82%ac", "%ff%fe", ".... . .-.. .-.. --- / .-- --- .-. .-.. -..", "... --- ...", ".-.-.- ..--.. -....- .--.-.",
		"......", "- . ...- / ", "Uryyb, Jbeyq! Guvf vf n frperg zrffntr.", "Gur dhvpx oebja sbk", "abcd", "Ab1!", hex.EncodeToString([]byte(b64([]byte(hello)))),
		b64([]byte(hex.EncodeToString([]byte(hello)))), b64([]byte(b64([]byte(hello)))), b64([]byte{0, 1, 2, 3, 255, 254, 253, 0x80, 0x81}),
		b64([]byte(strings.Repeat("\xffxyz", 80))), hex.EncodeToString([]byte(strings.Repeat("A", 300))), b64([]byte("%48%65%6c%6c%6f world wide")),
		"4a4b4c4d4e4f505152535455565758595a", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "€uro", "café au lait",
		"JBSWY3==", "JBSWY3D==", "JBSWY3DPEB======", "JBSWY3DPEBLW6===", "JBSWY3DPEBLW64Q=", "<~9jqo^~>", "<~87cU~>", "<~87c~>", "<~F)Po,~>",
		"<~uuu~>", "<~!!!~>", b64([]byte(Caesar(hello+" The enemy is near the old bridge.", 13))), hex.EncodeToString([]byte(formFeeds(b64([]byte(hello)))))}
	var looks, one []any
	for _, s := range inputs {
		var l []string
		for _, enc := range Encodings {
			if Looks(enc, s) {
				l = append(l, enc)
			}
		}
		looks = append(looks, []any{s, l})
		for _, enc := range append(append([]string{}, Encodings...), "uuencode") {
			out, err := DecodeOne(enc, s)
			if err == nil && len(out) == 0 {
				out = []byte{}
			}
			one = append(one, []any{enc, s, parity.Text(string(out)), parity.Err(err)})
		}
	}
	cases = append(cases, parity.Case{Fn: "looks", Out: looks}, parity.Case{Fn: "one", Out: one})
	var auto []any
	for _, s := range inputs {
		for _, depth := range []int{0, 1, 3} {
			auto = append(auto, []any{s, depth, parity.Text(DecodeAuto(s, depth).Describe())})
		}
	}
	cases = append(cases, parity.Case{Fn: "auto", Out: auto})
	parity.Fixture(t, "decode", cases)
}

// formFeeds puts a form feed (a regexp space that is not printable) after every 14th character: about 7 % of the text.
func formFeeds(s string) string {
	var b strings.Builder
	for i, r := range s {
		b.WriteRune(r)
		if i%14 == 13 {
			b.WriteByte('\f')
		}
	}
	return b.String()
}
