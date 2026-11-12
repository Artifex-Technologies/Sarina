package parity

import (
	"bufio"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"io"
	"math/big"
	"net"
	"net/textproto"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

// Step is one thing a scripted server does on a connection, in order: wait (milliseconds), send bytes, send Fill bytes
// of 'x' or Pad spaces, or hold the connection open until the test ends. After the last step the connection is closed.
type Step struct {
	Wait int
	Send string
	Fill int
	Pad  int
	Hold bool
}

// Steps is a script as the fixture holds it (so the Rust test replays the same answers with the same pauses).
func Steps(script [][]Step) []any {
	out := []any{}
	for _, conn := range script {
		steps := []any{}
		for _, s := range conn {
			m := map[string]any{}
			switch {
			case s.Hold:
				m["hold"] = true
			case s.Wait > 0:
				m["wait"] = s.Wait
			case s.Fill > 0:
				m["fill"] = s.Fill
			case s.Pad > 0:
				m["pad"] = s.Pad
			default:
				m["send"] = Text(s.Send)
			}
			steps = append(steps, m)
		}
		out = append(out, steps)
	}
	return out
}

// RawSeq listens on loopback (TLS with cfg when it is not nil) and answers the n-th connection with script[n] (a
// connection past the script is read and closed). Each connection is served on its own, so a held one does not stop the
// next. It returns the address and a function giving the requests seen so far, in the order they came: method, request
// URI, Content-Type, Authorization, x-api-key, anthropic-version and body; {"none": true} for a connection whose
// request could not be read (a TLS handshake the client refused).
func RawSeq(t *testing.T, cfg *tls.Config, script [][]Step) (string, func() []map[string]any) {
	t.Helper()
	var ln net.Listener
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	if cfg != nil {
		ln = tls.NewListener(ln, cfg)
	}
	var mu sync.Mutex
	var seen []map[string]any
	done := make(chan struct{})
	t.Cleanup(func() { close2(done); ln.Close() })
	go func() {
		for n := 0; ; n++ {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			var steps []Step
			if n < len(script) {
				steps = script[n]
			}
			go func() {
				defer c.Close()
				c.SetReadDeadline(time.Now().Add(10 * time.Second))
				r := bufio.NewReader(c)
				tp := textproto.NewReader(r)
				line, lerr := tp.ReadLine()
				var req map[string]any
				if lerr != nil {
					req = map[string]any{"none": true}
				} else {
					hdr, _ := tp.ReadMIMEHeader()
					n, _ := strconv.Atoi(hdr.Get("Content-Length"))
					body := make([]byte, min(n, 1<<20))
					io.ReadFull(r, body)
					parts := strings.SplitN(line, " ", 3)
					for len(parts) < 3 {
						parts = append(parts, "")
					}
					req = map[string]any{"method": parts[0], "uri": parts[1], "contentType": hdr.Get("Content-Type"),
						"auth": hdr.Get("Authorization"), "apiKey": hdr.Get("X-Api-Key"), "version": hdr.Get("Anthropic-Version"),
						"body": Text(string(body))}
				}
				mu.Lock()
				seen = append(seen, req)
				mu.Unlock()
				if lerr != nil {
					return
				}
				for _, s := range steps {
					switch {
					case s.Hold:
						<-done
					case s.Wait > 0:
						select {
						case <-time.After(time.Duration(s.Wait) * time.Millisecond):
						case <-done:
							return
						}
					case s.Fill > 0:
						c.Write([]byte(strings.Repeat("x", s.Fill)))
					case s.Pad > 0:
						c.Write([]byte(strings.Repeat(" ", s.Pad)))
					default:
						c.Write([]byte(s.Send))
					}
				}
			}()
		}
	}()
	return ln.Addr().String(), func() []map[string]any {
		mu.Lock()
		defer mu.Unlock()
		return append([]map[string]any(nil), seen...)
	}
}

// TestCA is an authority made for one test run and certificates from it for loopback servers.
type TestCA struct {
	Pool *x509.CertPool
	cert *x509.Certificate
	key  *ecdsa.PrivateKey
}

// NewTestCA makes an authority named name (P-256, valid for a day).
func NewTestCA(t *testing.T, name string) *TestCA {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	tmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: name},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour), IsCA: true, BasicConstraintsValid: true,
		KeyUsage: x509.KeyUsageCertSign}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	cert, _ := x509.ParseCertificate(der)
	pool := x509.NewCertPool()
	pool.AddCert(cert)
	return &TestCA{Pool: pool, cert: cert, key: key}
}

// Server is a server configuration with a certificate from the authority for the given IP addresses and DNS names.
func (ca *TestCA) Server(t *testing.T, ips []string, names []string) *tls.Config {
	t.Helper()
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	tmpl := &x509.Certificate{SerialNumber: big.NewInt(2), Subject: pkix.Name{CommonName: "parity server"},
		NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(24 * time.Hour), DNSNames: names,
		KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	for _, ip := range ips {
		tmpl.IPAddresses = append(tmpl.IPAddresses, net.ParseIP(ip))
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, ca.cert, &key.PublicKey, ca.key)
	if err != nil {
		t.Fatal(err)
	}
	return &tls.Config{Certificates: []tls.Certificate{{Certificate: [][]byte{der}, PrivateKey: key}}}
}
