// Package vecindex is a small in-process vector index for Sarina's memory (notes, saved sessions, later much more): vectors are stored at 2 or 4 bits per
// dimension and searched in that compressed form, so a million 768-number embeddings take about 400 MB instead of 3 GB, and a few thousand take nothing.
//
// The method is TurboQuant (Zandieh et al., https://arxiv.org/abs/2504.19874): scale a vector to length 1, spin it with a random rotation, and every coordinate then
// follows nearly the same bell curve whatever the data was, so one fixed set of levels per bit width (the Lloyd-Max levels of that curve) fits all data. That is why
// there is nothing to train and a vector can be added or removed at any time. Written for Sarina in Go from the paper; the Rust library turbovec (MIT, Ryan Codrai)
// shows the same idea and was the reference for what to measure.
//
// Plain Go, no cgo, no dependencies, nothing running in the background: the index is only memory you ask for.
package vecindex

import (
	"container/heap"
	"errors"
	"fmt"
	"math"
	"math/rand"
	"sort"
	"sync"
)

// Index holds vectors of one dimension at Bits (2 or 4) bits per number. It is safe for concurrent use.
type Index struct {
	mu     sync.RWMutex
	dim    int   // the caller's dimension
	cells  int   // dim rounded up to fill whole bytes (the extra places stay zero)
	blocks []int // the rotation works on power-of-two blocks that add up to dim
	perm   [rounds][]int32
	bits   int
	seed   int64
	signs  [rounds][]float32 // the random signs of each rotation round
	book   []float32         // the levels for this bit width, already scaled for pad
	cuts   []float32         // the decision thresholds between levels
	per    int               // bytes per stored vector
	codes  []byte
	norms  []float32
	ids    []uint64
	slot   map[uint64]int
}

const rounds = 3

// New makes an empty index. bits is 2 or 4; seed fixes the rotation (keep it with a saved index; Save does).
func New(dim, bits int, seed int64) (*Index, error) {
	if dim < 2 || dim > 1<<16 {
		return nil, fmt.Errorf("vecindex: dimension %d is not supported", dim)
	}
	if bits != 2 && bits != 4 {
		return nil, errors.New("vecindex: bits must be 2 or 4")
	}
	per := (dim*bits + 7) / 8
	x := &Index{dim: dim, cells: per * 8 / bits, bits: bits, seed: seed, slot: map[uint64]int{}, per: per}
	for rest, b := dim, 1<<15; rest > 0; b >>= 1 { // 768 = 512 + 256
		if rest >= b {
			x.blocks = append(x.blocks, b)
			rest -= b
		}
	}
	rng := rand.New(rand.NewSource(seed))
	for r := range x.signs {
		x.signs[r] = make([]float32, dim)
		for i := range x.signs[r] {
			x.signs[r][i] = float32(1 - 2*rng.Intn(2))
		}
		p := rng.Perm(dim)
		x.perm[r] = make([]int32, dim)
		for i, v := range p {
			x.perm[r][i] = int32(v)
		}
	}
	lv := lloydMax(1 << bits)
	scale := 1 / math.Sqrt(float64(dim))
	x.book = make([]float32, len(lv))
	for i, v := range lv {
		x.book[i] = float32(v * scale)
	}
	x.cuts = make([]float32, len(lv)-1)
	for i := range x.cuts {
		x.cuts[i] = (x.book[i] + x.book[i+1]) / 2
	}
	return x, nil
}

// lloydMax returns the n best levels for a standard normal variable (the classic iteration, run once and the same every time).
func lloydMax(n int) []float64 {
	// start evenly in [-2.5, 2.5], then alternate "cut halfway" and "move each level to the centre of its cell" on a fine grid of the bell curve
	lv := make([]float64, n)
	for i := range lv {
		lv[i] = -2.5 + 5*float64(i)/float64(n-1)
	}
	const steps = 40000
	lo, hi := -6.0, 6.0
	w := make([]float64, steps)
	xs := make([]float64, steps)
	for i := range w {
		xs[i] = lo + (hi-lo)*(float64(i)+0.5)/steps
		w[i] = math.Exp(-xs[i] * xs[i] / 2)
	}
	for it := 0; it < 200; it++ {
		sum := make([]float64, n)
		wt := make([]float64, n)
		for i, x := range xs {
			c := 0
			for c < n-1 && x > (lv[c]+lv[c+1])/2 {
				c++
			}
			sum[c] += w[i] * x
			wt[c] += w[i]
		}
		for c := range lv {
			if wt[c] > 0 {
				lv[c] = sum[c] / wt[c]
			}
		}
	}
	return lv
}

// rotate spins v (the first dim places, changed in place): random signs, a Walsh-Hadamard transform inside each block, and a shuffle across blocks, three times;
// every step keeps the vector's length.
func (x *Index) rotate(v []float32) {
	tmp := make([]float32, x.dim)
	for r := 0; r < rounds; r++ {
		for i := 0; i < x.dim; i++ {
			v[i] *= x.signs[r][i]
		}
		off := 0
		for _, b := range x.blocks {
			blk := v[off : off+b]
			for h := 1; h < b; h <<= 1 {
				for i := 0; i < b; i += h << 1 {
					for j := i; j < i+h; j++ {
						a, c := blk[j], blk[j+h]
						blk[j], blk[j+h] = a+c, a-c
					}
				}
			}
			sc := float32(1 / math.Sqrt(float64(b)))
			for i := range blk {
				blk[i] *= sc
			}
			off += b
		}
		if len(x.blocks) > 1 || r < rounds-1 {
			for i := 0; i < x.dim; i++ {
				tmp[i] = v[x.perm[r][i]]
			}
			copy(v, tmp)
		}
	}
}

