package agent

import "testing"

func TestUnbackedFileClaim(t *testing.T) {
	wrote := []map[string]any{{"tool_name": "edit_file", "ok": true, "path": "/w/todo.md"}}
	read := []map[string]any{{"tool_name": "read_file", "ok": true, "path": "/w/todo.md"}}
	failed := []map[string]any{{"tool_name": "write_file", "ok": false, "path": "/w/todo.md"}}
	cases := []struct {
		answer string
		hist   []map[string]any
		want   string
	}{
		{"I added 'water the plants' to todo.md.", nil, "todo.md"},
		{"I added 'water the plants' to todo.md.", read, "todo.md"},
		{"I added 'water the plants' to todo.md.", failed, "todo.md"},
		{"I added 'water the plants' to todo.md.", wrote, ""},
		{"notes.txt says the dock is pinned. I did not change notes.txt.", nil, ""},
		{"The summary of notes.txt: Dana finishes the icons by Friday.", nil, ""},
		{"Nothing was changed in config.env.", nil, ""},
	}
	for _, c := range cases {
		if got := unbackedFileClaim(c.answer, c.hist); got != c.want {
			t.Errorf("%q: got %q want %q", c.answer, got, c.want)
		}
	}
}
