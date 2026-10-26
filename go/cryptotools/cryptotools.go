// Package cryptotools gives Sarina cryptography she can do in code instead of in her head: what a string is (an encoding, a hash, a
// token, a classical cipher), decoding chains, the classical ciphers and their crackers, hashes and HMACs, and what a certificate,
// key or JWT says about itself. Everything is computed; nothing is guessed, nothing goes over the network, no file is changed.
//
// The tools are offered when SARINA_CRYPTO=on (their descriptions cost every request a little context). The "crypto" prefetch (for
// a skill with prefetch: [crypto]) identifies anything in the task that looks encoded before the model starts, so the small local
// model answers from the evidence instead of guessing.
package cryptotools

import (
	"context"
	"fmt"
	"os"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/skills"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// MaxInput is the most of a file a crypto tool reads.
const MaxInput = 64 << 20

// evidenceNote closes every tool answer: what the model may and may not conclude from it.
const evidenceNote = "\n[Computed, not guessed. Say what this shows, and only this. A hash cannot be reversed; encrypted data without its key cannot be read; a signature is verified only where this says VERIFIED.]"

func init() {
	tools.RegisterProvider(Tools)
	skills.RegisterContext("crypto", Prefetch)
	cli.Register("crypto", "identify, decode, hash or crack a string in code: crypto identify|decode|classic|hash|modern ... (sarina crypto for usage)", command)
}

// input returns the bytes a tool works on: the text argument, or the file at path (inside the workspace; never a credentials file or a
// private folder).
func input(args map[string]any, tc *tools.Context) ([]byte, string, error) {
	text, _ := args["text"].(string)
	path, _ := args["path"].(string)
	switch {
	case path == "" && text == "":
		return nil, "", fmt.Errorf("give text or a path")
	case path == "":
		return []byte(text), "text", nil
	}
	root := tc.Root
	if root == "" {
		root, _ = os.Getwd()
	}
	resolved, err := tools.ResolveInRoot(root, path)
	if err != nil {
		return nil, "", err
	}
	if tools.IsSecretPath(path) || tools.IsSecretPath(resolved) {
		return nil, "", fmt.Errorf("refusing to read %s: it looks like a credentials or key file; those never enter the conversation", path)
	}
	if tools.IsPrivate(resolved) {
		return nil, "", fmt.Errorf("refusing to read %s: the owner keeps that folder private from me", path)
	}
	st, err := os.Stat(resolved)
	if err != nil {
		return nil, "", fmt.Errorf("cannot read %s: %v", path, err)
	}
	if !st.Mode().IsRegular() {
		return nil, "", fmt.Errorf("%s is not a regular file", path)
	}
	if st.Size() > MaxInput {
		return nil, "", fmt.Errorf("%s is %d bytes; I read at most %d", path, st.Size(), MaxInput)
	}
	b, err := os.ReadFile(resolved)
	if err != nil {
		return nil, "", fmt.Errorf("cannot read %s: %v", path, err)
	}
	return b, "file " + path, nil
}

func str(a map[string]any, k string) string { s, _ := a[k].(string); return s }

// Tools returns the crypto tools when SARINA_CRYPTO=on.
func Tools() []tools.Tool {
	if os.Getenv("SARINA_CRYPTO") != "on" {
		return nil
	}
	textOrPath := map[string]any{
		"text": map[string]any{"type": "string", "description": "The string itself."},
		"path": map[string]any{"type": "string", "description": "Or a file in the workspace to read instead of text."},
	}
	return []tools.Tool{
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "crypto_identify",
				Description: "What is this string or file? Ranked guesses with evidence: an encoding (hex, base64, base32, base58, binary, Morse), a hash and which algorithms give that length, a password hash, JWT, UUID, PEM, SSH key, a shifted alphabet, Vigenère or substitution cipher, bytes XORed with a short key; plus character statistics and entropy. Use it before saying what anything is.",
				Parameters:  map[string]any{"type": "object", "properties": textOrPath},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				b, src, err := input(a, tc)
				if err != nil {
					return "", nil, err
				}
				return IdentifyReport(string(b), src) + evidenceNote, map[string]any{"action": "crypto_identify"}, nil
			},
		},
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "crypto_decode",
				Description: "Decode an encoded string: from auto tries hex, base64, base32, base58, ascii85, binary, decimal, url, rot13 and Morse, layered up to 3 deep, and shows the chain; or name one encoding.",
				Parameters: map[string]any{"type": "object", "required": []string{"text"}, "properties": map[string]any{
					"text": map[string]any{"type": "string"},
					"from": map[string]any{"type": "string", "description": "auto (default) or one of: " + strings.Join(Encodings, ", ")},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				from := str(a, "from")
				if from == "" || from == "auto" {
					return DecodeAuto(str(a, "text"), 3).Describe() + evidenceNote, map[string]any{"action": "crypto_decode"}, nil
				}
				out, err := DecodeOne(from, str(a, "text"))
				if err != nil {
					return "", nil, fmt.Errorf("%s: %v", from, err)
				}
				c := &Chain{Steps: []Step{{from, out}}, Text: LooksLikeText(string(out)) || Printable(string(out)) > 0.95}
				return c.Describe() + evidenceNote, map[string]any{"action": "crypto_decode"}, nil
			},
		},
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "crypto_classic",
				Description: "Classical ciphers, computed: encode or decode with caesar (key = shift), rot13, atbash, vigenere (key = word), railfence (key = rails), substitution (key = 26-letter alphabet); or crack=true to find the key (caesar, vigenere, railfence, substitution) scored against English.",
				Parameters: map[string]any{"type": "object", "required": []string{"text", "cipher"}, "properties": map[string]any{
					"text":   map[string]any{"type": "string"},
					"cipher": map[string]any{"type": "string", "enum": []string{"caesar", "rot13", "atbash", "vigenere", "railfence", "substitution"}},
					"key":    map[string]any{"type": "string", "description": "The key (a number for caesar and railfence)."},
					"decode": map[string]any{"type": "boolean", "description": "true to decode with the key (default: encode)."},
					"crack":  map[string]any{"type": "boolean", "description": "true to find the key without knowing it."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				crack, _ := a["crack"].(bool)
				decode, _ := a["decode"].(bool)
				out, err := Classic(str(a, "text"), str(a, "cipher"), str(a, "key"), decode, crack)
				if err != nil {
					return "", nil, err
				}
				return out + evidenceNote, map[string]any{"action": "crypto_classic", "cipher": str(a, "cipher")}, nil
			},
		},
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "crypto_hash",
				Description: "Hash text or a file (md5, sha1, sha256 default, sha512, sha3-256, crc32...), or an HMAC with key; with expected, compare in constant time and say which algorithm (if any) matches it and where two digests differ.",
				Parameters: map[string]any{"type": "object", "properties": map[string]any{
					"text":     textOrPath["text"],
					"path":     textOrPath["path"],
					"algo":     map[string]any{"type": "string", "description": "One of " + strings.Join(Algorithms, ", ") + " (default sha256)."},
					"key":      map[string]any{"type": "string", "description": "HMAC key, for a keyed hash."},
					"expected": map[string]any{"type": "string", "description": "A digest (hex) to compare against."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				b, src, err := input(a, tc)
				if err != nil {
					return "", nil, err
				}
				return HashReport(b, src, str(a, "algo"), str(a, "key"), str(a, "expected")) + evidenceNote, map[string]any{"action": "crypto_hash"}, nil
			},
		},
		{
			Class: tools.ClassReadOnly,
			Spec: llm.ToolSpec{
				Name:        "crypto_modern",
				Description: "What a certificate (PEM or DER), public key, SSH key, JWT or PGP block says about itself: subject, issuer, validity and expiry, key type and size, names, fingerprint; a JWT's header, claims and expiry, with its signature verified ONLY if key (an HMAC secret or a PEM public key) is given. Never decrypts; refuses private keys.",
				Parameters: map[string]any{"type": "object", "properties": map[string]any{
					"text": textOrPath["text"],
					"path": textOrPath["path"],
					"key":  map[string]any{"type": "string", "description": "For a JWT: the HMAC secret, or a PEM public key or certificate, to verify its signature."},
				}},
			},
			Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
				b, _, err := inputAllowingCerts(a, tc)
				if err != nil {
					return "", nil, err
				}
				out, err := Modern(ctx, string(b), str(a, "key"), time.Now())
				if err != nil {
					return "", nil, err
				}
				return out + evidenceNote, map[string]any{"action": "crypto_modern"}, nil
			},
		},
	}
}

