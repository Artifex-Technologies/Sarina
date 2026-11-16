package cryptotools

import (
	"fmt"
	"math"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityClassic records (testdata/parity/classic.json) the classical ciphers as the crypto_classic tool answers:
// every cipher encoded and decoded with good and bad keys, every cracker on English prose and on texts that are not, and
// the English statistics they rank by (bit for bit).
func TestParityClassic(t *testing.T) {
	bits := func(x float64) string { return fmt.Sprintf("%016x", math.Float64bits(x)) }
	var cases []parity.Case
	plain := strings.ToUpper(lowerLetters(prose)[:90])
	short := "THESPAREKEYISUNDERTHEBLUEFLOWERPOTBYTHEBACKDOOROFTHECOTTAGE"
	texts := []string{prose, plain, short, "Hello, World!", "", "   ", "a", "ÄÖÜ äöü ß Straße", "ǅ ǈ K Kelvin İ", "x\ny\tz",
		"The quick brown fox jumps over the lazy dog.", "zzzz qqqq xxxx", "12345 67890", "{\"json\": true}", "<html>",
		"ZZ", "the the the", "don't won't it's", "ab\r\ncd", "abc\uFFFDdef", "ab  ", "abc   ", " spaced out", strings.Repeat("ab", 400), strings.Repeat("é", 700)}
	var eng []any
	for _, s := range texts {
		eng = append(eng, []any{s, bits(ChiSquared(s)), bits(WordRatio(s)), bits(Printable(s)), LooksLikeText(s), bits(IndexOfCoincidence(s)),
			bits(Entropy([]byte(s))), lowerLetters(s), letterOrder(s), bits(ngramRate(s)), bits(wordCover(lowerLetters(s))), readsAsEnglish(s),
			bits(substitutionFitness(s)), TruncateText(s, 10)})
	}
	cases = append(cases, parity.Case{Fn: "english", Out: eng})

	// the ranking pieces on their own: key lengths by index of coincidence (two within 0.004 count as equal), the
	// readable-first order of keyed guesses, the meaning of an index of coincidence
	var rank []any
	for _, key := range []string{"ab", "abc", "lemon", "secretkey", "zebra", "abcdefgh", "qwertyuiopas", "kk", "xyzzy", "abab"} {
		v, _ := Vigenere(prose, key, false)
		ls := lowerLetters(v)
		var ics []string
		for n := 1; n <= 16; n++ {
			ics = append(ics, bits(columnIC(ls, n)))
		}
		rank = append(rank, []any{"keylengths", key, vigenereKeyLengths(ls, 16), ics})
	}
	for _, ic := range []float64{0.049, 0.05, 0.0579, 0.058, 0.059, 0.0599, 0.06, 0.066} {
		rank = append(rank, []any{"ic", bits(ic), icMeaning(ic)})
	}
	texts2 := []string{"the cat and dog", "the zzz qqq xxx yyy", "the and zzz qqq xxx", "one two three four five", "xx yy zz", "the be to of xx",
		"the be to of xx yy", "attack at dawn", "qqq www eee"}
	for i := range texts2 {
		var gs []Guess
		for j, t := range texts2 {
			gs = append(gs, Guess{Key: fmt.Sprint(j), Text: t, Score: float64((i*7+j*3)%11) - 5})
		}
		rankKeyedGuesses(gs)
		var keys []string
		for _, g := range gs {
			keys = append(keys, g.Key)
		}
		rank = append(rank, []any{"keyed", i, keys})
	}
	cases = append(cases, parity.Case{Fn: "rank", Out: rank})

	type call struct {
		text, cipher, key string
		decode, crack     bool
	}
	var calls []call
	enc := func(c, k string) string {
		out, err := Classic(prose, c, k, false, false)
		if err != nil {
			t.Fatal(err)
		}
		return out[strings.Index(out, ":\n")+2:]
	}
	for _, c := range []struct{ cipher, key string }{{"caesar", "7"}, {"caesar", "-3"}, {"caesar", "29"}, {"rot13", ""}, {"atbash", ""},
		{"vigenere", "lemon"}, {"vigenere", "Secret Key"}, {"affine", "5,8"}, {"affine", "-7 30"}, {"beaufort", "lemon"},
		{"autokey", "queenly"}, {"columnar", "zebras"}, {"columnar", "3 1 4 2"}, {"columnar", "3142"}, {"railfence", "4"},
		{"substitution", "qwertyuiopasdfghjklzxcvbnm"}} {
		calls = append(calls, call{prose, c.cipher, c.key, false, false})
		e := enc(c.cipher, c.key)
		calls = append(calls, call{e, c.cipher, c.key, true, false})
		calls = append(calls, call{e, c.cipher, "", false, true})
	}
	col2, _ := Columnar(plain, "cipher", false)
	colShort, _ := Columnar(short, "cipher", false)
	vigShort, _ := Vigenere("attack at dawn", "key", false)
	calls = append(calls, call{col2, "columnar", "", false, true}, call{colShort, "columnar", "", false, true},
		call{vigShort, "vigenere", "", false, true}, call{"xqzv jkwp", "caesar", "", false, true}, call{"hello", "substitution", "", false, true},
		call{"Hello, World!", "caesar", "abc", false, false}, call{"Hello", "caesar", " 0x3 ", false, false}, call{"Hello", "railfence", "1", false, false},
		call{"Hello", "railfence", "101", false, false}, call{"Hello", "railfence", "x", false, false}, call{"Hello", "vigenere", "123", false, false},
		call{"Hello", "affine", "13,2", false, false}, call{"Hello", "affine", "5", false, false}, call{"Hello", "affine", "1,2,3", false, false},
		call{"Hello", "beaufort", "", false, false}, call{"Hello", "autokey", "", false, false}, call{"Hello", "columnar", "a", false, false},
		call{"Hello", "columnar", "1 1", false, false}, call{"Hello", "columnar", "1 3", false, false}, call{"Hello", "columnar", "0 1", false, false},
		call{"Hello", "columnar", "21", false, false}, call{"Hello", "columnar", "-1,2", false, false}, call{"Hello", "columnar", "KEY", true, false},
		call{"Hello", "substitution", "abc", false, false}, call{"Hello", "substitution", "aacdefghijklmnopqrstuvwxyz", false, false},
		call{"Hello", "substitution", "ÄBCDEFGHIJKLMNOPQRSTUVWXYZ", false, false}, call{"Hello", "substitution", "Q W E R T Y U I O P A S D F G H J K L Z X C V B N M", true, false},
		call{"Hello", "enigma", "x", false, false}, call{"  ", "caesar", "1", false, false}, call{"Ünïcödé Straße", "rot13", "", false, false},
		call{"Ünïcödé Straße KK", "vigenere", "kelvin", false, false}, call{"ab", "columnar", "", false, true}, call{"abcd", "railfence", "", false, true},
		call{"a b c", "autokey", "", false, true}, call{"abcdefghijklm", "beaufort", "", false, true}, call{strings.Repeat("é", 700), "caesar", "1", false, false},
		call{strings.Repeat("Attack at dawn! ", 60), "rot13", "", false, false}, call{"Hello", "columnar", "zebras", true, false},
		call{"we meet at the old bridge at noon", "columnar", "", false, true}, call{"Hello", "affine", "99999999999999999999,8", false, false},
		call{"Hello", "affine", "5,-99999999999999999999", true, false}, call{"Hello", "columnar", "99999999999999999999 1", false, false})
	var out []any
	for _, c := range calls {
		got, err := Classic(c.text, c.cipher, c.key, c.decode, c.crack)
		out = append(out, []any{c.text, c.cipher, c.key, c.decode, c.crack, got, parity.Err(err)})
	}
	cases = append(cases, parity.Case{Fn: "classic", Out: out})
	parity.Fixture(t, "classic", cases)
}
