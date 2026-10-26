package stego

import "fmt"

// Diff compares two decoded images of the same size: how many pixels differ, by how much, where, and whether only their lowest bits do.
func Diff(a, b *Pixels) (string, error) {
	if a.W != b.W || a.H != b.H {
		return "", fmt.Errorf("the images differ in size (%dx%d and %dx%d), so they are not the same picture with something hidden in it", a.W, a.H, b.W, b.H)
	}
	n := a.W * a.H
	changed, lsbOnly := 0, 0
	maxDiff := 0
	var chan_ [4]int
	minX, minY, maxX, maxY := a.W, a.H, -1, -1
	for i := 0; i < n; i++ {
		pixelChanged, onlyLSB := false, true
		for c := 0; c < 4; c++ {
			d := int(a.RGBA[i*4+c]) - int(b.RGBA[i*4+c])
			if d < 0 {
				d = -d
			}
			if d > 0 {
				pixelChanged = true
				chan_[c]++
				if d > maxDiff {
					maxDiff = d
				}
				if d > 1 {
					onlyLSB = false
				}
			}
		}
		if pixelChanged {
			changed++
			if onlyLSB {
				lsbOnly++
			}
			x, y := i%a.W, i/a.W
			if x < minX {
				minX = x
			}
			if y < minY {
				minY = y
			}
			if x > maxX {
				maxX = x
			}
			if y > maxY {
				maxY = y
			}
		}
	}
	if changed == 0 {
		return fmt.Sprintf("The two images are identical pixel for pixel (%dx%d).", a.W, a.H), nil
	}
	out := fmt.Sprintf("%d of %d pixels differ (%.2f%%); largest change %d of 255; changes per channel: red %d, green %d, blue %d, alpha %d; "+
		"within x %d-%d, y %d-%d.", changed, n, 100*float64(changed)/float64(n), maxDiff, chan_[0], chan_[1], chan_[2], chan_[3], minX, maxX, minY, maxY)
	switch {
	case lsbOnly == changed:
		out += " Every change is in the lowest bit only: exactly what LSB steganography leaves (invisible to the eye). Extract the LSBs of the changed image (action lsb) to read it."
	case lsbOnly*2 > changed:
		out += " Most changes are in the lowest bit only: LSB steganography, or a lossy re-save."
	default:
		out += " The changes are larger than the lowest bit: a re-encode, a resize, or visible editing, not plain LSB hiding."
	}
	return out, nil
}
