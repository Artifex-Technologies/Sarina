package llm

import (
	"context"
	"fmt"
	"net/http"
	"testing"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityDiscover records the context-window discovery (testdata/parity/discover.json): serverRoot on base URLs of
// every shape, sameModel, and ContextWindow against scripted servers (the probes in order, what each answer gives, the
// 4 MiB bound, a transport failure ending the round, the cache and its TTL, a cancelled caller not cached).
func TestParityDiscover(t *testing.T) {
	if testing.Short() {
		t.Skip("timed cases")
	}
	var cases []parity.Case
	for _, b := range []string{"http://h:1/v1", "http://h:1/v1/", "http://h:1/", "http://h:1", "HTTP://h:1/v1", "http://u:p@h:1/x/v1/y?q=1#f",
		"http://h:1/a%20b/v1", "http://h:1/a%20b", "http://h:1/a%41b", "http://h:1/a%41b/v1/c", "http://h:1/v1beta", "h:1/v1", "localhost:11434", "",
		"/v1", "http://[::1]:11434/v1", "http://h:1/V1", "http://h:x/v1", "http://h:1/%zz/v1", "http://h:1/a b/v1", "http://h:1/é/v1",
		"http://h:1/a!b(c)*/v1", "http://h:1/x?y/v1", "https://api.openai.com/v1", "http://h:1//v1", "http://h:1/api/v1/v1", "://x",
		"http://h:1/a\x7f/v1", "http://u@h/v1", "http://u%zz@h/v1", "http://h:/v1", "http://:1/v1"} {
		cases = append(cases, parity.Case{Fn: "root", In: parity.Text(b), Out: parity.Text(serverRoot(b))})
	}
	for _, p := range [][2]string{{"a", "A"}, {"llama3", "llama3:latest"}, {" x ", "X"}, {"", ""}, {"a:latest", "a:LATEST"}, {"a", "b"},
		{"İ", "i̇"}, {"a:latest:latest", "a:latest"}} {
		cases = append(cases, parity.Case{Fn: "same", In: p[:], Out: sameModel(p[0], p[1])})
	}

	type dcase struct {
		name, model, path string
		script            [][]S
		timeoutMs         int
		calls             []int // minutes after the first call
		cancelMs          int   // the first call's caller cancels after this long
	}
	ps := func(name string, n any) []S {
		return okJSON(fmt.Sprintf(`{"models":[{"name":"other","context_length":999},{"name":%q,"model":"x","context_length":%v}]}`, name, n))
	}
	nf := statusJSON(404, "404 page not found")
	// a whole answer padded with spaces to just past and exactly at the 4 MiB bound
	psBody := `{"models":[{"name":"qwen3:8b","context_length":4096}]}`
	big := []S{{Send: fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n%s", 4<<20+1, psBody)}, {Pad: 4<<20 + 1 - len(psBody)}}
	exact := []S{{Send: fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n%s", 4<<20, psBody)}, {Pad: 4<<20 - len(psBody)}}
	junk := []S{{Send: fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Length: %d\r\n\r\n", 4<<20)}, {Fill: 4 << 20}}
	show := func(params string, info string) []S {
		return okJSON(fmt.Sprintf(`{"parameters":%q,"model_info":%s}`, params, info))
	}
	for _, tc := range []dcase{
		{name: "psMatch", script: [][]S{ps("qwen3:8b", 40960)}},
		{name: "psLatest", model: "Llama3", script: [][]S{ps("llama3:latest", 8192)}},
		{name: "psModelField", script: [][]S{okJSON(`{"models":[{"name":"a","model":"QWEN3:8b ","context_length":4096}]}`)}},
		{name: "psImplausible", script: [][]S{ps("qwen3:8b", 100), show("num_ctx 16384", "{}")}},
		{name: "psTooBig", script: [][]S{ps("qwen3:8b", 100000001), nf, nf, nf}},
		{name: "psBadType", script: [][]S{ps("qwen3:8b", `"4096"`), nf, nf, nf}},
		{name: "psFloat", script: [][]S{ps("qwen3:8b", "4096.0"), nf, nf, nf}},
		{name: "psCaseKeys", script: [][]S{okJSON(`{"MODELS":[{"NAME":"qwen3:8b","Context_Length":2048}]}`)}},
		{name: "psNull", script: [][]S{okJSON(`null`), nf, nf, nf}},
		{name: "showNumCtx", script: [][]S{nf, show("temperature 0.7\n  num_ctx    32768\nstop x", "{}")}},
		{name: "showNumCtxFirst", script: [][]S{nf, show("num_ctx 100\nnum_ctx 8192", `{"a.context_length":4096}`)}},
		{name: "showNumCtxHuge", script: [][]S{nf, show("num_ctx 99999999999999999999", `{"qwen3.context_length":40960}`)}},
		{name: "showNumCtxInline", script: [][]S{nf, show("stop num_ctx 9000", `{"general.architecture":"qwen3","qwen3.context_length":131072}`)}},
		{name: "showInfoString", script: [][]S{nf, show("", `{"x.context_length":"8192"}`)}},
		{name: "showInfoBool", script: [][]S{nf, show("", `{"x.context_length":true}`), nf, nf}},
		{name: "showBadParams", script: [][]S{nf, okJSON(`{"parameters":5,"model_info":{"x.context_length":4096}}`), nf, nf}},
		{name: "propsNested", script: [][]S{nf, nf, okJSON(`{"default_generation_settings":{"n_ctx":4096},"n_ctx":2048}`)}},
		{name: "propsTop", script: [][]S{nf, nf, okJSON(`{"default_generation_settings":5,"n_ctx":"16384"}`)}},
		{name: "propsBool", script: [][]S{nf, nf, okJSON(`{"n_ctx":true}`), nf}},
		{name: "modelsByID", script: [][]S{nf, nf, nf, okJSON(`{"data":[{"id":"other","max_model_len":1000},{"id":"QWEN3:8B","context_length":32768}]}`)}},
		{name: "modelsSingle", script: [][]S{nf, nf, nf, okJSON(`{"data":[{"id":"x","meta":{"n_ctx_train":8192}}]}`)}},
		{name: "modelsTopProvider", script: [][]S{nf, nf, nf, okJSON(`{"data":[{"id":"qwen3:8b","max_model_len":true,"n_ctx":"x","top_provider":{"context_length":200000}}]}`)}},
		{name: "modelsTwoNoMatch", script: [][]S{nf, nf, nf, okJSON(`{"data":[{"id":"a","n_ctx":4096},{"id":"b","n_ctx":4096}]}`)}},
		{name: "modelsBadData", script: [][]S{nf, nf, nf, okJSON(`{"data":[5]}`)}},
		{name: "modelsNullEntry", script: [][]S{nf, nf, nf, okJSON(`{"data":[null]}`)}},
		{name: "transportAbort", script: [][]S{{}}},
		{name: "abortAtShow", script: [][]S{nf, {}}},
		{name: "bigAnswer", script: [][]S{big, show("num_ctx 8192", "{}")}},
		{name: "exactly4MiB", script: [][]S{exact, show("num_ctx 8192", "{}")}},
		{name: "junk4MiB", script: [][]S{junk, show("num_ctx 8192", "{}")}},
		{name: "pathV1", path: "/v1/", script: [][]S{nf, nf, nf, okJSON(`{"data":[{"id":"qwen3:8b","n_ctx":4096}]}`)}},
		{name: "pathAPI", path: "/api/v1beta/", script: [][]S{nf, nf, nf, okJSON(`{"data":[{"id":"qwen3:8b","n_ctx":4096}]}`)}},
		{name: "timeoutCached", script: [][]S{{{Hold: true}}}, timeoutMs: 300, calls: []int{0, 1}},
		{name: "cacheTTL", script: [][]S{ps("qwen3:8b", 8192), ps("qwen3:8b", 16384)}, calls: []int{0, 1, 4, 6, 7}},
		{name: "cancelledNotCached", script: [][]S{{{Hold: true}}, ps("qwen3:8b", 8192)}, cancelMs: 200, calls: []int{0, 1}},
	} {
		addr, seen := parity.RawSeq(t, nil, tc.script)
		model := tc.model
		if model == "" {
			model = "qwen3:8b"
		}
		start := time.Date(2026, 11, 12, 8, 0, 0, 0, time.UTC)
		var now time.Time
		d := &Discoverer{HTTPClient: &http.Client{Transport: &http.Transport{DisableKeepAlives: true}},
			Timeout: time.Duration(tc.timeoutMs) * time.Millisecond, Now: func() time.Time { return now }}
		cfg := config.ModelConfig{Model: model, BaseURL: "http://" + addr + tc.path, APIKey: "k"}
		calls := tc.calls
		if calls == nil {
			calls = []int{0}
		}
		var results []any
		for i, m := range calls {
			now = start.Add(time.Duration(m) * time.Minute)
			ctx, cancel := context.Background(), context.CancelFunc(func() {})
			if i == 0 && tc.cancelMs > 0 {
				ctx, cancel = context.WithCancel(ctx)
				time.AfterFunc(time.Duration(tc.cancelMs)*time.Millisecond, cancel)
			}
			n, src := d.ContextWindow(ctx, cfg)
			cancel()
			results = append(results, []any{n, src, len(parity.Settle(seen))})
		}
		in := map[string]any{"name": tc.name, "model": model, "path": tc.path, "script": parity.Steps(tc.script), "timeoutMs": tc.timeoutMs,
			"calls": calls, "cancelMs": tc.cancelMs}
		cases = append(cases, parity.Case{Fn: "window", In: in, Out: map[string]any{"results": results, "requests": seen()}})
	}
	cases = append(cases, parity.Case{Fn: "defaults", Out: []any{int64(DefaultDiscoveryTimeout), int64(DefaultDiscoveryTTL), minPlausibleWindow, maxPlausibleWindow}})
	parity.Fixture(t, "discover", cases)
}
