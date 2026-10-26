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
			evidence.WriteString(toolEvidence(m.Content))
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
		lq := strings.ToLower(strings.Trim(strings.TrimSpace(q), "\"'`"))
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
			mat.WriteString("\n" + toolEvidence(m.Content))
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

var (
	setDefaultRE = regexp.MustCompile(`(?i)\b(wpctl|pactl) set-default(-sink)?\b`)
	// "Firefox plays no sound but Spotify does": another program plays, so the default output works
	oneAppSilentRE = regexp.MustCompile(`(?i)\b(but|while|though|yet)\b[^.?!]{0,60}\b(does|works|plays|is fine|has sound|is working)\b`)
)

// defaultOutputForOneApp reports advice to change the default audio output when the question says another program plays sound:
// the output works, and changing it moves every program (seen repeatedly with the desktop-fix skill, even when told not to).
func defaultOutputForOneApp(answer, prompt string) bool {
	return setDefaultRE.MatchString(answer) && oneAppSilentRE.MatchString(prompt)
}

const oneAppSilentText = "Check before you finish: another program plays sound, so the default output works; `wpctl set-default` would move every program and fix nothing. Leave the default as it is. The fix is the silent program's own stream: find it under Streams in `wpctl status` (unmute it, raise its volume, or move only that stream), or the program's own sound setting. Write your whole answer again, complete, without set-default. Do not talk about this check."

// evidenceOf is what this task's tools and the prompt said: the material an answer's specifics must come from.
func evidenceOf(prompt string, msgs []session.Message) (ev string, used bool) {
	var b strings.Builder
	b.WriteString(prompt)
	for _, m := range msgs {
		if m.Role == "tool" {
			used = true
			b.WriteString("\n" + toolEvidence(m.Content))
		}
	}
	return strings.ToLower(b.String()), used
}

// identifierRE finds the specifics a small model invents most: version numbers, CVE ids, hashes and addresses, line numbers, ports.
var identifierRE = regexp.MustCompile(`(?i)\b(?:v?\d+\.\d+(?:\.\d+)+(?:[-+][\w.]+)?|CVE-\d{4}-\d{4,}|0x[0-9a-f]{4,}|[0-9a-f]{12,64}|lines? \d{2,}|port \d{2,5})\b`)

// ungroundedIdentifier returns a version, CVE id, hash, line number or port the answer gives that is in none of this task's tool output
// nor in the prompt, when tools were used at all. Arithmetic is not checked (a sum of numbers from a file is her own work); these forms
// are never computed, only read, so one that was never read was made up.
func ungroundedIdentifier(answer, prompt string, msgs []session.Message) string {
	ev, used := evidenceOf(prompt, msgs)
	if !used {
		return ""
	}
	for _, m := range identifierRE.FindAllString(answer, -1) {
		lm := strings.ToLower(m)
		if strings.HasPrefix(lm, "line") || strings.HasPrefix(lm, "port") {
			// the number itself must have been seen ("line 42": 42 in the output as a line number or a ":42")
			n := strings.TrimSpace(lm[strings.IndexAny(lm, " ")+1:])
			if strings.Contains(ev, n) {
				continue
			}
			return m
		}
		if strings.Contains(ev, strings.TrimPrefix(lm, "v")) {
			continue
		}
		return m
	}
	return ""
}

func ungroundedIdentifierText(id string) string {
	return "Check before you finish: your answer gives \"" + id + "\", but no tool output in this task and nothing in the question contains it. Such a value is read, never worked out: if you did not see it, do not state it. Write your whole answer again, complete, using only values you saw, or look it up with your tools first. Do not talk about this check."
}

var checkedFileRE = regexp.MustCompile(`(?i)\bI (?:ran|checked|looked (?:at|in|into|through)|read|opened|inspected|examined|searched|scanned|reviewed|went through)\b[^.!?\n]{0,80}?\b([\w./-]+\.(?:md|txt|json|py|go|rs|toml|ya?ml|sh|conf|env|ini|csv|js|ts|html|css|c|h|cpp|java|lua|xml|log))\b`)

// checkedUnseenFile returns a file the answer says she read or checked that no tool call of this task named: "I checked config.env" when
// read_file was never called on it is a made-up look.
func checkedUnseenFile(answer string, msgs []session.Message) string {
	var args strings.Builder
	for _, m := range msgs {
		for _, c := range m.ToolCalls {
			args.WriteString(strings.ToLower(c.Function.Arguments))
			args.WriteByte('\n')
		}
		if m.Role == "tool" {
			args.WriteString(strings.ToLower(toolEvidence(m.Content))) // a listing or a search names files she then "looked at" in that output
			args.WriteByte('\n')
		}
	}
	seen := args.String()
	for _, sentence := range sentenceEnd.Split(answer, -1) {
		for _, loc := range checkedFileRE.FindAllStringSubmatchIndex(sentence, -1) {
			// "I did not check x" is no claim; "I checked x and it has no secrets" is one (the negation after the file is about the file)
			if negationRE.MatchString(sentence[:loc[1]]) {
				continue
			}
			file := sentence[loc[2]:loc[3]]
			if !strings.Contains(seen, strings.ToLower(filepath.Base(file))) {
				return file
			}
		}
	}
	return ""
}

