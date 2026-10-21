package reverse

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

// maxStage is the largest program that is copied for analysis.
const maxStage = 200 << 20

// stageRoots are the places a program may be copied from: where installed programs and drivers live. Nothing from /etc, other people's homes,
// /proc or /sys, so staging cannot be used to read private files.
func stageRoots() []string {
	r := []string{"/usr/bin", "/usr/sbin", "/usr/lib", "/usr/lib64", "/usr/libexec", "/usr/local/bin", "/usr/local/lib", "/opt"}
	if h, err := os.UserHomeDir(); err == nil {
		r = append(r, filepath.Join(h, ".local", "bin"), filepath.Join(h, ".local", "share", "labyrinth", "apps"))
	}
	return r
}

// Stage copies an installed program (a crashed one, a driver module, a suspicious binary) into the analysis folder as a read-only file, so the
// sandboxed reverse_engineer tool can read it. The original is never touched or run. It returns the copy's path.
func Stage(src string) (string, error) {
	abs, err := filepath.Abs(src)
	if err != nil {
		return "", err
	}
	real, err := filepath.EvalSymlinks(abs)
	if err != nil {
		return "", fmt.Errorf("%s does not exist", src)
	}
	ok := false
	for _, root := range stageRoots() {
		if rr, err := filepath.EvalSymlinks(root); err == nil {
			root = rr
		}
		if rel, err := filepath.Rel(root, real); err == nil && rel != ".." && !strings.HasPrefix(rel, "../") {
			ok = true
			break
		}
	}
	if !ok {
		return "", fmt.Errorf("%s is not an installed program: only files under /usr, /opt and your own program folders can be staged", src)
	}
	fi, err := os.Lstat(real)
	if err != nil || !fi.Mode().IsRegular() {
		return "", fmt.Errorf("%s is not a plain file", src)
	}
	if fi.Size() > maxStage {
		return "", fmt.Errorf("%s is larger than %d MB", src, maxStage>>20)
	}
	in, err := os.Open(real)
	if err != nil {
		return "", err
	}
	defer in.Close()
	dir := filepath.Join(SamplesDir(), "staged")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	tmp, err := os.CreateTemp(dir, ".stage-")
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp.Name())
	h := sha256.New()
	if _, err := io.Copy(io.MultiWriter(tmp, h), in); err != nil {
		tmp.Close()
		return "", err
	}
	if err := tmp.Close(); err != nil {
		return "", err
	}
	dst := filepath.Join(dir, fmt.Sprintf("%s-%s", filepath.Base(real), hex.EncodeToString(h.Sum(nil))[:8]))
	if err := os.Chmod(tmp.Name(), 0o400); err != nil {
		return "", err
	}
	if err := os.Rename(tmp.Name(), dst); err != nil {
		return "", err
	}
	return dst, nil
}
