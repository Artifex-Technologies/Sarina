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

	"github.com/Artifex-Technologies/Sarina/go/tools"
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

// E14 stage 1 part A: affine, Beaufort, autokey and columnar transposition, with known answers worked by hand or from the standard
// textbook examples, and crackers that recover the key from the shared test prose.
func TestStageOneCiphersKnownAnswers(t *testing.T) {
	if got, err := Affine("AFFINECIPHER", 5, 8, false); err != nil || got != "IHHWVCSWFRCP" {
		t.Fatalf("affine: %q %v", got, err)
	}
	if got, _ := Affine("IHHWVCSWFRCP", 5, 8, true); got != "AFFINECIPHER" {
		t.Fatalf("affine decode: %q", got)
	}
	if _, err := Affine("x", 13, 1, false); err == nil {
		t.Fatal("a = 13 shares a factor with 26 and must be refused")
	}
	for _, k := range []string{"5,8", "5 8", "a=5, b=8"} {
		if a, b, err := parseAffineKey(k); err != nil || a != 5 || b != 8 {
			t.Fatalf("affine key %q: %d %d %v", k, a, b, err)
		}
	}
	if _, _, err := parseAffineKey("5"); err == nil {
		t.Fatal("an affine key needs two numbers")
	}
	// Beaufort, by hand: K-H=3 D, E-E=0 A, Y-L=13 N, K-L=-1 Z, E-O=-10 Q
	if got, err := Beaufort("HELLO", "KEY"); err != nil || got != "DANZQ" {
		t.Fatalf("beaufort: %q %v", got, err)
	}
	if got, _ := Beaufort("DANZQ", "key"); got != "HELLO" {
		t.Fatalf("beaufort is its own inverse: %q", got)
	}
	if got, _ := Beaufort("DEFENDTHEEASTWALLOFTHECASTLE", "FORTIFICATION"); got != "CKMPVCPVWPIWUJOGIUAPVWRIWUUK" {
		t.Fatalf("beaufort, the textbook example: %q", got)
	}
	// autokey: the key stream is QUEENLY then the plaintext (a t t a c ...)
	if got, err := Autokey("attackatdawn", "QUEENLY", false); err != nil || got != "qnxepvytwtwp" {
		t.Fatalf("autokey: %q %v", got, err)
	}
	if got, _ := Autokey("Qnxe pvyt, wtwp!", "queenly", true); got != "Atta ckat, dawn!" {
		t.Fatalf("autokey decode keeps case and punctuation: %q", got)
	}
	if _, err := Autokey("x", "123", false); err == nil {
		t.Fatal("an autokey primer needs a letter")
	}
	// columnar, irregular (no padding): ZEBRAS reads the columns A B E R S Z, i.e. ranks 6 3 2 4 1 5
	for _, key := range []string{"ZEBRAS", "632415", "6 3 2 4 1 5"} {
		if got, err := Columnar("WEAREDISCOVEREDFLEEATONCE", key, false); err != nil || got != "EVLNACDTESEAROFODEECWIREE" {
			t.Fatalf("columnar with %q: %q %v", key, got, err)
		}
	}
	if got, _ := Columnar("EVLNACDTESEAROFODEECWIREE", "zebras", true); got != "WEAREDISCOVEREDFLEEATONCE" {
		t.Fatalf("columnar decode: %q", got)
	}
	if order, _ := columnOrder("zebras"); columnRanks(order) != "6 3 2 4 1 5" {
		t.Fatalf("ranks: %v", order)
	}
	for _, bad := range []string{"a", "1 1 2", "1 2 4", ""} {
		if _, err := Columnar("hello", bad, false); err == nil {
			t.Fatalf("columnar key %q must be refused", bad)
		}
	}
}

