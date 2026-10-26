package cryptotools

import (
	"bytes"
	"context"
	"crypto"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/hmac"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/sha512"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"hash"
	"math/big"
	"os/exec"
	"sort"
	"strings"
	"time"
)

// Modern explains a certificate, a public key, a JWT or a PGP block: what it is and what can be checked about it, without ever
// treating a signature as verified unless the key to verify it with was given. key is an HMAC secret or a PEM public key for a JWT.
func Modern(ctx context.Context, input, key string, now time.Time) (string, error) {
	t := strings.TrimSpace(input)
	switch {
	case t == "":
		return "", fmt.Errorf("nothing to look at")
	case strings.Contains(t, "-----BEGIN PGP"):
		return pgpInfo(ctx, t), nil
	case strings.Contains(t, "PRIVATE KEY-----"):
		return "This is a PRIVATE key (" + pemRE.FindStringSubmatch(t)[1] + "). I do not parse, print or load private keys: keep it out of the conversation, and if it was pasted somewhere it should not have been, treat it as exposed and make a new one.", nil
	case strings.Contains(t, "-----BEGIN"):
		return pemInfo(t, now)
	case sshRE.MatchString(t):
		return sshInfo(t)
	case jwtRE.MatchString(t):
		return jwtInfo(t, key, now)
	}
	if der, err := base64.StdEncoding.DecodeString(strings.Join(strings.Fields(t), "")); err == nil {
		if c, err := x509.ParseCertificate(der); err == nil {
			return certInfo(c, now, 1, 1), nil
		}
	}
	return "", fmt.Errorf("not a certificate, public key, SSH key, JWT or PGP block that I recognise; crypto_identify says what it might be")
}

func pemInfo(t string, now time.Time) (string, error) {
	var b strings.Builder
	rest := []byte(t)
	n, total := 0, strings.Count(t, "-----BEGIN")
	for {
		var block *pem.Block
		block, rest = pem.Decode(rest)
		if block == nil {
			break
		}
		n++
		switch block.Type {
		case "CERTIFICATE":
			c, err := x509.ParseCertificate(block.Bytes)
			if err != nil {
				fmt.Fprintf(&b, "Certificate %d of %d: cannot parse it (%v).\n", n, total, err)
				continue
			}
			b.WriteString(certInfo(c, now, n, total))
		case "PUBLIC KEY", "RSA PUBLIC KEY":
			var pub any
			var err error
			if block.Type == "RSA PUBLIC KEY" {
				pub, err = x509.ParsePKCS1PublicKey(block.Bytes)
			} else {
				pub, err = x509.ParsePKIXPublicKey(block.Bytes)
			}
			if err != nil {
				fmt.Fprintf(&b, "Public key: cannot parse it (%v).\n", err)
				continue
			}
			fmt.Fprintf(&b, "Public key: %s. SHA-256 of its DER: %x.\n", keyDesc(pub), sha256.Sum256(block.Bytes))
		case "CERTIFICATE REQUEST":
			r, err := x509.ParseCertificateRequest(block.Bytes)
			if err != nil {
				fmt.Fprintf(&b, "Certificate request: cannot parse it (%v).\n", err)
				continue
			}
			fmt.Fprintf(&b, "Certificate signing request for %s (SANs: %s), key %s, signature %s.\n", r.Subject, strings.Join(r.DNSNames, ", "), keyDesc(r.PublicKey), r.SignatureAlgorithm)
		default:
			fmt.Fprintf(&b, "PEM block of type %s (%d bytes): not something I parse.\n", block.Type, len(block.Bytes))
		}
	}
	if n == 0 {
		return "", fmt.Errorf("the PEM block does not decode (missing lines or a broken base64 body)")
	}
	return b.String(), nil
}

