package cryptotools

import (
	"context"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/hmac"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/pem"
	"math/big"
	"strings"
	"testing"
	"time"
)

const prose = "Attack at dawn. The enemy camp lies to the north of the river and we must move before the first light of the morning or all is lost. " +
	"Send word to the other side and tell them to hold the bridge until we come. The gold is hidden under the old tree by the water."

func TestClassicKnownAnswers(t *testing.T) {
	if got := Caesar("attack", 3); got != "dwwdfn" {
		t.Fatalf("caesar: %q", got)
	}
	if got := Caesar("Hello, World!", 13); got != "Uryyb, Jbeyq!" {
		t.Fatalf("rot13: %q", got)
	}
	if got := Atbash("hello"); got != "svool" {
		t.Fatalf("atbash: %q", got)
	}
	v, err := Vigenere("attackatdawn", "LEMON", false)
	if err != nil || v != "lxfopvefrnhr" {
		t.Fatalf("vigenere: %q %v", v, err)
	}
	back, _ := Vigenere(v, "lemon", true)
	if back != "attackatdawn" {
		t.Fatalf("vigenere decode: %q", back)
	}
	rf, err := RailFence("WEAREDISCOVEREDFLEEATONCE", 3, false)
	if err != nil || rf != "WECRLTEERDSOEEFEAOCAIVDEN" {
		t.Fatalf("railfence: %q %v", rf, err)
	}
	if back, _ := RailFence(rf, 3, true); back != "WEAREDISCOVEREDFLEEATONCE" {
		t.Fatalf("railfence decode: %q", back)
	}
	key := "qwertyuiopasdfghjklzxcvbnm"
	sub, err := Substitution("hello world", key, false)
	if err != nil || sub != "itssg vgksr" {
		t.Fatalf("substitution: %q %v", sub, err)
	}
	if back, _ := Substitution(sub, key, true); back != "hello world" {
		t.Fatalf("substitution decode: %q", back)
	}
	if _, err := Substitution("x", "abc", false); err == nil {
		t.Fatal("a short substitution key must be refused")
	}
}

func TestCrackers(t *testing.T) {
	if gs := CrackCaesar(Caesar(prose, 7)); gs[0].Key != "shift 7" || !strings.HasPrefix(gs[0].Text, "Attack at dawn") {
		t.Fatalf("caesar crack: %+v", gs[0])
	}
	enc, _ := Vigenere(prose, "lemon", false)
	if gs := CrackVigenere(enc, 16); len(gs) == 0 || gs[0].Key != "lemon" {
		t.Fatalf("vigenere crack: %+v", gs)
	}
	enc2, _ := Vigenere(prose, "secretkey", false)
	if gs := CrackVigenere(enc2, 16); len(gs) == 0 || gs[0].Key != "secretkey" {
		t.Fatalf("vigenere crack (9-letter key): %+v", gs)
	}
	rf, _ := RailFence(prose, 4, false)
	if gs := CrackRailFence(rf, 10); gs[0].Key != "4 rails" {
		t.Fatalf("railfence crack: %+v", gs)
	}
	sub, _ := Substitution(prose+" "+prose, "zyxwvutsrqponmlkjihgfedcba", false)
	g := CrackSubstitution(sub, 10)
	if WordRatio(g.Text) < 0.5 {
		t.Fatalf("substitution crack: %.2f common words: %q", WordRatio(g.Text), g.Text[:60])
	}
}

func TestDecoders(t *testing.T) {
	cases := map[string][2]string{
		"hex":     {"48656c6c6f20776f726c64", "Hello world"},
		"base64":  {"SGVsbG8gd29ybGQ=", "Hello world"},
		"base32":  {"JBSWY3DPEB3W64TMMQ======", "Hello world"},
		"base58":  {"JxF12TrwUP45BMd", "Hello World"},
		"ascii85": {"<~87cURD]i,\"Ebo80~>", "Hello World!"},
		"binary":  {"01001000 01101001", "Hi"},
		"decimal": {"72 105 33", "Hi!"},
		"url":     {"Hello%20world%21", "Hello world!"},
		"rot13":   {"Uryyb", "Hello"},
		"morse":   {".... . .-.. .-.. --- / .-- --- .-. .-.. -..", "HELLO WORLD"},
	}
	for enc, c := range cases {
		got, err := DecodeOne(enc, c[0])
		if err != nil || string(got) != c[1] {
			t.Errorf("%s: %q %v", enc, got, err)
		}
		if !Looks(enc, c[0]) {
			t.Errorf("%s: Looks says no for %q", enc, c[0])
		}
	}
	chain := DecodeAuto(base64.StdEncoding.EncodeToString([]byte(hex.EncodeToString([]byte("The treasure is buried under the big oak tree")))), 3)
	if chain == nil || !chain.Text || len(chain.Steps) != 2 || chain.Steps[0].Encoding != "base64" || chain.Steps[1].Encoding != "hex" {
		t.Fatalf("auto chain: %+v", chain)
	}
	if !strings.Contains(chain.Describe(), "base64 -> hex") {
		t.Fatal(chain.Describe())
	}
	if DecodeAuto("just some words here", 3) != nil && DecodeAuto("just some words here", 3).Text {
		t.Fatal("plain words must not decode to something")
	}
}

