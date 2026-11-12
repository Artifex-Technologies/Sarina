package session

import (
	"encoding/json"
	"fmt"
	"hash/fnv"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

func fixedDoc(t *testing.T, s *Session, id string) string {
	s.ID = id
	s.Runtime = config.DefaultRuntime("/nonexistent-sarina-parity/ws")
	s.ScratchpadDirectory = "/nonexistent-sarina-parity/ws/scratch"
	b, err := s.Marshal()
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

// cleanList is xs with the folder written as {DIR}.
func cleanList(xs []string, dir string) []string {
	var out []string
	for _, x := range xs {
		out = append(out, strings.ReplaceAll(x, dir, "{DIR}"))
	}
	return out
}

var tempPart = regexp.MustCompile(`\.[0-9]+\.tmp`)

// listing is a folder's files after a run: name, mode, size and a digest of the bytes (links as "-> target").
func listing(dir string) []string {
	var out []string
	filepath.Walk(dir, func(p string, fi os.FileInfo, err error) error {
		if err != nil || p == dir {
			return nil
		}
		rel, _ := filepath.Rel(dir, p)
		switch {
		case fi.Mode()&os.ModeSymlink != 0:
			target, _ := os.Readlink(p)
			out = append(out, rel+" -> "+strings.ReplaceAll(target, dir, "{DIR}"))
		case fi.IsDir():
			out = append(out, rel+"/")
		default:
			b, _ := os.ReadFile(p)
			h := fnv.New64a()
			h.Write(b)
			out = append(out, fmt.Sprintf("%s %o %d %016x", tempPart.ReplaceAllString(rel, ".N.tmp"), fi.Mode().Perm(), len(b), h.Sum64()))
		}
		return nil
	})
	sort.Strings(out)
	return out
}

// TestParitySave records Session.Save (the folder made, the temporary file and the rename, the mode from
// SARINA_SESSION_FILE_MODE, ids that are not file names) and MigrateDir (dry and real runs, backups, files it skips, a
// session id with a slash) on made-up folders (testdata/parity/save.json; {DIR} is the folder).
func TestParitySave(t *testing.T) {
	plain := fixedDoc(t, sample(t), "fixed-session")
	retired := fixedDoc(t, retiredSession(t), "retired-session")
	var cases []parity.Case
	docs := map[string]string{"plain": plain, "retired": retired}
	for _, id := range []string{"a/b", "../escape", "x*y", "dots..", "-", "é", " sp ace", "x.json", ".hidden", "a\\b", "x\x00y", "/abs"} {
		q, _ := json.Marshal(id)
		docs["id:"+id] = strings.Replace(plain, `"session_id": "fixed-session"`, `"session_id": `+string(q), 1)
	}
	names := make([]string, 0, len(docs))
	for k := range docs {
		names = append(names, k)
	}
	sort.Strings(names)
	for _, name := range names {
		for _, mode := range []string{"", "0640", "644", " 0644 ", "0777", "0600"} {
			for _, where := range []string{"new/sub", "exists", "file", "ro"} {
				base := t.TempDir()
				os.Mkdir(filepath.Join(base, "exists"), 0o755)
				os.WriteFile(filepath.Join(base, "file"), nil, 0o644)
				os.Mkdir(filepath.Join(base, "ro"), 0o555)
				t.Setenv("SARINA_SESSION_FILE_MODE", mode)
				s, err := Parse([]byte(docs[name]))
				if err != nil {
					t.Fatalf("%s: %v", name, err)
				}
				p, err := s.Save(filepath.Join(base, where))
				os.Chmod(filepath.Join(base, "ro"), 0o755)
				clean := func(x string) string {
					return tempPart.ReplaceAllString(strings.ReplaceAll(x, base, "{DIR}"), ".N.tmp")
				}
				cases = append(cases, parity.Case{Fn: "save", In: []any{name, mode, where},
					Out: []any{clean(p), clean(parity.Err(err)), listing(base)}})
			}
		}
	}
	docsOut := map[string]any{}
	for k, v := range docs {
		docsOut[k] = parity.Text(v)
	}
	docsOut["retired"], docsOut["plain"] = retired, plain
	cases = append(cases, parity.Case{Fn: "docs", In: docsOut})
	// MigrateDir
	type file struct{ name, body string }
	setups := map[string][]file{
		"mixed": {{"retired-session.json", retired}, {"fixed-session.json", plain}, {"junk.json", "nope"}, {"x.notes.md", "notes"},
			{"sub.json/", ""}, {"link.json", "-> retired-session.json"}, {"empty.json", ""}, {"other-name.json", retired}},
		"backup":  {{"retired-session.json", retired}, {"retired-session.json.bak", "kept"}},
		"slash":   {{"a.json", retired}, {"b.json", strings.Replace(retired, `"session_id": "retired-session"`, `"session_id": "x/y"`, 1)}, {"c.json", retired}},
		"bakdir":  {{"retired-session.json", retired}, {"retired-session.json.bak/", ""}},
		"nothing": {},
		// a folder the run cannot write: every file fails, each is reported, none stops the others
		"readonly": {{"a.json", retired}, {"b.json", retired}, {"c.json", plain}},
	}
	snames := make([]string, 0, len(setups))
	for k := range setups {
		snames = append(snames, k)
	}
	sort.Strings(snames)
	for _, sn := range snames {
		for _, dry := range []bool{true, false} {
			dir := t.TempDir()
			for _, f := range setups[sn] {
				p := filepath.Join(dir, strings.TrimSuffix(f.name, "/"))
				switch {
				case strings.HasSuffix(f.name, "/"):
					os.Mkdir(p, 0o755)
				case strings.HasPrefix(f.body, "-> "):
					os.Symlink(filepath.Join(dir, f.body[3:]), p)
				default:
					os.WriteFile(p, []byte(f.body), 0o600)
				}
			}
			if sn == "readonly" {
				os.Chmod(dir, 0o555)
			}
			var runs []any
			for run := 0; run < 2; run++ {
				r, err := MigrateDir(dir, dry)
				runs = append(runs, []any{r.Scanned, r.Changed, r.Skipped, r.Calls, r.Results, r.Files, strings.ReplaceAll(parity.Err(err), dir, "{DIR}"), listing(dir), cleanList(r.Failed, dir)})
			}
			os.Chmod(dir, 0o755)
			cases = append(cases, parity.Case{Fn: "migrateDir", In: []any{sn, dry}, Out: runs})
		}
	}
	base := t.TempDir()
	os.WriteFile(filepath.Join(base, "file"), nil, 0o644)
	for _, rel := range []string{"missing", "file", "missing/deeper"} {
		r, err := MigrateDir(filepath.Join(base, rel), false)
		cases = append(cases, parity.Case{Fn: "migrateOdd", In: rel, Out: []any{r.Scanned, r.Files, strings.ReplaceAll(parity.Err(err), base, "{DIR}")}})
	}
	parity.Fixture(t, "save", cases)
}
