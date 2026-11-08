package cryptotools

import (
	"fmt"
	"sort"
	"strings"
)

// howItFell is the work behind a crack, computed from the ciphertext: its letter counts against English, its index of coincidence,
// and for a repeating key Kasiski's repeated trigrams with their common factors and the index of coincidence per key length. It is
// what she shows when asked how a cipher was broken, so she never describes a method the tool did not use. perLength is the
// cracker's own score per key length (autokey and columnar), or "".
func howItFell(text, cipher, perLength string) string {
	ls := lowerLetters(text)
	if len(ls) == 0 {
		return ""
	}
	ic := IndexOfCoincidence(ls)
	var b strings.Builder
	b.WriteString("How it fell (computed from the ciphertext):\n")
	fmt.Fprintf(&b, "- Letter frequencies, most common first: %s.\n", letterOrder(ls))
	fmt.Fprintf(&b, "- Index of coincidence %.3f (English 0.066, random letters 0.038): %s.\n", ic, icMeaning(ic))
	switch cipher {
	case "vigenere", "beaufort":
		b.WriteString("- " + kasiski(ls, 16) + "\n")
		var ics []string
		for n := 1; n <= 12 && n*4 <= len(ls); n++ {
			ics = append(ics, fmt.Sprintf("%d %.3f", n, columnIC(ls, n)))
		}
		fmt.Fprintf(&b, "- Index of coincidence per key length (every n-th letter; English-like at the key length and its multiples): %s.\n", strings.Join(ics, ", "))
	case "autokey":
		b.WriteString("- The key does not repeat (the plaintext continues it), so Kasiski and the index of coincidence per key length do not apply. " +
			"With a primer of n letters every n-th letter forms a chain that one primer letter decides; each chain was solved by chi-squared over the 26 letters.\n")
		if perLength != "" {
			fmt.Fprintf(&b, "- Chi-squared of the decoding per primer length (lower is more like English): %s.\n", perLength)
		}
	case "columnar", "railfence":
		fmt.Fprintf(&b, "- Chi-squared of the ciphertext's own letters against English: %.0f. A transposition only moves letters, so their counts stay English.\n", ChiSquared(ls))
		if perLength != "" {
			fmt.Fprintf(&b, "- Every column order of each key length was tried; best common letter pairs per letter (plus the share of common words when there are spaces) per key length: %s.\n", perLength)
		}
	}
	return b.String()
}

func icMeaning(ic float64) string {
	switch {
	case ic >= 0.058:
		return "English-like, so one alphabet is used (a shift, affine or substitution) or the letters were only moved (a transposition)"
	case ic < 0.05:
		return "flat, so the letters were spread over several alphabets (a key that changes from letter to letter)"
	}
	return "between English and random"
}

// kasiski lists the trigrams that repeat in the letters, the distances between their repeats, and the factors (2..maxLen) those
// distances share: a repeating key's length divides most of them.
func kasiski(ls string, maxLen int) string {
	pos := map[string][]int{}
	for i := 0; i+3 <= len(ls); i++ {
		pos[ls[i:i+3]] = append(pos[ls[i:i+3]], i)
	}
	type rep struct {
		tri   string
		dists []int
	}
	var reps []rep
	var all []int
	for tri, ps := range pos {
		if len(ps) < 2 {
			continue
		}
		r := rep{tri: tri}
		for i := 1; i < len(ps); i++ {
			r.dists = append(r.dists, ps[i]-ps[i-1])
		}
		reps = append(reps, r)
		all = append(all, r.dists...)
	}
	if len(reps) == 0 {
		return "Kasiski: no trigram repeats, so there are no distances to factor (the text is short, or the key is long or does not repeat)."
	}
	sort.Slice(reps, func(i, j int) bool {
		if len(reps[i].dists) != len(reps[j].dists) {
			return len(reps[i].dists) > len(reps[j].dists)
		}
		return reps[i].tri < reps[j].tri
	})
	var ex []string
	for i, r := range reps {
		if i >= 4 {
			break
		}
		ds := make([]string, len(r.dists))
		for k, d := range r.dists {
			ds[k] = fmt.Sprint(d)
		}
		ex = append(ex, strings.ToUpper(r.tri)+" at distances "+strings.Join(ds, ", "))
	}
	type fc struct{ f, n int }
	var fs []fc
	for f := 2; f <= maxLen; f++ {
		n := 0
		for _, d := range all {
			if d%f == 0 {
				n++
			}
		}
		if n > 0 {
			fs = append(fs, fc{f, n})
		}
	}
	sort.SliceStable(fs, func(i, j int) bool { return fs[i].n > fs[j].n })
	var top []string
	for i, f := range fs {
		if i >= 3 {
			break
		}
		top = append(top, fmt.Sprintf("%d (%d of %d)", f.f, f.n, len(all)))
	}
	out := fmt.Sprintf("Kasiski: %d repeated trigram(s), e.g. %s.", len(reps), strings.Join(ex, "; "))
	if len(top) > 0 {
		out += " Factors the distances share most: " + strings.Join(top, ", ") + "."
	}
	return out
}
