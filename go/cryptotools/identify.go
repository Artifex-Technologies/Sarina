package cryptotools

import (
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"unicode"
)

// Candidate is one guess at what a piece of data is, with the evidence for it.
type Candidate struct {
	What     string
	Score    int // 0-100: how well the evidence fits
	Evidence string
	Preview  string // a decoding, when one makes sense
}

var (
	pemRE   = regexp.MustCompile(`-----BEGIN ([A-Z0-9 ]+)-----`)
	jwtRE   = regexp.MustCompile(`^[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{2,}\.[A-Za-z0-9_-]*$`)
	uuidRE  = regexp.MustCompile(`(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)
	cryptRE = regexp.MustCompile(`^\$(2[aby]?|argon2(id|i|d)|6|5|1|7|y|pbkdf2[-\w]*|sha1|md5|scrypt)\$`)
	ntlmRE  = regexp.MustCompile(`^[0-9a-fA-F]{32}:[0-9a-fA-F]{32}$`)
	sshRE   = regexp.MustCompile(`^(ssh-(rsa|ed25519|dss)|ecdsa-sha2-nistp(256|384|521)) [A-Za-z0-9+/=]+`)
	ipv4RE  = regexp.MustCompile(`^(\d{1,3}\.){3}\d{1,3}$`)
	macRE   = regexp.MustCompile(`(?i)^([0-9a-f]{2}[:-]){5}[0-9a-f]{2}$`)
	btcRE   = regexp.MustCompile(`^(1|3)[1-9A-HJ-NP-Za-km-z]{25,34}$|^bc1[a-z0-9]{25,60}$`)
	ethRE   = regexp.MustCompile(`^0x[0-9a-fA-F]{40}$`)
)

// hashKinds by hex digest length: the algorithms that produce it (several share a length, so a length never names one for sure).
var hashKinds = map[int]string{
	8: "CRC32 (or a short checksum)", 16: "a 64-bit hash (CRC64, xxHash64, SipHash) or half an MD5", 32: "MD5, NTLM, LM or MD4 (all 128 bits)",
	40: "SHA-1, RIPEMD-160 or a Git object id", 56: "SHA-224, SHA3-224", 64: "SHA-256, SHA3-256, BLAKE2s-256 or a Keccak-256 (Ethereum) hash",
	96: "SHA-384 or SHA3-384", 128: "SHA-512, SHA3-512, BLAKE2b-512 or Whirlpool",
}

func cryptKind(prefix string) string {
	switch {
	case strings.HasPrefix(prefix, "$2"):
		return "bcrypt password hash"
	case strings.HasPrefix(prefix, "$argon2"):
		return "Argon2 password hash"
	case prefix == "$6":
		return "sha512crypt password hash (Linux /etc/shadow)"
	case prefix == "$5":
		return "sha256crypt password hash"
	case prefix == "$1":
		return "md5crypt password hash"
	case prefix == "$y", prefix == "$7":
		return "yescrypt or scrypt password hash"
	case strings.HasPrefix(prefix, "$pbkdf2"):
		return "PBKDF2 password hash"
	}
	return "a modular-crypt password hash"
}

// Stats describes the characters of a text.
type Stats struct {
	Length, Letters, Digits, Punct, Spaces, NonPrintable int
	Upper, Lower                                         int
	Entropy                                              float64
	ZeroWidth                                            int // invisible Unicode characters (zero-width, tag characters)
}

// Describe writes the statistics in one line each.
func (st Stats) Describe() string {
	return fmt.Sprintf("Length %d characters: %d letters (%d upper, %d lower), %d digits, %d punctuation, %d spaces, %d non-printable; entropy %.2f bits per byte%s",
		st.Length, st.Letters, st.Upper, st.Lower, st.Digits, st.Punct, st.Spaces, st.NonPrintable, st.Entropy,
		map[bool]string{true: fmt.Sprintf("; %d INVISIBLE characters (zero-width or tag characters: hidden text, see stego_inspect)", st.ZeroWidth), false: ""}[st.ZeroWidth > 0])
}

// CharStats counts what s is made of.
func CharStats(s string) Stats {
	var st Stats
	for _, r := range s {
		st.Length++
		switch {
		case unicode.IsLetter(r):
			st.Letters++
			if unicode.IsUpper(r) {
				st.Upper++
			} else {
				st.Lower++
			}
		case unicode.IsDigit(r):
			st.Digits++
		case r == ' ' || r == '\n' || r == '\t' || r == '\r':
			st.Spaces++
		case isZeroWidth(r):
			st.ZeroWidth++
			st.NonPrintable++
		case unicode.IsPrint(r):
			st.Punct++
		default:
			st.NonPrintable++
		}
	}
	st.Entropy = Entropy([]byte(s))
	return st
}

func isZeroWidth(r rune) bool {
	switch r {
	case 0x200B, 0x200C, 0x200D, 0x2060, 0x2061, 0x2062, 0x2063, 0x2064, 0xFEFF, 0x00AD, 0x180E, 0x200E, 0x200F, 0x202A, 0x202B, 0x202C, 0x202D, 0x202E:
		return true
	}
	return r >= 0xE0000 && r <= 0xE007F
}

// Identify ranks what s could be: an encoding, a hash, a key or token format, a classical cipher, with the evidence for each.
// best is the highest score found so far (0 when nothing was).
func best(cs []Candidate) int {
	n := 0
	for _, c := range cs {
		if c.Score > n {
			n = c.Score
		}
	}
	return n
}

func Identify(s string) ([]Candidate, Stats) {
	st := CharStats(s)
	t := strings.TrimSpace(s)
	var cs []Candidate
	add := func(what string, score int, evidence, preview string) {
		cs = append(cs, Candidate{what, score, evidence, preview})
	}
	if t == "" {
		return cs, st
	}
	if m := pemRE.FindStringSubmatch(t); m != nil {
		label := m[1]
		switch {
		case strings.HasPrefix(label, "PGP"):
			add("PGP armored block: "+label, 100, "the -----BEGIN "+label+"----- header", "crypto_modern reads its packets (what kind, which key id) without decrypting anything")
		case strings.Contains(label, "PRIVATE KEY"):
			add("a PRIVATE key ("+label+")", 100, "the PEM header", "a secret: I do not decode, print or load private keys; keep it out of the conversation")
		default:
			add("PEM block: "+label, 100, "the -----BEGIN "+label+"----- header", "crypto_modern parses certificates and public keys")
		}
		return cs, st
	}
	if sshRE.MatchString(t) {
		add("an SSH public key", 100, "the key type prefix", "crypto_modern gives its type, size and fingerprint")
		return cs, st
	}
	if jwtRE.MatchString(t) {
		parts := strings.Split(t, ".")
		if hdr, err := base64.RawURLEncoding.DecodeString(strings.TrimRight(parts[0], "=")); err == nil && json.Valid(hdr) && strings.Contains(string(hdr), "alg") {
			add("a JWT (JSON Web Token)", 98, "three base64url parts separated by dots; the first decodes to a JSON header: "+string(hdr), "crypto_modern decodes it; the signature is NOT verified without the key")
			return cs, st
		}
	}
	if uuidRE.MatchString(t) {
		add("a UUID (version "+string(t[14])+")", 95, "8-4-4-4-12 hex groups with a version digit", "")
		return cs, st
	}
	if ntlmRE.MatchString(t) {
		add("an LM:NTLM hash pair (Windows)", 90, "two 32-hex-digit halves separated by a colon", "a hash: not reversible; it can only be compared against candidates")
		return cs, st
	}
	if m := cryptRE.FindStringSubmatch(t); m != nil {
		add(cryptKind("$"+m[1]), 95, "the $"+m[1]+"$ prefix of the modular crypt format", "a salted, slow password hash: not reversible; only a guess can be checked against it")
		return cs, st
	}
	if ethRE.MatchString(t) {
		add("an Ethereum address (or a 160-bit hash with 0x)", 70, "0x and 40 hex digits", "")
	}
	if btcRE.MatchString(t) {
		add("a Bitcoin address", 60, "base58 with a 1 or 3 prefix (or bech32 bc1)", "")
	}
	if ipv4RE.MatchString(t) {
		add("an IPv4 address", 90, "four dotted numbers", "")
		return cs, st
	}
	if macRE.MatchString(t) {
		add("a MAC address", 85, "six hex pairs", "")
		return cs, st
	}
	compact := stripSep(t)
	if hexRE.MatchString(t) {
		if kind, ok := hashKinds[len(compact)]; ok && len(compact) >= 32 {
			add("a hex digest: "+kind, 85, fmt.Sprintf("%d hex digits = %d bits", len(compact), len(compact)*4), "a hash is one-way: it says nothing about its input; it can be compared against the hash of a candidate (crypto_hash with expected)")
		}
		if b, err := hex.DecodeString(compact); err == nil {
			text := string(b)
			if LooksLikeText(text) {
				add("hex-encoded text", 92, "every character is a hex digit and the bytes read as text", TruncateText(text, 200))
			} else if len(compact) < 32 || !strings.Contains(hashKinds[len(compact)], "SHA") {
				add("hex-encoded bytes", 60, fmt.Sprintf("hex digits; the %d bytes do not read as text (entropy %.2f)", len(b), Entropy(b)), "")
			}
			if len(b) >= 8 && !LooksLikeText(text) {
				if g, ok := crackXOR(b); ok {
					add("bytes XORed with a short key", int(g.Score), g.Note, TruncateText(g.Text, 200))
				}
			}
		}
	}
	// base64 of real data has no short words in it: a sentence of short words is not base64 however the characters fit
	shortWords := false
	for _, f := range strings.Fields(t) {
		if len(f) < 12 {
			shortWords = true
		}
	}
	// prose can happen to use only base64 characters ("Cryptography"): too short to carry anything, or already readable English, is not base64
	longEnough := len(strings.ReplaceAll(t, " ", "")) >= 16
	alreadyEnglish := WordRatio(t) >= 0.3 || (len(strings.Fields(t)) == 1 && LooksLikeText(t) && WordRatio(t) > 0)
	if Looks("base64", t) && !hexRE.MatchString(t) && !(shortWords && strings.Contains(t, " ")) && longEnough && !alreadyEnglish {
		if b, err := DecodeOne("base64", t); err == nil {
			text := string(b)
			switch {
			case LooksLikeText(text):
				add("base64-encoded text", 90, "base64 characters, and the bytes read as text", TruncateText(text, 200))
			case Looks("hex", text):
				// peeled to the end, so the text is computed here and not decoded by eye (eval crypto-hex-in-base64, 2026-10-26: given only
				// the hex, the model "read" it as "the old milk and net")
				preview := "hex: " + TruncateText(text, 120)
				if inner, err := DecodeOne("hex", text); err == nil && LooksLikeText(string(inner)) {
					preview = "text (base64 -> hex -> text): " + TruncateText(string(inner), 200)
				}
				add("base64 of hex", 80, "base64 whose bytes are hex digits", preview)
			default:
				add("base64-encoded bytes", 55, fmt.Sprintf("base64 characters; the %d bytes do not read as text (entropy %.2f): could be ciphertext, a key, compressed or binary data", len(b), Entropy(b)), "")
				if len(b) >= 8 {
					if g, ok := crackXOR(b); ok {
						add("base64 of bytes XORed with a short key", int(g.Score), g.Note, TruncateText(g.Text, 200))
					}
				}
			}
		}
	}
	if Looks("base32", t) {
		if b, err := DecodeOne("base32", t); err == nil && LooksLikeText(string(b)) {
			add("base32-encoded text", 88, "A-Z and 2-7 only, and the bytes read as text", TruncateText(string(b), 200))
		}
	}
	if Looks("base58", t) && len(cs) == 0 {
		if b, err := decodeBase58(t); err == nil && LooksLikeText(string(b)) {
			add("base58-encoded text", 85, "base58 alphabet (no 0, O, I, l), and the bytes read as text", TruncateText(string(b), 200))
		}
	}
	if Looks("ascii85", t) {
		if b, err := DecodeOne("ascii85", t); err == nil && LooksLikeText(string(b)) {
			add("Ascii85-encoded text", 90, "the <~ ~> frame, and the bytes read as text", TruncateText(string(b), 200))
		}
	}
	if Looks("binary", t) {
		if b, err := DecodeOne("binary", t); err == nil && LooksLikeText(string(b)) {
			add("binary (0/1) encoded text", 90, "only 0 and 1, eight bits per character", TruncateText(string(b), 200))
		}
	}
	if Looks("decimal", t) {
		if b, err := DecodeOne("decimal", t); err == nil && LooksLikeText(string(b)) {
			add("decimal byte values (ASCII codes)", 85, "numbers 0-255 separated by spaces or commas", TruncateText(string(b), 200))
		}
	}
	if Looks("morse", t) {
		if b, err := DecodeOne("morse", t); err == nil {
			add("Morse code", 90, "dots and dashes", TruncateText(string(b), 200))
		}
	}
	if Looks("url", t) {
		if b, err := DecodeOne("url", t); err == nil && string(b) != t {
			add("URL (percent) encoded text", 80, "%XX escapes", TruncateText(string(b), 200))
		}
	}
	// letters: plain English, a shifted alphabet, or a polyalphabetic cipher (not for a token an encoding already explained)
	decoded := false
	for _, c := range cs {
		if c.Score >= 80 {
			decoded = true
		}
	}
	if st.Letters >= 8 && st.Letters*2 > st.Length && !decoded {
		if WordRatio(t) >= 0.3 {
			add("plain English text", 90, fmt.Sprintf("%.0f%% of its words are common English words", 100*WordRatio(t)), "")
		} else {
			if gs := CrackCaesar(t); len(gs) > 0 && WordRatio(gs[0].Text) >= 0.25 && gs[0].Key != "shift 0" {
				add("a shifted alphabet (Caesar/ROT), "+gs[0].Key, 85, gs[0].Note, TruncateText(gs[0].Text, 200))
			} else if at := Atbash(t); WordRatio(at) >= 0.3 {
				add("Atbash (mirrored alphabet)", 85, fmt.Sprintf("%.0f%% common words after mirroring", 100*WordRatio(at)), TruncateText(at, 200))
			} else {
				ic := IndexOfCoincidence(t)
				if gs := CrackVigenere(t, 16); len(gs) > 0 && WordRatio(gs[0].Text) >= 0.25 {
					add("a Vigenère cipher, key "+strings.ToUpper(gs[0].Key), 80, fmt.Sprintf("letters with index of coincidence %.3f (English 0.066, random 0.038); %s", ic, gs[0].Note), TruncateText(gs[0].Text, 200))
				} else if ic < 0.05 && st.Letters >= 40 {
					add("a polyalphabetic cipher (Vigenère-like) or random letters", 50, fmt.Sprintf("flat letter frequencies: index of coincidence %.3f (English 0.066); the Vigenère cracker found no readable key", ic), "")
				} else if ic >= 0.055 && st.Letters >= 60 {
					add("a monoalphabetic substitution", 55, fmt.Sprintf("English-like letter spread (index of coincidence %.3f) but no English words: each letter replaced by another; crypto_classic with cipher substitution and crack tries it", ic), "")
				} else if best(cs) < 50 {
					// only when nothing better was found: next to a real reading this line is noise
					add("letters, not English: a cipher, another language or a name", 30, fmt.Sprintf("%.0f%% common English words, index of coincidence %.3f", 100*WordRatio(t), ic), "")
				}
			}
		}
	}
	if len(cs) == 0 {
		add("nothing I recognise", 10, fmt.Sprintf("entropy %.2f bits per byte: %s", st.Entropy, map[bool]string{true: "high, like encrypted, compressed or random data", false: "low: structured data or text in a form I do not know"}[st.Entropy > 7.2]), "")
	}
	sort.SliceStable(cs, func(i, j int) bool { return cs[i].Score > cs[j].Score })
	return cs, st
}

// crackXOR tries every single-byte key and, for longer data, repeating keys of 2 to 8 bytes (key length by the Hamming distance
// between blocks, each key byte by the English fit of its column). ok is true when the result reads as text.
func crackXOR(b []byte) (Guess, bool) {
	best := Guess{Score: -1}
	try := func(key []byte, how string) {
		out := make([]byte, len(b))
		for i := range b {
			out[i] = b[i] ^ key[i%len(key)]
		}
		t := string(out)
		if !LooksLikeText(t) || WordRatio(t) < 0.15 {
			return
		}
		score := int(70 + 30*WordRatio(t))
		if score > int(best.Score) {
			best = Guess{Key: hex.EncodeToString(key), Text: t, Score: float64(score), Note: fmt.Sprintf("%s: key %s (hex), %.0f%% common words", how, hex.EncodeToString(key), 100*WordRatio(t))}
		}
	}
	for k := 1; k < 256; k++ {
		try([]byte{byte(k)}, "single-byte XOR")
	}
	if len(b) < 24 {
		return best, best.Score > 0
	}
	for keyLen := 2; keyLen <= 8 && keyLen*3 <= len(b); keyLen++ {
		key := make([]byte, keyLen)
		for c := 0; c < keyLen; c++ {
			var col []byte
			for i := c; i < len(b); i += keyLen {
				col = append(col, b[i])
			}
			bestK, bestFit := 0, -1.0
			for k := 0; k < 256; k++ {
				out := make([]byte, len(col))
				for i := range col {
					out[i] = col[i] ^ byte(k)
				}
				fit := xorFit(out)
				if fit > bestFit {
					bestK, bestFit = k, fit
				}
			}
			key[c] = byte(bestK)
		}
		try(key, fmt.Sprintf("repeating-key XOR (key length %d)", keyLen))
	}
	return best, best.Score > 0
}

// xorFit scores a column of a repeating-key XOR guess: English letter frequency with spaces, printable bytes rewarded.
func xorFit(b []byte) float64 {
	score := 0.0
	for _, c := range b {
		switch {
		case c == ' ':
			score += 13
		case c >= 'a' && c <= 'z':
			score += englishFreq[c-'a'] * 100
		case c >= 'A' && c <= 'Z':
			score += englishFreq[c-'A'] * 50
		case c >= 0x20 && c < 0x7f, c == '\n':
			score += 1
		default:
			score -= 20
		}
	}
	return score
}
