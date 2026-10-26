package cryptotools

import (
	"fmt"
	"math"
	"math/rand"
	"sort"
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
	Score float64 // chi-squared against English (lower is better) for shift ciphers; word ratio for the others (higher is better)
	Note  string
}

// CrackCaesar scores all 26 shifts against English letter frequencies and returns the best three (best first).
func CrackCaesar(s string) []Guess {
	var gs []Guess
	for shift := 0; shift < 26; shift++ {
		t := Caesar(s, 26-shift)
		gs = append(gs, Guess{Key: fmt.Sprintf("shift %d", shift), Text: t, Score: ChiSquared(t), Note: fmt.Sprintf("chi-squared %.0f, %.0f%% common words", ChiSquared(t), 100*WordRatio(t))})
	}
	// Letter frequency alone misleads on short text ("Uryyb jbeyq" is Hello world, but another shift can score better), so a shift
	// whose text reads as English wins; chi-squared decides between the rest and between two readable ones.
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
		sum := 0.0
		for c := 0; c < n; c++ {
			var col strings.Builder
			for i := c; i < len(letters); i += n {
				col.WriteByte(letters[i])
			}
			sum += IndexOfCoincidence(col.String())
		}
		ks = append(ks, kl{n, sum / float64(n)})
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
	// a key that gives readable text beats one that only fits the letter statistics better (a long key overfits them: key length 15
	// scored chi-squared 13 with 19% words, the real 5-letter key 19 with 74%); among the unreadable, chi-squared decides
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
	return gs
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