// inputAllowingCerts is input, but a .pem/.crt file may be read when it holds only certificates or public keys (never a private key:
// Modern refuses those by content, and the file's bytes are never shown).
func inputAllowingCerts(args map[string]any, tc *tools.Context) ([]byte, string, error) {
	path, _ := args["path"].(string)
	if path == "" || !tools.IsSecretPath(path) {
		return input(args, tc)
	}
	root := tc.Root
	if root == "" {
		root, _ = os.Getwd()
	}
	resolved, err := tools.ResolveInRoot(root, path)
	if err != nil {
		return nil, "", err
	}
	if tools.IsPrivate(resolved) {
		return nil, "", fmt.Errorf("refusing to read %s: the owner keeps that folder private from me", path)
	}
	st, err := os.Stat(resolved)
	if err != nil || !st.Mode().IsRegular() || st.Size() > 1<<20 {
		return nil, "", fmt.Errorf("cannot read %s as a certificate file", path)
	}
	b, err := os.ReadFile(resolved)
	if err != nil {
		return nil, "", err
	}
	if !strings.Contains(string(b), "-----BEGIN") || strings.Contains(string(b), "PRIVATE") {
		return nil, "", fmt.Errorf("refusing to read %s: it looks like a key or credentials file, not a certificate", path)
	}
	return b, "file " + path, nil
}