func TestStageOneCrackers(t *testing.T) {
	aff, _ := Affine(prose, 5, 8, false)
	if gs := CrackAffine(aff); gs[0].Key != "a=5, b=8" || !strings.HasPrefix(gs[0].Text, "Attack at dawn") {
		t.Fatalf("affine crack: %+v", gs[0])
	}
	bf, _ := Beaufort(prose, "lemon")
	if gs := CrackBeaufort(bf, 16); len(gs) == 0 || gs[0].Key != "lemon" || gs[0].Text != prose {
		t.Fatalf("beaufort crack: %+v", gs)
	}
	ak, _ := Autokey(prose, "queenly", false)
	if gs := CrackAutokey(ak, 12); len(gs) == 0 || gs[0].Key != "queenly" || gs[0].Text != prose {
		t.Fatalf("autokey crack: %+v", gs)
	}
	col, _ := Columnar(prose, "zebras", false)
	if gs := CrackColumnar(col, 8); len(gs) == 0 || gs[0].Key != "6 3 2 4 1 5" || gs[0].Text != prose {
		t.Fatalf("columnar crack: %+v", gs)
	}
	// no spaces to find words by: the letter pairs decide
	plain := strings.ToUpper(lowerLetters(prose)[:90])
	col2, _ := Columnar(plain, "cipher", false)
	if gs := CrackColumnar(col2, 8); len(gs) == 0 || gs[0].Text != plain || !readsAsEnglish(gs[0].Text) {
		t.Fatalf("columnar crack without spaces: %+v", gs)
	}
	if readsAsEnglish(col2) {
		t.Fatal("the transposed letters do not read as English")
	}
	for _, key := range []string{"zebras", "lantern", "abc", "dbca", "windows"} {
		c, _ := Columnar(plain, key, false)
		if gs := CrackColumnar(c, 8); gs[0].Text != plain {
			t.Fatalf("columnar crack with %s: %+v", key, gs[0])
		}
	}
	// short and without spaces, the message can come out rotated (it did with the key cipher on these 59 letters); the answer says so
	short := "THESPAREKEYISUNDERTHEBLUEFLOWERPOTBYTHEBACKDOOROFTHECOTTAGE"
	c, _ := Columnar(short, "cipher", false)
	if out, _ := Classic(c, "columnar", "", false, true); !strings.Contains(out, "read it as a circle") || !strings.Contains(out, "SPAREKEY") {
		t.Fatal(out)
	}
	if out, _ := Classic(col2, "columnar", "", false, true); strings.Contains(out, "read it as a circle") {
		t.Fatal("90 letters are not short: " + out)
	}
	if wordCover("") != 0 || wordCover("thekeyisunder") < 0.99 || wordCover("xqzjvkwpqxzj") > 0 {
		t.Fatal("word cover")
	}
}