func keyDesc(pub any) string {
	switch k := pub.(type) {
	case *rsa.PublicKey:
		return fmt.Sprintf("RSA %d bits%s", k.N.BitLen(), map[bool]string{true: " (under 2048: too small for today)", false: ""}[k.N.BitLen() < 2048])
	case *ecdsa.PublicKey:
		return "ECDSA " + k.Curve.Params().Name
	case ed25519.PublicKey:
		return "Ed25519"
	}
	return fmt.Sprintf("%T", pub)
}

func certInfo(c *x509.Certificate, now time.Time, n, total int) string {
	var b strings.Builder
	fmt.Fprintf(&b, "Certificate %d of %d\n", n, total)
	fmt.Fprintf(&b, "  Subject: %s\n  Issuer: %s%s\n", c.Subject, c.Issuer, map[bool]string{true: " (self-signed: subject and issuer are the same)", false: ""}[c.Subject.String() == c.Issuer.String()])
	if len(c.DNSNames)+len(c.IPAddresses)+len(c.EmailAddresses) > 0 {
		var sans []string
		sans = append(sans, c.DNSNames...)
		for _, ip := range c.IPAddresses {
			sans = append(sans, ip.String())
		}
		sans = append(sans, c.EmailAddresses...)
		fmt.Fprintf(&b, "  Names (SANs): %s\n", strings.Join(sans, ", "))
	}
	state := "valid now"
	switch {
	case now.Before(c.NotBefore):
		state = "NOT YET valid"
	case now.After(c.NotAfter):
		state = fmt.Sprintf("EXPIRED %s ago", durationWords(now.Sub(c.NotAfter)))
	default:
		state = fmt.Sprintf("valid now, expires in %s", durationWords(c.NotAfter.Sub(now)))
	}
	fmt.Fprintf(&b, "  Valid: %s to %s (%s)\n", c.NotBefore.UTC().Format("2006-01-02"), c.NotAfter.UTC().Format("2006-01-02"), state)
	fmt.Fprintf(&b, "  Key: %s; signature: %s%s\n", keyDesc(c.PublicKey), c.SignatureAlgorithm, map[bool]string{true: " (SHA-1 or MD5 signatures are no longer trusted by browsers)", false: ""}[c.SignatureAlgorithm == x509.SHA1WithRSA || c.SignatureAlgorithm == x509.MD5WithRSA || c.SignatureAlgorithm == x509.ECDSAWithSHA1])
	fmt.Fprintf(&b, "  Serial: %s; CA: %v; SHA-256 fingerprint: %x\n", c.SerialNumber, c.IsCA, sha256.Sum256(c.Raw))
	var usages []string
	for _, u := range c.ExtKeyUsage {
		usages = append(usages, map[x509.ExtKeyUsage]string{x509.ExtKeyUsageServerAuth: "server", x509.ExtKeyUsageClientAuth: "client", x509.ExtKeyUsageCodeSigning: "code signing", x509.ExtKeyUsageEmailProtection: "email"}[u])
	}
	if len(usages) > 0 {
		fmt.Fprintf(&b, "  For: %s\n", strings.Join(usages, ", "))
	}
	b.WriteString("  Trust: not checked against any root store here; this is what the certificate says about itself.\n")
	return b.String()
}

func durationWords(d time.Duration) string {
	days := int(d.Hours() / 24)
	switch {
	case days >= 365:
		return fmt.Sprintf("%d years", days/365)
	case days >= 1:
		return fmt.Sprintf("%d days", days)
	}
	return fmt.Sprintf("%d hours", int(d.Hours()))
}

