// Package desktop categorises the application menu: it repairs .desktop entries
// that carry no registered Main Category and proposes entries for installed
// tools that ship none. Everything is a proposal until the caller applies it.
//
// Signals, cheapest and most reliable first: pacman group, description keyword
// rules, then an optional semantic classifier (behind the Scorer interface).
package desktop

import "strings"

// MainCategories are the 13 registered freedesktop Main Categories.
var MainCategories = map[string]bool{
	"AudioVideo": true, "Audio": true, "Video": true, "Development": true, "Education": true,
	"Game": true, "Graphics": true, "Network": true, "Office": true, "Science": true,
	"Settings": true, "System": true, "Utility": true,
}

// AdditionalCategories are the registered additional categories this package emits.
var AdditionalCategories = map[string]bool{
	"Archiving": true, "ArtificialIntelligence": true, "Calculator": true, "Compression": true,
	"Database": true, "Debugger": true, "Documentation": true, "Electronics": true,
	"Emulator": true, "FileManager": true, "FileTools": true, "HamRadio": true, "IDE": true,
	"ImageProcessing": true, "Monitor": true, "Network": true, "Player": true,
	"Publishing": true, "RemoteAccess": true, "Security": true, "TerminalEmulator": true,
	"TextEditor": true, "Viewer": true, "WebBrowser": true, "WebDevelopment": true,
}

// Environment/toolkit tokens: harmless, but not a menu location.
var environmentTokens = map[string]bool{
	"KDE": true, "GNOME": true, "XFCE": true, "DDE": true, "GTK": true, "Qt": true,
	"Motif": true, "Java": true, "ConsoleOnly": true,
}

func cats(c ...string) []string { return c }

// BlackArchGroupCategories maps the 52 BlackArch pacman groups (including the
// upstream typo "reocn") to spec-valid categories.
var BlackArchGroupCategories = map[string][]string{
	"ai":            cats("Science", "ArtificialIntelligence"),
	"anti-forensic": cats("System", "Security"),
	"automation":    cats("Utility", "Security"),
	"automobile":    cats("System", "Security"),
	"backdoor":      cats("Network", "Security"),
	"binary":        cats("Development", "Security"),
	"bluetooth":     cats("Network", "Security"),
	"code-audit":    cats("Development", "Security"),
	"config":        cats("Settings", "Security"),
	"cracker":       cats("System", "Security"),
	"crypto":        cats("Utility", "Security"),
	"database":      cats("Development", "Database"),
	"debugger":      cats("Development", "Debugger"),
	"decompiler":    cats("Development", "Security"),
	"defensive":     cats("System", "Security"),
	"disassembler":  cats("Development", "Security"),
	"dos":           cats("Network", "Security"),
	"drone":         cats("System", "Security"),
	"exploitation":  cats("System", "Security"),
	"fingerprint":   cats("Network", "Security"),
	"firmware":      cats("System", "Security"),
	"forensic":      cats("System", "Security"),
	"fuzzer":        cats("Development", "Security"),
	"gpu":           cats("System", "Security"),
	"hardware":      cats("System", "Security"),
	"honeypot":      cats("Network", "Security"),
	"ids":           cats("Network", "Security"),
	"keylogger":     cats("System", "Security"),
	"malware":       cats("System", "Security"),
	"misc":          cats("Utility", "Security"),
	"mobile":        cats("System", "Security"),
	"networking":    cats("Network", "Security"),
	"nfc":           cats("Network", "Security"),
	"packer":        cats("Development", "Security"),
	"proxy":         cats("Network", "Security"),
	"radio":         cats("Network", "Security"),
	"recon":         cats("Network", "Security"),
	"reocn":         cats("Network", "Security"),
	"reversing":     cats("Development", "Security"),
	"scanner":       cats("Network", "Security"),
	"sniffer":       cats("Network", "Security"),
	"social":        cats("Network", "Security"),
	"spoof":         cats("Network", "Security"),
	"stego":         cats("Utility", "Security"),
	"threat-model":  cats("System", "Security"),
	"tunnel":        cats("Network", "Security"),
	"unpacker":      cats("Development", "Security"),
	"voip":          cats("Network", "Security"),
	"webapp":        cats("Network", "Security"),
	"windows":       cats("System", "Security"),
	"wireless":      cats("Network", "Security"),
	"wordlist":      cats("Utility", "Security"),
}

