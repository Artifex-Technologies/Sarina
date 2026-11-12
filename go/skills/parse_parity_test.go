package skills

import (
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

var skillDocs = []string{
	"---\nname: review\ndescription: Review a diff\n---\nReview $ARGUMENTS carefully.", "---\ndescription: d\n---\nbody",
	"\xef\xbb\xbf---\r\nname: x\r\n---\r\nbody\r\n", "--- \t\nname: x\n---  \nbody", "---\nname: x\nbody", "no front matter", "", "---",
	"---\nname: x\n---\n   ", "---\nname: x\n---", "---\nname: x\n# comment\n\n---\nb", "---\nname: x\n  indented: 1\n---\nb",
	"---\nname: x\n\tindented: 1\n---\nb", "---\nname x\n---\nb", "---\n: value\n---\nb", "---\nname: a\nname: b\n---\nb",
	"---\nname: x\nallowed_tools: [read_file, \"grep_search\", 'list_dir', , ]\n---\nb", "---\nname: x\nallowed_tools: []\n---\nb",
	"---\nname: x\nallowed_tools: [ ]\n---\nb", "---\nname: x\nallowed_tools: [a, b\n---\nb", "---\nname: x\nallowed_tools:\n- a\n- \"b\"\n-   'c d'\n-\n---\nb",
	"---\nname: x\nallowed_tools:\n---\nb", "---\nname: x\nallowed_tools: a, b ,, c\n---\nb", "---\nname: x\n- orphan\n---\nb",
	"---\nname: x\nallowed_tools:\n- a\nprefetch:\n- status\n- \"weather\"\n---\nb", "---\nname: x\nprefetch: [status]\n---\nb",
	"---\nname: x\ndescription: {a: 1}\n---\nb", "---\nname: x\ndescription: |\n---\nb", "---\nname: x\ndescription: >folded\n---\nb",
	"---\nname: x\ndescription: &anchor\n---\nb", "---\nname: x\ndescription: *ref\n---\nb", "---\nname: x\ndescription: !tag\n---\nb",
	"---\nname: x\ndescription: \"esc \\\"q\\\" \\n \\t \\u00e9 \\x41 \\101 \\U0001F600\"\n---\nb", "---\nname: x\ndescription: \"bad \\q escape\"\n---\nb",
	"---\nname: x\ndescription: \"a\"b\"\n---\nb", "---\nname: x\ndescription: 'it''s'\n---\nb", "---\nname: x\ndescription: 'a'b'\n---\nb",
	"---\nname: x\ndescription: \"\\xff byte\"\n---\nb", "---\nname: x\ndescription: \"raw \xff byte\"\n---\nb", "---\nname: x\ndescription: \"\\ud800\"\n---\nb",
	"---\nname: x\ndescription: \"\\400\"\n---\nb", "---\nname: x\ndescription: \"\\a\\b\\f\\v\\r\\\\ \\000 \\x7f\"\n---\nb", "---\nname: x\ndescription: \"\\0\"\n---\nb", "---\nname: x\ndescription: \"\\'\"\n---\nb", "---\nname: x\ndescription: \"\\\"\n---\nb",
	"---\nname: x\ndescription: \"\"\n---\nb", "---\nname: x\ndescription: '\n---\nb", "---\nname: x\ndescription: \"\n---\nb",
	"---\nname: x\nuser_invocable: no\nmodel_invocable: YES\n---\nb", "---\nname: x\nuser_invocable: maybe\n---\nb",
	"---\nname: x\nmodel_invocable: 1\n---\nb", "---\nname: x\nuser_invocable:\n---\nb", "---\nname: x\nuser_invocable: \"true\"\n---\nb",
	"---\nname: x\nwhen_to_use: when: asked\n---\nb", "---\nname: \"quoted-name\"\n---\nb", "---\nname: a:b.c_d-e\n---\nb", "---\nname: -dash\n---\nb",
	"---\nname: " + strings.Repeat("n", 64) + "\n---\nb", "---\nname: " + strings.Repeat("n", 65) + "\n---\nb", "---\nname: caf\u00e9\n---\nb",
	"---\nname: \n---\nb", "---\nname: x\nNAME: y\n---\nb", "---\nname: x\nmodel_invocable: false\nuser_invocable: false\n---\nbody $ARGUMENTS and $ARGUMENTS",
	"---\nname: x\n---\nline1\n---\nline2", "---\nname: x\ndescription: d # not a comment\n---\nb", "---\nname: x\n   # indented comment\n---\nb",
}

// TestParitySkillParse records Parse (the skill file format: front matter with scalars, quotes as strconv.Unquote reads them,
// inline, block and comma lists, switches, the name rule), ValidName and Expand (testdata/parity/skillparse.json).
func TestParitySkillParse(t *testing.T) {
	var cases []parity.Case
	for _, d := range skillDocs {
		for _, fb := range []string{"fallback", "bad name", ""} {
			s, err := Parse([]byte(d), fb)
			var out any
			if s != nil {
				var allowed, prefetch any
				if s.AllowedTools != nil {
					allowed = parity.Texts(s.AllowedTools)
				}
				if s.Prefetch != nil {
					prefetch = parity.Texts(s.Prefetch)
				}
				out = map[string]any{"name": parity.Text(s.Name), "description": parity.Text(s.Description), "whenToUse": parity.Text(s.WhenToUse),
					"allowed": allowed, "hasAllowed": s.HasAllowed, "user": s.UserInvocable, "model": s.ModelInvocable, "body": parity.Text(s.Body),
					"prefetch": prefetch, "expanded": parity.Text(s.Expand("ARGS $ARGUMENTS"))}
			}
			cases = append(cases, parity.Case{Fn: "parse", In: []any{parity.Text(d), fb}, Out: []any{out, parity.Text(parity.Err(err))}})
		}
	}
	for _, n := range []string{"a", "A1", "a.b:c_d-e", "-a", ".a", "_a", ":a", "a b", "", strings.Repeat("x", 64), strings.Repeat("x", 65), "caf\u00e9",
		"a\n", "a\x00", "\u212a", "a/b", "a*"} {
		cases = append(cases, parity.Case{Fn: "validName", In: parity.Text(n), Out: ValidName(n)})
	}
	parity.Fixture(t, "skillparse", cases)
}
