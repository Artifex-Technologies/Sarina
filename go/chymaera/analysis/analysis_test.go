package analysis

import (
	"reflect"
	"strings"
	"testing"
)

// Capture shapes copied from a real Chymaera_Zero_App build.
func nfc(name, uid string) map[string]any {
	return map[string]any{
		"id": 1, "type": "nfc", "name": name, "size": 107,
		"meta": `{"atqa":"00 44","deviceType":"NTAG215","extra":{},"filetype":"Flipper NFC device","kind":"nfc","sak":"00","uid":"` + uid + `","version":4}`,
	}
}

func subghz(name string) map[string]any {
	return map[string]any{
		"id": 2, "type": "subghz", "name": name, "size": 156,
		"meta": `{"filetype":"Flipper SubGhz RAW File","frequency":"433920000","kind":"subghz","preset":"FuriHalSubGhzPresetOok650Async","protocol":"RAW","rawDurationUs":2100,"rawSampleCount":6,"version":1}`,
	}
}

func TestEmptyAndCounts(t *testing.T) {
	a := Analyze(nil, "")
	if a.Total != 0 || len(a.ByType) != 0 {
		t.Fatalf("%+v", a)
	}
	a = Analyze([]map[string]any{nfc("a", "04 A2"), subghz("g"), nfc("b", "04 A2")}, "")
	if a.Total != 3 || !reflect.DeepEqual(a.ByType, map[string]int{"nfc": 2, "subghz": 1}) {
		t.Fatalf("%+v", a)
	}
}

func TestDuplicateUIDs(t *testing.T) {
	a := Analyze([]map[string]any{nfc("badge1.nfc", "AA BB"), nfc("badge2.nfc", "AA BB"), nfc("x.nfc", "CC DD")}, "")
	want := []NameList{{"AA BB", []string{"badge1.nfc", "badge2.nfc"}}}
	if !reflect.DeepEqual(a.DuplicateUIDs, want) {
		t.Fatalf("%+v", a.DuplicateUIDs)
	}
	if !strings.Contains(a.AsText(), "Duplicate NFC UIDs") || !strings.Contains(a.AsText(), "- AA BB: badge1.nfc, badge2.nfc") {
		t.Fatal(a.AsText())
	}
	if got := Analyze([]map[string]any{nfc("a", "AA"), nfc("b", "CC")}, "").DuplicateUIDs; len(got) != 0 {
		t.Fatalf("distinct uids flagged: %v", got)
	}
}

func TestFrequencyGroupsAndRare(t *testing.T) {
	a := Analyze([]map[string]any{subghz("test_gate.sub")}, "")
	if !reflect.DeepEqual(a.FrequencyGroups, []NameList{{"433920000", []string{"test_gate.sub"}}}) {
		t.Fatalf("%+v", a.FrequencyGroups)
	}
	if !reflect.DeepEqual(a.RareFrequencies, []string{"433920000"}) || !strings.Contains(a.AsText(), "- 433920000 Hz") {
		t.Fatalf("%+v\n%s", a.RareFrequencies, a.AsText())
	}
	if got := Analyze([]map[string]any{subghz("a"), subghz("b")}, "").RareFrequencies; len(got) != 0 {
		t.Fatalf("not rare: %v", got)
	}
}

func TestNumericFrequencyKeepsItsDigits(t *testing.T) {
	c := map[string]any{"type": "subghz", "name": "n", "meta": `{"frequency":433920000}`}
	if got := Analyze([]map[string]any{c}, "").RareFrequencies; !reflect.DeepEqual(got, []string{"433920000"}) {
		t.Fatalf("%v", got)
	}
}

func TestBadMetaDoesNotCrash(t *testing.T) {
	missing := nfc("a", "AA")
	delete(missing, "meta")
	bad := nfc("b", "AA")
	bad["meta"] = "not valid json{{{"
	notObject := nfc("c", "AA")
	notObject["meta"] = `["x"]`
	trailing := nfc("d", "AA")
	trailing["meta"] = `{"uid":"AA"} junk`
	a := Analyze([]map[string]any{missing, bad, notObject, trailing}, "")
	if a.Total != 4 || len(a.DuplicateUIDs) != 0 {
		t.Fatalf("%+v", a)
	}
}

func TestSemanticReasonShownAndTextShape(t *testing.T) {
	a := Analyze([]map[string]any{nfc("a", "AA")}, "needs helper")
	text := a.AsText()
	if !strings.Contains(text, "Semantic grouping unavailable: needs helper") {
		t.Fatal(text)
	}
	if !strings.HasPrefix(text, "# Capture analysis\n\ntotal captures: 1\n  nfc: 1\n") {
		t.Fatal(text)
	}
	if strings.HasSuffix(text, "\n") {
		t.Fatal("trailing newline must be stripped")
	}
}
