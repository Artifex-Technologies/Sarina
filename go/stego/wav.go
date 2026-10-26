package stego

import (
	"encoding/binary"
	"fmt"
)

// WAV is the PCM samples of a RIFF WAVE file.
type WAV struct {
	Channels, Rate, Bits int
	Samples              []int32 // interleaved
}

// ParseWAV reads an uncompressed PCM WAV (8, 16, 24 or 32 bits).
func ParseWAV(b []byte) (*WAV, error) {
	if len(b) < 12 || string(b[:4]) != "RIFF" || string(b[8:12]) != "WAVE" {
		return nil, fmt.Errorf("not a WAV file")
	}
	w := &WAV{}
	pos := 12
	var data []byte
	format := 0
	for pos+8 <= len(b) {
		id := string(b[pos : pos+4])
		n := int(binary.LittleEndian.Uint32(b[pos+4:]))
		if pos+8+n > len(b) {
			n = len(b) - pos - 8
		}
		chunk := b[pos+8 : pos+8+n]
		switch id {
		case "fmt ":
			if len(chunk) >= 16 {
				format = int(binary.LittleEndian.Uint16(chunk))
				w.Channels = int(binary.LittleEndian.Uint16(chunk[2:]))
				w.Rate = int(binary.LittleEndian.Uint32(chunk[4:]))
				w.Bits = int(binary.LittleEndian.Uint16(chunk[14:]))
			}
		case "data":
			data = chunk
		}
		pos += 8 + n + (n & 1)
	}
	if format != 1 && format != 0xfffe {
		return nil, fmt.Errorf("WAV format %d is compressed: I read only PCM", format)
	}
	if data == nil || w.Bits == 0 {
		return nil, fmt.Errorf("no PCM data chunk")
	}
	bytesPer := w.Bits / 8
	if bytesPer < 1 || bytesPer > 4 {
		return nil, fmt.Errorf("%d-bit samples are not something I read", w.Bits)
	}
	count := len(data) / bytesPer
	if count > MaxPixels {
		count = MaxPixels
	}
	w.Samples = make([]int32, count)
	for i := 0; i < count; i++ {
		s := data[i*bytesPer : (i+1)*bytesPer]
		switch bytesPer {
		case 1:
			w.Samples[i] = int32(s[0])
		case 2:
			w.Samples[i] = int32(int16(binary.LittleEndian.Uint16(s)))
		case 3:
			w.Samples[i] = int32(uint32(s[0]) | uint32(s[1])<<8 | uint32(s[2])<<16)
		case 4:
			w.Samples[i] = int32(binary.LittleEndian.Uint32(s))
		}
	}
	return w, nil
}

// LSBBytes packs the lowest bit of each sample into bytes (most significant first, or least), at most max bytes.
func (w *WAV) LSBBytes(msbFirst bool, max int) []byte {
	out := make([]byte, 0, max)
	var cur, have uint
	for _, s := range w.Samples {
		bit := uint(s) & 1
		if msbFirst {
			cur = cur<<1 | bit
		} else {
			cur |= bit << have
		}
		have++
		if have == 8 {
			out = append(out, byte(cur))
			cur, have = 0, 0
			if len(out) >= max {
				break
			}
		}
	}
	return out
}

// SearchWAV looks for a payload in the sample LSBs, both bit orders.
func SearchWAV(w *WAV, maxBytes int) []Found {
	var found []Found
	for _, msb := range []bool{true, false} {
		if f, ok := judge(w.LSBBytes(msb, maxBytes)); ok {
			f.Order, f.MSBFirst, f.Bits = "sample LSB", msb, 1
			found = append(found, f)
		}
	}
	return found
}

// LSBStats is the share of ones in the sample LSBs: 16-bit music sits near 0.5; silence or 8-bit speech can sit far from it.
func (w *WAV) LSBStats() (onesRate float64, n int) {
	ones := 0
	for _, s := range w.Samples {
		if s&1 == 1 {
			ones++
		}
	}
	if len(w.Samples) == 0 {
		return 0, 0
	}
	return float64(ones) / float64(len(w.Samples)), len(w.Samples)
}
