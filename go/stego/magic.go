// Package stego gives Sarina one read-only tool, stego_inspect, that looks for things hidden in files the way a careful analyst would,
// in code: what a file really is, its metadata, data stuck on after its end, files embedded inside it, strings worth reading, the
// least-significant bits of images and sound (both measured and extracted), invisible characters in text, and the difference between
// two images. Nothing is guessed, nothing goes over the network, no file is changed; extracted bytes go to a temporary folder.
//
// The tool is offered when SARINA_CRYPTO=on (with the crypto tools). The "stego" prefetch inspects a file named in a skill's task before
// the model starts.
package stego

import (
	"bytes"
	"strings"
)

// Kind is a file type by its first bytes.
type Kind struct {
	Name string   // png, jpeg, ...
	Exts []string // extensions that usually carry it
}

type magic struct {
	offset int
	bytes  []byte
	kind   Kind
}

var magics = []magic{
	{0, []byte("\x89PNG\r\n\x1a\n"), Kind{"png", []string{"png"}}},
	{0, []byte("\xff\xd8\xff"), Kind{"jpeg", []string{"jpg", "jpeg"}}},
	{0, []byte("GIF87a"), Kind{"gif", []string{"gif"}}},
	{0, []byte("GIF89a"), Kind{"gif", []string{"gif"}}},
	{0, []byte("BM"), Kind{"bmp", []string{"bmp"}}},
	{8, []byte("WEBP"), Kind{"webp", []string{"webp"}}},
	{8, []byte("WAVE"), Kind{"wav", []string{"wav"}}},
	{8, []byte("AVI "), Kind{"avi", []string{"avi"}}},
	{0, []byte("fLaC"), Kind{"flac", []string{"flac"}}},
	{0, []byte("OggS"), Kind{"ogg", []string{"ogg", "oga", "ogv", "opus"}}},
	{0, []byte("ID3"), Kind{"mp3", []string{"mp3"}}},
	{4, []byte("ftyp"), Kind{"mp4", []string{"mp4", "m4a", "mov", "heic"}}},
	{0, []byte("II*\x00"), Kind{"tiff", []string{"tif", "tiff"}}},
	{0, []byte("MM\x00*"), Kind{"tiff", []string{"tif", "tiff"}}},
	{0, []byte("PK\x03\x04"), Kind{"zip", []string{"zip", "jar", "docx", "xlsx", "pptx", "odt", "apk", "epub"}}},
	{0, []byte("PK\x05\x06"), Kind{"zip (empty)", []string{"zip"}}},
	{0, []byte("%PDF-"), Kind{"pdf", []string{"pdf"}}},
	{0, []byte("\x1f\x8b"), Kind{"gzip", []string{"gz", "tgz"}}},
	{0, []byte("BZh"), Kind{"bzip2", []string{"bz2"}}},
	{0, []byte("\xfd7zXZ\x00"), Kind{"xz", []string{"xz"}}},
	{0, []byte("7z\xbc\xaf\x27\x1c"), Kind{"7z", []string{"7z"}}},
	{0, []byte("Rar!\x1a\x07"), Kind{"rar", []string{"rar"}}},
	{0, []byte("\x7fELF"), Kind{"elf", []string{"", "so", "o"}}},
	{0, []byte("MZ"), Kind{"pe (Windows program)", []string{"exe", "dll"}}},
	{0, []byte("SQLite format 3\x00"), Kind{"sqlite", []string{"db", "sqlite", "sqlite3"}}},
	{257, []byte("ustar"), Kind{"tar", []string{"tar"}}},
	{0, []byte("-----BEGIN PGP"), Kind{"pgp armor", []string{"asc", "gpg"}}},
	{0, []byte("-----BEGIN"), Kind{"pem", []string{"pem", "crt", "cer", "pub"}}},
	{0, []byte("\xef\xbb\xbf"), Kind{"text (UTF-8 with BOM)", []string{"txt"}}},
	{0, []byte("<?xml"), Kind{"xml", []string{"xml", "svg"}}},
	{0, []byte("{\\rtf"), Kind{"rtf", []string{"rtf"}}},
	{0, []byte("\xd0\xcf\x11\xe0"), Kind{"ole (old Office)", []string{"doc", "xls", "ppt"}}},
	{0, []byte("#!"), Kind{"script", []string{"sh", "py"}}},
}

// Detect returns the kind of b by its first bytes; "text" for printable data, "unknown" otherwise.
func Detect(b []byte) Kind {
	for _, m := range magics {
		if len(b) >= m.offset+len(m.bytes) && bytes.Equal(b[m.offset:m.offset+len(m.bytes)], m.bytes) {
			if m.kind.Name == "mp3" || m.kind.Name == "pe (Windows program)" || m.kind.Name == "bmp" {
				// short magics: ask a little more
				if m.kind.Name == "bmp" && (len(b) < 14 || int(uint32(b[2])|uint32(b[3])<<8|uint32(b[4])<<16|uint32(b[5])<<24) != len(b)) {
					continue
				}
			}
			return m.kind
		}
	}
	if len(b) >= 2 && b[0] == 0xff && b[1] == 0xfb {
		return Kind{"mp3", []string{"mp3"}}
	}
	if isText(b) {
		return Kind{"text", []string{"txt", "md", "csv", "json", "html"}}
	}
	return Kind{"unknown", nil}
}

func isText(b []byte) bool {
	if len(b) == 0 {
		return true
	}
	sample := b
	if len(sample) > 4096 {
		sample = sample[:4096]
	}
	bad := 0
	for _, c := range sample {
		if c == 0 {
			return false
		}
		if c < 0x20 && c != '\n' && c != '\r' && c != '\t' && c != 0x0c && c != 0x1b {
			bad++
		}
	}
	return bad*20 < len(sample)
}

// Embedded signatures found inside a file (after its start): another file's start.
type Embedded struct {
	Offset int
	Kind   string
}

// FindEmbedded scans for file signatures that start after the first byte; a byte pattern inside random data can match by chance, so
// the caller says so for short magics.
func FindEmbedded(b []byte, max int) []Embedded {
	var out []Embedded
	for _, m := range magics {
		if m.offset != 0 || len(m.bytes) < 4 {
			continue // only strong, offset-0 signatures
		}
		from := 1
		for len(out) < max {
			i := bytes.Index(b[from:], m.bytes)
			if i < 0 {
				break
			}
			out = append(out, Embedded{from + i, m.kind.Name})
			from += i + 1
		}
	}
	// RIFF containers carry their type at offset 8
	from := 1
	for len(out) < max {
		i := bytes.Index(b[from:], []byte("RIFF"))
		if i < 0 {
			break
		}
		at := from + i
		if at+12 <= len(b) {
			out = append(out, Embedded{at, "riff " + strings.TrimSpace(string(b[at+8:at+12]))})
		}
		from = at + 1
	}
	sortEmbedded(out)
	return out
}

func sortEmbedded(e []Embedded) {
	for i := 1; i < len(e); i++ {
		for j := i; j > 0 && e[j].Offset < e[j-1].Offset; j-- {
			e[j], e[j-1] = e[j-1], e[j]
		}
	}
}