func TestIdentify(t *testing.T) {
	top := func(s string) Candidate {
		cs, _ := Identify(s)
		if len(cs) == 0 {
			t.Fatalf("no candidates for %q", s)
		}
		return cs[0]
	}
	if c := top("5d41402abc4b2a76b9719d911017c592"); !strings.Contains(c.What, "MD5") {
		t.Fatalf("md5: %+v", c)
	}
	if c := top("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"); !strings.Contains(c.What, "SHA-256") {
		t.Fatalf("sha256: %+v", c)
	}
	if c := top("$2b$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW"); !strings.Contains(c.What, "bcrypt") {
		t.Fatalf("bcrypt: %+v", c)
	}
	if c := top("$argon2id$v=19$m=65536,t=3,p=4$c29tZXNhbHQ$RdescudvJCsgt3ub+b+dWRWJTmaaJObG"); !strings.Contains(c.What, "Argon2") {
		t.Fatalf("argon2: %+v", c)
	}
	if c := top("123e4567-e89b-12d3-a456-426614174000"); !strings.Contains(c.What, "UUID") {
		t.Fatalf("uuid: %+v", c)
	}
	jwt := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
	if c := top(jwt); !strings.Contains(c.What, "JWT") || !strings.Contains(c.Preview, "NOT verified") {
		t.Fatalf("jwt: %+v", c)
	}
	if c := top("-----BEGIN RSA PRIVATE KEY-----\nMIIB\n-----END RSA PRIVATE KEY-----"); !strings.Contains(c.What, "PRIVATE") || !strings.Contains(c.Preview, "do not") {
		t.Fatalf("private key: %+v", c)
	}
	if c := top("48656c6c6f20776f726c64"); c.What != "hex-encoded text" || c.Preview != "Hello world" {
		t.Fatalf("hex text: %+v", c)
	}
	if c := top(base64.StdEncoding.EncodeToString([]byte(hex.EncodeToString([]byte("hello"))))); c.What != "base64 of hex" {
		t.Fatalf("base64 of hex: %+v", c)
	}
	if c := top(Caesar(prose, 11)); !strings.Contains(c.What, "shift 11") {
		t.Fatalf("caesar: %+v", c)
	}
	if c := top(prose); c.What != "plain English text" {
		t.Fatalf("prose: %+v", c)
	}
	plain := []byte("Hello world, this is a test of the single byte xor cipher.")
	x := make([]byte, len(plain))
	for i := range plain {
		x[i] = plain[i] ^ 0x42
	}
	if c := top(hex.EncodeToString(x)); !strings.Contains(c.What, "XOR") || !strings.HasPrefix(c.Preview, "Hello world") {
		t.Fatalf("xor: %+v", c)
	}
	for i := range plain {
		x[i] = plain[i] ^ []byte("key")[i%3]
	}
	if c := top(base64.StdEncoding.EncodeToString(x)); !strings.Contains(c.What, "XOR") || !strings.HasPrefix(c.Preview, "Hello world") {
		t.Fatalf("repeating xor: %+v", c)
	}
	_, st := Identify("plain​‌text")
	if st.ZeroWidth != 2 || !strings.Contains(st.Describe(), "INVISIBLE") {
		t.Fatalf("zero-width: %+v", st)
	}
	if !strings.Contains(IdentifyReport("48656c6c6f", "string"), "Hello") {
		t.Fatal("report")
	}
}

