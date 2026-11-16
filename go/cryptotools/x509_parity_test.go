package cryptotools

import (
	"context"
	"crypto"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/base64"
	"encoding/hex"
	"encoding/pem"
	"math/big"
	"net"
	"net/url"
	"regexp"
	"strings"
	"testing"
	"testing/cryptotest"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// tagPointer is the address encoding/asn1 prints for a field with an implicit tag in its "tags don't match" error (the
// certificate request's attributes): it differs on every run, so the fixture holds it as 0xADDR.
var tagPointer = regexp.MustCompile(`tag:0x[0-9a-f]+`)

// TestParityModernX509 records (testdata/parity/modernx509.json) crypto_modern on certificates and certificate requests:
// every key kind, names with every attribute and escape, SANs, validity before, during and after, the signature
// algorithms, every extension crypto/x509 reads in well-formed and broken forms, every truncation and three byte changes
// at every position of a small certificate and a request, bundles, the base64 DER form, and JWTs checked with a
// certificate as the key.
func TestParityModernX509(t *testing.T) {
	cryptotest.SetGlobalRandom(t, 2) // the same keys, serials and signatures on every run
	now := time.Unix(1700000000, 0)
	modern := func(input, key string) []any {
		got, err := Modern(context.Background(), input, key, now)
		return []any{parity.Text(tagPointer.ReplaceAllString(got, "tag:0xADDR")), parity.Err(err)}
	}
	must := func(b []byte, err error) []byte {
		t.Helper()
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	pemOf := func(label string, der []byte) string {
		return string(pem.EncodeToMemory(&pem.Block{Type: label, Bytes: der}))
	}
	row := func(label string, der []byte) []any {
		return append([]any{label, hex.EncodeToString(der)}, modern(pemOf(label, der), "")...)
	}
	caKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	rsaKey, _ := rsa.GenerateKey(rand.Reader, 2048)
	ca := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "Test CA", Organization: []string{"Example"}},
		NotBefore: time.Unix(1600000000, 0), NotAfter: time.Unix(1900000000, 0), IsCA: true, BasicConstraintsValid: true,
		KeyUsage: x509.KeyUsageCertSign}
	base := func() *x509.Certificate {
		return &x509.Certificate{SerialNumber: big.NewInt(4096), Subject: pkix.Name{CommonName: "leaf"},
			NotBefore: time.Unix(1690000000, 0), NotAfter: time.Unix(1710000000, 0)}
	}
	sign := func(tmpl *x509.Certificate, pub any) []byte {
		return must(x509.CreateCertificate(rand.Reader, tmpl, ca, pub, caKey))
	}
	edPub, edPriv, _ := ed25519.GenerateKey(rand.Reader)
	keys := []any{&rsaKey.PublicKey, &caKey.PublicKey, edPub}
	for _, c := range []elliptic.Curve{elliptic.P224(), elliptic.P384(), elliptic.P521()} {
		k, _ := ecdsa.GenerateKey(c, rand.Reader)
		keys = append(keys, &k.PublicKey)
	}
	small, _ := rsa.GenerateKey(rand.Reader, 1024)
	keys = append(keys, &small.PublicKey)

	var rows []any
	// the CA itself, self-signed, and leaves for every key kind
	caDER := must(x509.CreateCertificate(rand.Reader, ca, ca, &caKey.PublicKey, caKey))
	rows = append(rows, row("CERTIFICATE", caDER))
	for _, k := range keys {
		rows = append(rows, row("CERTIFICATE", sign(base(), k)))
	}
	// names
	oid := func(a ...int) asn1.ObjectIdentifier { return a }
	for _, n := range []pkix.Name{
		{Country: []string{"NL", "BE"}, Province: []string{"Utrecht"}, Locality: []string{"Here"}, StreetAddress: []string{"1 Main St"},
			PostalCode: []string{"1234 AB"}, Organization: []string{"Org, Inc.", "Other+Org"}, OrganizationalUnit: []string{"Unit"},
			CommonName: `name "quoted" <b>; \slash`, SerialNumber: "42"},
		{CommonName: " leading and trailing "}, {CommonName: "#hash first #"}, {CommonName: "één ünïcode ✓"}, {},
		{CommonName: "x", ExtraNames: []pkix.AttributeTypeAndValue{{Type: oid(1, 2, 3, 4), Value: "plain"}, {Type: oid(1, 2, 3, 5), Value: 42},
			{Type: oid(1, 2, 3, 6), Value: true}, {Type: oid(1, 2, 3, 7), Value: oid(1, 2, 840, 99)}, {Type: oid(1, 2, 3, 8), Value: []byte{1, 2}},
			{Type: oid(2, 5, 4, 3), Value: 7}, {Type: oid(1, 2, 840, 113549, 1, 9, 1), Value: "a@b.example"},
			{Type: oid(1, 2, 3, 9), Value: time.Date(2030, 1, 2, 3, 4, 5, 0, time.UTC)}, {Type: oid(1, 2, 3, 10), Value: time.Date(2060, 1, 2, 3, 4, 5, 0, time.UTC)},
			{Type: oid(1, 2, 3, 11), Value: asn1.BitString{Bytes: []byte{0xf0}, BitLength: 4}}, {Type: oid(1, 2, 3, 12), Value: asn1.RawValue{FullBytes: []byte{0x30, 0x03, 0x02, 0x01, 0x05}}},
			{Type: oid(1, 2, 3, 13), Value: asn1.RawValue{FullBytes: []byte{5, 0}}}, {Type: oid(2, 5, 4, 10), Value: "Extra Org"}}},
	} {
		tmpl := base()
		tmpl.Subject = n
		rows = append(rows, row("CERTIFICATE", sign(tmpl, &caKey.PublicKey)))
	}
	// SANs, usages, validity, serials
	u1, _ := url.Parse("https://svc.example/path?q=1")
	u2, _ := url.Parse("spiffe://trust.example/workload")
	for i, f := range []func(c *x509.Certificate){
		func(c *x509.Certificate) {
			c.DNSNames = []string{"a.example", "*.b.example"}
			c.IPAddresses = []net.IP{net.ParseIP("192.0.2.1"), net.ParseIP("2001:db8::1"), net.ParseIP("2001:db8:0:0:1:0:0:1"), net.ParseIP("::"), net.ParseIP("fe80::1:0:0:0")}
			c.EmailAddresses = []string{"x@a.example"}
			c.URIs = []*url.URL{u1, u2}
		},
		func(c *x509.Certificate) {
			c.ExtKeyUsage = []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth, x509.ExtKeyUsageClientAuth, x509.ExtKeyUsageAny, x509.ExtKeyUsageCodeSigning,
				x509.ExtKeyUsageEmailProtection, x509.ExtKeyUsageTimeStamping}
			c.UnknownExtKeyUsage = []asn1.ObjectIdentifier{oid(1, 2, 3)}
			c.KeyUsage = x509.KeyUsageDigitalSignature | x509.KeyUsageKeyEncipherment
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Unix(1800000000, 0), time.Unix(1900000000, 0)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Unix(1000000000, 0), time.Unix(1600000000, 0)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Unix(1699990000, 0), time.Unix(1700000000+3600*5, 0)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Unix(1699990000, 0), time.Unix(1700000000+86400*3+5, 0)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Unix(1600000000, 0), time.Unix(1700000000-60, 0)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Unix(1700000000, 0), time.Unix(1700000000, 0)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Date(1950, 1, 1, 0, 0, 0, 0, time.UTC), time.Date(9999, 12, 31, 23, 59, 59, 0, time.UTC)
		},
		func(c *x509.Certificate) {
			c.NotBefore, c.NotAfter = time.Date(1, 1, 1, 0, 0, 0, 0, time.UTC), time.Date(1949, 12, 31, 0, 0, 0, 0, time.UTC)
		},
		func(c *x509.Certificate) { c.SerialNumber = new(big.Int).Lsh(big.NewInt(1), 150) },
		func(c *x509.Certificate) { c.SerialNumber = big.NewInt(0) },
		func(c *x509.Certificate) { c.IsCA, c.BasicConstraintsValid, c.MaxPathLen = true, true, 2 },
		func(c *x509.Certificate) {
			c.PermittedDNSDomains, c.ExcludedDNSDomains = []string{".example", "a.example"}, []string{"bad.example"}
			_, n4, _ := net.ParseCIDR("10.0.0.0/8")
			_, n6, _ := net.ParseCIDR("2001:db8::/32")
			c.PermittedIPRanges = []*net.IPNet{n4, n6}
			c.PermittedEmailAddresses, c.PermittedURIDomains = []string{"x@a.example", "a.example"}, []string{".example"}
			c.PermittedDNSDomainsCritical = true
		},
		func(c *x509.Certificate) {
			c.CRLDistributionPoints, c.OCSPServer, c.IssuingCertificateURL = []string{"http://crl.example/a.crl"}, []string{"http://ocsp.example"}, []string{"http://ca.example/ca.crt"}
			c.SubjectKeyId, c.AuthorityKeyId = []byte{1, 2, 3}, []byte{4, 5, 6}
			c.Policies = []x509.OID{must2(x509.ParseOID("1.2.3.4")), must2(x509.ParseOID("2.5.29.32.0"))}
		},
	} {
		tmpl := base()
		tmpl.SerialNumber = big.NewInt(int64(5000 + i))
		f(tmpl)
		rows = append(rows, row("CERTIFICATE", sign(tmpl, &caKey.PublicKey)))
	}
	// signature algorithms: RSA ones made by the RSA key, others by patching the algorithm's OID in both places
	rsaCA := &x509.Certificate{SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "RSA CA"}, NotBefore: ca.NotBefore, NotAfter: ca.NotAfter}
	var rsaDER []byte
	for _, alg := range []x509.SignatureAlgorithm{x509.SHA256WithRSA, x509.SHA384WithRSA, x509.SHA512WithRSA, x509.SHA256WithRSAPSS, x509.SHA384WithRSAPSS, x509.SHA512WithRSAPSS} {
		rsaCA.SignatureAlgorithm = alg
		der := must(x509.CreateCertificate(rand.Reader, rsaCA, rsaCA, &rsaKey.PublicKey, rsaKey))
		rows = append(rows, row("CERTIFICATE", der))
		if alg == x509.SHA256WithRSA {
			rsaDER = der
		}
	}
	rows = append(rows, row("CERTIFICATE", must(x509.CreateCertificate(rand.Reader, &x509.Certificate{SerialNumber: big.NewInt(3), NotBefore: ca.NotBefore,
		NotAfter: ca.NotAfter}, &x509.Certificate{SerialNumber: big.NewInt(3)}, edPub, edPriv))))
	sha256RSA := []byte{0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b}
	for _, last := range []byte{4, 5, 2, 0x63, 0x0a, 0x0c} {
		patched := []byte(strings.ReplaceAll(string(rsaDER), string(sha256RSA), string(append(sha256RSA[:10:10], last))))
		rows = append(rows, row("CERTIFICATE", patched))
	}
	ecdsaOID := []byte{0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02}
	leaf := sign(base(), &caKey.PublicKey)
	for _, rep := range [][]byte{{0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x01}, {0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x04},
		{0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x38, 0x04, 0x03}} {
		rows = append(rows, row("CERTIFICATE", fixLengths(t, leaf, ecdsaOID, rep)))
	}
	// times as name attributes: read as crypto/x509 reads validity times, shown as asn1.Marshal writes them back
	for _, tm := range []string{"\x17\x0b2301021504Z", "\x17\x11230102150405+0100", "\x17\x11230102150405-0000", "\x17\x11230102150405+0000",
		"\x17\x11230102150405+2400", "\x17\x11230102150405+2460", "\x17\x11230102150405+2500", "\x17\x0d230229000000Z", "\x17\x0d240229000000Z",
		"\x17\x0d500101000000Z", "\x17\x0d491231235959Z", "\x17\x0d690101000000Z", "\x17\x0d680101000000Z", "\x17\x0d230102150460Z",
		"\x17\x0d230102156005Z", "\x17\x0a2301021504", "\x17\x0f230102150405.5Z", "\x17\x0d231301000000Z", "\x17\x0d230102240000Z",
		"\x17\x0f2301021504+0100", "\x17\x0d23010215040aZ", "\x18\x0f20230102150405Z", "\x18\x1320230102150405.123Z", "\x18\x0f19500101000000Z",
		"\x18\x0f20500101000000Z", "\x18\x0f00010101000000Z", "\x18\x1320230102150405+0130", "\x18\x1320230102150405-1159", "\x18\x0d202301021504Z",
		"\x18\x0f20230230150405Z", "\x18\x1100230102150405Z"} {
		tmpl := base()
		tmpl.Subject = pkix.Name{CommonName: "t", ExtraNames: []pkix.AttributeTypeAndValue{{Type: oid(1, 2, 3, 99), Value: asn1.RawValue{FullBytes: []byte(tm)}}}}
		rows = append(rows, row("CERTIFICATE", sign(tmpl, &caKey.PublicKey)))
	}
	// a length with a leading zero byte, which DER forbids
	long := sign(base(), &caKey.PublicKey)
	if long[1] == 0x82 {
		rows = append(rows, row("CERTIFICATE", append([]byte{0x30, 0x83, 0x00}, long[2:]...)))
	}
	cases := []parity.Case{{Fn: "cert", Out: rows}}

	// every extension crypto/x509 reads, well formed and broken, one per certificate
	ext := func(id asn1.ObjectIdentifier, critical bool, value ...byte) pkix.Extension {
		return pkix.Extension{Id: id, Critical: critical, Value: value}
	}
	e := func(n int) asn1.ObjectIdentifier { return oid(2, 5, 29, n) }
	aia := oid(1, 3, 6, 1, 5, 5, 7, 1, 1)
	seq := func(b ...byte) []byte { return append([]byte{0x30, byte(len(b))}, b...) }
	tlv := func(tag byte, b ...byte) []byte { return append([]byte{tag, byte(len(b))}, b...) }
	cat := func(parts ...[]byte) []byte {
		var out []byte
		for _, p := range parts {
			out = append(out, p...)
		}
		return out
	}
	oidBytes := func(o asn1.ObjectIdentifier) []byte { return must(asn1.Marshal(o)) }
	ocsp, issuers := oidBytes(oid(1, 3, 6, 1, 5, 5, 7, 48, 1)), oidBytes(oid(1, 3, 6, 1, 5, 5, 7, 48, 2))
	str := func(tag byte, s string) []byte { return tlv(tag, []byte(s)...) }
	ip := func(b ...byte) []byte { return tlv(0x87, b...) }
	subtree := func(b []byte) []byte { return seq(b...) }
	nc := func(permitted, excluded []byte) []byte {
		var body []byte
		if permitted != nil {
			body = append(body, tlv(0xa0, permitted...)...)
		}
		if excluded != nil {
			body = append(body, tlv(0xa1, excluded...)...)
		}
		return seq(body...)
	}
	var extensions [][]pkix.Extension
	for _, x := range []pkix.Extension{
		ext(e(15), true, 0x03, 0x02, 0x05, 0xa0), ext(e(15), false, 0x04, 0x00), ext(e(15), false, 0x03, 0x02, 0x07, 0x01), ext(e(15), false),
		ext(e(15), false, 0x03, 0x03, 0x07, 0xff, 0x80),
		ext(e(19), true, seq(0x01, 0x01, 0xff)...), ext(e(19), false, seq(0x01, 0x01, 0xff, 0x02, 0x01, 0x03)...), ext(e(19), false, seq()...),
		ext(e(19), false, seq(0x02, 0x01, 0xff)...), ext(e(19), false, seq(0x01, 0x01, 0x01)...), ext(e(19), false, 0x02, 0x01, 0x00),
		ext(e(19), false, seq(0x02, 0x09, 0x00, 0x80, 0, 0, 0, 0, 0, 0, 0)...), ext(e(19), false, seq(0x02, 0x01, 0x00)...),
		ext(e(17), false, seq(0x81, 0x01, 0xff)...), ext(e(17), false, seq(0x82, 0x01, 0xff)...), ext(e(17), false, seq(0x86, 0x01, 0xff)...),
		ext(e(17), false, seq(str(0x86, "http://a..b/")...)...), ext(e(17), false, seq(str(0x86, "%zz")...)...),
		ext(e(17), false, seq(str(0x86, "http://[::1]:80/x")...)...), ext(e(17), false, seq(str(0x86, "mailto:x@a.example")...)...),
		ext(e(17), false, seq(ip(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 1, 2, 3, 4)...)...), ext(e(17), false, seq(ip(1, 2, 3, 4, 5)...)...),
		ext(e(17), false, seq(0x88, 0x01, 0x2a)...), ext(e(17), true, seq()...), ext(e(17), false, 0x04, 0x00), ext(e(17), false, seq(0x82, 0x05, 'a')...),
		ext(e(17), false, seq(str(0x82, "x.example")...)...), ext(e(17), false, seq(str(0x86, "http://a.example./")...)...),
		ext(e(17), false, seq(str(0x86, "http://a.example:8080/")...)...), ext(e(30), false, nc(subtree(str(0x82, "a.example.")), nil)...),
		ext(e(30), true, nc(subtree(str(0x82, ".example")), subtree(ip(10, 0, 0, 0, 255, 0, 0, 0)))...),
		ext(e(30), false, nc(cat(subtree(str(0x81, "x@a.example")), subtree(str(0x81, ".a.example")), subtree(str(0x86, "a.example"))), nil)...),
		ext(e(30), false, nc(nil, nil)...), ext(e(30), false, nc([]byte{}, []byte{})...), ext(e(30), false, append(nc(subtree(str(0x82, "a")), nil), 0)...),
		ext(e(30), false, nc(subtree(str(0x82, "a..b")), nil)...), ext(e(30), false, nc(subtree(ip(1, 2, 3, 4, 5, 6, 7)), nil)...),
		ext(e(30), false, nc(subtree(ip(10, 0, 0, 0, 255, 0, 255, 0)), nil)...),
		ext(e(30), false, nc(subtree(ip(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 1, 2, 3, 4, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0)), nil)...),
		ext(e(30), false, nc(subtree(str(0x81, "x..y@a.example")), nil)...), ext(e(30), false, nc(subtree(str(0x81, "a..example")), nil)...),
		ext(e(30), false, nc(subtree(str(0x81, `"quoted\ local"@a.example`)), nil)...), ext(e(30), false, nc(subtree(str(0x81, "x@a@b")), nil)...),
		ext(e(30), false, nc(subtree(str(0x86, "1.2.3.4")), nil)...), ext(e(30), false, nc(subtree(str(0x86, "a..b")), nil)...),
		ext(e(30), false, nc(subtree(str(0x82, "é")), nil)...), ext(e(30), true, nc(subtree(str(0xa4, "")), nil)...),
		ext(e(30), false, nc(seq(), nil)...),
		ext(e(31), false, seq(seq(tlv(0xa0, tlv(0xa0, str(0x86, "http://crl.example/x")...)...)...)...)...), ext(e(31), false, seq(seq()...)...),
		ext(e(31), false, seq(0x02, 0x01, 0x00)...), ext(e(31), false, 0x04, 0x00), ext(e(31), false, seq(seq(tlv(0xa0, 0x02, 0x01, 0x00)...)...)...),
		ext(e(31), false, seq(seq(tlv(0xa0, tlv(0xa0, 0x82, 0x01, 'x')...)...)...)...),
		ext(e(35), true, seq(0x80, 0x01, 0x05)...), ext(e(35), false, seq(0x80, 0x01, 0x05)...), ext(e(35), false, 0x04, 0x00),
		ext(e(35), false, seq(0x81, 0x01, 0x05)...), ext(e(35), false, seq(0x80, 0x05, 0x05)...),
		ext(e(36), false, seq(0x80, 0x01, 0x00, 0x81, 0x01, 0x02)...), ext(e(36), false, seq(0x80, 0x02, 0x00, 0x01)...), ext(e(36), false, 0x04, 0x00),
		ext(e(36), false, seq(0x81, 0x09, 1, 2, 3, 4, 5, 6, 7, 8, 9)...),
		ext(e(37), false, seq(cat(oidBytes(oid(1, 3, 6, 1, 5, 5, 7, 3, 1)), oidBytes(oid(1, 2, 3)), oidBytes(oid(2, 5, 29, 37, 0)))...)...),
		ext(e(37), false, seq(0x02, 0x01, 0x00)...), ext(e(37), false, 0x04, 0x00),
		ext(e(14), true, 0x04, 0x01, 0x07), ext(e(14), false, 0x04, 0x01, 0x07), ext(e(14), false, 0x02, 0x01, 0x07),
		ext(e(32), false, seq(seq(oidBytes(oid(1, 2, 3))...)...)...), ext(e(32), false, seq(cat(seq(oidBytes(oid(1, 2, 3))...), seq(oidBytes(oid(1, 2, 3))...))...)...),
		ext(e(32), false, seq(seq(0x06, 0x02, 0x80, 0x01)...)...), ext(e(32), false, seq(seq(0x06, 0x01, 0x81)...)...), ext(e(32), false, seq(seq()...)...),
		ext(e(32), false, 0x04, 0x00),
		ext(e(33), false, seq(seq(cat(oidBytes(oid(1, 2, 3)), oidBytes(oid(1, 2, 4)))...)...)...), ext(e(33), false, seq(seq(oidBytes(oid(1, 2, 3))...)...)...),
		ext(e(33), false, 0x04, 0x00),
		ext(e(54), false, 0x02, 0x01, 0x00), ext(e(54), false, 0x02, 0x01, 0x05), ext(e(54), false, 0x04, 0x00),
		ext(aia, false, seq(cat(seq(cat(ocsp, str(0x86, "http://o.example"))...), seq(cat(issuers, str(0x86, "http://i.example/c"))...), seq(cat(oidBytes(oid(1, 2)), str(0x86, "x"))...), seq(cat(ocsp, str(0x82, "dns"))...))...)...),
		ext(aia, true, seq()...), ext(aia, false, 0x04, 0x00), ext(aia, false, seq(seq(0x02, 0x01, 0x00)...)...), ext(aia, false, seq(0x02, 0x01, 0x00)...),
		ext(aia, false, seq(seq(cat(ocsp, []byte{0x86, 0x05, 'x'})...)...)...),
		ext(oid(1, 2, 3, 4), true, 0x05, 0x00), ext(oid(1, 2, 3, 4), false), ext(e(99), true),
	} {
		extensions = append(extensions, []pkix.Extension{x})
	}
	extensions = append(extensions, []pkix.Extension{ext(oid(1, 2, 3), false), ext(oid(1, 2, 3), false)})
	var extRows []any
	for i, xs := range extensions {
		tmpl := base()
		tmpl.SerialNumber = big.NewInt(int64(6000 + i))
		tmpl.ExtraExtensions = xs
		extRows = append(extRows, row("CERTIFICATE", sign(tmpl, &caKey.PublicKey)))
	}
	cases = append(cases, parity.Case{Fn: "cert", In: "extensions", Out: extRows})

	// every truncation and three changes at every byte of a small certificate and a request
	smallTmpl := &x509.Certificate{SerialNumber: big.NewInt(7), Subject: pkix.Name{CommonName: "s", Organization: []string{"o"}},
		NotBefore: time.Unix(1690000000, 0), NotAfter: time.Unix(1710000000, 0), DNSNames: []string{"s.example"}, IPAddresses: []net.IP{{10, 0, 0, 1}},
		ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}, BasicConstraintsValid: true}
	smallCert := must(x509.CreateCertificate(rand.Reader, smallTmpl, smallTmpl, edPub, edPriv))
	csrTmpl := &x509.CertificateRequest{Subject: pkix.Name{CommonName: "req", Country: []string{"NL"}}, DNSNames: []string{"r.example", "s.example"},
		EmailAddresses: []string{"r@example.org"}}
	smallCSR := must(x509.CreateCertificateRequest(rand.Reader, csrTmpl, edPriv))
	for _, k := range []struct {
		label string
		der   []byte
	}{{"CERTIFICATE", smallCert}, {"CERTIFICATE REQUEST", smallCSR}} {
		var rows []any
		for i := range k.der {
			rows = append(rows, row(k.label, k.der[:i]))
			for _, x := range []byte{0x01, 0x80, 0xff} {
				c := append([]byte(nil), k.der...)
				c[i] ^= x
				rows = append(rows, row(k.label, c))
			}
		}
		rows = append(rows, row(k.label, append(append([]byte(nil), k.der...), 0)))
		cases = append(cases, parity.Case{Fn: "cert", In: k.label, Out: rows})
	}

	// certificate requests
	var csrRows []any
	for _, r := range []*x509.CertificateRequest{
		csrTmpl, {Subject: pkix.Name{CommonName: "plain"}},
		{Subject: pkix.Name{CommonName: "all", Organization: []string{"O"}}, DNSNames: []string{"a.example"}, IPAddresses: []net.IP{net.ParseIP("2001:db8::5")},
			URIs: []*url.URL{u1}, ExtraExtensions: []pkix.Extension{ext(oid(1, 2, 3), false, 0x05, 0x00)}},
		{Subject: pkix.Name{CommonName: "dup"}, ExtraExtensions: []pkix.Extension{ext(oid(1, 2, 3), false), ext(oid(1, 2, 3), true)}},
		{Subject: pkix.Name{CommonName: "badsan"}, ExtraExtensions: []pkix.Extension{ext(e(17), false, seq(0x82, 0x01, 0xff)...)}},
		{Subject: pkix.Name{CommonName: "badsan2"}, ExtraExtensions: []pkix.Extension{ext(e(17), false, 0x02, 0x01, 0x00)}},
	} {
		for _, priv := range []crypto.Signer{caKey, rsaKey, edPriv} {
			csrRows = append(csrRows, row("CERTIFICATE REQUEST", must(x509.CreateCertificateRequest(rand.Reader, r, priv))))
		}
	}
	// requests with hand-made attributes: an extension request whose extensions do not parse, attributes that are not one
	csrBody := func(attrs []byte) []byte {
		pki := must(x509.MarshalPKIXPublicKey(edPub))
		subj := must(asn1.Marshal(pkix.Name{CommonName: "hand"}.ToRDNSequence()))
		tbs := seqLong(cat([]byte{0x02, 0x01, 0x00}, subj, pki, attrs))
		sig := must(asn1.Marshal(asn1.BitString{Bytes: ed25519.Sign(edPriv, tbs), BitLength: 512}))
		return seqLong(cat(tbs, seq(0x06, 0x03, 0x2b, 0x65, 0x70), sig))
	}
	extReq := oidBytes(oid(1, 2, 840, 113549, 1, 9, 14))
	for _, attrs := range [][]byte{
		tlv(0xa0), tlv(0xa0, seq(cat(extReq, tlv(0x31, 0x02, 0x01, 0x00))...)...), tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(seq(0x02, 0x01, 0x00)...)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(seq(cat(oidBytes(oid(1, 2)), []byte{0x01, 0x01, 0x05}, []byte{0x04, 0x00})...)...)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(seq(cat(oidBytes(oid(1, 2)), []byte{0x02, 0x01, 0x05})...)...)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(seq(oidBytes(oid(1, 2))...)...)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31))...)...), tlv(0xa0, seq(oidBytes(oid(1, 2, 3))...)...), tlv(0xa0, 0x02, 0x01, 0x00),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(seq(cat(oidBytes(e(17)), tlv(0x04, seq(str(0x82, "q.example")...)...))...)...)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(seq(cat(oidBytes(e(17)), tlv(0x04, 0x02, 0x01, 0x00))...)...)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, tlv(0x04, 0x00)...))...)...), tlv(0xa1), tlv(0x80), nil, tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(0x30, 0x80)...))...)...),
		tlv(0xa0, seq(cat(extReq, tlv(0x31, seq(0x02, 0x01, 0x00)...))...)...),
	} {
		csrRows = append(csrRows, row("CERTIFICATE REQUEST", csrBody(attrs)))
	}
	csrRows = append(csrRows, row("CERTIFICATE REQUEST", csrBody(tlv(0xa0))[:5]), row("CERTIFICATE REQUEST", append(smallCSR, 0)))
	cases = append(cases, parity.Case{Fn: "cert", In: "requests", Out: csrRows})

	// bundles, the base64 DER form, and JWTs checked with certificates
	keyPEM := pemOf("PUBLIC KEY", must(x509.MarshalPKIXPublicKey(edPub)))
	b64 := base64.StdEncoding.EncodeToString(smallCert)
	var raw []any
	for _, in := range []string{pemOf("CERTIFICATE", smallCert) + pemOf("CERTIFICATE", caDER) + keyPEM,
		"-----BEGIN junk\n" + pemOf("CERTIFICATE", smallCert) + "-----BEGIN CERTIFICATE-----\n!!\n-----END CERTIFICATE-----\n" + pemOf("CERTIFICATE", []byte{0x30, 0}),
		pemOf("CERTIFICATE REQUEST", smallCSR) + pemOf("CERTIFICATE", smallCert), b64, b64[:40] + "\n " + b64[40:] + "\n", base64.StdEncoding.EncodeToString(append(smallCert, 0)),
		base64.StdEncoding.EncodeToString(smallCSR), "QUJD", base64.StdEncoding.EncodeToString(caDER)} {
		raw = append(raw, append([]any{in}, modern(in, "")...))
	}
	cases = append(cases, parity.Case{Fn: "raw", Out: raw})
	enc := base64.RawURLEncoding.EncodeToString
	signedPart := enc([]byte(`{"alg":"RS256"}`)) + "." + enc([]byte(`{"sub":"x"}`))
	h := sha256.Sum256([]byte(signedPart))
	rsSig := must(rsa.SignPKCS1v15(nil, rsaKey, crypto.SHA256, h[:]))
	rsaCA.SignatureAlgorithm = x509.SHA256WithRSA
	rsaCert := pemOf("CERTIFICATE", rsaDER)
	ecPart := enc([]byte(`{"alg":"ES256"}`)) + "." + enc([]byte(`{"sub":"x"}`))
	eh := sha256.Sum256([]byte(ecPart))
	r, s, _ := ecdsa.Sign(rand.Reader, caKey, eh[:])
	esSig := append(r.FillBytes(make([]byte, 32)), s.FillBytes(make([]byte, 32))...)
	var jwts []any
	for _, c := range []struct{ tok, key string }{
		{signedPart + "." + enc(rsSig), rsaCert}, {signedPart + "." + enc(rsSig), pemOf("CERTIFICATE", caDER)},
		{signedPart + "." + enc(rsSig), pemOf("RSA PUBLIC KEY", x509.MarshalPKCS1PublicKey(&rsaKey.PublicKey))},
		{signedPart + "." + enc(rsSig), pemOf("CERTIFICATE", rsaDER[:len(rsaDER)-1])},
		{ecPart + "." + enc(esSig), pemOf("CERTIFICATE", caDER)}, {ecPart + "." + enc(esSig), pemOf("CERTIFICATE", smallCert)},
		{ecPart + "." + enc(esSig), pemOf("CERTIFICATE", sign(base(), edPub))},
	} {
		jwts = append(jwts, append([]any{c.tok, c.key}, modern(c.tok, c.key)...))
	}
	cases = append(cases, parity.Case{Fn: "jwt", Out: jwts})
	parity.Fixture(t, "modernx509", cases)
}

