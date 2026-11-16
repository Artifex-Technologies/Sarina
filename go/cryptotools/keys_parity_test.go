package cryptotools

import (
	"context"
	"crypto"
	"crypto/ecdh"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/sha512"
	"crypto/x509"
	"encoding/asn1"
	"encoding/base64"
	"encoding/hex"
	"encoding/pem"
	"math/big"
	"strings"
	"testing"
	"testing/cryptotest"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityModernKeys records (testdata/parity/modernkeys.json) crypto_modern on public keys in PEM (PKIX and PKCS#1:
// RSA, ECDSA on the four NIST curves, Ed25519, X25519, ML-DSA, DSA, unknown ones; every truncation and three byte
// changes at every position of small keys; malformed parameters and bit strings), the PEM framing itself, and JWTs signed
// RS/PS/ES at 256/384/512 checked against right, wrong and unfitting keys and against broken signatures. Certificates and
// certificate requests are the next part.
func TestParityModernKeys(t *testing.T) {
	cryptotest.SetGlobalRandom(t, 1) // the same keys and signatures on every run
	now := time.Unix(1700000000, 0)
	modern := func(input, key string) []any {
		got, err := Modern(context.Background(), input, key, now)
		return []any{parity.Text(got), parity.Err(err)}
	}
	pemOf := func(label string, der []byte) string {
		return string(pem.EncodeToMemory(&pem.Block{Type: label, Bytes: der}))
	}
	pemRow := func(label string, der []byte) []any {
		return append([]any{label, hex.EncodeToString(der)}, modern(pemOf(label, der), "")...)
	}
	must := func(b []byte, err error) []byte {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	type spkiAI struct {
		Algorithm  asn1.ObjectIdentifier
		Parameters asn1.RawValue `asn1:"optional"`
	}
	type spki struct {
		Algorithm spkiAI
		PublicKey asn1.BitString
	}
	oidDER := func(o asn1.ObjectIdentifier) asn1.RawValue { return asn1.RawValue{FullBytes: must(asn1.Marshal(o))} }
	mkSPKI := func(alg asn1.ObjectIdentifier, params asn1.RawValue, key []byte, bits int) []byte {
		return must(asn1.Marshal(spki{spkiAI{alg, params}, asn1.BitString{Bytes: key, BitLength: bits}}))
	}
	null := asn1.RawValue{FullBytes: []byte{5, 0}}
	none := asn1.RawValue{}
	var (
		oRSA     = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 1, 1}
		oDSA     = asn1.ObjectIdentifier{1, 2, 840, 10040, 4, 1}
		oEC      = asn1.ObjectIdentifier{1, 2, 840, 10045, 2, 1}
		oX25519  = asn1.ObjectIdentifier{1, 3, 101, 110}
		oEd25519 = asn1.ObjectIdentifier{1, 3, 101, 112}
		oP256    = asn1.ObjectIdentifier{1, 2, 840, 10045, 3, 1, 7}
	)
	pkcs1 := func(n *big.Int, e any) []byte {
		return must(asn1.Marshal(struct {
			N *big.Int
			E any
		}{n, e}))
	}
	rsaSPKI := func(n *big.Int, e any) []byte { return mkSPKI(oRSA, null, pkcs1(n, e), 0) }

	rsaKeys := map[int]*rsa.PrivateKey{}
	for _, bits := range []int{1024, 1025, 2048, 3072} {
		k, err := rsa.GenerateKey(rand.Reader, bits)
		if err != nil {
			t.Fatal(err)
		}
		rsaKeys[bits] = k
	}
	t.Setenv("GODEBUG", "rsa1024min=0")
	small, err := rsa.GenerateKey(rand.Reader, 512)
	if err != nil {
		t.Fatal(err)
	}
	t.Setenv("GODEBUG", "") // Sarina runs with Go's default: keys under 1024 bits are refused
	ecKeys := map[string]*ecdsa.PrivateKey{}
	for _, c := range []elliptic.Curve{elliptic.P224(), elliptic.P256(), elliptic.P384(), elliptic.P521()} {
		k, err := ecdsa.GenerateKey(c, rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		ecKeys[c.Params().Name] = k
	}
	edPub, _, _ := ed25519.GenerateKey(rand.Reader)
	xKey, _ := ecdh.X25519().GenerateKey(rand.Reader)
	pkix := func(pub any) []byte { return must(x509.MarshalPKIXPublicKey(pub)) }

	// whole keys, each kind
	var rows []any
	for _, bits := range []int{1024, 1025, 2048, 3072} {
		rows = append(rows, pemRow("PUBLIC KEY", pkix(&rsaKeys[bits].PublicKey)), pemRow("RSA PUBLIC KEY", x509.MarshalPKCS1PublicKey(&rsaKeys[bits].PublicKey)))
	}
	rows = append(rows, pemRow("PUBLIC KEY", pkix(&small.PublicKey)))
	for _, name := range []string{"P-224", "P-256", "P-384", "P-521"} {
		rows = append(rows, pemRow("PUBLIC KEY", pkix(&ecKeys[name].PublicKey)))
	}
	rows = append(rows, pemRow("PUBLIC KEY", pkix(edPub)), pemRow("PUBLIC KEY", pkix(xKey.PublicKey())))
	mlRaw := make([]byte, 2592)
	_, _ = rand.Read(mlRaw)
	for i, size := range []int{1312, 1952, 2592} {
		o := asn1.ObjectIdentifier{2, 16, 840, 1, 101, 3, 4, 3, 17 + i}
		rows = append(rows, pemRow("PUBLIC KEY", mkSPKI(o, none, mlRaw[:size], size*8)), pemRow("PUBLIC KEY", mkSPKI(o, none, mlRaw[:size-1], (size-1)*8)),
			pemRow("PUBLIC KEY", mkSPKI(o, null, mlRaw[:size], size*8)))
	}
	dsaParams := func(p, q, g int64) asn1.RawValue {
		return asn1.RawValue{FullBytes: must(asn1.Marshal(struct{ P, Q, G *big.Int }{big.NewInt(p), big.NewInt(q), big.NewInt(g)}))}
	}
	dsaY := func(y int64) []byte { return must(asn1.Marshal(big.NewInt(y))) }
	rows = append(rows,
		pemRow("PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 11, 4), dsaY(8), 0)), pemRow("PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 0, 4), dsaY(8), 0)),
		pemRow("PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 11, 4), dsaY(-8), 0)), pemRow("PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 11, -4), dsaY(8), 0)),
		pemRow("PUBLIC KEY", mkSPKI(oDSA, null, dsaY(8), 0)), pemRow("PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 11, 4), []byte{4, 1, 8}, 0)),
		pemRow("PUBLIC KEY", mkSPKI(asn1.ObjectIdentifier{1, 2, 3, 4}, null, []byte{1, 2, 3}, 24)),
		pemRow("PUBLIC KEY", mkSPKI(asn1.ObjectIdentifier{1, 2, 3, 4}, none, nil, 0)))

	// RSA numbers and parameters
	n := rsaKeys[1024].N
	rows = append(rows,
		pemRow("PUBLIC KEY", mkSPKI(oRSA, none, pkcs1(n, 65537), 0)), pemRow("PUBLIC KEY", mkSPKI(oRSA, asn1.RawValue{FullBytes: []byte{5, 1, 0}}, pkcs1(n, 65537), 0)),
		pemRow("PUBLIC KEY", rsaSPKI(new(big.Int).Neg(n), 65537)), pemRow("PUBLIC KEY", rsaSPKI(big.NewInt(0), 65537)),
		pemRow("PUBLIC KEY", rsaSPKI(n, 0)), pemRow("PUBLIC KEY", rsaSPKI(n, -3)), pemRow("PUBLIC KEY", rsaSPKI(n, int64(1)<<40)),
		pemRow("PUBLIC KEY", rsaSPKI(n, new(big.Int).Lsh(big.NewInt(1), 70))), pemRow("PUBLIC KEY", rsaSPKI(big.NewInt(1), 3)),
		pemRow("PUBLIC KEY", rsaSPKI(new(big.Int).Lsh(big.NewInt(1), 2047), 3)), pemRow("PUBLIC KEY", rsaSPKI(new(big.Int).Lsh(big.NewInt(1), 2046), 3)),
		pemRow("PUBLIC KEY", mkSPKI(oRSA, null, append(pkcs1(n, 65537), 0, 0), 0)), pemRow("PUBLIC KEY", mkSPKI(oRSA, null, []byte{0x30, 0}, 0)),
		pemRow("RSA PUBLIC KEY", pkcs1(n, 0)), pemRow("RSA PUBLIC KEY", pkcs1(new(big.Int).Neg(n), 3)), pemRow("RSA PUBLIC KEY", pkcs1(n, int64(1)<<31-1)),
		pemRow("RSA PUBLIC KEY", pkcs1(n, int64(1)<<31)), pemRow("RSA PUBLIC KEY", pkcs1(n, new(big.Int).Lsh(big.NewInt(1), 70))),
		pemRow("RSA PUBLIC KEY", pkcs1(big.NewInt(0), 3)), pemRow("RSA PUBLIC KEY", append(pkcs1(n, 3), 0)),
		pemRow("RSA PUBLIC KEY", pkix(&rsaKeys[1024].PublicKey)), pemRow("PUBLIC KEY", x509.MarshalPKCS1PublicKey(&rsaKeys[1024].PublicKey)),
		pemRow("PUBLIC KEY", append(pkix(&rsaKeys[1024].PublicKey), 5, 0)), pemRow("PUBLIC KEY", nil), pemRow("RSA PUBLIC KEY", nil),
		pemRow("RSA PUBLIC KEY", pkcs1(n, "e")), pemRow("RSA PUBLIC KEY", must(asn1.Marshal(struct{ N, E, F int }{1, 2, 3}))),
		pemRow("RSA PUBLIC KEY", must(asn1.Marshal(struct{ N int }{1}))), pemRow("RSA PUBLIC KEY", must(asn1.Marshal(struct{}{}))),
		pemRow("PUBLIC KEY", must(asn1.Marshal(struct {
			A spkiAI
			B int
		}{spkiAI{oRSA, null}, 3}))),
		pemRow("PUBLIC KEY", must(asn1.Marshal(struct{ A, B int }{1, 2}))), pemRow("PUBLIC KEY", must(asn1.Marshal(struct{ A int }{1}))),
		pemRow("PUBLIC KEY", must(asn1.Marshal(asn1.ObjectIdentifier{1, 2}))), pemRow("PUBLIC KEY", []byte{0x30, 0x80, 0, 0}),
		pemRow("PUBLIC KEY", []byte{0x30, 0x81, 0x05, 0, 0, 0, 0, 0}), pemRow("PUBLIC KEY", []byte{0x30, 0x82, 0, 0x90}),
		pemRow("PUBLIC KEY", []byte{0x30, 0x85, 1, 1, 1, 1, 1}), pemRow("PUBLIC KEY", []byte{0x1f, 0x05, 0}), pemRow("PUBLIC KEY", []byte{0x3f, 0x81, 0x00, 0}),
		pemRow("PUBLIC KEY", []byte{0x30, 0x84, 0x7f, 0xff, 0xff, 0xff}), pemRow("PUBLIC KEY", []byte{0x30, 0x84, 0x80, 0, 0, 0}),
		pemRow("PUBLIC KEY", []byte{0x30, 0x84, 0x00, 0x80, 0, 0}), pemRow("PUBLIC KEY", append([]byte{0x30, 0x81, 0x7f}, make([]byte, 0x7f)...)), pemRow("PUBLIC KEY", []byte{0x30, 0x06, 0x30, 0x04, 0x06, 0x02, 0x80, 0x01}),
		pemRow("PUBLIC KEY", []byte{0x30, 0x0a, 0x30, 0x05, 0x06, 0x03, 0x88, 0x80, 0x80, 0x03, 0x01, 0x00}))

	// ECDSA points and parameters
	p256 := ecKeys["P-256"]
	pt := elliptic.Marshal(elliptic.P256(), p256.X, p256.Y) //nolint:staticcheck // the uncompressed point bytes
	bad := func(f func(b []byte)) []byte { c := append([]byte(nil), pt...); f(c); return c }
	pp := elliptic.P256().Params().P.FillBytes(make([]byte, 32))
	for _, key := range [][]byte{pt, pt[:64], nil, {4}, {0}, bad(func(b []byte) { b[0] = 2 }), bad(func(b []byte) { b[64] ^= 1 }),
		bad(func(b []byte) { copy(b[1:33], pp) }), bad(func(b []byte) { copy(b[33:], pp) }), append(append([]byte(nil), pt...), 0)} {
		rows = append(rows, pemRow("PUBLIC KEY", mkSPKI(oEC, oidDER(oP256), key, len(key)*8)))
	}
	for _, name := range []string{"P-224", "P-384", "P-521"} {
		k := ecKeys[name]
		q := elliptic.Marshal(k.Curve, k.X, k.Y) //nolint:staticcheck // the uncompressed point bytes
		params := oidDER(map[string]asn1.ObjectIdentifier{"P-224": {1, 3, 132, 0, 33}, "P-384": {1, 3, 132, 0, 34}, "P-521": {1, 3, 132, 0, 35}}[name])
		cut := func(f func(b []byte)) []byte {
			c := append([]byte(nil), q...)
			f(c)
			return mkSPKI(oEC, params, c, len(c)*8)
		}
		pb := k.Curve.Params().P.FillBytes(make([]byte, (k.Curve.Params().BitSize+7)/8))
		rows = append(rows, pemRow("PUBLIC KEY", cut(func(b []byte) { b[len(b)-1] ^= 1 })), pemRow("PUBLIC KEY", cut(func(b []byte) { copy(b[1:], pb) })),
			pemRow("PUBLIC KEY", cut(func(b []byte) { copy(b[1+len(pb):], pb) })), pemRow("PUBLIC KEY", mkSPKI(oEC, oidDER(oP256), q, len(q)*8)))
	}
	rows = append(rows,
		pemRow("PUBLIC KEY", mkSPKI(oEC, none, pt, 0)), pemRow("PUBLIC KEY", mkSPKI(oEC, null, pt, 0)),
		pemRow("PUBLIC KEY", mkSPKI(oEC, oidDER(asn1.ObjectIdentifier{1, 3, 132, 0, 10}), pt, 0)),
		pemRow("PUBLIC KEY", mkSPKI(oEC, asn1.RawValue{FullBytes: append(must(asn1.Marshal(oP256)), 1, 2)}, pt, 0)),
		pemRow("PUBLIC KEY", mkSPKI(oEd25519, null, edPub, 0)), pemRow("PUBLIC KEY", mkSPKI(oEd25519, none, edPub[:31], 0)),
		pemRow("PUBLIC KEY", mkSPKI(oEd25519, none, edPub, 255)), pemRow("PUBLIC KEY", mkSPKI(oEd25519, none, append(edPub[:31:31], 0x80), 249)),
		pemRow("PUBLIC KEY", mkSPKI(oX25519, null, xKey.PublicKey().Bytes(), 0)), pemRow("PUBLIC KEY", mkSPKI(oX25519, none, edPub[:31], 0)))
	cases := []parity.Case{{Fn: "pem", Out: rows}}

	// every truncation and three changes at every byte of small keys
	for _, k := range []struct {
		label string
		der   []byte
	}{{"PUBLIC KEY", pkix(&small.PublicKey)}, {"RSA PUBLIC KEY", x509.MarshalPKCS1PublicKey(&small.PublicKey)}, {"PUBLIC KEY", pkix(&p256.PublicKey)},
		{"PUBLIC KEY", pkix(edPub)}, {"PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 11, 4), dsaY(8), 0)}} {
		var rows []any
		for i := range k.der {
			rows = append(rows, pemRow(k.label, k.der[:i]))
			for _, x := range []byte{0x01, 0x80, 0xff} {
				c := append([]byte(nil), k.der...)
				c[i] ^= x
				rows = append(rows, pemRow(k.label, c))
			}
		}
		cases = append(cases, parity.Case{Fn: "pem", In: k.label, Out: rows})
	}

	// the PEM framing
	body := base64.StdEncoding.EncodeToString(pkix(edPub))
	blk := "-----BEGIN PUBLIC KEY-----\n" + body + "\n-----END PUBLIC KEY-----"
	var raw []any
	for _, in := range []string{blk, blk + "\n", "intro text\n" + blk + "\noutro", strings.ReplaceAll(blk, "\n", "\r\n"),
		"-----BEGIN PUBLIC KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES\n\n" + body + "\n-----END PUBLIC KEY-----",
		"-----BEGIN PUBLIC KEY-----\nA: b\n-----END PUBLIC KEY-----", "-----BEGIN PUBLIC KEY-----\nA: b",
		"-----BEGIN PUBLIC KEY-----\n" + body[:20] + " \t" + body[20:] + "\n-----END PUBLIC KEY-----",
		"-----BEGIN PUBLIC KEY-----\n" + body + "\n-----END RSA PUBLIC KEY-----", "-----BEGIN PUBLIC KEY-----\n" + body + "\n-----END PUBLIC KEY----- trailing",
		"-----BEGIN PUBLIC KEY-----\n" + body + "\n-----END PUBLIC KEY-----   \nnext", "-----BEGIN PUBLIC KEY-----\n" + body + "\n-----END PUBLIC KEY",
		"x-----BEGIN PUBLIC KEY-----\n" + body + "\n-----END PUBLIC KEY-----", "-----BEGIN PUBLIC KEY-----\n" + body[:10] + "!" + body[11:] + "\n-----END PUBLIC KEY-----",
		"-----BEGIN PUBLIC KEY-----\n-----END PUBLIC KEY-----", "-----BEGIN FOO-----\nAAEC\n-----END FOO-----", "-----BEGIN -----\n\n----- END -----",
		"-----BEGIN PUBLIC KEY-----\n!!\n-----END PUBLIC KEY-----\n" + blk, blk + "\n" + blk + "\n-----BEGIN X-----\nAA==\n-----END X-----\n",
		"-----BEGIN PUBLIC KEY-----", "-----BEGIN PUBLIC KEY-----" + body + "-----END PUBLIC KEY-----", "-----BEGIN PUBLIC KEY-----\n" + body + "-----END PUBLIC KEY-----",
		"-----BEGIN PUBLIC KEY-----\r\n" + body + "\r\n-----END PUBLIC KEY-----\r\n", "-----BEGIN A-----\n-----BEGIN B-----\nAAAA\n-----END B-----",
		"-----BEGIN B-----\nAAAA\n-----END A-----\n-----END B-----", "-----BEGIN B-----  \nAAAA\n-----END B-----", "-----BEGIN B-----\nAAA=\n-----END B-----",
		"-----BEGIN B-----\nAA\n==\n-----END B-----", "-----BEGIN B-----\nk: v: w\n\nAAAA\n-----END B-----", "-----BEGIN B-----\n : \nAAAA\n-----END B-----",
		"-----BEGIN B-----\nAAAA\n-----END B-----\n-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----"} {
		raw = append(raw, append([]any{in}, modern(in, "")...))
	}
	cases = append(cases, parity.Case{Fn: "raw", Out: raw})

	// JWTs signed with RS/PS/ES keys, checked against keys right and wrong
	enc := base64.RawURLEncoding.EncodeToString
	digest := func(size int, s string) (crypto.Hash, []byte) {
		switch size {
		case 384:
			h := sha512.Sum384([]byte(s))
			return crypto.SHA384, h[:]
		case 512:
			h := sha512.Sum512([]byte(s))
			return crypto.SHA512, h[:]
		}
		h := sha256.Sum256([]byte(s))
		return crypto.SHA256, h[:]
	}
	signed := func(alg string) string { return enc([]byte(`{"alg":"`+alg+`"}`)) + "." + enc([]byte(`{"sub":"x"}`)) }
	rsaSig := func(k *rsa.PrivateKey, alg string, size int, pss *rsa.PSSOptions) []byte {
		h, d := digest(size, signed(alg))
		if pss != nil {
			return must(rsa.SignPSS(rand.Reader, k, h, d, pss))
		}
		return must(rsa.SignPKCS1v15(nil, k, h, d))
	}
	ecSig := func(k *ecdsa.PrivateKey, alg string, size int) []byte {
		_, d := digest(size, signed(alg))
		r, s, err := ecdsa.Sign(rand.Reader, k, d)
		if err != nil {
			t.Fatal(err)
		}
		b := (k.Curve.Params().BitSize + 7) / 8
		return append(r.FillBytes(make([]byte, b)), s.FillBytes(make([]byte, b))...)
	}
	pemKey := func(pub any) string { return pemOf("PUBLIC KEY", pkix(pub)) }
	var jwts []any
	add := func(alg string, sig []byte, key string) {
		tok := signed(alg) + "." + enc(sig)
		jwts = append(jwts, append([]any{tok, key}, modern(tok, key)...))
	}
	flip := func(b []byte, i int) []byte { c := append([]byte(nil), b...); c[(i+len(c))%len(c)] ^= 1; return c }
	for _, size := range []int{256, 384, 512} {
		for _, bits := range []int{1024, 1025, 2048} {
			k := rsaKeys[bits]
			rs, alg := rsaSig(k, "RS"+itoa(size), size, nil), "RS"+itoa(size)
			add(alg, rs, pemKey(&k.PublicKey))
			add(alg, flip(rs, -1), pemKey(&k.PublicKey))
			add(alg, rs, pemKey(&rsaKeys[3072].PublicKey))
			add(alg, rs[1:], pemKey(&k.PublicKey))
			add(alg, append([]byte{0}, rs...), pemKey(&k.PublicKey))
			add(alg, k.N.Bytes(), pemKey(&k.PublicKey))
			add("PS"+itoa(size), rs, pemKey(&k.PublicKey))
			for _, opts := range []*rsa.PSSOptions{{SaltLength: rsa.PSSSaltLengthAuto}, {SaltLength: rsa.PSSSaltLengthEqualsHash}, {SaltLength: 0}} {
				if bits <= 1025 && size == 512 && opts.SaltLength == rsa.PSSSaltLengthEqualsHash {
					continue // a 512-bit hash and salt do not fit a key of 1025 bits or fewer
				}
				ps, alg := rsaSig(k, "PS"+itoa(size), size, opts), "PS"+itoa(size)
				add(alg, ps, pemKey(&k.PublicKey))
				add(alg, flip(ps, 0), pemKey(&k.PublicKey))
				add(alg, flip(ps, -1), pemKey(&k.PublicKey))
				add("RS"+itoa(size), ps, pemKey(&k.PublicKey))
			}
		}
		if size == 256 { // the larger hashes do not fit a 512-bit key
			t.Setenv("GODEBUG", "rsa1024min=0")
			rs, ps := rsaSig(small, "RS256", size, nil), rsaSig(small, "PS256", size, &rsa.PSSOptions{SaltLength: 0})
			t.Setenv("GODEBUG", "")
			add("RS256", rs, pemKey(&small.PublicKey))
			add("PS256", ps, pemKey(&small.PublicKey))
		}
		for _, name := range []string{"P-224", "P-256", "P-384", "P-521"} {
			k := ecKeys[name]
			alg := "ES" + itoa(size)
			es := ecSig(k, alg, size)
			add(alg, es, pemKey(&k.PublicKey))
			add(alg, flip(es, 0), pemKey(&k.PublicKey))
			add(alg, flip(es, -1), pemKey(&k.PublicKey))
			add(alg, es[1:], pemKey(&k.PublicKey))
			add(alg, es, pemKey(&ecKeys[map[bool]string{true: "P-384", false: "P-256"}[name == "P-256"]].PublicKey))
			add("RS"+itoa(size), es, pemKey(&k.PublicKey))
			b := len(es) / 2
			nb := k.Curve.Params().N.FillBytes(make([]byte, b))
			r, s := es[:b], es[b:]
			negS := new(big.Int).Sub(k.Curve.Params().N, new(big.Int).SetBytes(s)).FillBytes(make([]byte, b))
			add(alg, append(append([]byte(nil), r...), negS...), pemKey(&k.PublicKey))
			add(alg, append(make([]byte, b), s...), pemKey(&k.PublicKey))
			add(alg, append(append([]byte(nil), r...), make([]byte, b)...), pemKey(&k.PublicKey))
			add(alg, append(append([]byte(nil), nb...), s...), pemKey(&k.PublicKey))
			add(alg, append(append([]byte(nil), r...), nb...), pemKey(&k.PublicKey))
			rn := new(big.Int).Add(new(big.Int).SetBytes(r), k.Curve.Params().N)
			if rn.BitLen() <= 8*b {
				add(alg, append(rn.FillBytes(make([]byte, b)), s...), pemKey(&k.PublicKey))
			}
			add(alg, es, pemKey(&rsaKeys[1024].PublicKey))
			add("RS"+itoa(size), rsaSig(rsaKeys[1024], "RS"+itoa(size), size, nil), pemKey(&k.PublicKey))
		}
		alg := "ES" + itoa(size)
		add(alg, make([]byte, 64), pemKey(edPub))
		add(alg, make([]byte, 64), pemKey(xKey.PublicKey()))
		add(alg, make([]byte, 64), pemOf("PUBLIC KEY", mkSPKI(oDSA, dsaParams(23, 11, 4), dsaY(8), 0)))
	}
	// e = 1: the signature is its own padded digest, which Go refuses for the exponent
	h, d := digest(256, signed("RS256"))
	em, prefix := make([]byte, 128), map[crypto.Hash][]byte{crypto.SHA256: {0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20}}[h]
	em[1] = 1
	for i := 2; i < 128-len(prefix)-len(d)-1; i++ {
		em[i] = 0xff
	}
	copy(em[128-len(prefix)-len(d):], prefix)
	copy(em[128-len(d):], d)
	for _, e := range []int64{1, 2, 3, 1<<31 + 1, 1 << 31} {
		add("RS256", em, pemOf("PUBLIC KEY", rsaSPKI(rsaKeys[1024].N, e)))
	}
	evenN := new(big.Int).Add(rsaKeys[1024].N, big.NewInt(1))
	add("RS256", em, pemOf("PUBLIC KEY", rsaSPKI(evenN, 1)))
	add("RS256", em, pemOf("PUBLIC KEY", rsaSPKI(evenN, 3)))
	add("RS256", rsaSig(rsaKeys[1024], "RS256", 256, nil), "")
	add("RS256", rsaSig(rsaKeys[1024], "RS256", 256, nil), "garbage")
	add("RS256", rsaSig(rsaKeys[1024], "RS256", 256, nil), "-----BEGIN PUBLIC KEY-----\n!!\n-----END PUBLIC KEY-----")
	add("Rs256", rsaSig(rsaKeys[1024], "Rs256", 256, nil), pemKey(&rsaKeys[1024].PublicKey))
	add("es256", ecSig(p256, "es256", 256), pemKey(&p256.PublicKey))
	add("ES256", ecSig(p256, "ES256", 256), " \n"+pemKey(&p256.PublicKey)+"junk")
	tok := signed("RS256") + "." + enc(rsaSig(rsaKeys[1024], "RS256", 256, nil)) + "=="
	jwts = append(jwts, append([]any{tok, pemKey(&rsaKeys[1024].PublicKey)}, modern(tok, pemKey(&rsaKeys[1024].PublicKey))...))
	tok = signed("RS256") + ".AAA!"
	jwts = append(jwts, append([]any{tok, pemKey(&rsaKeys[1024].PublicKey)}, modern(tok, pemKey(&rsaKeys[1024].PublicKey))...))
	cases = append(cases, parity.Case{Fn: "jwt", Out: jwts})
	parity.Fixture(t, "modernkeys", cases)
}

func itoa(n int) string { return big.NewInt(int64(n)).String() }