// Every crack shows its work, so she can say how it fell from what was computed.
func TestCrackOutputShowsTheWork(t *testing.T) {
	vig, _ := Vigenere(prose, "lemon", false)
	out, err := Classic(vig, "vigenere", "", false, true)
	if err != nil || !strings.Contains(out, "How it fell") || !strings.Contains(out, "Kasiski:") || !strings.Contains(out, "Factors the distances share most: 5 (") ||
		!strings.Contains(out, "Index of coincidence per key length") || !strings.Contains(out, "5 0.087") || !strings.Contains(out, "Letter frequencies") {
		t.Fatalf("%v\n%s", err, out)
	}
	bf, _ := Beaufort(prose, "lemon")
	if out, _ := Classic(bf, "beaufort", "", false, true); !strings.Contains(out, "key lemon") || !strings.Contains(out, "Kasiski:") {
		t.Fatal(out)
	}
	ak, _ := Autokey(prose, "queenly", false)
	if out, _ := Classic(ak, "autokey", "", false, true); !strings.Contains(out, "key queenly") || !strings.Contains(out, "does not repeat") || !strings.Contains(out, "per primer length") {
		t.Fatal(out)
	}
	col, _ := Columnar(prose, "zebras", false)
	if out, _ := Classic(col, "columnar", "", false, true); !strings.Contains(out, "key 6 3 2 4 1 5") || !strings.Contains(out, "only moves letters") || !strings.Contains(out, "per key length: 2 ") {
		t.Fatal(out)
	}
	aff, _ := Affine(prose, 5, 8, false)
	if out, _ := Classic(aff, "affine", "", false, true); !strings.Contains(out, "key a=5, b=8") || !strings.Contains(out, "English-like, so one alphabet") || strings.Contains(out, "Kasiski") {
		t.Fatal(out)
	}
	if out, _ := Classic("IHHWVCSWFRCP", "affine", "5,8", true, false); !strings.Contains(out, "AFFINECIPHER") {
		t.Fatal(out)
	}
	if out, _ := Classic("HELLO", "beaufort", "KEY", false, false); !strings.Contains(out, "DANZQ") {
		t.Fatal(out)
	}
	if out, _ := Classic("attackatdawn", "autokey", "queenly", false, false); !strings.Contains(out, "qnxepvytwtwp") {
		t.Fatal(out)
	}
	if out, _ := Classic("WEAREDISCOVEREDFLEEATONCE", "columnar", "ZEBRAS", false, false); !strings.Contains(out, "EVLNACDTESEAROFODEECWIREE") || !strings.Contains(out, "6 3 2 4 1 5") {
		t.Fatal(out)
	}
	if _, err := Classic("hello", "affine", "13,2", false, false); err == nil {
		t.Fatal("affine a=13 must be refused")
	}
	// a text with no repeating trigram says so instead of inventing distances
	if k := kasiski("abcdefghij", 16); !strings.Contains(k, "no trigram repeats") {
		t.Fatal(k)
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
	if p := Prefetch("Look at /home/sam/Documents/Samples/x.bin please"); p != "" {
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

func TestAClassicalCipherInTheQuestionIsComputedFirst(t *testing.T) {
	for prompt, want := range map[string]string{
		`A friend sent me this: "fn unjen cqn lrch cxwrpqc jwm vnnc cqn xcqnab jc cqn qjakxda knoxan cqn kxjcb px xdc frcq cqn crmn". Can you read it?`: "we leave the city tonight",
		`My friend sent me this: Gur fgngvba vf pybfrq hagvy Zbaqnl. What does it say?`:                                                                 "station is closed until monday",
	} {
		got := strings.ToLower(Prefetch(prompt))
		if !strings.Contains(got, want) {
			t.Errorf("%q: the prefetch does not read it:\n%s", prompt, got)
		}
	}
	if Prefetch("Can you explain how a Caesar cipher works: it shifts every letter by a fixed amount.") != "" {
		t.Fatal("plain English after a colon is not a cipher")
	}
}

func TestVigenereIsReadInThePrefetch(t *testing.T) {
	got := strings.ToLower(Prefetch("This is a Vigenere cipher and I lost the key: hji gbzf ms oitmeq iphee hji oyr qek gfgi bl hji rvjgv aar yi mhgv qois kx brtqve gvg jiegv pitvv sf gvg qoebkrg bf cpl vg nssg hq xhr spiml"))
	if !strings.Contains(got, "key ocean") || !strings.Contains(got, "old oak tree") {
		t.Fatalf("the readable key does not lead:\n%s", got)
	}
}

// The empty string is a real input for a hash (eval crypto-hash-says, 2026-11-04: the tool refused it, so the one check that
// answers "what does e3b0c442... say" failed); for every other tool, no text and no path is still an error.
func TestHashToolTakesTheEmptyString(t *testing.T) {
	t.Setenv("SARINA_CRYPTO", "on")
	var hash tools.Tool
	for _, tl := range Tools() {
		if tl.Spec.Name == "crypto_hash" {
			hash = tl
		}
	}
	out, _, err := hash.Handler(context.Background(), map[string]any{"text": "", "algo": "sha256",
		"expected": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}, &tools.Context{Root: t.TempDir()})
	if err != nil || !strings.Contains(strings.ToLower(out), "match") {
		t.Fatalf("empty string: %v\n%s", err, out)
	}
	if _, _, err := hash.Handler(context.Background(), map[string]any{"algo": "sha256"}, &tools.Context{Root: t.TempDir()}); err == nil {
		t.Fatal("neither text nor path given: want an error")
	}
}

// The token itself given as "path" is read as text (crypto-jwt-not-verified, night 2): a missing short file name stays an error.
func TestDataGivenAsPathIsReadAsText(t *testing.T) {
	root := t.TempDir()
	tok := "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2ln"
	b, src, err := input(map[string]any{"path": tok}, &tools.Context{Root: root})
	if err != nil || string(b) != tok || src != "text" {
		t.Fatalf("token as path: %q %q %v", b, src, err)
	}
	if _, _, err := input(map[string]any{"path": "notes.txt"}, &tools.Context{Root: root}); err == nil {
		t.Fatal("a missing file name must stay an error")
	}
}
