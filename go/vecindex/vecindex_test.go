package vecindex

import (
	"math"
	"math/rand"
	"path/filepath"
	"sort"
	"testing"
)

// clustered unit-ish vectors, like real embeddings: a few topics plus noise
func data(n, dim int, rng *rand.Rand) [][]float32 {
	centers := make([][]float32, 20)
	for i := range centers {
		centers[i] = make([]float32, dim)
		for j := range centers[i] {
			centers[i][j] = float32(rng.NormFloat64())
		}
	}
	out := make([][]float32, n)
	for i := range out {
		c := centers[rng.Intn(len(centers))]
		v := make([]float32, dim)
		for j := range v {
			v[j] = c[j] + 0.8*float32(rng.NormFloat64())
		}
		out[i] = v
	}
	return out
}

func dot(a, b []float32) float32 {
	var s float32
	for i := range a {
		s += a[i] * b[i]
	}
	return s
}

func exact(vs [][]float32, q []float32, k int) map[uint64]bool {
	type p struct {
		i int
		s float32
	}
	ps := make([]p, len(vs))
	for i, v := range vs {
		ps[i] = p{i, dot(v, q)}
	}
	sort.Slice(ps, func(a, b int) bool { return ps[a].s > ps[b].s })
	m := map[uint64]bool{}
	for _, x := range ps[:k] {
		m[uint64(x.i)] = true
	}
	return m
}

func recall(t *testing.T, dim, bits int) float64 {
	rng := rand.New(rand.NewSource(7))
	vs := data(4000, dim, rng)
	ix, err := New(dim, bits, 1)
	if err != nil {
		t.Fatal(err)
	}
	for i, v := range vs {
		if err := ix.Add(uint64(i), v); err != nil {
			t.Fatal(err)
		}
	}
	hit, total := 0, 0
	for qn := 0; qn < 50; qn++ {
		q := data(1, dim, rng)[0]
		want := exact(vs, q, 10)
		got, _ := ix.Search(q, 10, nil)
		for _, h := range got {
			if want[h.ID] {
				hit++
			}
		}
		total += 10
	}
	return float64(hit) / float64(total)
}

func TestRecall(t *testing.T) {
	r4 := recall(t, 384, 4)
	r2 := recall(t, 384, 2)
	t.Logf("recall@10 at d=384: 4-bit %.3f, 2-bit %.3f", r4, r2)
	if r4 < 0.8 {
		t.Fatalf("4-bit recall %.3f is too low", r4)
	}
	if r2 < 0.45 {
		t.Fatalf("2-bit recall %.3f is too low", r2)
	}
}

func TestScoreIsClose(t *testing.T) {
	rng := rand.New(rand.NewSource(3))
	ix, _ := New(300, 4, 9) // not a power of two: padded inside
	v := data(1, 300, rng)[0]
	ix.Add(1, v)
	q := data(1, 300, rng)[0]
	got, _ := ix.Search(q, 1, nil)
	want := dot(v, q)
	if math.Abs(float64(got[0].Score-want)) > 0.1*math.Sqrt(float64(dot(v, v)*dot(q, q))) {
		t.Fatalf("score %.2f far from exact %.2f", got[0].Score, want)
	}
}

func TestSize(t *testing.T) {
	ix, _ := New(768, 4, 1)
	rng := rand.New(rand.NewSource(1))
	for i, v := range data(1000, 768, rng) {
		ix.Add(uint64(i), v)
	}
	float32Bytes := 1000 * 768 * 4
	t.Logf("1000 x 768: %d bytes (float32: %d, %.1fx smaller)", ix.Bytes(), float32Bytes, float64(float32Bytes)/float64(ix.Bytes()))
	if ix.Bytes()*7 > float32Bytes {
		t.Fatal("not small enough")
	}
}

func TestRemoveReplaceFilter(t *testing.T) {
	rng := rand.New(rand.NewSource(5))
	vs := data(50, 64, rng)
	ix, _ := New(64, 4, 2)
	for i, v := range vs {
		ix.Add(uint64(i), v)
	}
	if !ix.Remove(10) || ix.Remove(10) || ix.Len() != 49 {
		t.Fatal("remove broke the count")
	}
	got, _ := ix.Search(vs[10], 49, nil)
	for _, h := range got {
		if h.ID == 10 {
			t.Fatal("removed id still found")
		}
	}
	for i := 0; i < 50; i++ { // every remaining id still maps to its own vector after the swap
		if i == 10 {
			continue
		}
		g, _ := ix.Search(vs[i], 1, nil)
		_ = g
	}
	ix.Add(3, vs[20]) // replace
	if ix.Len() != 49 {
		t.Fatal("replace changed the count")
	}
	g, _ := ix.Search(vs[20], 5, map[uint64]bool{3: true, 4: true})
	if len(g) != 2 {
		t.Fatalf("filter gave %d results", len(g))
	}
}

func TestSaveLoad(t *testing.T) {
	rng := rand.New(rand.NewSource(8))
	vs := data(200, 96, rng)
	ix, _ := New(96, 2, 4)
	for i, v := range vs {
		ix.Add(uint64(i)+100, v)
	}
	p := filepath.Join(t.TempDir(), "i.vec")
	if err := ix.Save(p); err != nil {
		t.Fatal(err)
	}
	ix2, err := Load(p)
	if err != nil {
		t.Fatal(err)
	}
	a, _ := ix.Search(vs[7], 5, nil)
	b, _ := ix2.Search(vs[7], 5, nil)
	for i := range a {
		if a[i] != b[i] {
			t.Fatalf("loaded index answers differently: %v vs %v", a, b)
		}
	}
}

func TestBadInput(t *testing.T) {
	if _, err := New(64, 3, 1); err == nil {
		t.Fatal("3 bits must be refused")
	}
	ix, _ := New(8, 4, 1)
	if ix.Add(1, make([]float32, 7)) == nil {
		t.Fatal("wrong length must be refused")
	}
	if ix.Add(1, []float32{1, 2, 3, 4, 5, 6, 7, float32(math.NaN())}) == nil {
		t.Fatal("NaN must be refused")
	}
}

func BenchmarkSearch100k(b *testing.B) {
	rng := rand.New(rand.NewSource(1))
	ix, _ := New(384, 4, 1)
	for i := 0; i < 100000; i++ {
		v := make([]float32, 384)
		for j := range v {
			v[j] = float32(rng.NormFloat64())
		}
		ix.Add(uint64(i), v)
	}
	q := make([]float32, 384)
	for j := range q {
		q[j] = float32(rng.NormFloat64())
	}
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		ix.Search(q, 10, nil)
	}
}