func TestHashes(t *testing.T) {
	vectors := []struct{ algo, in, want string }{
		{"md5", "hello", "5d41402abc4b2a76b9719d911017c592"},
		{"sha1", "abc", "a9993e364706816aba3e25717850c26c9cd0d89d"},
		{"sha256", "", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
		{"sha512", "abc", "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"},
		{"sha3-256", "abc", "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"},
		{"crc32", "The quick brown fox jumps over the lazy dog", "414fa339"},
	}
	for _, v := range vectors {
		got, err := Digest(v.algo, []byte(v.in), nil)
		if err != nil || got != v.want {
			t.Errorf("%s(%q) = %s %v", v.algo, v.in, got, err)
		}
	}
	mac, _ := Digest("sha256", []byte("The quick brown fox jumps over the lazy dog"), []byte("key"))
	if mac != "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8" {
		t.Fatalf("hmac: %s", mac)
	}
	if _, err := Digest("crc32", []byte("x"), []byte("k")); err == nil {
		t.Fatal("crc32 hmac must be refused")
	}
	same, _ := Compare("5D41402ABC4B2A76B9719D911017C592", "5d41402abc4b2a76b9719d911017c592")
	if !same {
		t.Fatal("case and compare")
	}
	if same, d := Compare("5d41402abc4b2a76b9719d911017c592", "5d41402abc4b2a76b9719d911017c593"); same || !strings.Contains(d, "digit 32") {
		t.Fatalf("differ: %v %s", same, d)
	}
	if hits := WhichMatches([]byte("hello"), "5d41402abc4b2a76b9719d911017c592"); len(hits) != 1 || hits[0] != "md5" {
		t.Fatalf("which: %v", hits)
	}
	r := HashReport([]byte("hello"), "text", "sha256", "", "5d41402abc4b2a76b9719d911017c592")
	if !strings.Contains(r, "NO MATCH") || !strings.Contains(r, "IS the md5") {
		t.Fatal(r)
	}
}

func TestJWT(t *testing.T) {
	now := time.Unix(1700000000, 0)
	token := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
	out, err := Modern(context.Background(), token, "", now)
	if err != nil || !strings.Contains(out, "NOT VERIFIED") || !strings.Contains(out, "John Doe") {
		t.Fatalf("%v\n%s", err, out)
	}
	out, _ = Modern(context.Background(), token, "your-256-bit-secret", now)
	if !strings.Contains(out, "VERIFIED with the key") {
		t.Fatal(out)
	}
	out, _ = Modern(context.Background(), token, "wrong", now)
	if !strings.Contains(out, "does NOT verify") {
		t.Fatal(out)
	}
	none := base64.RawURLEncoding.EncodeToString([]byte(`{"alg":"none"}`)) + "." + base64.RawURLEncoding.EncodeToString([]byte(`{"sub":"x","exp":1600000000}`)) + "."
	out, err = Modern(context.Background(), none, "", now)
	if err != nil || !strings.Contains(out, "NO signature") || !strings.Contains(out, "EXPIRED") {
		t.Fatalf("%v\n%s", err, out)
	}
	// RS256 with a key made here
	priv, _ := rsa.GenerateKey(rand.Reader, 2048)
	h := base64.RawURLEncoding.EncodeToString([]byte(`{"alg":"RS256"}`))
	p := base64.RawURLEncoding.EncodeToString([]byte(`{"sub":"rs"}`))
	digest := sha256.Sum256([]byte(h + "." + p))
	sig, _ := rsa.SignPKCS1v15(rand.Reader, priv, 0, digest[:])
	_ = sig
	sig, _ = rsa.SignPKCS1v15(rand.Reader, priv, cryptoSHA256(), digest[:])
	der, _ := x509.MarshalPKIXPublicKey(&priv.PublicKey)
	pubPEM := string(pem.EncodeToMemory(&pem.Block{Type: "PUBLIC KEY", Bytes: der}))
	out, _ = Modern(context.Background(), h+"."+p+"."+base64.RawURLEncoding.EncodeToString(sig), pubPEM, now)
	if !strings.Contains(out, "VERIFIED with the key") {
		t.Fatal(out)
	}
}

func TestCertsAndKeys(t *testing.T) {
	priv, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	tmpl := &x509.Certificate{SerialNumber: big.NewInt(7), Subject: pkix.Name{CommonName: "test.example"}, DNSNames: []string{"test.example", "www.test.example"},
		NotBefore: time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC), NotAfter: time.Date(2021, 1, 1, 0, 0, 0, 0, time.UTC), IsCA: true, BasicConstraintsValid: true}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &priv.PublicKey, priv)
	if err != nil {
		t.Fatal(err)
	}
	certPEM := string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}))
	out, err := Modern(context.Background(), certPEM, "", time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC))
	if err != nil || !strings.Contains(out, "self-signed") || !strings.Contains(out, "EXPIRED") || !strings.Contains(out, "www.test.example") || !strings.Contains(out, "ECDSA P-256") {
		t.Fatalf("%v\n%s", err, out)
	}
	out, _ = Modern(context.Background(), certPEM, "", time.Date(2020, 6, 1, 0, 0, 0, 0, time.UTC))
	if !strings.Contains(out, "valid now") {
		t.Fatal(out)
	}
	keyDER, _ := x509.MarshalECPrivateKey(priv)
	out, err = Modern(context.Background(), string(pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: keyDER})), "", time.Now())
	if err != nil || !strings.Contains(out, "PRIVATE key") || strings.Contains(out, "MHcC") {
		t.Fatalf("private key must be refused without being shown: %v %s", err, out)
	}
	// ssh-ed25519
	pub, _, _ := ed25519.GenerateKey(rand.Reader)
	blob := sshString([]byte("ssh-ed25519"))
	blob = append(blob, sshString(pub)...)
	line := "ssh-ed25519 " + base64.StdEncoding.EncodeToString(blob) + " me@host"
	out, err = Modern(context.Background(), line, "", time.Now())
	fp := sha256.Sum256(blob)
	if err != nil || !strings.Contains(out, "256 bits") || !strings.Contains(out, strings.TrimRight(base64.StdEncoding.EncodeToString(fp[:]), "=")) || !strings.Contains(out, "me@host") {
		t.Fatalf("%v\n%s", err, out)
	}
	rsaKey, _ := rsa.GenerateKey(rand.Reader, 2048)
	blob = sshString([]byte("ssh-rsa"))
	blob = append(blob, sshString(big.NewInt(int64(rsaKey.E)).Bytes())...)
	blob = append(blob, sshString(append([]byte{0}, rsaKey.N.Bytes()...))...)
	out, _ = Modern(context.Background(), "ssh-rsa "+base64.StdEncoding.EncodeToString(blob), "", time.Now())
	if !strings.Contains(out, "2048 bits") {
		t.Fatal(out)
	}
}

