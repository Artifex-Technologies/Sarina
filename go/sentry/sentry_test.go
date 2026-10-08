package sentry

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func fake(t *testing.T, status int, body string, seen *[]map[string]any) string {
	t.Helper()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var m map[string]any
		_ = json.NewDecoder(r.Body).Decode(&m)
		if seen != nil {
			*seen = append(*seen, m)
		}
		w.WriteHeader(status)
		w.Write([]byte(body))
	}))
	t.Cleanup(srv.Close)
	return srv.URL + "/v1/review"
}

func TestUnconfiguredIsNil(t *testing.T) {
	t.Setenv(EnvVar, "")
	if Consult("x", "memory_proposal", "") != nil {
		t.Fatal("want nil")
	}
}

func TestUnreachableIsNil(t *testing.T) {
	t.Setenv(EnvVar, "http://127.0.0.1:1/v1/review")
	if Consult("x", "memory_proposal", "") != nil {
		t.Fatal("want nil")
	}
}

func TestAllowAndDeny(t *testing.T) {
	var seen []map[string]any
	t.Setenv(EnvVar, fake(t, 200, `{"decision":"allow","reasons":[]}`, &seen))
	v := Consult("hello", "memory_proposal", "s1")
	if v == nil || v.Denied {
		t.Fatalf("verdict = %+v", v)
	}
	if seen[0]["kind"] != "memory_proposal" || seen[0]["text"] != "hello" || seen[0]["session_id"] != "s1" {
		t.Fatalf("wire body = %v", seen[0])
	}
	t.Setenv(EnvVar, fake(t, 200, `{"decision":"deny","reasons":["a","b"]}`, nil))
	v = Consult("hello", "memory_proposal", "")
	if v == nil || !v.Denied || len(v.Reasons) != 2 || v.Reasons[0] != "a" {
		t.Fatalf("verdict = %+v", v)
	}
}

func TestMalformedAndServerErrorAreNil(t *testing.T) {
	for _, tc := range []struct {
		status int
		body   string
	}{{200, "not json"}, {200, "[1,2]"}, {500, `{"decision":"deny"}`}} {
		t.Setenv(EnvVar, fake(t, tc.status, tc.body, nil))
		if Consult("x", "k", "") != nil {
			t.Fatalf("%d %q should fail open", tc.status, tc.body)
		}
	}
}
