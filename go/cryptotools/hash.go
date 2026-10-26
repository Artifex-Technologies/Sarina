package cryptotools

import (
	"crypto/hmac"
	"crypto/md5"
	"crypto/sha1"
	"crypto/sha256"
	"crypto/sha3"
	"crypto/sha512"
	"crypto/subtle"
	"encoding/hex"
	"fmt"
	"hash"
	"hash/crc32"
	"sort"
	"strings"
)

// Algorithms crypto_hash knows (the standard library's; BLAKE2 is not in it).
var Algorithms = []string{"md5", "sha1", "sha224", "sha256", "sha384", "sha512", "sha3-256", "sha3-512", "crc32"}

func newHash(algo string) (func() hash.Hash, error) {
	switch strings.ToLower(strings.ReplaceAll(algo, "_", "-")) {
	case "md5":
		return md5.New, nil
	case "sha1", "sha-1":
		return sha1.New, nil
	case "sha224", "sha-224":
		return sha256.New224, nil
	case "sha256", "sha-256", "":
		return sha256.New, nil
	case "sha384", "sha-384":
		return sha512.New384, nil
	case "sha512", "sha-512":
		return sha512.New, nil
	case "sha3-256", "sha3":
		return func() hash.Hash { return sha3.New256() }, nil
	case "sha3-512":
		return func() hash.Hash { return sha3.New512() }, nil
	case "crc32":
		return func() hash.Hash { return crc32.NewIEEE() }, nil
	}
	return nil, fmt.Errorf("unknown algorithm %q (one of %s)", algo, strings.Join(Algorithms, ", "))
}

// Digest hashes data with algo, or with HMAC-algo when key is not empty. The result is hex.
func Digest(algo string, data, key []byte) (string, error) {
	f, err := newHash(algo)
	if err != nil {
		return "", err
	}
	if len(key) > 0 {
		if strings.EqualFold(algo, "crc32") {
			return "", fmt.Errorf("CRC32 is a checksum, not a hash: it has no HMAC")
		}
		m := hmac.New(f, key)
		m.Write(data)
		return hex.EncodeToString(m.Sum(nil)), nil
	}
	h := f()
	h.Write(data)
	return hex.EncodeToString(h.Sum(nil)), nil
}

// Compare checks got against expected in constant time (hex, case and separators ignored) and, when they differ, says where.
func Compare(got, expected string) (same bool, detail string) {
	a, b := strings.ToLower(stripSep(got)), strings.ToLower(stripSep(expected))
	if len(a) != len(b) {
		return false, fmt.Sprintf("different lengths (%d and %d hex digits): not the same kind of digest", len(a), len(b))
	}
	if subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1 {
		return true, "identical"
	}
	for i := 0; i < len(a); i++ {
		if a[i] != b[i] {
			return false, fmt.Sprintf("they differ from hex digit %d on (byte %d)", i+1, i/2)
		}
	}
	return false, "they differ"
}

// WhichMatches hashes data with every algorithm and says which, if any, gives expected.
func WhichMatches(data []byte, expected string) []string {
	var hits []string
	for _, algo := range Algorithms {
		if d, err := Digest(algo, data, nil); err == nil {
			if same, _ := Compare(d, expected); same {
				hits = append(hits, algo)
			}
		}
	}
	sort.Strings(hits)
	return hits
}
