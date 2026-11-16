package cryptotools

import (
	"encoding/base32"
	"encoding/base64"
	"encoding/hex"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityIdentify records (testdata/parity/identify.json) crypto_identify's report on strings and file bytes of
// every kind it names: framed keys and tokens, hashes and addresses, encodings in layers, XOR, the classical ciphers it
// cracks itself (with the work), text, bytes that are not UTF-8, invisible characters, and nothing it knows.
func TestParityIdentify(t *testing.T) {
	b64 := base64.StdEncoding.EncodeToString
	xor := func(s string, key string) []byte {
		out := []byte(s)
		for i := range out {
			out[i] ^= key[i%len(key)]
		}
		return out
	}
	msg := "the treasure is hidden under the old oak tree near the river"
	aff, _ := Affine("the treasure map is hidden behind the clock in the library, meet me there at noon", 7, 3, false)
	bf, _ := Beaufort("the signal fire will be lit on the north tower when the ships are seen, then ride to the castle and warn the king before the gates close at dusk", "falcon")
	ak, _ := Autokey("send the letters to the old lighthouse on the hill and wait there until the keeper comes back with the boat", "storm", false)
	col, _ := Columnar("THESPAREKEYISUNDERTHEBLUEFLOWERPOTBYTHEBACKDOOROFTHECOTTAGESOLETYOURSELFINANDFEEDTHECAT", "garden", false)
	vig, _ := Vigenere(prose, "lemon", false)
	rf, _ := RailFence(strings.ToUpper(lowerLetters(prose)), 3, false)
	sub, _ := Substitution(prose, "qwertyuiopasdfghjklzxcvbnm", false)
	rs := []rune(strings.Repeat("THEKEYISUNDERTHEMAT", 4))
	for i := range rs {
		j := (i*7 + 3) % len(rs)
		rs[i], rs[j] = rs[j], rs[i]
	}
	inputs := []string{"", "   ", "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----", "-----BEGIN PGP MESSAGE-----", "-----BEGIN OPENSSH PRIVATE KEY-----",
		"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl user@host",
		"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
		"eyJ0eXAiOiJKV1QifQ.eyJzdWIiOiIxIn0.", "abcdefgh.ij.kl", "123e4567-e89b-12d3-a456-426614174000", "123e4567-e89b-92d3-a456-426614174000",
		"aad3b435b51404eeaad3b435b51404ee:31d6cfe0d16ae931b73c59d7e0c089c0", "$2b$12$abcdefghijklmnopqrstuuabcdefghijklmnopqrstuvwxyz12345",
		"$6$salt$hash", "$argon2id$v=19$m=65536", "$pbkdf2-sha256$29000$x", "$y$j9T$x", "$9$nope", "0x52908400098527886E0F7030069857D2E4169EE7",
		"1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa", "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq", "192.168.1.1", "999.1.1.1", "00:1A:2B:3C:4D:5E",
		"d41d8cd98f00b204e9800998ecf8427e", "da39a3ee5e6b4b0d3255bfef95601890afd80709", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
		"deadbeef", "cafebabe12345678", hex.EncodeToString([]byte("hello world, this is text")), hex.EncodeToString(xor(msg, "\x5a")),
		hex.EncodeToString(xor(msg, "key")), hex.EncodeToString([]byte{0, 1, 2, 3, 200, 201, 202, 203, 9, 9}), "4142 4344\f",
		b64([]byte(msg)), b64(xor(msg, "\x13")), b64(xor(msg, "secret")), b64([]byte(hex.EncodeToString([]byte(msg)))),
		b64([]byte(hex.EncodeToString([]byte{1, 2, 3, 4, 5, 6, 7, 200}))), b64([]byte{1, 2, 3, 250, 251, 252, 1, 2, 3, 4, 5, 6}),
		base32.StdEncoding.EncodeToString([]byte(msg)), "3yZe7d5Ld8GdKTQBXjk8vJA", "<~87cURD]i,\"Ebo80~>", "01101000 01101001 00100000 01110100 01101000 01100101 01110010 01100101",
		"104 101 108 108 111 32 116 104 101 114 101", ".... . .-.. .-.. --- / .-- --- .-. .-.. -..", "hello%20world%21%20this%20is%20it", "a%2Fb",
		prose, Caesar(prose, 7), Caesar("we leave the city tonight and meet the others at the harbour", 9), Atbash(prose), aff, bf, ak, col, vig, rf,
		sub, string(rs), "Je ne sais pas pourquoi elle est partie ce matin sans rien dire a personne",
		"Kubernetes ingress controllers terminate TLS before proxying upstream workloads", "xqzv jkwp mmnb", "QWERTYUIOPASDFGHJKLZ",
		"hello​world‌ secret\U000E0041", "\xff\xfe\x00\x01binary\x00data\xff", strings.Repeat("\x00\x01\x02\xff", 64),
		"SGVsbG8sIFdvcmxkIQ==", "aGVsbG8gd29ybGQ", "Hello, how are you today?", "Straße und Größe", "{\"a\": 1}",
		"ÄÖÜ ÉÈ Ωmega", "abcdefgh 12345678901234567890", "the xqzv bnml kjhg", "eeeeeeeeeettttttttttaaaaaaaaaaoooooooooo", strings.Repeat("eeeeettttaaaooonnnxz", 4),
		hex.EncodeToString(xor("Order 66, 1,2,3; 4.5! (6) [7] {8} <9> #10 $11 %12 &13 *14 +15 =16 ?17 @18", "ab")),
		b64(xor("Total: 1,234.56; tax 7.8% -> 98.76 (paid) [ok] {id:42} #7 the end", "k3y"))}
	var out []any
	for _, s := range inputs {
		out = append(out, []any{parity.Text(s), parity.Text(IdentifyReport(s, "string"))})
	}
	parity.Fixture(t, "identify", []parity.Case{{Fn: "report", Out: out}})
}
