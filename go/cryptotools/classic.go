package cryptotools

import (
	"fmt"
	"math"
	"math/rand"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"unicode"
)

// Caesar shifts every letter by shift places (case kept, other characters untouched). Decoding is a shift of 26-n.
func Caesar(s string, shift int) string {
	shift = ((shift % 26) + 26) % 26
	var b strings.Builder
	for _, r := range s {
		switch {
		case r >= 'a' && r <= 'z':
			b.WriteRune('a' + (r-'a'+rune(shift))%26)
		case r >= 'A' && r <= 'Z':
			b.WriteRune('A' + (r-'A'+rune(shift))%26)
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

// Atbash mirrors the alphabet (a<->z); it is its own inverse.
func Atbash(s string) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r >= 'a' && r <= 'z':
			b.WriteRune('z' - (r - 'a'))
		case r >= 'A' && r <= 'Z':
			b.WriteRune('Z' - (r - 'A'))
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

// Vigenere shifts each letter by the matching letter of the key (letters only advance the key). decode reverses it.
func Vigenere(s, key string, decode bool) (string, error) {
	var k []int
	for _, r := range strings.ToLower(key) {
		if r >= 'a' && r <= 'z' {
			k = append(k, int(r-'a'))
		}
	}
	if len(k) == 0 {
		return "", fmt.Errorf("the Vigenère key needs at least one letter")
	}
	var b strings.Builder
	i := 0
	for _, r := range s {
		shift := k[i%len(k)]
		if decode {
			shift = 26 - shift
		}
		switch {
		case r >= 'a' && r <= 'z':
			b.WriteRune('a' + (r-'a'+rune(shift))%26)
			i++
		case r >= 'A' && r <= 'Z':
			b.WriteRune('A' + (r-'A'+rune(shift))%26)
			i++
		default:
			b.WriteRune(r)
		}
	}
	return b.String(), nil
}

// railPattern is the row each character of a text of length n lands on with this many rails (the zigzag).
func railPattern(n, rails int) []int {
	rows := make([]int, n)
	row, dir := 0, 1
	for i := 0; i < n; i++ {
		rows[i] = row
		if rails > 1 {
			if row == 0 {
				dir = 1
			} else if row == rails-1 {
				dir = -1
			}
			row += dir
		}
	}
	return rows
}

// RailFence writes the text in a zigzag over this many rails and reads it row by row; decode reverses it.
func RailFence(s string, rails int, decode bool) (string, error) {
	if rails < 2 || rails > 100 {
		return "", fmt.Errorf("rails must be between 2 and 100")
	}
	rs := []rune(s)
	n := len(rs)
	rows := railPattern(n, rails)
	if !decode {
		out := make([]rune, 0, n)
		for r := 0; r < rails; r++ {
			for i := 0; i < n; i++ {
				if rows[i] == r {
					out = append(out, rs[i])
				}
			}
		}
		return string(out), nil
	}
	out := make([]rune, n)
	pos := 0
	for r := 0; r < rails; r++ {
		for i := 0; i < n; i++ {
			if rows[i] == r {
				out[i] = rs[pos]
				pos++
			}
		}
	}
	return string(out), nil
}

// Substitution maps each plain letter to the key's letter at the same place (the key is a 26-letter alphabet, a..z in order);
// decode maps the other way.
func Substitution(s, key string, decode bool) (string, error) {
	key = strings.ToLower(strings.Map(func(r rune) rune {
		if unicode.IsLetter(r) {
			return r
		}
		return -1
	}, key))
	if len(key) != 26 {
		return "", fmt.Errorf("a substitution key is the whole alphabet in cipher order, 26 letters (got %d)", len(key))
	}
	var seen [26]bool
	for _, r := range key {
		if r < 'a' || r > 'z' || seen[r-'a'] {
			return "", fmt.Errorf("a substitution key uses each of a..z exactly once")
		}
		seen[r-'a'] = true
	}
	var table [26]rune
	for i, r := range key {
		if decode {
			table[r-'a'] = rune('a' + i)
		} else {
			table[i] = r
		}
	}
	return applyTable(s, table), nil
}

func applyTable(s string, table [26]rune) string {
	var b strings.Builder
	for _, r := range s {
		switch {
		case r >= 'a' && r <= 'z':
			b.WriteRune(table[r-'a'])
		case r >= 'A' && r <= 'Z':
			b.WriteRune(unicode.ToUpper(table[r-'A']))
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

// Guess is one candidate decoding with its evidence.
type Guess struct {
	Key   string
	Text  string
	Score float64 // chi-squared against English (lower is better) for the key-finding crackers; word ratio or letter-pair fit for the transpositions (higher is better)
	Note  string
}

// CrackCaesar scores all 26 shifts against English letter frequencies and returns the best three (best first).
func CrackCaesar(s string) []Guess {
	var gs []Guess
	for shift := 0; shift < 26; shift++ {
		t := Caesar(s, 26-shift)
		gs = append(gs, Guess{Key: fmt.Sprintf("shift %d", shift), Text: t, Score: ChiSquared(t), Note: fmt.Sprintf("chi-squared %.0f, %.0f%% common words", ChiSquared(t), 100*WordRatio(t))})
	}
	return rankShiftGuesses(gs)
}

// rankShiftGuesses orders the guesses of a cipher with few keys (Caesar, affine) and keeps the best three. Letter frequency alone
// misleads on short text ("Uryyb jbeyq" is Hello world, but another shift can score better), so a key whose text reads as English
// wins; chi-squared decides between the rest and between two readable ones.
func rankShiftGuesses(gs []Guess) []Guess {
	readable := func(g Guess) bool { return WordRatio(g.Text) >= 0.25 }
	sort.SliceStable(gs, func(i, j int) bool {
		if readable(gs[i]) != readable(gs[j]) {
			return readable(gs[i])
		}
		if readable(gs[i]) && WordRatio(gs[i].Text) != WordRatio(gs[j].Text) {
			return WordRatio(gs[i].Text) > WordRatio(gs[j].Text)
		}
		return gs[i].Score < gs[j].Score
	})
	if len(gs) > 3 {
		gs = gs[:3]
	}
	return gs
}

// vigenereKeyLengths ranks key lengths 1..maxLen by the average index of coincidence of their columns (English-like columns ~0.066).
func vigenereKeyLengths(letters string, maxLen int) []int {
	type kl struct {
		n  int
		ic float64
	}
	var ks []kl
	for n := 1; n <= maxLen && n*4 <= len(letters); n++ {
		ks = append(ks, kl{n, columnIC(letters, n)})
	}
	// prefer the shortest length whose columns look English; multiples of the true length score the same
	sort.SliceStable(ks, func(i, j int) bool {
		a, b := ks[i], ks[j]
		if math.Abs(a.ic-b.ic) < 0.004 {
			return a.n < b.n
		}
		return a.ic > b.ic
	})
	var out []int
	for i, k := range ks {
		if i >= 4 {
			break
		}
		out = append(out, k.n)
	}
	return out
}

// columnIC is the average index of coincidence of the n columns of every n-th letter: English-like (~0.066) when n is a multiple of a
// repeating key's length, flat (~0.038) otherwise.
func columnIC(letters string, n int) float64 {
	sum := 0.0
	for c := 0; c < n; c++ {
		var col strings.Builder
		for i := c; i < len(letters); i += n {
			col.WriteByte(letters[i])
		}
		sum += IndexOfCoincidence(col.String())
	}
	return sum / float64(n)
}

// CrackVigenere finds the key length from the columns' index of coincidence (Friedman's method) and each key letter by the best
// Caesar shift of its column, trying the most likely lengths; best guess first.
func CrackVigenere(s string, maxKeyLen int) []Guess {
	if maxKeyLen <= 0 {
		maxKeyLen = 16
	}
	var letters strings.Builder
	for _, r := range strings.ToLower(s) {
		if r >= 'a' && r <= 'z' {
			letters.WriteRune(r)
		}
	}
	ls := letters.String()
	if len(ls) < 12 {
		return nil
	}
	var gs []Guess
	for _, n := range vigenereKeyLengths(ls, maxKeyLen) {
		key := make([]byte, n)
		for c := 0; c < n; c++ {
			var col strings.Builder
			for i := c; i < len(ls); i += n {
				col.WriteByte(ls[i])
			}
			best, bestChi := 0, math.Inf(1)
			for shift := 0; shift < 26; shift++ {
				if chi := ChiSquared(Caesar(col.String(), 26-shift)); chi < bestChi {
					best, bestChi = shift, chi
				}
			}
			key[c] = byte('a' + best)
		}
		t, _ := Vigenere(s, string(key), true)
		gs = append(gs, Guess{Key: string(key), Text: t, Score: ChiSquared(t), Note: fmt.Sprintf("key length %d, chi-squared %.0f, %.0f%% common words", n, ChiSquared(t), 100*WordRatio(t))})
	}
	rankKeyedGuesses(gs)
	return gs
}

// rankKeyedGuesses orders the guesses of a cipher with a key word (Vigenère, Beaufort, autokey), one per key length tried. A key that
// gives readable text beats one that only fits the letter statistics better (a long key overfits them: key length 15 scored
// chi-squared 13 with 19% words, the real 5-letter key 19 with 74%); among the unreadable, chi-squared decides.
func rankKeyedGuesses(gs []Guess) {
	sort.SliceStable(gs, func(i, j int) bool {
		ri, rj := WordRatio(gs[i].Text) >= 0.4, WordRatio(gs[j].Text) >= 0.4
		if ri != rj {
			return ri
		}
		if ri {
			return WordRatio(gs[i].Text) > WordRatio(gs[j].Text)
		}
		return gs[i].Score < gs[j].Score
	})
}

// CrackRailFence tries 2..maxRails rails and ranks by common English words.
func CrackRailFence(s string, maxRails int) []Guess {
	if maxRails <= 0 {
		maxRails = 10
	}
	var gs []Guess
	for rails := 2; rails <= maxRails; rails++ {
		t, err := RailFence(s, rails, true)
		if err != nil {
			continue
		}
		gs = append(gs, Guess{Key: fmt.Sprintf("%d rails", rails), Text: t, Score: WordRatio(t), Note: fmt.Sprintf("%.0f%% common words", 100*WordRatio(t))})
	}
	sort.SliceStable(gs, func(i, j int) bool { return gs[i].Score > gs[j].Score })
	if len(gs) > 3 {
		gs = gs[:3]
	}
	return gs
}

// affineCoprimes are the values of a that make an affine cipher reversible (coprime with 26): 12 of them, so 12 x 26 = 312 keys.
var affineCoprimes = []int{1, 3, 5, 7, 9, 11, 15, 17, 19, 21, 23, 25}

func inverseMod26(a int) int {
	for x := 1; x < 26; x++ {
		if a*x%26 == 1 {
			return x
		}
	}
	return 0
}

// Affine maps each letter x to a*x+b (mod 26), case kept, other characters untouched; decode maps y back to a⁻¹(y-b). a must be
// coprime with 26, or two letters would share a cipher letter and the text could not be read back.
func Affine(s string, a, b int, decode bool) (string, error) {
	a, b = ((a%26)+26)%26, ((b%26)+26)%26
	inv := inverseMod26(a)
	if inv == 0 {
		return "", fmt.Errorf("affine needs a coprime with 26 (one of 1 3 5 7 9 11 15 17 19 21 23 25), got %d", a)
	}
	var table [26]rune
	for x := 0; x < 26; x++ {
		if decode {
			table[x] = rune('a' + inv*(x-b+26)%26)
		} else {
			table[x] = rune('a' + (a*x+b)%26)
		}
	}
	return applyTable(s, table), nil
}

var numberRE = regexp.MustCompile(`-?\d+`)

// parseAffineKey reads "5,8", "5 8" or "a=5 b=8" as a and b.
func parseAffineKey(key string) (int, int, error) {
	ns := numberRE.FindAllString(key, -1)
	if len(ns) != 2 {
		return 0, 0, fmt.Errorf("affine needs two numbers as key, a and b (for example 5,8)")
	}
	a, _ := strconv.Atoi(ns[0])
	b, _ := strconv.Atoi(ns[1])
	return a, b, nil
}

// CrackAffine tries all 312 keys and returns the best three (best first), scored like Caesar.
func CrackAffine(s string) []Guess {
	var gs []Guess
	for _, a := range affineCoprimes {
		for b := 0; b < 26; b++ {
			t, _ := Affine(s, a, b, true)
			chi := ChiSquared(t)
			gs = append(gs, Guess{Key: fmt.Sprintf("a=%d, b=%d", a, b), Text: t, Score: chi, Note: fmt.Sprintf("chi-squared %.0f, %.0f%% common words", chi, 100*WordRatio(t))})
		}
	}
	return rankShiftGuesses(gs)
}

// keyShifts are the key's letters as shifts (a=0), other characters dropped.
func keyShifts(key string) []int {
	var k []int
	for _, r := range strings.ToLower(key) {
		if r >= 'a' && r <= 'z' {
			k = append(k, int(r-'a'))
		}
	}
	return k
}

// Beaufort replaces each letter p by k-p (mod 26), k the matching letter of the repeating key (letters only advance it). It is its
// own inverse: decoding is the same operation.
func Beaufort(s, key string) (string, error) {
	k := keyShifts(key)
	if len(k) == 0 {
		return "", fmt.Errorf("the Beaufort key needs at least one letter")
	}
	var b strings.Builder
	i := 0
	for _, r := range s {
		switch {
		case r >= 'a' && r <= 'z':
			b.WriteRune('a' + rune((k[i%len(k)]-int(r-'a')+26)%26))
			i++
		case r >= 'A' && r <= 'Z':
			b.WriteRune('A' + rune((k[i%len(k)]-int(r-'A')+26)%26))
			i++
		default:
			b.WriteRune(r)
		}
	}
	return b.String(), nil
}

// CrackBeaufort finds the key length like CrackVigenere (each column of a Beaufort text is one alphabet too) and each key letter as
// the one whose column reads most like English (chi-squared); best guess first.
func CrackBeaufort(s string, maxKeyLen int) []Guess {
	if maxKeyLen <= 0 {
		maxKeyLen = 16
	}
	ls := lowerLetters(s)
	if len(ls) < 12 {
		return nil
	}
	var gs []Guess
	for _, n := range vigenereKeyLengths(ls, maxKeyLen) {
		key := make([]byte, n)
		for c := 0; c < n; c++ {
			best, bestChi := 0, math.Inf(1)
			for k := 0; k < 26; k++ {
				var col strings.Builder
				for i := c; i < len(ls); i += n {
					col.WriteByte(byte('a' + (k-int(ls[i]-'a')+26)%26))
				}
				if chi := ChiSquared(col.String()); chi < bestChi {
					best, bestChi = k, chi
				}
			}
			key[c] = byte('a' + best)
		}
		t, _ := Beaufort(s, string(key))
		gs = append(gs, Guess{Key: string(key), Text: t, Score: ChiSquared(t), Note: fmt.Sprintf("key length %d, chi-squared %.0f, %.0f%% common words", n, ChiSquared(t), 100*WordRatio(t))})
	}
	rankKeyedGuesses(gs)
	return gs
}

// Autokey is Vigenère with a key that never repeats: the key word (the primer) is followed by the plaintext itself, so after the
// primer each letter is shifted by the plain letter that many places back. decode reverses it.
func Autokey(s, primer string, decode bool) (string, error) {
	stream := keyShifts(primer)
	if len(stream) == 0 {
		return "", fmt.Errorf("the autokey primer needs at least one letter")
	}
	var b strings.Builder
	i := 0
	for _, r := range s {
		var base rune
		switch {
		case r >= 'a' && r <= 'z':
			base = 'a'
		case r >= 'A' && r <= 'Z':
			base = 'A'
		default:
			b.WriteRune(r)
			continue
		}
		x, p := int(r-base), int(r-base)
		if decode {
			p = (x - stream[i] + 26) % 26
			b.WriteRune(base + rune(p))
		} else {
			b.WriteRune(base + rune((x+stream[i])%26))
		}
		stream = append(stream, p)
		i++
	}
	return b.String(), nil
}

// CrackAutokey tries primer lengths 1..maxPrimer. With a primer of length n, letter i is shifted by plain letter i-n, so every n-th
// letter forms a chain that one primer letter decides: each chain is solved by chi-squared over the 26 letters, then each primer
// letter is checked once more against the whole decoding (a short chain can pick a wrong letter). Best guess first.
func CrackAutokey(s string, maxPrimer int) []Guess {
	gs, _ := crackAutokey(s, maxPrimer)
	return gs
}

// crackAutokey also returns the chi-squared of the best decoding per primer length, for the "How it fell" lines.
func crackAutokey(s string, maxPrimer int) ([]Guess, string) {
	if maxPrimer <= 0 {
		maxPrimer = 12
	}
	ls := lowerLetters(s)
	if len(ls) < 12 {
		return nil, ""
	}
	var gs []Guess
	var per []string
	for n := 1; n <= maxPrimer && n*3 <= len(ls); n++ {
		key := make([]byte, n)
		for j := 0; j < n; j++ {
			best, bestChi := 0, math.Inf(1)
			for k := 0; k < 26; k++ {
				var chain strings.Builder
				prev := k
				for i := j; i < len(ls); i += n {
					p := (int(ls[i]-'a') - prev + 26) % 26
					chain.WriteByte(byte('a' + p))
					prev = p
				}
				if chi := ChiSquared(chain.String()); chi < bestChi {
					best, bestChi = k, chi
				}
			}
			key[j] = byte('a' + best)
		}
		for j := 0; j < n; j++ {
			t, _ := Autokey(s, string(key), true)
			bestLetter, bestFit := key[j], substitutionFitness(t)
			for k := byte('a'); k <= 'z'; k++ {
				key[j] = k
				t, _ := Autokey(s, string(key), true)
				if f := substitutionFitness(t); f > bestFit {
					bestLetter, bestFit = k, f
				}
			}
			key[j] = bestLetter
		}
		t, _ := Autokey(s, string(key), true)
		chi := ChiSquared(t)
		gs = append(gs, Guess{Key: string(key), Text: t, Score: chi, Note: fmt.Sprintf("primer length %d, chi-squared %.0f, %.0f%% common words", n, chi, 100*WordRatio(t))})
		per = append(per, fmt.Sprintf("%d %.0f", n, chi))
	}
	rankKeyedGuesses(gs)
	if len(gs) > 3 {
		gs = gs[:3]
	}
	return gs, strings.Join(per, ", ")
}

// columnOrder reads a columnar key as the order the columns are read in (order[r] is the column read r-th): a word, whose letters in
// alphabetical order give that order (equal letters left to right), or the rank of each column as numbers ("3 1 4 2"; "3142" for at
// most 9 columns).
func columnOrder(key string) ([]int, error) {
	key = strings.TrimSpace(key)
	var ranks []int
	if ns := numberRE.FindAllString(key, -1); len(ns) > 0 && strings.Trim(key, "0123456789 ,-") == "" {
		if len(ns) == 1 && len(ns[0]) > 1 {
			ns = strings.Split(ns[0], "")
		}
		for _, n := range ns {
			r, _ := strconv.Atoi(n)
			ranks = append(ranks, r)
		}
	} else {
		k := keyShifts(key)
		idx := make([]int, len(k))
		for i := range idx {
			idx[i] = i
		}
		sort.SliceStable(idx, func(i, j int) bool { return k[idx[i]] < k[idx[j]] })
		ranks = make([]int, len(k))
		for r, c := range idx {
			ranks[c] = r + 1
		}
	}
	if len(ranks) < 2 || len(ranks) > 40 {
		return nil, fmt.Errorf("a columnar key is a word or the column order as numbers (3 1 4 2), 2 to 40 columns")
	}
	order := make([]int, len(ranks))
	seen := make([]bool, len(ranks))
	for c, r := range ranks {
		if r < 1 || r > len(ranks) || seen[r-1] {
			return nil, fmt.Errorf("a numeric columnar key uses each of 1..%d once", len(ranks))
		}
		seen[r-1] = true
		order[r-1] = c
	}
	return order, nil
}

// columnRanks writes a read order as the rank of each column ("6 3 2 4 1 5"), the form a key word turns into.
func columnRanks(order []int) string {
	ranks := make([]string, len(order))
	for r, c := range order {
		ranks[c] = strconv.Itoa(r + 1)
	}
	return strings.Join(ranks, " ")
}

// Columnar writes the text in rows as wide as the key and reads it column by column in the key's order (an irregular grid: the last
// row may be short, nothing is padded). Every character moves, spaces too, as in RailFence. decode reverses it.
func Columnar(s, key string, decode bool) (string, error) {
	order, err := columnOrder(key)
	if err != nil {
		return "", err
	}
	return columnar([]rune(s), order, decode), nil
}

func columnar(rs []rune, order []int, decode bool) string {
	w, n := len(order), len(rs)
	rows := (n + w - 1) / w
	full := n % w // the first full columns have rows characters, the others one fewer (all of them when it is 0)
	out := make([]rune, n)
	pos := 0
	for _, c := range order {
		h := rows
		if full != 0 && c >= full {
			h--
		}
		for r := 0; r < h; r++ {
			if decode {
				out[r*w+c] = rs[pos]
			} else {
				out[pos] = rs[r*w+c]
			}
			pos++
		}
	}
	return string(out)
}

// CrackColumnar tries every column order of every key length 2..maxWidth (at most 8: 8! = 40,320 orders) and scores each decoding by
// its common English letter pairs and triples, and by its words when the text has spaces; the best orders of each length are then
// scored again by how much of their letters common words cover, which reads text without spaces. Best guess first.
func CrackColumnar(s string, maxWidth int) []Guess {
	gs, _ := crackColumnar(s, maxWidth)
	return gs
}

// crackColumnar also returns the best score per key length, for the "How it fell" lines.
func crackColumnar(s string, maxWidth int) ([]Guess, string) {
	if maxWidth <= 0 || maxWidth > 8 {
		maxWidth = 8
	}
	rs := []rune(s)
	spaced := strings.ContainsAny(s, " \n")
	fit := func(t string) float64 {
		f := ngramRate(t)
		if spaced {
			f += WordRatio(t)
		}
		return f
	}
	type cand struct {
		f     float64
		order []int
	}
	const keep = 30 // the orders of one length scored again by word cover
	var gs []Guess
	var per []string
	for w := 2; w <= maxWidth && w*2 <= len(rs); w++ {
		perm := make([]int, w)
		for i := range perm {
			perm[i] = i
		}
		var top []cand // best first
		permutations(perm, func(p []int) {
			f := fit(columnar(rs, p, true))
			if len(top) == keep && f <= top[keep-1].f {
				return
			}
			i := sort.Search(len(top), func(i int) bool { return top[i].f < f })
			top = append(top, cand{})
			copy(top[i+1:], top[i:])
			top[i] = cand{f, append([]int(nil), p...)}
			if len(top) > keep {
				top = top[:keep]
			}
		})
		best := cand{f: math.Inf(-1)}
		for _, c := range top {
			if f := c.f + wordCover(lowerLetters(columnar(rs, c.order, true))); f > best.f {
				best = cand{f, c.order}
			}
		}
		t := columnar(rs, best.order, true)
		note := fmt.Sprintf("key length %d, %.2f common letter pairs per letter, %.0f%% of the letters in common words", w, ngramRate(t), 100*wordCover(lowerLetters(t)))
		if spaced {
			note += fmt.Sprintf(", %.0f%% common words", 100*WordRatio(t))
		}
		gs = append(gs, Guess{Key: columnRanks(best.order), Text: t, Score: best.f, Note: note})
		per = append(per, fmt.Sprintf("%d %.2f", w, best.f))
	}
	sort.SliceStable(gs, func(i, j int) bool { return gs[i].Score > gs[j].Score })
	if len(gs) > 3 {
		gs = gs[:3]
	}
	return gs, strings.Join(per, ", ")
}

// permutations calls f with every order of p (Heap's algorithm); f must not keep p.
func permutations(p []int, f func([]int)) {
	c := make([]int, len(p))
	f(p)
	for i := 0; i < len(p); {
		if c[i] < i {
			if i%2 == 0 {
				p[0], p[i] = p[i], p[0]
			} else {
				p[c[i]], p[i] = p[i], p[c[i]]
			}
			f(p)
			c[i]++
			i = 0
		} else {
			c[i] = 0
			i++
		}
	}
}

func lowerLetters(s string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(s) {
		if r >= 'a' && r <= 'z' {
			b.WriteRune(r)
		}
	}
	return b.String()
}

// Common English bigrams and trigrams for scoring a substitution guess (log-probability of what the text contains).
var commonBigrams = strings.Fields(`th he in er an re on at en nd ti es or te of ed is it al ar st to nt ng se ha as ou io le ve co me de hi ri ro ic ne ea ra ce li ch ll be ma si om ur`)
var commonTrigrams = strings.Fields(`the and ing ent ion her for tha nth int ere tio ter est ers ati hat ate all eth hes ver his oft ith fth sth oth res ont`)

func substitutionFitness(t string) float64 {
	t = strings.ToLower(t)
	score := -ChiSquared(t) / 10
	for _, bg := range commonBigrams {
		score += float64(strings.Count(t, bg))
	}
	for _, tg := range commonTrigrams {
		score += 3 * float64(strings.Count(t, tg))
	}
	score += 120 * WordRatio(t) * float64(len(strings.Fields(t)))
	return score
}

var commonPair [26 * 26]bool
var commonTriple [26 * 26 * 26]bool

func init() {
	for _, bg := range commonBigrams {
		commonPair[int(bg[0]-'a')*26+int(bg[1]-'a')] = true
	}
	for _, tg := range commonTrigrams {
		commonTriple[(int(tg[0]-'a')*26+int(tg[1]-'a'))*26+int(tg[2]-'a')] = true
	}
}

// ngramRate counts the common English letter pairs, and triples twice, per letter, across spaces and punctuation: it scores text
// without spaces too, where WordRatio sees no words. The test prose scores 0.80, its letters shuffled 0.22; the best wrong column order
// of 40,320 reached 0.61 on 90 letters.
func ngramRate(s string) float64 {
	n, hits, p1, p2 := 0, 0, -1, -1
	for _, r := range s {
		if r >= 'A' && r <= 'Z' {
			r += 'a' - 'A'
		}
		if r < 'a' || r > 'z' {
			continue
		}
		x := int(r - 'a')
		n++
		if p1 >= 0 && commonPair[p1*26+x] {
			hits++
		}
		if p2 >= 0 && commonTriple[(p2*26+p1)*26+x] {
			hits += 2
		}
		p2, p1 = p1, x
	}
	if n == 0 {
		return 0
	}
	return float64(hits) / float64(n)
}

// wordCover is the share of the letters (a..z only) that common English words of two or more letters cover when the text is split
// into words the best way: it reads text written without spaces.
func wordCover(ls string) float64 {
	if ls == "" {
		return 0
	}
	best := make([]int, len(ls)+1) // best[i]: most letters of ls[:i] covered
	for i := 1; i <= len(ls); i++ {
		best[i] = best[i-1]
		for l := 2; l <= 12 && l <= i; l++ {
			if commonWords[ls[i-l:i]] && best[i-l]+l > best[i] {
				best[i] = best[i-l] + l
			}
		}
	}
	return float64(best[len(ls)]) / float64(len(ls))
}

// readsAsEnglish: common words, or for text without spaces, common letter pairs or letters covered by common words.
func readsAsEnglish(s string) bool {
	if strings.ContainsAny(strings.TrimSpace(s), " \n") {
		return WordRatio(s) >= 0.2
	}
	return ngramRate(s) >= 0.7 || wordCover(lowerLetters(s)) >= 0.5
}

// CrackSubstitution hill-climbs the key (swapping two letters at a time, several restarts) towards English letter, bigram and trigram
// frequencies. It needs a few hundred letters to be reliable; with less it returns its best try and says so.
func CrackSubstitution(s string, restarts int) Guess {
	if restarts <= 0 {
		restarts = 10
	}
	letters := 0
	for _, r := range s {
		if unicode.IsLetter(r) {
			letters++
		}
	}
	rng := rand.New(rand.NewSource(1))
	var best Guess
	best.Score = math.Inf(-1)
	for rs := 0; rs < restarts; rs++ {
		// start from the frequency order: the most common cipher letter stands for e, and so on
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
		sort.SliceStable(order, func(i, j int) bool { return counts[order[i]] > counts[order[j]] })
		byFreq := []rune("etaoinshrdlcumwfgypbvkjxqz")
		var table [26]rune // cipher letter -> plain letter
		for rank, c := range order {
			table[c] = byFreq[rank]
		}
		if rs > 0 {
			for i := 0; i < 10; i++ { // a different start each restart
				a, b := rng.Intn(26), rng.Intn(26)
				table[a], table[b] = table[b], table[a]
			}
		}
		cur := substitutionFitness(applyTable(s, table))
		for it := 0; it < 4000; it++ {
			a, b := rng.Intn(26), rng.Intn(26)
			if a == b {
				continue
			}
			table[a], table[b] = table[b], table[a]
			if f := substitutionFitness(applyTable(s, table)); f >= cur {
				cur = f
			} else {
				table[a], table[b] = table[b], table[a]
			}
		}
		if cur > best.Score {
			key := make([]rune, 26)
			for c, p := range table {
				key[p-'a'] = rune('a' + c)
			}
			best = Guess{Key: string(key), Text: applyTable(s, table), Score: cur}
		}
	}
	best.Note = fmt.Sprintf("%.0f%% common words", 100*WordRatio(best.Text))
	if letters < 150 {
		best.Note += "; the text is short for this (under 150 letters), so the key is a guess: check it against the words that came out"
	}
	return best
}