func checkedUnseenText(file string) string {
	return "Check before you finish: your answer says you read or checked " + file + ", but no tool call in this task touched " + file + ", so that did not happen. Either read it now with your tools, or write your whole answer again saying plainly that you did not look at it. Do not talk about this check."
}

var failureWordRE = regexp.MustCompile(`(?i)\b(error|errors|fail|failed|failing|failure|could not|couldn't|can't|cannot|unable|denied|refused|not found|missing|does not exist|doesn't exist|no such|timed out|timeout|not run|didn't run|did not run|not possible|wasn't able|was not able)\b`)

// toolErrorIgnored reports an answer that ends a task whose last tool call failed without a word about any failure: the model answered
// as if the call had worked ("the file holds..." after read_file was denied).
func toolErrorIgnored(answer string, msgs []session.Message) bool {
	last := -1
	for i, m := range msgs {
		if m.Role == "tool" {
			last = i
		}
	}
	if last < 0 {
		return false
	}
	ok, known := msgs[last].Metadata["ok"].(bool)
	if !known {
		ok = !strings.Contains(msgs[last].Content, `"ok":false`) && !strings.Contains(msgs[last].Content, `"ok": false`)
	}
	// a call that never ran as such (a duplicate id, arguments that were not JSON, one skipped for a budget) is answered by the loop
	// itself with what to do next; only a call that ran and failed, or was refused, is a result the answer must not hide
	kind, _ := msgs[last].Metadata["error_kind"].(string)
	if kind == "" {
		if m := regexp.MustCompile(`"error_kind":\s*"([a-z_]+)"`).FindStringSubmatch(msgs[last].Content); m != nil {
			kind = m[1]
		}
	}
	switch kind {
	case "duplicate_call", "invalid_arguments", "not_run":
		return false
	}
	return !ok && !failureWordRE.MatchString(answer)
}

const toolErrorIgnoredText = "Check before you finish: your last tool call failed, but your answer does not say that anything failed. Do not answer as if it had worked. Write your whole answer again, complete, saying plainly what failed and what you could not find out because of it; keep whatever earlier tools really showed. Do not talk about this check."

var (
	codeFileRE   = regexp.MustCompile(`(?i)\.(go|py|rs|sh|bash|js|mjs|ts|c|h|cpp|java|lua)$`)
	worksClaimRE = regexp.MustCompile(`(?i)\b(compiles?|compiled|builds?( successfully| fine| cleanly)?|built successfully|(all )?tests? (now )?(pass|passed|passes|succeed|succeeded|are passing|run successfully)|passes (all )?(the )?tests|works( as expected| correctly| fine)?|is working|runs? (correctly|fine|successfully)|verified (that )?it works|no errors|without errors|error-free)\b`)
)

// codeWorksUnverified reports an answer claiming code compiles, builds, passes tests or works when the file tools changed code in this
// task and the verification that runs after each change did not pass (it failed, or the claim names tests but only a build ran, or it was
// skipped). The model may say what it wrote and what the check said; it may not vouch for code nothing checked.
func codeWorksUnverified(answer string, history []map[string]any) string {
	changed, verifiedOK, testsOK := false, false, false
	var failed string
	for _, e := range history {
		name, _ := e["tool_name"].(string)
		ok, _ := e["ok"].(bool)
		p, _ := e["path"].(string)
		if !fileWriteTools[name] || !ok || !codeFileRE.MatchString(p) {
			continue
		}
		changed = true
		switch v, _ := e["verify"].(string); v {
		case "ok":
			verifiedOK = true
			if t, _ := e["verify_tests"].(bool); t {
				testsOK = true
			}
		case "failed":
			failed = filepath.Base(p)
			verifiedOK = false
		}
	}
	if !changed {
		return ""
	}
	for _, s := range sentenceEnd.Split(answer, -1) {
		if negationRE.MatchString(s) || !worksClaimRE.MatchString(s) {
			continue
		}
		claimsTests := regexp.MustCompile(`(?i)\btests?\b`).MatchString(s)
		switch {
		case failed != "":
			return "the check after changing " + failed + " failed"
		case !verifiedOK:
			return "no build or test check ran on what you changed"
		case claimsTests && !testsOK:
			return "no tests were run (only a build check), so you cannot say tests pass"
		}
	}
	return ""
}

