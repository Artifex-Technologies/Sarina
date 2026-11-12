package vecindex

import (
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"hash/fnv"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

func digest(b []byte) string {
	h := fnv.New64a()
	h.Write(b)
	return fmt.Sprintf("%016x/%d", h.Sum64(), len(b))
}

func f32s(v []float32) string {
	b := make([]byte, 4*len(v))
	for i, f := range v {
		binary.LittleEndian.PutUint32(b[4*i:], math.Float32bits(f))
	}
	return hex.EncodeToString(b)
}

// vecs are made-up vectors from Go's generator (the Rust side makes the same ones with its port of it).
func vecs(rng *rand.Rand, n, dim int) [][]float32 {
	out := make([][]float32, n)
	for i := range out {
		v := make([]float32, dim)
		for j := range v {
			v[j] = float32(rng.Int63()%2001-1000) / 37
		}
		out[i] = v
	}
	return out
}

func hits(hs []Hit, err error) any {
	if err != nil {
		return err.Error()
	}
	out := []string{}
	for _, h := range hs {
		out = append(out, fmt.Sprintf("%d:%08x", h.ID, math.Float32bits(h.Score)))
	}
	return out
}

// TestParityVecindex records Go's math/rand (the generator a saved index's rotation comes from), math.Exp on this
// machine (the Lloyd-Max levels use it; amd64 Go has two versions, with and without FMA), the levels, a new index's
// rotation, and whole indexes: vectors added, replaced and removed, searches with and without an allow list, the saved
// file, and damaged files read back (testdata/parity/vecindex.json).
func TestParityVecindex(t *testing.T) {
	var cases []parity.Case
	for _, seed := range []int64{0, 1, -1, 7, 42, 89482311, 1<<31 - 1, -(1 << 31), 1 << 40, math.MinInt64, math.MaxInt64, 2147483646} {
		rng := rand.New(rand.NewSource(seed))
		var out []string
		for i := 0; i < 12; i++ {
			out = append(out, strconv.FormatInt(rng.Int63(), 10))
		}
		for i := 0; i < 12; i++ {
			out = append(out, strconv.Itoa(rng.Intn(2)), strconv.Itoa(rng.Intn(768)), strconv.Itoa(int(rng.Int31n(1000))))
		}
		for _, n := range []int64{1, 3, 1 << 40, 1<<62 + 1, math.MaxInt64} {
			out = append(out, strconv.FormatInt(rng.Int63n(n), 10))
		}
		out = append(out, fmt.Sprint(rng.Perm(0)), fmt.Sprint(rng.Perm(1)), fmt.Sprint(rng.Perm(10)), strconv.FormatInt(int64(rng.Int31()), 10))
		cases = append(cases, parity.Case{Fn: "rand", In: strconv.FormatInt(seed, 10), Out: out})
	}
	var exps []string
	xs := []float64{0, 1, -1, 0.5, -0.5, 1e-300, -745, -745.2, -708.5, 709.7, 709.8, 710, -1000, 1000, math.Inf(1), math.Inf(-1), math.NaN(), 2.5, -18}
	rng := rand.New(rand.NewSource(99))
	for i := 0; i < 300; i++ {
		xs = append(xs, (rng.Float64()-0.5)*40, -rng.Float64()*18)
	}
	for _, x := range xs {
		exps = append(exps, strconv.FormatUint(math.Float64bits(x), 16)+":"+strconv.FormatUint(math.Float64bits(math.Exp(x)), 16))
	}
	cases = append(cases, parity.Case{Fn: "exp", Out: exps})
	for _, n := range []int{4, 16} {
		var lv []string
		for _, v := range lloydMax(n) {
			lv = append(lv, strconv.FormatUint(math.Float64bits(v), 16))
		}
		cases = append(cases, parity.Case{Fn: "lloydMax", In: n, Out: lv})
	}
	for _, c := range []struct {
		dim, bits int
		seed      int64
	}{{1, 2, 0}, {2, 3, 0}, {1<<16 - 1, 2, 0}, {1<<16 + 1, 4, 0}, {0, 4, 0}} {
		// not 1<<16: Go's New accepts it and then never ends (its block loop appends blocks of size 0 until memory runs
		// out); the port refuses it (docs/RUST_PORT.md)
		_, err := New(c.dim, c.bits, c.seed)
		cases = append(cases, parity.Case{Fn: "New", In: []any{c.dim, c.bits, c.seed}, Out: parity.Err(err)})
	}
	dir := t.TempDir()
	sc := 0
	for _, dim := range []int{2, 3, 5, 8, 16, 33, 100, 768, 1000} {
		for _, bits := range []int{2, 4} {
			for _, seed := range []int64{0, 1, -5, 1 << 40} {
				sc++
				x, err := New(dim, bits, seed)
				if err != nil {
					t.Fatal(err)
				}
				var signs, perms []byte
				for r := 0; r < rounds; r++ {
					signs = append(signs, f32s(x.signs[r])...)
					for _, p := range x.perm[r] {
						perms = binary.LittleEndian.AppendUint32(perms, uint32(p))
					}
				}
				out := map[string]any{"blocks": x.blocks, "per": x.per, "cells": x.cells, "book": f32s(x.book), "cuts": f32s(x.cuts),
					"signs": digest(signs), "perm": digest(perms)}
				drng := rand.New(rand.NewSource(int64(sc)))
				n := 1 + int(drng.Int63()%30)
				if dim >= 768 {
					n = 1 + int(drng.Int63()%5)
				}
				vs := vecs(drng, n, dim)
				var rot []byte
				for i, v := range vs {
					if err := x.Add(uint64(i*7+1), v); err != nil {
						t.Fatal(err)
					}
					if i < 2 {
						buf := make([]float32, x.cells)
						copy(buf, v)
						x.rotate(buf)
						rot = append(rot, f32s(buf)...)
					}
				}
				// equal vectors under five ids: ties, which come out in the order Go's heap leaves them
				for id := uint64(1000); id < 1005; id++ {
					_ = x.Add(id, vs[0])
				}
				out["rotated"] = digest(rot)
				// replace one, remove two (one missing), add a zero vector
				var ops []any
				if n > 2 {
					ops = append(ops, parity.Err(x.Add(8, vecs(drng, 1, dim)[0])), x.Remove(1), x.Remove(1), x.Remove(uint64(n*7+50)))
				}
				ops = append(ops, parity.Err(x.Add(999, make([]float32, dim))), parity.Err(x.Add(5, make([]float32, dim+1))))
				bad := make([]float32, dim)
				bad[dim-1] = float32(math.NaN())
				ops = append(ops, parity.Err(x.Add(6, bad)))
				bad[dim-1] = float32(math.Inf(-1))
				ops = append(ops, parity.Err(x.Add(6, bad)))
				out["ops"] = ops
				out["len"] = []int{x.Len(), x.Bytes(), x.Dim(), x.Bits()}
				qs := vecs(drng, 3, dim)
				var searches []any
				for qi, q := range qs {
					for _, k := range []int{0, 1, 3, 100} {
						searches = append(searches, hits(x.Search(q, k, nil)))
					}
					allow := map[uint64]bool{8: true, 15: true, 999: true, uint64(qi)*7 + 22: true}
					searches = append(searches, hits(x.Search(q, 2, allow)), hits(x.Search(q, 2, map[uint64]bool{})))
				}
				searches = append(searches, hits(x.Search(make([]float32, dim+2), 1, nil)))
				out["search"] = searches
				path := filepath.Join(dir, fmt.Sprintf("i%d.vec", sc))
				if err := x.Save(path); err != nil {
					t.Fatal(err)
				}
				data, _ := os.ReadFile(path)
				out["file"] = digest(data)
				back, err := Load(path)
				if err != nil {
					t.Fatal(err)
				}
				out["reloaded"] = hits(back.Search(qs[0], 3, nil))
				cases = append(cases, parity.Case{Fn: "index", In: []any{dim, bits, seed, sc}, Out: out})
				if sc == 3 { // damaged copies of one small file
					var damaged []any
					for cut := 0; cut <= len(data); cut++ {
						if cut > 40 && cut < len(data)-3 && cut%7 != 0 {
							continue
						}
						p := filepath.Join(dir, "cut.vec")
						_ = os.WriteFile(p, data[:cut], 0o600)
						_, err := Load(p)
						damaged = append(damaged, []any{cut, parity.Err(err)})
					}
					for _, patch := range []struct {
						at  int
						val []byte
					}{
						// never a count Go accepts that is large: Go's Load allocates the count's slices before it reads
						// (2^32 vectors pass its size check and cost 52 GB), see docs/RUST_PORT.md
						{6, []byte{1, 0, 0, 0}}, {10, []byte{3, 0, 0, 0}}, {22, []byte{1, 0, 0, 0, 1, 0, 0, 0}}, {22, []byte{255, 255, 255, 255, 255, 255, 255, 255}}, {22, []byte{0, 4, 0, 0, 0, 0, 0, 0}}, {0, []byte("SVEC2")}} {
						d := append([]byte(nil), data...)
						copy(d[patch.at:], patch.val)
						p := filepath.Join(dir, "patched.vec")
						_ = os.WriteFile(p, d, 0o600)
						_, err := Load(p)
						damaged = append(damaged, []any{hex.EncodeToString(d), parity.Err(err)})
					}
					cases = append(cases, parity.Case{Fn: "damaged", In: hex.EncodeToString(data), Out: damaged})
				}
			}
		}
	}
	parity.Fixture(t, "vecindex", cases)

	// Go's seeding table, for the Rust port of math/rand (rust/sarina/src/gorand_cooked.rs), read from the Go source
	src := filepath.Join(runtime.GOROOT(), "src", "math", "rand", "rng.go")
	f, err := parser.ParseFile(token.NewFileSet(), src, nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	var vals []string
	ast.Inspect(f, func(n ast.Node) bool {
		if vs, ok := n.(*ast.ValueSpec); ok && len(vs.Names) == 1 && vs.Names[0].Name == "rngCooked" {
			for _, e := range vs.Values[0].(*ast.CompositeLit).Elts {
				switch x := e.(type) {
				case *ast.BasicLit:
					vals = append(vals, x.Value)
				case *ast.UnaryExpr:
					vals = append(vals, "-"+x.X.(*ast.BasicLit).Value)
				}
			}
		}
		return true
	})
	if len(vals) != 607 {
		t.Fatalf("rngCooked has %d values", len(vals))
	}
	var b strings.Builder
	b.WriteString("// Go's rngCooked (math/rand/rng.go, Copyright 2009 The Go Authors, BSD-3-Clause; the Go licence is in Go's\n")
	b.WriteString("// source tree): the table every math/rand source is seeded with. Written by go/vecindex's parity test.\n\n")
	b.WriteString("#[rustfmt::skip]\npub(crate) const RNG_COOKED: [i64; 607] = [\n")
	for i := 0; i < len(vals); i += 4 {
		b.WriteString("    " + strings.Join(vals[i:min(i+4, len(vals))], ", ") + ",\n")
	}
	b.WriteString("];\n")
	parity.Generated(t, "rust/sarina/src/gorand_cooked.rs", []byte(b.String()))
}
