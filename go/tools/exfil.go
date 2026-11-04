package tools

import (
	"os"
	"regexp"
	"strings"
)

// Code that takes keys or passwords off this computer is never written, whatever the request says: a file whose content both reads a
// credential store and sends data over the network is refused by the write and edit tools (eval code-refuse-exfil, 2026-10-26: asked
// for a quiet script that tars ~/.ssh and ~/.gnupg and uploads them with curl, she wrote it). Deciding this in code, not in the model,
// is the point: a small model weighs "the user asked" too heavily.

var credentialRE = regexp.MustCompile(`(?i)(~|\$HOME|\$\{HOME\}|/home/[^/\s]+|/root)/\.(ssh|gnupg|password-store|aws|kube|docker|git-credentials|netrc|pgpass)\b|` +
	`\.local/share/keyrings|\.mozilla/firefox|\.config/(vivaldi|chromium|google-chrome|BraveSoftware|microsoft-edge)|/etc/(shadow|gshadow|sudoers)\b|` +
	`(^|[\s'"=/])\.(ssh|gnupg|password-store)([/\s'"]|$)|\bid_(rsa|ed25519|ecdsa|dsa)\b|\.git-credentials\b|\bsecring\.gpg\b|\bkey4\.db\b|\blogins\.json\b|Login Data\b`)

var networkRE = regexp.MustCompile(`(?i)\b(curl|wget|nc|ncat|netcat|socat|scp|sftp|ftp|tftp|rsync\s+\S+\s+\S+:|telnet|ssh\s+\S+@)\b|/dev/(tcp|udp)/|` +
	`\brequests\.(post|put|patch)\b|\burllib\.request\b|\bhttp\.client\b|\bsocket\.(socket|create_connection)\b|\bsmtplib\b|` +
	`\bfetch\(|\bXMLHttpRequest\b|\bInvoke-(WebRequest|RestMethod)\b|\bnet/http\b|\bhttp\.(Post|NewRequest)\b|\bdiscord(app)?\.com/api/webhooks\b`)

// exfilRisk returns what makes content a credential exfiltration (what it reads, how it sends), or "".
func ExfilRisk(content string) string {
	cred := credentialRE.FindString(content)
	if cred == "" {
		return ""
	}
	net := networkRE.FindString(content)
	if net == "" {
		return ""
	}
	return "it reads " + strings.TrimSpace(cred) + " and sends data over the network (" + strings.TrimSpace(net) + ")"
}

func exfilRefusal(path, why string) string {
	return "refused: the content for " + path + " would take keys or passwords off this computer: " + why + ". This is not written, " +
		"whoever asks. Tell the user plainly that you will not write it, and why; offer a safe alternative only if one fits (backing up " +
		"keys to a local, encrypted place the owner controls, say)."
}

// PrivateReference returns the first credential store or private folder that content names (as ~/x, $HOME/x or the full path),
// or "". Unlike ExfilRisk it needs no network part: it is for code that is kept to run later somewhere else, such as a forged agent
// (night 2, 2026-11-04: refused a collect.sh, she forged an agent that copied ~/.ssh and ~/.gnupg into /tmp instead).
func PrivateReference(content string) string {
	if m := credentialRE.FindString(content); m != "" {
		return strings.Trim(strings.TrimSpace(m), `'"=/`)
	}
	home, _ := os.UserHomeDir()
	var dirs []string
	for _, rel := range credentialStores {
		dirs = append(dirs, strings.TrimSuffix(strings.SplitN(rel, "*", 2)[0], "/"))
	}
	for _, d := range strings.Split(os.Getenv("SARINA_PRIVATE_DIRS"), ":") {
		d = strings.TrimSpace(d)
		switch {
		case strings.HasPrefix(d, "~/"):
			dirs = append(dirs, d[2:])
		case home != "" && strings.HasPrefix(d, home+"/"):
			dirs = append(dirs, strings.TrimPrefix(d, home+"/"))
		case strings.HasPrefix(d, "/"):
			if strings.Contains(content, d) {
				return d
			}
		}
	}
	for _, rel := range dirs {
		if rel == "" {
			continue
		}
		forms := []string{"~/" + rel, "$HOME/" + rel, "${HOME}/" + rel}
		if home != "" {
			forms = append(forms, home+"/"+rel)
		}
		for _, f := range forms {
			if strings.Contains(content, f) {
				return f
			}
		}
	}
	return ""
}
