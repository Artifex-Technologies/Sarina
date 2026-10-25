package agent

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/Artifex-Technologies/Sarina/go/session"
)

// A small model sometimes finishes by saying it changed a file it never wrote (it used the wrong tool, or none). Before such an answer is
// accepted, the claim is checked against what the file tools actually did in this task; if they did not touch that file, the model is told so
// once and asked to either do it or say plainly that it did not happen.

var (
	claimFileRE = regexp.MustCompile(`(?i)\b([\w.-]+\.(?:md|txt|json|py|go|rs|toml|ya?ml|sh|conf|env|ini|csv|js|ts|html|css|c|h|cpp|java|lua|xml))\b`)
	claimVerbRE = regexp.MustCompile(`(?i)\b(added|updated|changed|wrote|written|saved|edited|appended|created|modified|replaced|fixed|inserted|removed|deleted|renamed)\b`)
	negationRE  = regexp.MustCompile(`(?i)\b(not|n't|never|no|nothing|without|unable|cannot|could not|did not|didn't|won't|wasn't|haven't)\b`)
	sentenceEnd = regexp.MustCompile(`[.!?](?:\s+|$)|\n`)
)

var fileWriteTools = map[string]bool{"write_file": true, "edit_file": true, "vault_write": true}

// unbackedFileClaim returns the first file the answer says it changed that no file-writing tool changed successfully in this task, or "".
func unbackedFileClaim(answer string, history []map[string]any) string {
	written := map[string]bool{}
	for _, e := range history {
		name, _ := e["tool_name"].(string)
		ok, _ := e["ok"].(bool)
		if !fileWriteTools[name] || !ok {
			continue
		}
		if p, _ := e["path"].(string); p != "" {
			written[strings.ToLower(filepath.Base(p))] = true
		}
		if ps, ok := e["changed_paths"].([]string); ok {
			for _, p := range ps {
				written[strings.ToLower(filepath.Base(p))] = true
			}
		}
	}
	for _, s := range sentenceEnd.Split(answer, -1) {
		if !claimVerbRE.MatchString(s) || negationRE.MatchString(s) {
			continue
		}
		for _, m := range claimFileRE.FindAllStringSubmatch(s, -1) {
			f := strings.ToLower(filepath.Base(m[1]))
			if !written[f] {
				return m[1]
			}
		}
	}
	return ""
}

func claimCheckText(file string) string {
	return "Check before you finish: your answer says " + file + " was changed, but no file tool changed " + file +
		" in this task (a to-do or plan tool does not write files). Either make the change now with the file tools, or say plainly that " + file + " was not changed. Write your whole answer again, complete, and do not talk about this check."
}

// unreportedChange returns a file a file-writing tool changed in this task that the answer never mentions, or "": a change the user is not
// told about is as bad as one that is claimed but never made.
func unreportedChange(answer string, history []map[string]any) string {
	low := strings.ToLower(answer)
	for _, e := range history {
		name, _ := e["tool_name"].(string)
		ok, _ := e["ok"].(bool)
		if !fileWriteTools[name] || !ok {
			continue
		}
		p, _ := e["path"].(string)
		if p == "" {
			continue
		}
		base := filepath.Base(p)
		if !strings.Contains(low, strings.ToLower(base)) && !strings.Contains(low, strings.ToLower(strings.TrimSuffix(base, filepath.Ext(base)))) {
			return base
		}
	}
	return ""
}

func unreportedText(file string) string {
	return "Check before you finish: in this task you changed " + file + ", but your answer does not say so. Write your whole answer again, complete, and say plainly what you changed in " + file + " and what it holds now."
}

var quotedRE = regexp.MustCompile("(?:^|[\\s(:,])(?:`([^`\\n]{4,60})`|\"([^\"\\n]{4,60})\")(?:$|[\\s).,:;!?])")

// ungroundedQuote returns a literal the answer quotes (in backticks or double quotes) that appears in none of this task's tool output and not in
// the prompt, when tools were used at all: quoting a string as if it came from the evidence, when it did not, is how a small model invents
// passwords and messages a program never contained.
func ungroundedQuote(answer, prompt string, msgs []session.Message) string {
	var evidence strings.Builder
	used := false
	for _, m := range msgs {
		if m.Role == "tool" {
			used = true
			evidence.WriteString(m.Content)
			evidence.WriteByte('\n')
		}
	}
	if !used {
		return ""
	}
	ev := strings.ToLower(evidence.String())
	p := strings.ToLower(prompt)
	for _, m := range quotedRE.FindAllStringSubmatch(answer, -1) {
		q := m[1]
		if q == "" {
			q = m[2]
		}
		lq := strings.ToLower(strings.TrimSpace(q))
		if lq == "" || strings.Contains(ev, lq) || strings.Contains(p, lq) {
			continue
		}
		// a path, a command or a name the model writes in its own words is not a quote from the evidence: only literal-looking text counts
		if strings.ContainsAny(q, "/\\") || strings.HasPrefix(q, "-") {
			continue
		}
		return q
	}
	return ""
}

