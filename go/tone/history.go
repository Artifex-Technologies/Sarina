package tone

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// History is when each question was asked and the mood it was heard as: a time and one word per line, no audio, no words of the question. It lets
// Sarina notice a pattern over days ("low most evenings this week") the way a friend would, instead of only the one question in front of her.

// Entry is one heard question.
type Entry struct {
	T    int64  `json:"t"` // unix seconds
	Mood string `json:"mood"`
}

// keepEntries bounds the file (a few months of daily use).
const keepEntries = 600

// ReadHistory returns the entries in the file, oldest first (none when it is missing or unreadable).
func ReadHistory(path string) []Entry {
	f, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer f.Close()
	var out []Entry
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		var e Entry
		if json.Unmarshal(sc.Bytes(), &e) == nil && e.T > 0 && e.Mood != "" {
			out = append(out, e)
		}
	}
	return out
}

// AppendHistory adds one entry, keeping only the newest keepEntries, privately (owner only) through a temporary file.
func AppendHistory(path string, e Entry) error {
	h := append(ReadHistory(path), e)
	if len(h) > keepEntries {
		h = h[len(h)-keepEntries:]
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	tmp := path + ".tmp"
	f, err := os.OpenFile(tmp, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	w := bufio.NewWriter(f)
	for _, x := range h {
		raw, _ := json.Marshal(x)
		w.Write(raw)
		w.WriteByte('\n')
	}
	if err := w.Flush(); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

// Pattern is one or two sentences about how the owner has sounded lately, for Sarina only, or "" when nothing stands out or there is too little
// to go on. It compares the last seven days with the seven before, and today with the week, and says when in the day a low or tense mood clusters.
func Pattern(h []Entry, now time.Time) string {
	week, prev, today := count{}, count{}, count{}
	evening := map[string]int{}
	y, m, d := now.Date()
	midnight := time.Date(y, m, d, 0, 0, 0, 0, now.Location())
	for _, e := range h {
		t := time.Unix(e.T, 0).In(now.Location())
		age := now.Sub(t)
		switch {
		case age < 0:
			continue
		case age < 7*24*time.Hour:
			week.add(e.Mood)
			if !t.Before(midnight) {
				today.add(e.Mood)
			}
			if hr := t.Hour(); hr >= 20 || hr < 4 {
				evening[e.Mood]++
			}
		case age < 14*24*time.Hour:
			prev.add(e.Mood)
		}
	}
	var out []string
	if today.n >= 3 {
		for _, mood := range []string{Low, Tense} {
			if today.m[mood]*2 > today.n {
				out = append(out, fmt.Sprintf("Today they have sounded %s in %d of %d questions.", words(mood), today.m[mood], today.n))
				break
			}
		}
	}
	if week.n >= 8 {
		for _, mood := range []string{Low, Tense, Upbeat} {
			k := week.m[mood]
			share := float64(k) / float64(week.n)
			if k < 3 || share < 0.25 {
				continue
			}
			s := fmt.Sprintf("This week they have sounded %s in %d of %d questions", words(mood), k, week.n)
			if prev.n >= 8 {
				if was := float64(prev.m[mood]) / float64(prev.n); share >= 2*was {
					s += fmt.Sprintf(", more than the week before (%d of %d)", prev.m[mood], prev.n)
				}
			}
			if mood != Upbeat && evening[mood]*10 >= k*6 {
				s += ", mostly late in the day"
			}
			out = append(out, s+".")
			if len(out) == 2 {
				break
			}
		}
	}
	if len(out) == 0 {
		return ""
	}
	r := out[0]
	if len(out) > 1 {
		r += " " + out[1]
	}
	return r
}

type count struct {
	n int
	m map[string]int
}

func (c *count) add(mood string) {
	if c.m == nil {
		c.m = map[string]int{}
	}
	c.n++
	c.m[mood]++
}

func words(mood string) string {
	switch mood {
	case Low:
		return "low or tired"
	case Tense:
		return "rushed or tense"
	case Upbeat:
		return "in good spirits"
	}
	return mood
}
