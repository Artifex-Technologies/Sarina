package tools

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

var (
	secretCredentials = regexp.MustCompile(`^credentials(\.(json|ya?ml))?$`)
	secretSecrets     = regexp.MustCompile(`^secrets?(\.(json|ya?ml|toml))?$`)
	secretSSHKey      = regexp.MustCompile(`^id_(rsa|ed25519|ecdsa|dsa)(\.pub)?$`)
)

// secretName reports whether a single file name looks like it holds
// credentials. name is one path segment.
func secretName(name string) bool {
	b := strings.ToLower(name)
	// Windows quietly ignores trailing dots/spaces and addresses alternate data
	// streams as "name:stream", so ".env. " and ".env::$DATA" are the same file.
	b = strings.TrimRight(b, ". ")
	if i := strings.Index(b, ":"); i > 0 {
		b = strings.TrimRight(b[:i], ". ")
	}
	switch {
	case b == "":
		return false
	case b == ".env" || strings.HasPrefix(b, ".env."), b == ".envrc":
		return true
	case strings.HasSuffix(b, ".pem"), strings.HasSuffix(b, ".key"), strings.HasSuffix(b, ".p12"),
		strings.HasSuffix(b, ".pfx"), strings.HasSuffix(b, ".ppk"):
		return true
	case secretSSHKey.MatchString(b), secretCredentials.MatchString(b), secretSecrets.MatchString(b):
		return true
	case b == ".netrc", b == "_netrc", b == ".git-credentials", b == ".pgpass":
		return true
	}
	return false
}

// IsSecretPath reports whether p names a secret-bearing file: .env and
// .env.* (including .env.example), *.pem and *.key, default SSH identities,
// credentials/secrets files, .aws/credentials and .netrc. Matching is
// case-insensitive and works on either path separator, so it behaves the same
// on Windows and Linux. It looks only at the name; callers check both the path
// the model gave and the symlink-resolved path.
func IsSecretPath(p string) bool {
	p = strings.ReplaceAll(p, `\`, "/")
	segs := strings.Split(p, "/")
	return secretName(segs[len(segs)-1])
}

// secretGuard refuses access to secret-bearing files. given is the path as the
// model wrote it, resolved the fully resolved one: a harmless-looking name that
// is a symlink to a .env is caught through resolved, and a link named .env
// that points at something innocent is still refused through given. A hard
// link to a secret file in the same directory is caught by identity.
func secretGuard(given, resolved, verb string) error {
	if IsSecretPath(given) || IsSecretPath(resolved) || hardLinkedToSecret(resolved) {
		return fmt.Errorf("refusing to %s %s: it looks like a credentials file (.env, key, certificate or similar). "+
			"File tools never put secret files into the conversation or change them. "+
			"If the operator really wants this, they can do it themselves or tell you to use bash explicitly", verb, given)
	}
	return nil
}

func hardLinkedToSecret(resolved string) bool {
	st, err := os.Stat(resolved)
	if err != nil || !st.Mode().IsRegular() {
		return false
	}
	entries, err := os.ReadDir(filepath.Dir(resolved))
	if err != nil {
		return false
	}
	for _, e := range entries {
		if !secretName(e.Name()) {
			continue
		}
		other, err := os.Stat(filepath.Join(filepath.Dir(resolved), e.Name()))
		if err == nil && os.SameFile(st, other) {
			return true
		}
	}
	return false
}