// sshInfo reads an OpenSSH public key line: type, size, comment and the SHA256 fingerprint ssh-keygen -l prints.
func sshInfo(t string) (string, error) {
	fields := strings.Fields(t)
	blob, err := base64.StdEncoding.DecodeString(fields[1])
	if err != nil {
		return "", fmt.Errorf("the key's base64 body is broken: %v", err)
	}
	read := func(b []byte) ([]byte, []byte, bool) {
		if len(b) < 4 {
			return nil, nil, false
		}
		n := binary.BigEndian.Uint32(b)
		if uint32(len(b)-4) < n {
			return nil, nil, false
		}
		return b[4 : 4+n], b[4+n:], true
	}
	typ, rest, ok := read(blob)
	if !ok || string(typ) != fields[0] {
		return "", fmt.Errorf("the key's body does not match its type prefix")
	}
	size := ""
	switch fields[0] {
	case "ssh-rsa":
		_, rest2, ok := read(rest) // e
		if n, _, ok2 := read(rest2); ok && ok2 {
			size = fmt.Sprintf("%d bits", new(big.Int).SetBytes(n).BitLen())
		}
	case "ssh-ed25519":
		size = "256 bits"
	case "ssh-dss":
		size = "DSA (obsolete: OpenSSH no longer accepts it by default)"
	default:
		size = strings.TrimPrefix(fields[0], "ecdsa-sha2-")
	}
	comment := ""
	if len(fields) > 2 {
		comment = strings.Join(fields[2:], " ")
	}
	fp := sha256.Sum256(blob)
	return fmt.Sprintf("SSH public key: %s, %s. Fingerprint SHA256:%s. Comment: %q.\nA public key can be shared; it identifies the holder of the matching private key, which this is not.",
		fields[0], size, strings.TrimRight(base64.StdEncoding.EncodeToString(fp[:]), "="), comment), nil
}

func jwtInfo(t, key string, now time.Time) (string, error) {
	parts := strings.Split(t, ".")
	dec := func(s string) ([]byte, error) { return base64.RawURLEncoding.DecodeString(strings.TrimRight(s, "=")) }
	hdr, err := dec(parts[0])
	if err != nil {
		return "", fmt.Errorf("the header is not base64url: %v", err)
	}
	body, err := dec(parts[1])
	if err != nil {
		return "", fmt.Errorf("the payload is not base64url: %v", err)
	}
	var h, p map[string]any
	if json.Unmarshal(hdr, &h) != nil {
		return "", fmt.Errorf("the header is not JSON")
	}
	var b strings.Builder
	b.WriteString("JWT (JSON Web Token)\n")
	fmt.Fprintf(&b, "  Header: %s\n", compactJSON(hdr))
	if json.Unmarshal(body, &p) == nil {
		fmt.Fprintf(&b, "  Payload: %s\n", compactJSON(body))
		for _, claim := range []string{"iat", "nbf", "exp"} {
			if v, ok := p[claim].(float64); ok {
				ts := time.Unix(int64(v), 0).UTC()
				note := ""
				if claim == "exp" {
					note = map[bool]string{true: " (EXPIRED)", false: " (not expired)"}[now.After(ts)]
				}
				fmt.Fprintf(&b, "  %s: %s%s\n", claim, ts.Format(time.RFC3339), note)
			}
		}
	} else {
		fmt.Fprintf(&b, "  Payload: not JSON (%d bytes)\n", len(body))
	}
	alg, _ := h["alg"].(string)
	switch {
	case strings.EqualFold(alg, "none"):
		b.WriteString("  Signature: alg is \"none\": the token carries NO signature. Anyone can forge it; a server that accepts alg none is broken.\n")
	case len(parts) < 3 || parts[2] == "":
		fmt.Fprintf(&b, "  Signature: missing (alg %s): the token is unsigned or truncated.\n", alg)
	case key == "":
		fmt.Fprintf(&b, "  Signature: present (alg %s) but NOT VERIFIED: that needs the %s. Decoding a JWT proves nothing about who issued it; treat the claims as unverified.\n", alg,
			map[bool]string{true: "shared secret", false: "issuer's public key"}[strings.HasPrefix(alg, "HS")])
	default:
		ok, how := verifyJWT(parts, alg, key)
		fmt.Fprintf(&b, "  Signature: %s (%s).\n", map[bool]string{true: "VERIFIED with the key given", false: "does NOT verify with the key given"}[ok], how)
	}
	return b.String(), nil
}