func sshString(b []byte) []byte {
	out := make([]byte, 4+len(b))
	binary.BigEndian.PutUint32(out, uint32(len(b)))
	copy(out[4:], b)
	return out
}

func TestPrefetchAndClassic(t *testing.T) {
	if p := Prefetch("What is 48656c6c6f20776f726c64 here?"); !strings.Contains(p, "Hello world") {
		t.Fatal(p)
	}
	if p := Prefetch("How is the weather in Bozeman today, my friend?"); p != "" {
		t.Fatalf("plain words must prefetch nothing: %q", p)
	}
	if p := Prefetch("Look at /home/zlothra/Documents/Samples/x.bin please"); p != "" {
		t.Fatalf("a path is not data: %q", p)
	}
	out, err := Classic(Caesar(prose, 5), "caesar", "", false, true)
	if err != nil || !strings.Contains(out, "key shift 5") {
		t.Fatalf("%v %s", err, out)
	}
	if out, _ := Classic("hello", "caesar", "3", false, false); !strings.Contains(out, "khoor") {
		t.Fatal(out)
	}
	if _, err := Classic("hello", "caesar", "abc", false, false); err == nil {
		t.Fatal("caesar needs a number")
	}
	mac := hmac.New(sha256.New, []byte("k"))
	mac.Write([]byte("x"))
	if hex.EncodeToString(mac.Sum(nil)) == "" {
		t.Fatal("hmac")
	}
}

// Noise found while testing her answers: prose that happens to use only base64 letters was called base64, the weakest fallback was
// added next to a real reading, and a short Caesar was missed because letter frequency alone decides badly on short text.
func TestIdentifyDoesNotGuessOnProse(t *testing.T) {
	for _, word := range []string{"Cryptography", "steganography", "Labyrinth"} {
		cs, _ := Identify(word)
		for _, c := range cs {
			if strings.Contains(c.What, "base64") {
				t.Errorf("%q called %s", word, c.What)
			}
		}
	}
	cs, _ := Identify("SGVsbG8sIHdvcmxkISBUaGlzIGlzIGJhc2U2NC4=")
	if len(cs) == 0 || !strings.Contains(cs[0].What, "base64") {
		t.Fatalf("real base64 must still be found: %+v", cs)
	}
}

func TestTheWeakestGuessOnlyWhenNothingBetter(t *testing.T) {
	cs, _ := Identify("Gur dhvpx oebja sbk whzcf bire gur ynml qbt")
	for _, c := range cs {
		if strings.HasPrefix(c.What, "letters, not English") {
			t.Errorf("a weak fallback next to %q", cs[0].What)
		}
	}
}

func TestCaesarIsFoundInShortText(t *testing.T) {
	cs, _ := Identify("Uryyb jbeyq")
	if len(cs) == 0 || !strings.Contains(cs[0].What, "shift 13") {
		t.Fatalf("ROT13 of \"Hello world\" not found: %+v", cs)
	}
	gs := CrackCaesar("Uryyb jbeyq")
	if len(gs) == 0 || gs[0].Text != "Hello world" {
		t.Fatalf("%+v", gs)
	}
}

func TestBase64OfHexIsPeeledToTheText(t *testing.T) {
	got := IdentifyReport("NzQ2ODY1MjA2ZDY1NjU3NDY5NmU2NzIwNjk3MzIwNjE3NDIwNzQ2ODY1MjA2ZjZjNjQyMDZkNjk2YzZjMjA2MTc0MjA2ZTY5NmU2NQ==", "")
	if !strings.Contains(got, "the meeting is at the old mill at nine") {
		t.Fatalf("the text under both layers is not shown:\n%s", got)
	}
}