func codeWorksText(why string) string {
	return "Check before you finish: your answer says the code works, compiles or passes tests, but " + why + ". Say only what was really checked: what the check after your change reported (quote its result), and what was not checked. Write your whole answer again, complete, without vouching for anything unchecked. Do not talk about this check."
}

var cannotReadRE = regexp.MustCompile(`(?i)\b(cannot|can'?t|can ?not|unable to|do not have|don'?t have|no) (directly )?(access|read|open|see|view)\b|\bno access to\b|\b(is not|isn'?t|does not|doesn'?t|not) (present|exist|available)\b|\b(not found|missing)\b`)
var promptFileRE = regexp.MustCompile(`[\w./-]*[\w-]\.[A-Za-z0-9]{1,8}\b`)

// deniedReadable returns a file the question names that exists in the workspace when the answer says she cannot access or read it and no
// tool was called in this task: the false "I can't" a small model gives when the question sounds sensitive ("do not print any secret
// values" made her refuse to open config.env at all, eval secret-in-file 2026-10-26). Secrets are blanked from her answer anyway.
func deniedReadable(answer, prompt, cwd string, msgs []session.Message, canRead bool) string {
	if !canRead || cwd == "" || !cannotReadRE.MatchString(answer) {
		return ""
	}
	for _, m := range msgs {
		if m.Role == "tool" {
			return ""
		}
	}
	for _, f := range promptFileRE.FindAllString(prompt, -1) {
		p := f
		if !filepath.IsAbs(p) {
			p = filepath.Join(cwd, f)
		}
		if st, err := os.Stat(p); err == nil && st.Mode().IsRegular() {
			return f
		}
	}
	return ""
}

func deniedReadableText(f string) string {
	return "You said you cannot access " + f + " or that it is not there, but it is in your workspace and read_file can open it. Read it now and answer the " +
		"question; any secret values are blanked from your answer automatically, so you need not refuse to look."
}

var changeAskRE = regexp.MustCompile(`(?i)\b(fix|change|add|implement|update|modify|correct|rename|refactor|remove|replace)\b`)
var adviceAskRE = regexp.MustCompile(`(?i)^\s*(how|what|why|which|where|when|can|could|would|should|is|are|do|does|explain|tell me|show me)\b`)
var describedChangeRE = regexp.MustCompile("(?is)```|\\b(change|replace)\\b.{1,80}\\b(to|with)\\b|\\bthis will (make|fix)\\b|\\bthe fix\\b")

// fixDescribedNotMade reports an answer that describes a code change the user told her to make, when she could write files and changed
// none: "Change return a - b to return a + b. This will make the test pass." with calc.go untouched (eval code-fix-failing-test,
// 2026-10-26). Advice questions ("how would I fix…") are left alone.
func fixDescribedNotMade(prompt, answer string, canWrite bool, history []map[string]any) bool {
	if !canWrite || len(history) > 0 || adviceAskRE.MatchString(prompt) || !changeAskRE.MatchString(prompt) {
		return false
	}
	return describedChangeRE.MatchString(answer)
}

const fixDescribedText = "Check before you finish: you were asked to make this change, and you only described it; no file was changed. Make the change " +
	"now with edit_file (read the file first if you have not), then say what you changed and what the build check after it said. Do not " +
	"say the code works unless that check passed."

// toolEvidence is a tool result as text to compare an answer against: the stored JSON as is, and every string inside it unescaped,
// since a file's "quotes" and line breaks are \" and \n in the JSON (eval find-function, 2026-10-26: a correct quote of the file was
// sent back as unsupported, and the rewrite made up a function).
func toolEvidence(content string) string {
	return strings.Join(toolTexts(content), "\n")
}

var (
	fenceRE      = regexp.MustCompile("(?s)```[\\w+-]*\n(.*?)```")
	quoteAskRE   = regexp.MustCompile(`(?i)\bquote\b|\bwhich (line|function|method)\b|\bshow (me )?the (line|code|function)\b`)
	quoteFrameRE = regexp.MustCompile(`(?i)\b(line|lines|code|function|snippet|excerpt)s? (from|in)\b|\bthe relevant lines?\b`)
	omittedRE    = regexp.MustCompile(`\[\.\.\. lines \d+-\d+ of \d+ omitted[^\]]*\]`)
)