func ungroundedText(q string) string {
	return "Check before you finish: your answer quotes \"" + q + "\", but no tool output in this task contains it. Write your whole answer again, complete: keep everything the tools showed, and either leave that quote out, mark it plainly as your own example, or look it up with your tools first. Do not talk about this check."
}

var toolClaimRE = regexp.MustCompile(`(?i)\b(the (reverse[- ]engineering |analysis |search )?tool (showed|shows|analy[sz]ed|found|returned|identified|reported)|i (ran|checked|looked (at|up|into)|analy[sz]ed|decompiled|read|searched|inspected|examined)|decompil(ed|ing|ation) (shows|showed|reveals|revealed)|after (analy[sz]ing|decompiling|reading|checking))\b`)

// claimsToolWork reports an answer that describes tool work ("I checked", "the tool showed") in a task where no tool ran: the answer is made up.
func claimsToolWork(answer, prompt string, msgs []session.Message) bool {
	// a spoken question can arrive with the system's report or her guide notes already looked up and placed in it: that is evidence too
	if strings.Contains(prompt, "already looked up for you") || strings.Contains(prompt, "From your guides") {
		return false
	}
	for _, m := range msgs {
		if m.Role == "tool" {
			return false
		}
	}
	return toolClaimRE.MatchString(answer)
}

const toolWorkText = "Check before you finish: your answer describes looking, checking or analysing, but you used no tool in this task, so none of that happened. Use the tools now, or write your answer again saying plainly that you have not looked."

var planningTools = map[string]bool{"task_create": true, "task_update": true, "todo_write": true, "plan_write": true}

// taskInsteadOfFile returns a file the prompt names (and that exists in the workspace) when, in this task, a planning tool was used and no file
// tool changed that file: "add an item to the todo list in todo.md" answered with her own task list leaves the file untouched.
func taskInsteadOfFile(prompt, cwd string, msgs []session.Message, history []map[string]any) string {
	planned := false
	for _, m := range msgs {
		for _, c := range m.ToolCalls {
			if planningTools[c.Function.Name] {
				planned = true
			}
		}
	}
	if !planned {
		return ""
	}
	for _, m := range claimFileRE.FindAllStringSubmatch(prompt, -1) {
		name := m[1]
		p := name
		if !filepath.IsAbs(p) {
			p = filepath.Join(cwd, name)
		}
		if fi, err := os.Stat(p); err != nil || !fi.Mode().IsRegular() {
			continue
		}
		if unbackedFileClaim("I changed "+name+".", history) == name {
			return name
		}
	}
	return ""
}

func taskInsteadOfFileText(file string) string {
	return "Check before you finish: the user asked about " + file + ", which is a file, but you used your own task list, which does not change any file. Make the change in " + file + " with the file tools now, then write your whole answer again."
}

var securityOffRE = regexp.MustCompile(`(?i)\b(disabl(e|ing)|turn(ing)? off|switch(ing)? off|stop(ping)?|mask(ing)?|remov(e|ing)|uninstall(ing)?)\b[^.\n]{0,60}\b(ufw|firewall|firewalld|heimdall|apparmor|selinux|keyring|freshclam|clamav|antivirus|secure boot|limbo)\b`)

// suggestsSecurityOff returns the security piece an answer suggests turning off, or "": speed or convenience never justifies that advice.
func suggestsSecurityOff(answer string) string {
	if m := securityOffRE.FindStringSubmatch(answer); m != nil {
		return m[len(m)-1]
	}
	return ""
}

func securityOffText(what string) string {
	return "Check before you finish: your answer suggests turning off " + what + ", a security piece. Never advise that for speed or convenience. Write your whole answer again without that suggestion; if nothing else would help, say so."
}

// commandsMarker is in a report whose commands must be copied, not written by the model (the security report, sysinfo's
// computedSummary). Only there are the answer's commands checked: in other tasks she may suggest commands of her own.
const commandsMarker = "Commands to copy exactly"

var (
	backtickRE = regexp.MustCompile("`([^`\n]{3,400})`")
	cmdStartRE = regexp.MustCompile(`^(sudo |pacman |heimdalld |systemctl |printf |echo )`)
	// standard steps the security skill names itself, with a name or id that must come from the report
	stdCmdRE = regexp.MustCompile(`^(sudo pacman -Syu|pacman -Qi [\w.+-]+|sudo pacman -Rs [\w.+-]+|sudo heimdalld immunity (confirm|release) ([0-9a-f]{8,64}))$`)
)

func squash(s string) string { return strings.Join(strings.Fields(s), " ") }