func compactJSON(b []byte) string {
	var m map[string]any
	if json.Unmarshal(b, &m) != nil {
		return string(b)
	}
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	var parts []string
	for _, k := range keys {
		v, _ := json.Marshal(m[k])
		parts = append(parts, fmt.Sprintf("%s: %s", k, v))
	}
	return strings.Join(parts, ", ")
}

func verifyJWT(parts []string, alg, key string) (bool, string) {
	signed := []byte(parts[0] + "." + parts[1])
	sig, err := base64.RawURLEncoding.DecodeString(strings.TrimRight(parts[2], "="))
	if err != nil {
		return false, "the signature is not base64url"
	}
	var hf func() hash.Hash
	var ch crypto.Hash
	switch strings.ToUpper(alg[2:]) {
	case "256":
		hf, ch = sha256.New, crypto.SHA256
	case "384":
		hf, ch = sha512.New384, crypto.SHA384
	case "512":
		hf, ch = sha512.New, crypto.SHA512
	default:
		return false, "algorithm " + alg + " is not one I verify"
	}
	switch strings.ToUpper(alg[:2]) {
	case "HS":
		m := hmac.New(hf, []byte(key))
		m.Write(signed)
		return hmac.Equal(m.Sum(nil), sig), "HMAC with the shared secret"
	case "RS", "ES", "PS":
		block, _ := pem.Decode([]byte(key))
		if block == nil {
			return false, "the key is not a PEM public key"
		}
		pub, err := x509.ParsePKIXPublicKey(block.Bytes)
		if err != nil {
			if c, err2 := x509.ParseCertificate(block.Bytes); err2 == nil {
				pub = c.PublicKey
			} else {
				return false, "the PEM is not a public key or certificate"
			}
		}
		h := hf()
		h.Write(signed)
		digest := h.Sum(nil)
		switch k := pub.(type) {
		case *rsa.PublicKey:
			if strings.ToUpper(alg[:2]) == "PS" {
				return rsa.VerifyPSS(k, ch, digest, sig, nil) == nil, "RSA-PSS with the public key"
			}
			return rsa.VerifyPKCS1v15(k, ch, digest, sig) == nil, "RSA with the public key"
		case *ecdsa.PublicKey:
			n := (k.Curve.Params().BitSize + 7) / 8
			if len(sig) != 2*n {
				return false, "ECDSA signature has the wrong length"
			}
			return ecdsa.Verify(k, digest, new(big.Int).SetBytes(sig[:n]), new(big.Int).SetBytes(sig[n:])), "ECDSA with the public key"
		}
		return false, "the key type does not fit " + alg
	}
	return false, "algorithm " + alg + " is not one I verify"
}

// pgpInfo describes an armored PGP block with gpg --list-packets (nothing is decrypted or imported), or by its header alone without gpg.
func pgpInfo(ctx context.Context, t string) string {
	kind := pemRE.FindStringSubmatch(t)[1]
	if _, err := exec.LookPath("gpg"); err != nil {
		return "PGP armored block: " + kind + ". gpg is not installed, so I cannot list its packets (key ids, algorithms); the header is all I can say."
	}
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "gpg", "--batch", "--no-tty", "--list-packets", "--list-only")
	cmd.Stdin = strings.NewReader(t)
	var out, errb bytes.Buffer
	cmd.Stdout, cmd.Stderr = &out, &errb
	_ = cmd.Run()
	text := strings.TrimSpace(out.String())
	if text == "" {
		text = strings.TrimSpace(errb.String())
	}
	if len(text) > 3000 {
		text = text[:3000] + "\n[…]"
	}
	return "PGP armored block: " + kind + ". Packets as gpg lists them (nothing was decrypted or imported):\n" + text
}