func must2[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

// seqLong is a SEQUENCE of any length.
func seqLong(b []byte) []byte {
	out, err := asn1.Marshal(asn1.RawValue{Class: 0, Tag: 16, IsCompound: true, Bytes: b})
	if err != nil {
		panic(err)
	}
	return out
}

// fixLengths replaces old with rep inside a DER certificate's two signature algorithm identifiers, fixing the lengths of
// the enclosing SEQUENCEs (certificate, TBS, both identifiers) for a replacement of another length.
func fixLengths(t *testing.T, der, old, rep []byte) []byte {
	var cert struct {
		TBS    asn1.RawValue
		SigAlg asn1.RawValue
		Sig    asn1.RawValue
	}
	if _, err := asn1.Unmarshal(der, &cert); err != nil {
		t.Fatal(err)
	}
	swap := func(b []byte) []byte { return []byte(strings.ReplaceAll(string(b), string(old), string(rep))) }
	var tbs []asn1.RawValue
	rest := cert.TBS.Bytes
	for len(rest) > 0 {
		var v asn1.RawValue
		var err error
		if rest, err = asn1.Unmarshal(rest, &v); err != nil {
			t.Fatal(err)
		}
		tbs = append(tbs, v)
	}
	var body []byte
	for _, v := range tbs {
		if strings.Contains(string(v.FullBytes), string(old)) && v.Tag == 16 && len(v.FullBytes) < 20 {
			body = append(body, seqLong(swap(v.Bytes))...)
		} else {
			body = append(body, v.FullBytes...)
		}
	}
	return seqLong(append(append(seqLong(body), seqLong(swap(cert.SigAlg.Bytes))...), cert.Sig.FullBytes...))
}
