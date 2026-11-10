package tools

import "testing"

// tt-code-test (nights 4-6): she wrote test_rule11.py for the owner's rule11.py; her test failed and "Fix this before you
// answer" made her edit rule11.py. Owner's yes 2026-11-10: that case gets "report it" instead.
func TestOwnTestOnOthersCode(t *testing.T) {
	st := &fileState{edits: map[string]int{}, authored: map[string]bool{}, read: map[string]int{}, failedTest: map[string]string{}}
	st.authored["/w/test_rule11.py"] = true
	if !ownTestOnOthersCode(st, "/w/test_rule11.py") {
		t.Fatal("her own test on the owner's code was not recognised")
	}
	if ownTestOnOthersCode(st, "/w/rule11.py") {
		t.Fatal("a code file is not a test")
	}
	if ownTestOnOthersCode(st, "/w/test_other.py") {
		t.Fatal("a test she did not create is not hers")
	}
	st.edits["/w/rule11.py"] = 1
	if ownTestOnOthersCode(st, "/w/test_rule11.py") {
		t.Fatal("after she changed the code herself, the failure is hers to fix")
	}
	st2 := &fileState{edits: map[string]int{}, authored: map[string]bool{"/w/slug.py": true, "/w/test_slug.py": true}}
	if ownTestOnOthersCode(st2, "/w/test_slug.py") {
		t.Fatal("code she wrote herself in this session is hers to fix")
	}
	st3 := &fileState{edits: map[string]int{}, authored: map[string]bool{"/w/text_test.go": true}}
	if !ownTestOnOthersCode(st3, "/w/text_test.go") {
		t.Fatal("a Go test she created was not recognised")
	}
}