// unsourcedCommand reports a command in the answer that the report and the tools never gave, when the task's report says its
// commands must be copied (the 8B model shortened Heimdall's fixes and made up "sudo pacman -Syu lua51" for a package with no fix).
func unsourcedCommand(answer, prompt string, msgs []session.Message) string {
	if !strings.Contains(prompt, commandsMarker) {
		return ""
	}
	var mat strings.Builder
	mat.WriteString(prompt)
	for _, m := range msgs {
		if m.Role == "tool" {
			mat.WriteString("\n" + m.Content)
		}
	}
	material := squash(mat.String())
	for _, m := range backtickRE.FindAllStringSubmatch(answer, -1) {
		c := squash(m[1])
		if !cmdStartRE.MatchString(c) || strings.Contains(material, c) {
			continue
		}
		if sm := stdCmdRE.FindStringSubmatch(c); sm != nil {
			// a name or id in it must be one the report gave
			parts := strings.Fields(c)
			if last := parts[len(parts)-1]; c == "sudo pacman -Syu" || strings.Contains(material, last) {
				continue
			}
		}
		return c
	}
	return ""
}

func unsourcedText(c string) string {
	return "Check before you finish: your answer gives the command `" + c + "`, but the report does not. Use only the commands the report gives, copied whole and unchanged (the block \"" + commandsMarker + "\"), or the standard steps the instructions name; if no command is given for a finding, say so plainly. Write your whole answer again, complete. Do not talk about this check."
}

// pastClaimRE finds a sentence that reports shared history: something the user said or did before, or something said in an earlier
// conversation ("as you told me last week", "a joke from earlier", "your usual").
var pastClaimRE = regexp.MustCompile(`(?i)\b(you (told|mentioned|said|asked|showed) (me|that|it|this|so)?|as you (told|mentioned|said)|we (talked|spoke|discussed|chatted)|last (time|week|night|month)|the other day|yesterday|from earlier|earlier (today|you|we|on)|your usual|as (always|usual|before)|you usually|you always)\b`)

// timeClaimRE is the part of pastClaimRE that places the claim before this question: such a claim needs evidence from earlier,
// while "as you said" may point at the question itself.
var timeClaimRE = regexp.MustCompile(`(?i)\b(we (talked|spoke|discussed|chatted)|last (time|week|night|month)|the other day|yesterday|from earlier|earlier (today|you|we|on)|your usual|as (always|usual|before)|you usually|you always)\b`)

var wordRE = regexp.MustCompile(`[\p{L}\p{N}]{4,}`)

var commonWords = map[string]bool{"that": true, "this": true, "with": true, "your": true, "from": true, "have": true, "were": true, "what": true, "told": true,
	"said": true, "last": true, "week": true, "time": true, "earlier": true, "today": true, "yesterday": true, "usual": true, "always": true,
	"before": true, "mentioned": true, "asked": true, "still": true, "about": true, "just": true, "like": true, "they": true, "there": true,
	"been": true, "will": true, "would": true, "could": true, "should": true, "when": true, "then": true, "than": true, "them": true,
	"those": true, "these": true, "talked": true, "discussed": true, "spoke": true, "usually": true, "night": true, "month": true}

// inventedHistory reports a sentence of the answer that claims shared history none of the evidence holds: earlier turns of this
// conversation, curated memory and the system context, and tool output. The current question does not count: "You told me
// yesterday that..." asked by the user is not evidence that it happened. Seen with Qwen3 8B: "You had avocado toast, as you told me
// last week" when nothing was ever said about breakfast.
func inventedHistory(answer string, all []session.Message, msgStart int) string {
	var ev, now strings.Builder
	for i, m := range all {
		if i >= msgStart && m.Role == "user" {
			if !strings.HasPrefix(m.MessageID, "claim_check") {
				now.WriteString(strings.ToLower(m.Content)) // this task's own question: evidence for "as you said", not for "last week"
			}
			continue
		}
		if m.Role == "assistant" && i >= msgStart {
			continue
		}
		ev.WriteString(strings.ToLower(m.Content))
		ev.WriteByte('\n')
	}
	past, current := ev.String(), now.String()
	for _, sentence := range regexp.MustCompile(`[^.!?\n]+[.!?]?`).FindAllString(answer, -1) {
		if !pastClaimRE.MatchString(sentence) {
			continue
		}
		evidence := past
		if !timeClaimRE.MatchString(sentence) {
			evidence += "\n" + current
		}
		words := 0
		for _, w := range wordRE.FindAllString(strings.ToLower(sentence), -1) {
			if commonWords[w] {
				continue
			}
			words++
			if strings.Contains(evidence, w) {
				return "" // the claim touches something that was really said or recorded
			}
		}
		if words > 0 {
			return strings.TrimSpace(sentence)
		}
	}
	return ""
}

func inventedHistoryText(s string) string {
	return "Check before you finish: your answer says \"" + s + "\" as if it came from an earlier conversation or something the user told you, but nothing you were given shows it (not this conversation, your memory or your tools). Do not invent what was said or done before. If you do not know or do not remember, say so plainly. Write your whole answer again, complete. Do not talk about this check."
}