// IdentifyReport renders Identify for the model.
func IdentifyReport(s, src string) string {
	cs, st := Identify(s)
	var b strings.Builder
	fmt.Fprintf(&b, "What the %s looks like, most likely first:\n", src)
	for i, c := range cs {
		fmt.Fprintf(&b, "%d. %s (fit %d/100). Evidence: %s.", i+1, c.What, c.Score, c.Evidence)
		if c.Preview != "" {
			fmt.Fprintf(&b, "\n   -> %s", strings.ReplaceAll(c.Preview, "\n", "\n   "))
		}
		b.WriteString("\n")
	}
	b.WriteString(st.Describe() + ".\n")
	return b.String()
}

// Classic runs crypto_classic: one cipher, with a key or cracked.
func Classic(text, cipher, key string, decode, crack bool) (string, error) {
	if strings.TrimSpace(text) == "" {
		return "", fmt.Errorf("no text")
	}
	var b strings.Builder
	guesses := func(gs []Guess, what string) string {
		if len(gs) == 0 {
			return "Could not crack it: the text is too short, or it is not a " + what + "."
		}
		var sb strings.Builder
		fmt.Fprintf(&sb, "Best %s guesses, best first:\n", what)
		for i, g := range gs {
			fmt.Fprintf(&sb, "%d. key %s (%s):\n   %s\n", i+1, g.Key, g.Note, strings.ReplaceAll(TruncateText(g.Text, 600), "\n", "\n   "))
		}
		if WordRatio(gs[0].Text) < 0.2 {
			sb.WriteString("None of these reads as English (few common words): the cipher is probably something else, or the language is not English.\n")
		}
		return sb.String()
	}
	switch cipher {
	case "caesar":
		if crack {
			return guesses(CrackCaesar(text), "Caesar"), nil
		}
		var n int
		if _, err := fmt.Sscan(key, &n); err != nil {
			return "", fmt.Errorf("caesar needs a numeric key (the shift)")
		}
		if decode {
			n = -n
		}
		fmt.Fprintf(&b, "Caesar shift %d:\n%s", n, Caesar(text, n))
	case "rot13":
		fmt.Fprintf(&b, "ROT13:\n%s", Caesar(text, 13))
	case "atbash":
		fmt.Fprintf(&b, "Atbash:\n%s", Atbash(text))
	case "vigenere":
		if crack {
			return guesses(CrackVigenere(text, 16), "Vigenère"), nil
		}
		out, err := Vigenere(text, key, decode)
		if err != nil {
			return "", err
		}
		fmt.Fprintf(&b, "Vigenère with key %s (%s):\n%s", strings.ToUpper(key), map[bool]string{true: "decoded", false: "encoded"}[decode], out)
	case "railfence":
		if crack {
			return guesses(CrackRailFence(text, 10), "rail fence"), nil
		}
		var n int
		if _, err := fmt.Sscan(key, &n); err != nil {
			return "", fmt.Errorf("railfence needs a numeric key (the number of rails)")
		}
		out, err := RailFence(text, n, decode)
		if err != nil {
			return "", err
		}
		fmt.Fprintf(&b, "Rail fence with %d rails (%s):\n%s", n, map[bool]string{true: "decoded", false: "encoded"}[decode], out)
	case "substitution":
		if crack {
			g := CrackSubstitution(text, 6)
			fmt.Fprintf(&b, "Substitution guess (%s). Cipher alphabet for plain a..z: %s\n%s", g.Note, strings.ToUpper(g.Key), TruncateText(g.Text, 1500))
			if WordRatio(g.Text) < 0.2 {
				b.WriteString("\nThis does not read as English: the hill-climb did not find the key (too little text, or not a simple substitution). Letter frequencies, most common first: " + letterOrder(text))
			}
			return b.String(), nil
		}
		out, err := Substitution(text, key, decode)
		if err != nil {
			return "", err
		}
		fmt.Fprintf(&b, "Substitution (%s):\n%s", map[bool]string{true: "decoded", false: "encoded"}[decode], out)
	default:
		return "", fmt.Errorf("unknown cipher %q", cipher)
	}
	return b.String(), nil
}

