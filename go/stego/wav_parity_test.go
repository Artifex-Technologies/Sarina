package stego

import (
	"encoding/binary"
	"fmt"
	"math"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityWAV records (testdata/parity/wav.json) ParseWAV on good and broken files, the samples' lowest bits read
// both ways, SearchWAV's judgement (a file signature, a length then text, text) and the LSB statistics.
func TestParityWAV(t *testing.T) {
	wav := func(format, bits uint16, data []byte, extra ...[]byte) []byte {
		fmtc := make([]byte, 16)
		binary.LittleEndian.PutUint16(fmtc, format)
		binary.LittleEndian.PutUint16(fmtc[2:], 2)
		binary.LittleEndian.PutUint32(fmtc[4:], 44100)
		binary.LittleEndian.PutUint16(fmtc[14:], bits)
		var b []byte
		b = append(b, "RIFF\x00\x00\x00\x00WAVE"...)
		for _, e := range extra {
			b = append(b, e...)
		}
		b = append(b, "fmt "...)
		b = binary.LittleEndian.AppendUint32(b, 16)
		b = append(b, fmtc...)
		b = append(b, "data"...)
		b = binary.LittleEndian.AppendUint32(b, uint32(len(data)))
		return append(b, data...)
	}
	hide := func(msg []byte, bytesPer int, msb bool) []byte {
		var bits []byte
		for _, c := range msg {
			for i := 0; i < 8; i++ {
				if msb {
					bits = append(bits, c>>(7-i)&1)
				} else {
					bits = append(bits, c>>i&1)
				}
			}
		}
		data := make([]byte, len(bits)*bytesPer)
		for i, bit := range bits {
			for k := 0; k < bytesPer; k++ {
				data[i*bytesPer+k] = byte(i*37+k*11) & 0xfe
			}
			data[i*bytesPer] |= bit
		}
		return data
	}
	lenText := func(s string, le bool) []byte {
		b := make([]byte, 4)
		if le {
			binary.LittleEndian.PutUint32(b, uint32(len(s)))
		} else {
			binary.BigEndian.PutUint32(b, uint32(len(s)))
		}
		return append(b, s...)
	}
	files := [][]byte{nil, []byte("RIFF"), []byte("RIFF\x00\x00\x00\x00AVI "), wav(3, 32, make([]byte, 64)), wav(1, 0, make([]byte, 64)),
		wav(1, 40, make([]byte, 64)), wav(1, 4, make([]byte, 64)), []byte("RIFF\x00\x00\x00\x00WAVEfmt \x10\x00\x00\x00"),
		wav(1, 16, hide([]byte("hello hidden world"), 2, true)), wav(1, 16, hide([]byte("hello hidden world"), 2, false)),
		wav(1, 8, hide(lenText("short message here", false), 1, true)), wav(0xfffe, 24, hide(lenText("little endian length!", true), 3, false)),
		wav(1, 32, hide([]byte("PK\x03\x04zipzipzip"), 4, true)), wav(1, 16, hide([]byte("text then\x00\x00\x00\x00garbage"), 2, true)),
		wav(1, 16, hide([]byte{0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88}, 2, true)), wav(1, 16, hide([]byte("abc"), 2, true)),
		wav(1, 8, []byte{0xff, 0x80, 0x7f, 1}, []byte("LIST\x03\x00\x00\x00abc\x00")), wav(1, 16, []byte{0x00, 0x80, 0xff, 0x7f, 0x01}),
		append(wav(1, 16, []byte{1, 2}), "junk\xff\xff\xff\x7f"...)}
	var out []any
	for _, f := range files {
		w, err := ParseWAV(f)
		row := []any{parity.Text(string(f)), parity.Err(err)}
		if err == nil {
			rate, n := w.LSBStats()
			var fs []any
			for _, fd := range SearchWAV(w, 4096) {
				fs = append(fs, []any{fd.Order, fd.Bits, fd.MSBFirst, fd.Length, fd.Why, parity.Text(string(fd.Data))})
			}
			row = append(row, w.Channels, w.Rate, w.Bits, fmt.Sprint(w.Samples), fmt.Sprintf("%016x", math.Float64bits(rate)), n,
				parity.Text(string(w.LSBBytes(true, 5))), parity.Text(string(w.LSBBytes(false, 1000))), fs)
		}
		out = append(out, row)
	}
	// the judge on its own, at its boundaries
	var judged []any
	for _, d := range []string{"\x00\x00\x00\x04abcd" + "\x01\x01", "\x04\x00\x00\x00wxyz\xff\xff", "abcdefg\x00\x00\x00\x00", "abcdefgh\x00\x00\x00\x00",
		"abcdefghijklmnopqrs\x01", "abcdefghijklmnopqr\x01\x02", "#!/bin/sh\necho hi", "short", "\x00\x00\x00\x03abcxxxxx", "%PDF-1.4 x"} {
		f, ok := judge([]byte(d))
		judged = append(judged, []any{parity.Text(d), ok, f.Length, f.Why, parity.Text(string(f.Data))})
	}
	parity.Fixture(t, "wav", []parity.Case{{Fn: "wav", Out: out}, {Fn: "judge", Out: judged}})
}
