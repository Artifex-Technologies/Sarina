package cryptotools

import (
	"encoding/ascii85"
	"encoding/base32"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"math/big"
	"net/url"
	"regexp"
	"strconv"
	"strings"
)

// Encodings crypto_decode knows, in the order auto tries them.
var Encodings = []string{"hex", "base64", "base32", "base58", "ascii85", "binary", "decimal", "url", "rot13", "morse"}

var (
	hexRE     = regexp.MustCompile(`^(?:[0-9a-fA-F]{2}[\s:,-]*)+$`)
	base64RE  = regexp.MustCompile(`^[A-Za-z0-9+/\s]+={0,2}\s*$`)
	base64URL = regexp.MustCompile(`^[A-Za-z0-9_-]+={0,2}$`)
	base32RE  = regexp.MustCompile(`^[A-Z2-7\s]+={0,6}\s*$`)
	base58RE  = regexp.MustCompile(`^[1-9A-HJ-NP-Za-km-z]+$`)
	binaryRE  = regexp.MustCompile(`^[01\s]+$`)
	decimalRE = regexp.MustCompile(`^(?:\d{1,3}[\s,]+)+\d{1,3}\s*$`)
	morseRE   = regexp.MustCompile(`^[.\-/ ]+$`)
	ascii85RE = regexp.MustCompile(`^<~[!-u\s]*~>$`)
)

const base58Alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

var morseTable = map[string]rune{
	".-": 'A', "-...": 'B', "-.-.": 'C', "-..": 'D', ".": 'E', "..-.": 'F', "--.": 'G', "....": 'H', "..": 'I', ".---": 'J', "-.-": 'K',
	".-..": 'L', "--": 'M', "-.": 'N', "---": 'O', ".--.": 'P', "--.-": 'Q', ".-.": 'R', "...": 'S', "-": 'T', "..-": 'U', "...-": 'V',
	".--": 'W', "-..-": 'X', "-.--": 'Y', "--..": 'Z', "-----": '0', ".----": '1', "..---": '2', "...--": '3', "....-": '4', ".....": '5',
	"-....": '6', "--...": '7', "---..": '8', "----.": '9', ".-.-.-": '.', "--..--": ',', "..--..": '?', "-....-": '-', ".--.-.": '@',
}

// Looks says whether s has the shape of the encoding (a cheap check before decoding; "auto" uses it to pick what to try).
func Looks(enc, s string) bool {
	s = strings.TrimSpace(s)
	if s == "" {
		return false
	}
	switch enc {
	case "hex":
		return hexRE.MatchString(s) && len(stripSep(s)) >= 2
	case "base64":
		c := strings.Join(strings.Fields(s), "")
		return len(c) >= 4 && (base64RE.MatchString(s) || base64URL.MatchString(c)) && !hexRE.MatchString(s)
	case "base32":
		return len(s) >= 8 && base32RE.MatchString(s) && strings.ContainsAny(s, "234567=") && !hexRE.MatchString(s)
	case "base58":
		return len(s) >= 6 && base58RE.MatchString(s) && !hexRE.MatchString(s)
	case "ascii85":
		return ascii85RE.MatchString(s)
	case "binary":
		return binaryRE.MatchString(s) && len(stripSep(s)) >= 8
	case "decimal":
		return decimalRE.MatchString(s)
	case "url":
		return strings.Contains(s, "%") && regexp.MustCompile(`%[0-9A-Fa-f]{2}`).MatchString(s)
	case "rot13":
		letters := 0
		for _, r := range s {
			if (r >= 'a' && r <= 'z') || (r >= 'A' && r <= 'Z') {
				letters++
			}
		}
		return letters >= 4
	case "morse":
		return morseRE.MatchString(s) && strings.ContainsAny(s, ".-")
	}
	return false
}

func stripSep(s string) string {
	return strings.Map(func(r rune) rune {
		if r == ' ' || r == '\n' || r == '\t' || r == '\r' || r == ':' || r == ',' || r == '-' {
			return -1
		}
		return r
	}, s)
}

