package embed

import (
	"fmt"
	"math"
	"net"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

func okJSON(body string) string {
	return fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: %d\r\n\r\n%s", len(body), body)
}

func vectorsOut(vs [][]float32) any {
	out := []any{}
	for _, v := range vs {
		var parts []string
		for _, f := range v {
			parts = append(parts, strconv.FormatUint(uint64(math.Float32bits(f)), 16))
		}
		out = append(out, strings.Join(parts, ","))
	}
	return out
}

// TestParityEmbed records Default's settings and Embed (testdata/parity/embed.json): the request it sends to Ollama and
// what it makes of scripted answers from a raw loopback server (vectors as float32 bits, Ollama's errors, counts that do
// not match, damaged answers), and a refused connection. {ADDR} is the server's address.
func TestParityEmbed(t *testing.T) {
	var cases []parity.Case
	for _, env := range [][3]string{{"", "", ""}, {"http://127.0.0.1:11434/", "", ""}, {"127.0.0.1:11434", "", "m"}, {"", "http://h:1/v1/", ""},
		{"", "http://h:1/v1", "x"}, {"", "http://h:1", ""}, {"httpfoo", "", ""}, {"HTTP://X", "", ""}, {"", "/v1", ""}, {"https://o/", "", ""}} {
		t.Setenv("OLLAMA_HOST", env[0])
		t.Setenv("OPENAI_BASE_URL", env[1])
		t.Setenv("SARINA_EMBED_MODEL", env[2])
		e := Default().(ollamaEmbedder)
		cases = append(cases, parity.Case{Fn: "default", In: env[:], Out: []string{e.base, e.model, e.Name()}})
	}
	replies := []string{
		okJSON(`{"model":"m","embeddings":[[0.1,-0.2,3.4028234663852886e38],[1e-46,0,-0]]}`),
		okJSON(`{"embeddings":[[0.1],[0.2]],"error":""}`), okJSON(`{"embeddings":[[1e39],[1]]}`), okJSON(`{"embeddings":[[0.1]]}`),
		okJSON(`{"embeddings":null}`), okJSON(`{"embeddings":[null,[]]}`), okJSON(`{"error":"model \"m\" not found, try pulling it first"}`),
		"HTTP/1.1 404 Not Found\r\nContent-Length: 18\r\n\r\n{\"error\":\"nope\"}  ",
		okJSON(`{"embeddings":[["a"],[1]]}`), okJSON(`{"embeddings":[[1],[2]],"error":5}`), okJSON(`{"embeddings":{"a":1}}`),
		okJSON(`{"embeddings":[[1],[2]]} trailing junk`), okJSON(``), okJSON(`[]`), okJSON(`{"Embeddings":[[1],[2]]}`),
		"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n{\"embeddings\":[[1],[2]]}",
		"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n{\"embeddings\":[[1],",
		"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n{\"embedd\r\n10\r\nings\":[[1],[2]]}\r\n0\r\n\r\n",
		"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n{\"embedd\r\nZZ\r\n",
		"HTTP/1.1 200 OK\r\n\r\n{\"embeddings\":[[1.5],[2.5]]}", "garbage\r\n\r\n", "",
		okJSON(`{"embeddings":[[16777217,0.1000000000000000055511151231257827],[1.401298464324817e-45,7e-46]]}`),
	}
	for i, r := range replies {
		addr, got := parity.RawServer(t, []byte(r), 0, true)
		e := ollamaEmbedder{base: "http://" + addr, model: "nomic-embed-text"}
		vs, err := e.Embed([]string{"first <text> é", ""})
		out := map[string]any{"request": got()}
		if err != nil {
			out["error"] = strings.ReplaceAll(err.Error(), addr, "{ADDR}")
		} else {
			out["vectors"] = vectorsOut(vs)
		}
		cases = append(cases, parity.Case{Fn: "reply", In: i, Out: out})
	}
	cases = append(cases, parity.Case{Fn: "replies", In: parity.Texts(replies)})
	// a small limit: answers just under, at and over it, cut short, chunked
	limited := []string{okJSON(`{"embeddings":[[1],[2]]}`), okJSON(`{"embeddings":[[1],[2]]}xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx`),
		"HTTP/1.1 200 OK\r\n\r\n{\"embeddings\":[[1],[2]]}" + strings.Repeat(" ", 60),
		"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n18\r\n{\"embeddings\":[[1],[2]]}\r\n30\r\n" + strings.Repeat(" ", 48) + "\r\n0\r\n\r\n",
		"HTTP/1.1 200 OK\r\nContent-Length: 90\r\n\r\n{\"embeddings\":[[1],",
		okJSON(`{"embeddings":[[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15],[2]]}`)}
	for _, limit := range []int64{24, 25, 40, 64} {
		for i, r := range limited {
			addr, got := parity.RawServer(t, []byte(r), 0, true)
			e := ollamaEmbedder{base: "http://" + addr, model: "m", limit: limit}
			vs, err := e.Embed([]string{"a", "b"})
			out := map[string]any{"request": got()}
			if err != nil {
				out["error"] = strings.ReplaceAll(err.Error(), addr, "{ADDR}")
			} else {
				out["vectors"] = vectorsOut(vs)
			}
			cases = append(cases, parity.Case{Fn: "limited", In: []any{limit, i}, Out: out})
		}
	}
	cases = append(cases, parity.Case{Fn: "limitedReplies", In: parity.Texts(limited)}, parity.Case{Fn: "maxAnswerBytes", Out: MaxAnswerBytes})
	// a port nobody listens on
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	addr := ln.Addr().String()
	ln.Close()
	_, err := ollamaEmbedder{base: "http://" + addr, model: "m"}.Embed([]string{"x"})
	cases = append(cases, parity.Case{Fn: "refused", Out: strings.ReplaceAll(parity.Err(err), addr, "{ADDR}")})
	parity.Fixture(t, "embed", cases)
}
