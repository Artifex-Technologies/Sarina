package insight

import (
	"regexp"
	"strings"
)

// Her own voice, heard back by the microphone and logged as the owner's words (the speakers were on while she talked, 2026-10-23).
// Such a line must never become evidence about the owner: it is what she said, not what they said.
var ownVoiceRx = regexp.MustCompile(`(?i)\b(i('m| am) (right )?here whenever|standing by whenever|whenever you('re| are| would like| want)|dive back into|your custom ai|` +
	`ready to (tackle|help you|assist)|feel free to ask|let me know (if|when|how)|how can i (help|assist)|is there anything (else )?i can)\b`)

// IsEcho says whether what the owner "said" is really one of her answers heard back: it repeats most of an answer she gave just
// before (the previous lines), or it is phrased the way an assistant talks to the person it serves.
func IsEcho(said string, previous []Line) bool {
	s := strings.TrimSpace(said)
	if s == "" {
		return false
	}
	if ownVoiceRx.MatchString(s) {
		return true
	}
	from := len(previous) - 6
	if from < 0 {
		from = 0
	}
	for _, p := range previous[from:] {
		if len(strings.Fields(norm(p.Me))) >= 4 && similar(p.Me, s) {
			return true
		}
	}
	return false
}

// WithoutEchoes drops the lines that are her own voice heard back, and returns them separately.
func WithoutEchoes(lines []Line) (kept, echoes []Line) {
	for i, l := range lines {
		if IsEcho(l.Said, lines[:i]) {
			echoes = append(echoes, l)
			continue
		}
		kept = append(kept, l)
	}
	return kept, echoes
}
