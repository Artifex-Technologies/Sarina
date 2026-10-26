package cryptotools

import (
	"math"
	"strings"
	"unicode"
)

// English letter frequencies (a..z), as fractions, for the chi-squared fitness the crackers use.
var englishFreq = [26]float64{
	0.08167, 0.01492, 0.02782, 0.04253, 0.12702, 0.02228, 0.02015, 0.06094, 0.06966, 0.00153, 0.00772, 0.04025, 0.02406,
	0.06749, 0.07507, 0.01929, 0.00095, 0.05987, 0.06327, 0.09056, 0.02758, 0.00978, 0.02360, 0.00150, 0.01974, 0.00074,
}

// commonWords are frequent English words; how many of a text's words are in the list is the plainest sign that a decoding worked.
var commonWords = map[string]bool{}

func init() {
	for _, w := range strings.Fields(`the be to of and a in that have i it for not on with he as you do at this but his by from they we say
her she or an will my one all would there their what so up out if about who get which go me when make can like time no just him know
take people into year your good some could them see other than then now look only come its over think also back after use two how our
work first well way even new want because any these give day most us is are was were been has had did does said says very much more
than many where why here each both own same too under again further once during before above below between through off down little
much such other still should those while must may might shall find found call called made need keep let put ask tell seem try leave
help start show hear play run move live believe hold bring happen write provide sit stand lose pay meet include continue set learn
change lead understand watch follow stop create speak read allow add spend grow open walk win offer remember love consider appear buy
wait serve die send expect build stay fall cut reach kill remain suggest raise pass sell require report decide pull return explain
hope develop carry break receive agree support hit produce eat cover catch draw choose cause point attack secret message key cipher
code flag password hello world test text plain data file name user system computer network security hidden meet midnight tonight
north south east west enemy money gold treasure map letter dear love friend yours truly sincerely never always`) {
		commonWords[w] = true
	}
}

// ChiSquared compares the letter counts of s with English: 0 is a perfect fit; English prose scores around 20-40, a Caesar shift of it in
// the hundreds. Only letters count; a text with none scores +Inf.
func ChiSquared(s string) float64 {
	var counts [26]float64
	n := 0.0
	for _, r := range strings.ToLower(s) {
		if r >= 'a' && r <= 'z' {
			counts[r-'a']++
			n++
		}
	}
	if n == 0 {
		return math.Inf(1)
	}
	chi := 0.0
	for i := 0; i < 26; i++ {
		expected := englishFreq[i] * n
		d := counts[i] - expected
		chi += d * d / expected
	}
	return chi
}

// WordRatio is the share of the words in s (letters only, 2+ characters) that are common English words.
func WordRatio(s string) float64 {
	words := strings.FieldsFunc(strings.ToLower(s), func(r rune) bool { return !unicode.IsLetter(r) && r != '\'' })
	if len(words) == 0 {
		return 0
	}
	hits := 0
	for _, w := range words {
		w = strings.Trim(w, "'")
		if len(w) >= 2 && commonWords[w] {
			hits++
		}
	}
	return float64(hits) / float64(len(words))
}

// Printable is the share of runes in s that are printable text (letters, digits, punctuation, spaces, newlines and tabs).
func Printable(s string) float64 {
	if s == "" {
		return 0
	}
	ok, n := 0, 0
	for _, r := range s {
		n++
		if r == '\n' || r == '\r' || r == '\t' || (unicode.IsPrint(r) && r != unicode.ReplacementChar) {
			ok++
		}
	}
	return float64(ok) / float64(n)
}

// LooksLikeText says whether s reads as plain text: nearly all printable and either English words or a text-like mix of letters and
// spaces (JSON and code count as text too).
func LooksLikeText(s string) bool {
	if len(s) < 2 || Printable(s) < 0.95 {
		return false
	}
	if WordRatio(s) >= 0.25 {
		return true
	}
	letters, spaces, digits := 0, 0, 0
	for _, r := range s {
		switch {
		case unicode.IsLetter(r):
			letters++
		case r == ' ' || r == '\n':
			spaces++
		case unicode.IsDigit(r):
			digits++
		}
	}
	n := len([]rune(s))
	if strings.HasPrefix(strings.TrimSpace(s), "{") || strings.HasPrefix(strings.TrimSpace(s), "[") || strings.HasPrefix(strings.TrimSpace(s), "<") {
		return true
	}
	// prose: mostly letters, with spaces; a bare base64 or hex string has no spaces and too many digits or odd case
	return letters > n/2 && spaces > 0 && float64(digits) < 0.3*float64(n)
}

// Entropy is the Shannon entropy of b in bits per byte (0 for a constant, 8 for uniform random).
func Entropy(b []byte) float64 {
	if len(b) == 0 {
		return 0
	}
	var counts [256]float64
	for _, c := range b {
		counts[c]++
	}
	h := 0.0
	n := float64(len(b))
	for _, c := range counts {
		if c > 0 {
			p := c / n
			h -= p * math.Log2(p)
		}
	}
	return h
}

// IndexOfCoincidence of the letters in s: English prose is about 0.066, random letters 0.038.
func IndexOfCoincidence(s string) float64 {
	var counts [26]float64
	n := 0.0
	for _, r := range strings.ToLower(s) {
		if r >= 'a' && r <= 'z' {
			counts[r-'a']++
			n++
		}
	}
	if n < 2 {
		return 0
	}
	sum := 0.0
	for _, c := range counts {
		sum += c * (c - 1)
	}
	return sum / (n * (n - 1))
}
