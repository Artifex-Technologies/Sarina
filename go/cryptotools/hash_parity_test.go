package cryptotools

import (
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityCryptoHash records (testdata/parity/cryptohash.json) crypto_hash's digests, HMACs and comparisons: every
// algorithm and its spellings over data of several sizes (block boundaries, bytes that are not UTF-8) with and without
// keys (a key longer than the block too), and HashReport against expected digests that match, differ or are another
// algorithm's.
func TestParityCryptoHash(t *testing.T) {
	var cases []parity.Case
	datas := []string{"", "abc", "The quick brown fox jumps over the lazy dog", strings.Repeat("a", 55), strings.Repeat("a", 56),
		strings.Repeat("a", 64), strings.Repeat("b", 135), strings.Repeat("b", 136), strings.Repeat("c", 137), strings.Repeat("d", 1000),
		"\xff\xfe\x00\x01 bytes", strings.Repeat("xyz", 400)}
	keys := []string{"", "k", "key", strings.Repeat("K", 72), strings.Repeat("K", 129), strings.Repeat("Q", 200)}
	algos := append(append([]string{}, Algorithms...), "SHA-256", "sha_256", "", "sha3", "CRC32", "Sha-1", "whirlpool", "sha3_512")
	cases = append(cases, parity.Case{Fn: "inputs", In: parity.Texts(datas), Out: parity.Texts(keys)})
	var dig []any
	for _, a := range algos {
		for i, d := range datas {
			for j, k := range keys {
				got, err := Digest(a, []byte(d), []byte(k))
				dig = append(dig, []any{a, i, j, got, parity.Err(err)})
			}
		}
	}
	cases = append(cases, parity.Case{Fn: "digest", Out: dig})
	md5abc, _ := Digest("md5", []byte("abc"), nil)
	sha256abc, _ := Digest("sha256", []byte("abc"), nil)
	var rep []any
	for _, c := range []struct{ data, src, algo, key, expected string }{
		{"abc", "text", "", "", ""}, {"abc", "text", "sha256", "", sha256abc}, {"abc", "file a.txt", "SHA256", "", strings.ToUpper(sha256abc)},
		{"abc", "text", "sha256", "", md5abc}, {"abc", "text", "sha256", "", sha256abc[:10] + "0" + sha256abc[11:]},
		{"abc", "text", "sha256", "", "aa:bb:cc"}, {"abc", "text", "md5", "secret", md5abc}, {"abc", "text", "crc32", "k", ""},
		{"abc", "text", "nope", "", ""}, {"", "text (empty)", "", "", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
		{"abc", "text", "sha1", "", "a9993e36-4706816a-ba3e2571-7850c26c-9cd0d89d"}, {"abc", "text", "sha_256", "k", "x"},
		{"abc", "text", "md5", "", "İ" + md5abc[2:]}, {"abc", "text", "sha3-512", "", ""}, {"abc", "text", "crc32", "", "352441c2"},
	} {
		rep = append(rep, []any{c.data, c.src, c.algo, c.key, c.expected, HashReport([]byte(c.data), c.src, c.algo, c.key, c.expected)})
	}
	cases = append(cases, parity.Case{Fn: "report", Out: rep})
	parity.Fixture(t, "cryptohash", cases)
}