func letterOrder(s string) string {
	var counts [26]int
	for _, r := range strings.ToLower(s) {
		if r >= 'a' && r <= 'z' {
			counts[r-'a']++
		}
	}
	order := make([]int, 26)
	for i := range order {
		order[i] = i
	}
	for i := 1; i < 26; i++ { // insertion sort, stable
		for j := i; j > 0 && counts[order[j]] > counts[order[j-1]]; j-- {
			order[j], order[j-1] = order[j-1], order[j]
		}
	}
	var parts []string
	for _, i := range order[:10] {
		if counts[i] > 0 {
			parts = append(parts, fmt.Sprintf("%c %d", 'a'+i, counts[i]))
		}
	}
	return strings.Join(parts, ", ") + " (English: e t a o i n s h r d)"
}

// HashReport runs crypto_hash.
func HashReport(data []byte, src, algo, key, expected string) string {
	if algo == "" {
		algo = "sha256"
	}
	var b strings.Builder
	d, err := Digest(algo, data, []byte(key))
	if err != nil {
		return err.Error()
	}
	what := strings.ToUpper(algo)
	if key != "" {
		what = "HMAC-" + what
	}
	fmt.Fprintf(&b, "%s of the %s (%d bytes): %s\n", what, src, len(data), d)
	if expected != "" {
		same, detail := Compare(d, expected)
		fmt.Fprintf(&b, "Compared with the expected digest: %s (%s).\n", map[bool]string{true: "MATCH", false: "NO MATCH"}[same], detail)
		if !same && key == "" {
			if hits := WhichMatches(data, expected); len(hits) > 0 {
				fmt.Fprintf(&b, "The expected digest IS the %s of this data: it matches with that algorithm.\n", strings.Join(hits, "/"))
			} else {
				b.WriteString("No algorithm I know gives the expected digest for this data: the data differs from what was hashed (or a key was involved).\n")
			}
		}
	}
	return b.String()
}