// DecodeOne decodes s with one named encoding.
func DecodeOne(enc, s string) ([]byte, error) {
	s = strings.TrimSpace(s)
	switch enc {
	case "hex":
		return hex.DecodeString(stripSep(s))
	case "base64":
		c := strings.Join(strings.Fields(s), "")
		if b, err := base64.StdEncoding.DecodeString(c); err == nil {
			return b, nil
		}
		if b, err := base64.RawStdEncoding.DecodeString(strings.TrimRight(c, "=")); err == nil {
			return b, nil
		}
		if b, err := base64.URLEncoding.DecodeString(c); err == nil {
			return b, nil
		}
		return base64.RawURLEncoding.DecodeString(strings.TrimRight(c, "="))
	case "base32":
		c := strings.Join(strings.Fields(s), "")
		if b, err := base32.StdEncoding.DecodeString(c); err == nil {
			return b, nil
		}
		return base32.StdEncoding.WithPadding(base32.NoPadding).DecodeString(strings.TrimRight(c, "="))
	case "base58":
		return decodeBase58(s)
	case "ascii85":
		body := strings.TrimSuffix(strings.TrimPrefix(s, "<~"), "~>")
		out := make([]byte, 4*len(body))
		n, _, err := ascii85.Decode(out, []byte(body), true)
		if err != nil {
			return nil, err
		}
		return out[:n], nil
	case "binary":
		bits := stripSep(s)
		if len(bits)%8 != 0 {
			return nil, fmt.Errorf("%d bits is not a whole number of bytes", len(bits))
		}
		out := make([]byte, len(bits)/8)
		for i := range out {
			v, err := strconv.ParseUint(bits[i*8:i*8+8], 2, 8)
			if err != nil {
				return nil, err
			}
			out[i] = byte(v)
		}
		return out, nil
	case "decimal":
		var out []byte
		for _, f := range strings.FieldsFunc(s, func(r rune) bool { return r == ' ' || r == ',' || r == '\n' || r == '\t' }) {
			v, err := strconv.Atoi(f)
			if err != nil || v < 0 || v > 255 {
				return nil, fmt.Errorf("%q is not a byte value", f)
			}
			out = append(out, byte(v))
		}
		return out, nil
	case "url":
		t, err := url.QueryUnescape(s)
		return []byte(t), err
	case "rot13":
		return []byte(Caesar(s, 13)), nil
	case "morse":
		var b strings.Builder
		for _, word := range strings.Split(s, "/") {
			for _, letter := range strings.Fields(word) {
				r, ok := morseTable[letter]
				if !ok {
					return nil, fmt.Errorf("%q is not Morse", letter)
				}
				b.WriteRune(r)
			}
			b.WriteByte(' ')
		}
		return []byte(strings.TrimSpace(b.String())), nil
	}
	return nil, fmt.Errorf("unknown encoding %q (one of %s)", enc, strings.Join(Encodings, ", "))
}

func decodeBase58(s string) ([]byte, error) {
	n := new(big.Int)
	radix := big.NewInt(58)
	for _, r := range s {
		i := strings.IndexRune(base58Alphabet, r)
		if i < 0 {
			return nil, fmt.Errorf("%q is not a base58 character", r)
		}
		n.Mul(n, radix)
		n.Add(n, big.NewInt(int64(i)))
	}
	out := n.Bytes()
	for _, r := range s { // leading 1s are leading zero bytes
		if r != '1' {
			break
		}
		out = append([]byte{0}, out...)
	}
	return out, nil
}

// Step is one decoding in a chain.
type Step struct {
	Encoding string
	Out      []byte
}

// Chain is what DecodeAuto found: the steps taken and the final bytes.
type Chain struct {
	Steps []Step
	Text  bool // the final bytes read as plain text
}

// DecodeAuto tries encodings whose shape matches, up to depth layers deep (base64 of hex of ..., say), and stops at the first chain
// whose result reads as text. With none, it returns the longest chain that decoded at all (a binary result), or nil.
func DecodeAuto(s string, depth int) *Chain {
	if depth <= 0 {
		depth = 3
	}
	type node struct {
		text  string
		steps []Step
	}
	queue := []node{{text: s}}
	var longest *Chain
	for len(queue) > 0 {
		cur := queue[0]
		queue = queue[1:]
		if len(cur.steps) >= depth {
			continue
		}
		for _, enc := range Encodings {
			if enc == "rot13" && len(cur.steps) > 0 {
				continue // rot13 of a decoded layer is never the point
			}
			if !Looks(enc, cur.text) {
				continue
			}
			out, err := DecodeOne(enc, cur.text)
			if err != nil || len(out) == 0 {
				continue
			}
			if enc == "rot13" && WordRatio(string(out)) <= WordRatio(cur.text) {
				continue
			}
			steps := append(append([]Step(nil), cur.steps...), Step{enc, out})
			text := string(out)
			if LooksLikeText(text) && (enc != "rot13" || WordRatio(text) >= 0.3) {
				return &Chain{Steps: steps, Text: true}
			}
			if longest == nil || len(steps) > len(longest.Steps) {
				longest = &Chain{Steps: steps}
			}
			if Printable(text) > 0.9 {
				queue = append(queue, node{text: text, steps: steps})
			}
		}
	}
	return longest
}

// Describe renders a chain for the model: each step and the final result, as text or as a hex dump.
func (c *Chain) Describe() string {
	if c == nil {
		return "Nothing decoded: none of the encodings I know fit, or the result was not readable."
	}
	var b strings.Builder
	names := make([]string, len(c.Steps))
	for i, st := range c.Steps {
		names[i] = st.Encoding
	}
	fmt.Fprintf(&b, "Decoded %s (%d layer(s): %s).\n", map[bool]string{true: "to text", false: "to bytes that are not plain text"}[c.Text], len(c.Steps), strings.Join(names, " -> "))
	final := c.Steps[len(c.Steps)-1].Out
	if c.Text {
		fmt.Fprintf(&b, "Result (%d characters):\n%s\n", len([]rune(string(final))), TruncateText(string(final), 4000))
	} else {
		fmt.Fprintf(&b, "Result: %d bytes, entropy %.2f bits/byte. Hex dump of the start:\n%s\n", len(final), Entropy(final), hex.Dump(final[:min(len(final), 256)]))
	}
	return b.String()
}

// TruncateText keeps the first n characters of s.
func TruncateText(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n]) + fmt.Sprintf("\n[... %d more characters]", len(r)-n)
}
