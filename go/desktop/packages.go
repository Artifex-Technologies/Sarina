package desktop

import (
	"context"
	"os/exec"
	"sort"
	"strings"
	"time"
	"unicode"
)

var binaryDirs = []string{"/usr/bin/", "/usr/local/bin/"}

const queryTimeout = 120 * time.Second

// InstalledPackage is one installed package reduced to what categorisation uses.
type InstalledPackage struct {
	Name        string
	Description string
	Groups      []string
	Binaries    []string
	// Explicit is false for a dependency; the most effective filter on menu noise.
	Explicit bool
}

// PrimaryBinary is the executable most likely to be the package's command: an
// exact name match, else the shortest binary (ties alphabetical).
func (p InstalledPackage) PrimaryBinary() string {
	if len(p.Binaries) == 0 {
		return ""
	}
	best := p.Binaries[0]
	for _, b := range p.Binaries {
		if b == p.Name {
			return b
		}
		if len(b) < len(best) || (len(b) == len(best) && b < best) {
			best = b
		}
	}
	return best
}

var nonApplicationPrefixes = []string{
	"lib", "python-", "python2-", "python313-", "perl-", "ruby-", "php-",
	"nodejs-", "haskell-", "rust-", "lua-", "lua51-", "lua52-", "lua53-",
	"ocaml-", "texlive-", "ttf-", "otf-", "woff-", "woff2-", "font-", "fonts-",
	"noto-fonts", "adobe-source", "qt5-", "qt6-", "kf5-", "kf6-", "gtk2",
	"gtk3", "gtk4", "glibc", "gcc-", "llvm", "clang", "linux-api-headers",
	"xorg-font", "vulkan-", "mesa", "opencl-", "cuda",
}

var nonApplicationSuffixes = []string{
	"-libs", "-lib", "-doc", "-docs", "-man", "-headers", "-dev", "-devel",
	"-dkms", "-keyring", "-mirrorlist", "-fonts", "-font", "-icon-theme",
	"-themes", "-theme", "-wordlist", "-wordlists", "-data", "-common",
	"-i18n", "-l10n", "-locale", "-runtime", "-sdk", "-src", "-static",
}

var nonApplicationNames = map[string]bool{}

func init() {
	for _, n := range strings.Fields(`base base-devel archiso arch-install-scripts filesystem
		pacman pacman-contrib pacman-mirrorlist systemd systemd-libs
		systemd-sysvcompat coreutils util-linux bash sh grep
		sed gawk findutils diffutils which less tar gzip
		bzip2 xz zstd licenses iana-etc tzdata ca-certificates`) {
		nonApplicationNames[n] = true
	}
}

// LooksLikeApplication is a conservative name-convention filter: it rejects
// packages whose names follow Arch conventions for non-applications (libraries,
// fonts, docs, metapackages) and packages with no binaries.
func LooksLikeApplication(p InstalledPackage) bool {
	name := strings.ToLower(p.Name)
	if nonApplicationNames[name] {
		return false
	}
	for _, pre := range nonApplicationPrefixes {
		if strings.HasPrefix(name, pre) {
			return false
		}
	}
	for _, suf := range nonApplicationSuffixes {
		if strings.HasSuffix(name, suf) {
			return false
		}
	}
	return len(p.Binaries) > 0
}

// ParsePacmanInfo parses `pacman -Qi` output (one or many packages), handling
// wrapped continuation lines and the literal "None" for an empty field.
func ParsePacmanInfo(text string) []InstalledPackage {
	var packages []InstalledPackage
	fields := map[string]string{}
	lastKey := ""
	flush := func() {
		if name := strings.TrimSpace(fields["Name"]); name != "" {
			var groups []string
			if raw := strings.TrimSpace(fields["Groups"]); raw != "" && raw != "None" {
				groups = strings.Fields(raw)
			}
			packages = append(packages, InstalledPackage{
				Name:        name,
				Description: strings.TrimSpace(fields["Description"]),
				Groups:      groups,
				Explicit:    strings.Contains(strings.ToLower(fields["Install Reason"]), "explicit"),
			})
		}
		fields = map[string]string{}
		lastKey = ""
	}
	for _, raw := range splitLines(text) {
		if strings.TrimSpace(raw) == "" {
			flush()
			continue
		}
		if r := []rune(raw)[0]; unicode.IsSpace(r) {
			if lastKey != "" {
				fields[lastKey] = strings.TrimSpace(fields[lastKey] + " " + strings.TrimSpace(raw))
			}
			continue
		}
		i := strings.Index(raw, ":")
		if i < 0 {
			continue
		}
		lastKey = strings.TrimSpace(raw[:i])
		fields[lastKey] = strings.TrimSpace(raw[i+1:])
	}
	flush()
	return packages
}

// ParsePacmanFileList parses `pacman -Ql` output into {package: binaries}.
// Only files directly inside a binary directory count.
func ParsePacmanFileList(text string) map[string][]string {
	found := map[string][]string{}
	for _, line := range splitLines(text) {
		i := strings.Index(line, " ")
		if i < 0 {
			continue
		}
		name, path := line[:i], strings.TrimSpace(line[i+1:])
		for _, dir := range binaryDirs {
			if !strings.HasPrefix(path, dir) {
				continue
			}
			if rest := path[len(dir):]; rest != "" && !strings.Contains(rest, "/") {
				found[name] = append(found[name], rest)
			}
			break
		}
	}
	for _, v := range found {
		sort.Strings(v)
	}
	return found
}

// Runner runs a command and returns its stdout; an error means "no answer".
type Runner func(name string, args ...string) (string, error)

// ExecRunner is the real subprocess runner (120s timeout, non-zero exit is an error).
func ExecRunner(name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), queryTimeout)
	defer cancel()
	out, err := exec.CommandContext(ctx, name, args...).Output()
	return string(out), err
}

// QueryInstalledPackages returns every installed package with description,
// groups and binaries using two pacman calls for the whole system. It never
// fails: with pacman missing or failing it returns nil.
func QueryInstalledPackages(pacmanBin string, run Runner) []InstalledPackage {
	if pacmanBin == "" {
		pacmanBin = "pacman"
	}
	if run == nil {
		if _, err := exec.LookPath(pacmanBin); err != nil {
			return nil
		}
		run = ExecRunner
	}
	info, err := run(pacmanBin, "-Qi")
	if err != nil {
		return nil
	}
	packages := ParsePacmanInfo(info)
	if len(packages) == 0 {
		return nil
	}
	var files map[string][]string
	if ql, err := run(pacmanBin, "-Ql"); err == nil {
		files = ParsePacmanFileList(ql)
	}
	for i := range packages {
		packages[i].Binaries = files[packages[i].Name]
	}
	return packages
}