// inventedCode reports a line of a code block the answer presents as quoted from a file, when the task's tools never showed that
// line; omitted is the read's note of lines it left out, if any (eval find-function, 2026-10-26: the read left out lines 214-604, the
// function was at 515, and she "quoted" a function she made up). Code she wrote herself in this task is not checked.
func inventedCode(answer, prompt string, msgs []session.Message) (line, omitted string) {
	blocks := fenceRE.FindAllStringSubmatch(answer, -1)
	if len(blocks) == 0 || !(quoteAskRE.MatchString(prompt) || quoteFrameRE.MatchString(answer)) {
		return "", ""
	}
	var ev strings.Builder
	used := false
	for _, m := range msgs {
		for _, c := range m.ToolCalls {
			if c.Function.Name == "write_file" || c.Function.Name == "edit_file" {
				return "", ""
			}
		}
		if m.Role == "tool" {
			used = true
			ev.WriteString(toolEvidence(m.Content))
			ev.WriteByte('\n')
		}
	}
	if !used {
		return "", ""
	}
	evidence := squash(ev.String())
	var unseen []string
	total := 0
	for _, b := range blocks {
		for _, l := range strings.Split(b[1], "\n") {
			l = squash(l)
			if len(strings.ReplaceAll(l, " ", "")) < 10 {
				continue
			}
			total++
			if !strings.Contains(evidence, l) {
				unseen = append(unseen, l)
			}
		}
	}
	if len(unseen) < 2 || len(unseen)*2 < total {
		return "", ""
	}
	return unseen[0], omittedRE.FindString(ev.String())
}

func inventedCodeText(line, omitted string) string {
	t := "Check before you finish: your answer quotes code as if it were in the file, but the tools never showed it (for example: " + line + "). Do not quote code you have not seen."
	if omitted != "" {
		t += " The file was not read in full: " + omitted + ". Read those lines (read_file with start_line and end_line) or search for the name with grep_search, then quote the real lines."
	} else {
		t += " Look the code up with your tools, then quote the real lines, or say plainly that you have not found them."
	}
	return t + " Write your whole answer again. Do not talk about this check."
}

var (
	decompiledRE    = regexp.MustCompile(`(?i)\bdecompil(ed|ing|ation)\b`)
	notDecompiledRE = regexp.MustCompile(`(?i)\b(not|never|without|no|before|until|would need|need to|should|could|can|will|let me|i'll)\b[^.]{0,30}\bdecompil`)
)

// decompileClaimed is an answer that says code was decompiled when no decompile ran in this task (eval reverse-explain, 2026-10-26:
// "determined by decompiling the function" after an overview only, with a made-up rule).
func decompileClaimed(answer string, msgs []session.Message) bool {
	if !decompiledRE.MatchString(answer) || notDecompiledRE.MatchString(answer) {
		return false
	}
	used := false
	for _, m := range msgs {
		for _, c := range m.ToolCalls {
			if c.Function.Name == "reverse_engineer" {
				used = true
				if strings.Contains(strings.ReplaceAll(c.Function.Arguments, " ", ""), `"command":"decompile"`) {
					return false
				}
			}
		}
	}
	return used
}

const decompileClaimedText = "Check before you finish: your answer says code was decompiled, but no decompile ran in this task; the overview does not show any logic. Decompile the function now (reverse_engineer, command decompile, target its name) and explain the rule from its code. Write your whole answer again. Do not talk about this check."

var (
	announceRE  = regexp.MustCompile(`(?i)^(ok(ay)?,? |now,? |first,? |next,? |so,? )*(let me|let's|i'?ll|i will|i'?m going to|i am going to|i need to)\s+(first |now |then |start by |begin by |go ahead and |quickly )*(check|read|look|examine|inspect|list|open|run|search|find|investigate|verify|see|fix|make|apply|edit|update|start|begin|review|take|test|try|create|write)\b`)
	sentencesRE = regexp.MustCompile(`[.!?:]\s+`)
)

// announcedNotDone is a reply that ends by saying what she will do next ("Let me start by listing the files:"), with no tool call: the
// reply ends the task, so nothing is done (eval code-fix-failing-test, 2026-10-26: two runs of three stopped like this).
func announcedNotDone(answer string) bool {
	a := strings.TrimSpace(answer)
	if a == "" || len(strings.Fields(a)) > 150 || strings.Contains(a, "```") {
		return false
	}
	lines := strings.Split(a, "\n")
	last := strings.TrimSpace(lines[len(lines)-1])
	parts := sentencesRE.Split(strings.TrimRight(last, ".!?: "), -1)
	s := strings.TrimSpace(parts[len(parts)-1])
	if strings.HasSuffix(last, ":") || len(parts) == 1 {
		s = strings.TrimSpace(strings.TrimRight(last, ":"))
		if p := sentencesRE.Split(s, -1); len(p) > 0 {
			s = strings.TrimSpace(p[len(p)-1])
		}
	}
	s = strings.TrimLeft(s, "*-#0123456789. ")
	return announceRE.MatchString(s) && !strings.Contains(strings.ToLower(s), "know")
}

const announcedText = "Carry on: you said what you would do next and then stopped, so nothing was done. Do it now with your tools, without announcing it first; answer when the task is finished."