func (x *Index) level(c float32) int {
	// the cuts are sorted: a short binary search
	return sort.Search(len(x.cuts), func(i int) bool { return x.cuts[i] >= c })
}

// Dim reports the dimension, Len the number of vectors, Bits the bits per number.
func (x *Index) Dim() int  { return x.dim }
func (x *Index) Bits() int { return x.bits }
func (x *Index) Len() int  { x.mu.RLock(); defer x.mu.RUnlock(); return len(x.ids) }

// Bytes is about how much memory the stored vectors take.
func (x *Index) Bytes() int {
	x.mu.RLock()
	defer x.mu.RUnlock()
	return len(x.codes) + 4*len(x.norms) + 8*len(x.ids)
}

// Add stores v under id (an existing id is replaced). v is not changed.
func (x *Index) Add(id uint64, v []float32) error {
	if len(v) != x.dim {
		return fmt.Errorf("vecindex: vector has %d numbers, the index wants %d", len(v), x.dim)
	}
	buf := make([]float32, x.cells)
	var n2 float64
	for i, f := range v {
		if math.IsNaN(float64(f)) || math.IsInf(float64(f), 0) {
			return errors.New("vecindex: vector has a NaN or infinite number")
		}
		buf[i] = f
		n2 += float64(f) * float64(f)
	}
	norm := float32(math.Sqrt(n2))
	if norm > 0 {
		inv := 1 / norm
		for i := range buf {
			buf[i] *= inv
		}
	}
	x.rotate(buf)
	code := make([]byte, x.per)
	for i, c := range buf {
		if i >= x.dim {
			break
		}
		l := byte(x.level(c))
		if x.bits == 4 {
			code[i>>1] |= l << (4 * uint(i&1))
		} else {
			code[i>>2] |= l << (2 * uint(i&3))
		}
	}
	x.mu.Lock()
	defer x.mu.Unlock()
	if s, ok := x.slot[id]; ok {
		copy(x.codes[s*x.per:], code)
		x.norms[s] = norm
		return nil
	}
	x.slot[id] = len(x.ids)
	x.ids = append(x.ids, id)
	x.norms = append(x.norms, norm)
	x.codes = append(x.codes, code...)
	return nil
}

// Remove deletes id (false if it was not there). The last vector takes its place, so it is quick.
func (x *Index) Remove(id uint64) bool {
	x.mu.Lock()
	defer x.mu.Unlock()
	s, ok := x.slot[id]
	if !ok {
		return false
	}
	last := len(x.ids) - 1
	if s != last {
		copy(x.codes[s*x.per:(s+1)*x.per], x.codes[last*x.per:(last+1)*x.per])
		x.norms[s] = x.norms[last]
		x.ids[s] = x.ids[last]
		x.slot[x.ids[s]] = s
	}
	x.codes = x.codes[:last*x.per]
	x.norms = x.norms[:last]
	x.ids = x.ids[:last]
	delete(x.slot, id)
	return true
}

// Hit is one search result: the id and the (approximate) inner product with the query.
type Hit struct {
	ID    uint64
	Score float32
}

type minHeap []Hit

func (h minHeap) Len() int            { return len(h) }
func (h minHeap) Less(i, j int) bool  { return h[i].Score < h[j].Score }
func (h minHeap) Swap(i, j int)       { h[i], h[j] = h[j], h[i] }
func (h *minHeap) Push(v interface{}) { *h = append(*h, v.(Hit)) }
func (h *minHeap) Pop() interface{}   { o := *h; v := o[len(o)-1]; *h = o[:len(o)-1]; return v }

// Search returns up to k best matches by inner product (normalise the vectors first for cosine similarity), best first. allow, when not nil, limits the search to those ids.
func (x *Index) Search(q []float32, k int, allow map[uint64]bool) ([]Hit, error) {
	if len(q) != x.dim {
		return nil, fmt.Errorf("vecindex: query has %d numbers, the index wants %d", len(q), x.dim)
	}
	if k < 1 {
		return nil, nil
	}
	buf := make([]float32, x.cells)
	copy(buf, q)
	x.rotate(buf)
	// per query, a small table: what each possible stored level adds for each place
	nl := 1 << x.bits
	lut := make([]float32, x.cells*nl)
	for i := 0; i < x.cells; i++ {
		for l := 0; l < nl; l++ {
			lut[i*nl+l] = buf[i] * x.book[l]
		}
	}
	x.mu.RLock()
	defer x.mu.RUnlock()
	h := &minHeap{}
	for s := range x.ids {
		if allow != nil && !allow[x.ids[s]] {
			continue
		}
		code := x.codes[s*x.per : (s+1)*x.per]
		var sum float32
		if x.bits == 4 {
			for i, b := range code {
				sum += lut[(2*i)*16+int(b&15)] + lut[(2*i+1)*16+int(b>>4)]
			}
		} else {
			for i, b := range code {
				o := 4 * i
				sum += lut[o*4+int(b&3)] + lut[(o+1)*4+int(b>>2&3)] + lut[(o+2)*4+int(b>>4&3)] + lut[(o+3)*4+int(b>>6)]
			}
		}
		sc := sum * x.norms[s]
		if h.Len() < k {
			heap.Push(h, Hit{x.ids[s], sc})
		} else if sc > (*h)[0].Score {
			(*h)[0] = Hit{x.ids[s], sc}
			heap.Fix(h, 0)
		}
	}
	out := make([]Hit, h.Len())
	for i := len(out) - 1; i >= 0; i-- {
		out[i] = heap.Pop(h).(Hit)
	}
	return out, nil
}