// DescriptionRule is one keyword rule; the first matching rule wins.
type DescriptionRule struct {
	Keywords   []string
	Categories []string
}

func rule(k []string, c ...string) DescriptionRule { return DescriptionRule{k, c} }

// DescriptionRules, most specific first. Wireless is checked before credential
// attacks on purpose (aircrack-ng is a network tool); security rules sit late so
// a plain "network scanner" is not claimed by the generic Network rule.
var DescriptionRules = []DescriptionRule{
	rule([]string{"terminal emulator"}, "System", "TerminalEmulator"),
	rule([]string{"file manager"}, "System", "FileManager"),
	rule([]string{"text editor"}, "Utility", "TextEditor"),
	rule([]string{"web browser"}, "Network", "WebBrowser"),
	rule([]string{"system monitor", "process monitor"}, "System", "Monitor"),
	rule([]string{"calculator"}, "Utility", "Calculator"),
	rule([]string{"archive", "compression", "compressor"}, "Utility", "Archiving"),
	rule([]string{"emulator"}, "System", "Emulator"),
	rule([]string{"wireless", "802.11", "wifi", "wi-fi", "wpa", "bluetooth"}, "Network", "Security"),
	rule([]string{"password crack", "password recovery", "password cracker", "logon",
		"login cracker", "cracker", "brute force", "bruteforce", "hash crack"}, "System", "Security"),
	rule([]string{"sql injection", "web server scanner", "web application",
		"web vulnerability"}, "Network", "Security"),
	rule([]string{"exploit", "penetration test", "pentest", "vulnerability",
		"payload"}, "System", "Security"),
	rule([]string{"packet", "sniffer", "sniffing", "network attack", "network discovery",
		"port scan", "network scanner", "spoof", "man-in-the-middle"}, "Network", "Security"),
	rule([]string{"forensic", "malware", "rootkit", "antivirus", "steganograph"}, "System", "Security"),
	rule([]string{"disassem", "decompil", "reverse engineer", "binary analysis",
		"binary image", "binary file", "firmware", "embedded file",
		"debugger", "fuzzing", "fuzzer"}, "Development", "Security"),
	rule([]string{"encrypt", "decrypt", "cryptograph", "cipher"}, "Utility", "Security"),
	rule([]string{"image editor", "image processing", "photo", "graphics", "vector"}, "Graphics"),
	rule([]string{"video player", "media player", "video"}, "AudioVideo", "Video"),
	rule([]string{"audio", "music", "sound"}, "AudioVideo", "Audio"),
	rule([]string{"spreadsheet", "word processor", "presentation"}, "Office"),
	rule([]string{"compiler", "programming language", "sdk", "build system",
		"version control", "source code"}, "Development"),
	rule([]string{"documentation", "manual page"}, "Utility", "Documentation"),
	rule([]string{"network", "http", "dns", "tcp", "ftp", "ssh"}, "Network"),
	rule([]string{"filesystem", "disk", "partition"}, "System", "FileTools"),
}

// HasMainCategory reports whether any token is a registered Main Category.
func HasMainCategory(categories []string) bool {
	for _, t := range categories {
		if MainCategories[t] {
			return true
		}
	}
	return false
}

// NonstandardTokens returns tokens that are neither registered nor a known
// environment hint (X-BlackArch-* and friends strand an entry).
func NonstandardTokens(categories []string) []string {
	var out []string
	for _, t := range categories {
		if !MainCategories[t] && !AdditionalCategories[t] && !environmentTokens[t] {
			out = append(out, t)
		}
	}
	return out
}

// CategoriesFromGroup maps a pacman group (blackarch-scanner) or the desktop
// token (X-BlackArch-Scanner) to categories, or nil if unrecognised.
func CategoriesFromGroup(group string) []string {
	key := strings.ToLower(strings.TrimSpace(group))
	for _, prefix := range []string{"x-blackarch-", "blackarch-"} {
		if strings.HasPrefix(key, prefix) {
			return BlackArchGroupCategories[key[len(prefix):]]
		}
	}
	return nil
}

// CategoriesFromDescription returns the first matching description rule, or nil.
func CategoriesFromDescription(description string) []string {
	text := strings.ToLower(description)
	for _, r := range DescriptionRules {
		for _, k := range r.Keywords {
			if strings.Contains(text, k) {
				return r.Categories
			}
		}
	}
	return nil
}