var (
	tokenRE = regexp.MustCompile(`[A-Za-z0-9+/=_.$:-]{16,}`)
	wordsRE = regexp.MustCompile(`^[A-Za-z]+$`)
)

// Prefetch (skill prefetch "crypto") identifies anything in the task that looks encoded, hashed or like a token, so the model starts
// from the evidence. At most three; nothing for plain words.
func Prefetch(task string) string {
	var parts []string
	seen := map[string]bool{}
	if m := pemRE.FindStringIndex(task); m != nil {
		block := task[m[0]:]
		if end := strings.Index(block, "-----END"); end > 0 {
			if e2 := strings.Index(block[end:], "-----"); e2 > 0 {
				if e3 := strings.Index(block[end+e2+5:], "-----"); e3 >= 0 {
					block = block[:end+e2+5+e3+5]
				}
			}
		}
		if out, err := Modern(context.Background(), block, "", time.Now()); err == nil {
			parts = append(parts, out)
		} else {
			parts = append(parts, IdentifyReport(block, "block in the question"))
		}
	}
	for _, tok := range tokenRE.FindAllString(task, -1) {
		tok = strings.Trim(tok, ".,:;")
		if len(parts) >= 3 || seen[tok] || len(tok) < 16 {
			continue
		}
		if wordsRE.MatchString(tok) && WordRatio(tok) > 0 {
			continue
		}
		if strings.Contains(tok, "/") && strings.Count(tok, "/") > 1 && !Looks("base64", tok) {
			continue // a path, not data
		}
		seen[tok] = true
		cs, _ := Identify(tok)
		if len(cs) == 0 || cs[0].Score < 50 {
			continue
		}
		parts = append(parts, "Looked at "+TruncateText(tok, 80)+":\n"+IdentifyReport(tok, "string"))
	}
	if len(parts) == 0 {
		return ""
	}
	return "Already computed for you (crypto_identify on what the question contains; call the crypto tools for anything else):\n\n" + strings.Join(parts, "\n")
}

func command(env *cli.Env, args []string) int {
	if len(args) < 2 {
		fmt.Fprintln(env.Stderr, "usage: sarina crypto identify TEXT | decode [ENC] TEXT | classic CIPHER [KEY|crack] TEXT | hash [ALGO] TEXT | modern TEXT")
		return 2
	}
	var out string
	var err error
	switch args[0] {
	case "identify":
		out = IdentifyReport(strings.Join(args[1:], " "), "string")
	case "decode":
		if len(args) > 2 {
			var b []byte
			b, err = DecodeOne(args[1], strings.Join(args[2:], " "))
			out = string(b)
		} else {
			out = DecodeAuto(args[1], 3).Describe()
		}
	case "classic":
		if len(args) < 3 {
			err = fmt.Errorf("classic CIPHER [KEY|crack] TEXT")
		} else if args[2] == "crack" {
			out, err = Classic(strings.Join(args[3:], " "), args[1], "", false, true)
		} else if len(args) > 3 {
			out, err = Classic(strings.Join(args[3:], " "), args[1], args[2], true, false)
		} else {
			out, err = Classic(args[2], args[1], "", false, false)
		}
	case "hash":
		if len(args) > 2 {
			out = HashReport([]byte(strings.Join(args[2:], " ")), "text", args[1], "", "")
		} else {
			out = HashReport([]byte(args[1]), "text", "sha256", "", "")
		}
	case "modern":
		out, err = Modern(context.Background(), strings.Join(args[1:], " "), "", time.Now())
	default:
		err = fmt.Errorf("unknown subcommand %q", args[0])
	}
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina crypto:", err)
		return 2
	}
	fmt.Fprintln(env.Stdout, out)
	return 0
}
