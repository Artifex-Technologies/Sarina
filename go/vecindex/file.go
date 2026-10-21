package vecindex

import (
	"bufio"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
)

const magic = "SVEC1\n"

// Save writes the index to path: to a temporary file first, then moved into place, so a crash never leaves half a file.
func (x *Index) Save(path string) error {
	x.mu.RLock()
	defer x.mu.RUnlock()
	tmp, err := os.CreateTemp(filepath.Dir(path), ".vec-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	w := bufio.NewWriter(tmp)
	w.WriteString(magic)
	for _, v := range []any{uint32(x.dim), uint32(x.bits), x.seed, uint64(len(x.ids))} {
		binary.Write(w, binary.LittleEndian, v)
	}
	binary.Write(w, binary.LittleEndian, x.ids)
	binary.Write(w, binary.LittleEndian, x.norms)
	w.Write(x.codes)
	if err := w.Flush(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Sync(); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Chmod(tmp.Name(), 0o600); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}

// Load reads an index written by Save.
func Load(path string) (*Index, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	r := bufio.NewReader(f)
	head := make([]byte, len(magic))
	if _, err := io.ReadFull(r, head); err != nil || string(head) != magic {
		return nil, errors.New("vecindex: not an index file")
	}
	var dim, bits uint32
	var seed int64
	var n uint64
	for _, p := range []any{&dim, &bits, &seed, &n} {
		if err := binary.Read(r, binary.LittleEndian, p); err != nil {
			return nil, err
		}
	}
	x, err := New(int(dim), int(bits), seed)
	if err != nil {
		return nil, err
	}
	if n > 1<<32 || n*uint64(x.per) > 1<<36 {
		return nil, fmt.Errorf("vecindex: implausible size %d", n)
	}
	x.ids = make([]uint64, n)
	x.norms = make([]float32, n)
	x.codes = make([]byte, int(n)*x.per)
	if err := binary.Read(r, binary.LittleEndian, x.ids); err != nil {
		return nil, err
	}
	if err := binary.Read(r, binary.LittleEndian, x.norms); err != nil {
		return nil, err
	}
	if _, err := io.ReadFull(r, x.codes); err != nil {
		return nil, err
	}
	for i, id := range x.ids {
		x.slot[id] = i
	}
	return x, nil
}
