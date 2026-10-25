package sysinfo

import (
	"context"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/embed"
	"github.com/Artifex-Technologies/Sarina/go/route"
	"github.com/Artifex-Technologies/Sarina/go/skills"
)

// A skill with "prefetch: [status]" (desktop-fix) starts from what the computer reports about the task's topic, looked up before the
// model answers: the same as a spoken question (labyrinth-ask-sarina), where this made the small model answer from evidence instead
// of general advice.
func init() {
	skills.RegisterContext("status", statusContext)
}

// keywordTopics decide when the embedding model is not there (the voice path's list).
var keywordTopics = []struct {
	re    *regexp.Regexp
	topic string
}{
	{regexp.MustCompile(`(?i)\b(crash|crashed|crashes|core dump|segfault|segmentation|aborted|froze|freez|hung|hang)\w*`), "crashes"},
	{regexp.MustCompile(`(?i)\b(slow|lag|laggy|sluggish|performance|cpu|processor|memory|ram|swap|busy|fan|hot|temperature|heat|load)\b`), "performance"},
	{regexp.MustCompile(`(?i)\b(internet|wi-?fi|network|connection|connect|online|offline|dns|ethernet)\b`), "network"},
	{regexp.MustCompile(`(?i)\b(disk|storage|space|full)\b`), "disk"},
	{regexp.MustCompile(`(?i)\b(boot|startup|start up|takes? long to (start|boot))\b`), "boot"},
	{regexp.MustCompile(`(?i)\b(sound|audio|speaker|speakers|headphones?|headset|microphone|mic|volume)\b`), "audio"},
	{regexp.MustCompile(`(?i)\b(error|errors|log|logs|failed|failing|broke|broken|problem|wrong|issue)\b`), "services"},
	{regexp.MustCompile(`(?i)\b(game|games|gaming|steam|proton|wine|mods?|modding|fps|frame ?rates?|stutter\w*|controller|gamepad)\b`), "gaming"},
}

func topicsFor(task string) []string {
	var out []string
	if found, err := route.Route(task, embed.Default()); err == nil {
		out = found
	} else {
		for _, k := range keywordTopics {
			if k.re.MatchString(task) {
				out = append(out, k.topic)
			}
		}
	}
	var keep []string
	for _, t := range out {
		if _, ok := Topics[t]; ok && len(keep) < 2 {
			keep = append(keep, t)
		}
	}
	return keep
}

// perTopic is the most of one topic's report that goes in front of the model.
const perTopic = 3500

func statusContext(task string) string {
	topics := topicsFor(task)
	if len(topics) == 0 {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	var parts []string
	for _, t := range topics {
		out, err := Report(ctx, t)
		if err != nil || strings.TrimSpace(out) == "" {
			continue
		}
		if len(out) > perTopic {
			out = out[:perTopic] + "\n[…]"
		}
		parts = append(parts, "## "+t+"\n"+out)
	}
	if len(parts) == 0 {
		return ""
	}
	return "What the computer reports right now (read-only, already looked up for you; call system_status for any other topic):\n\n" + strings.Join(parts, "\n\n")
}
