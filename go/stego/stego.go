package stego

import (
	"bytes"
	"context"
	"encoding/hex"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/Artifex-Technologies/Sarina/go/cli"
	"github.com/Artifex-Technologies/Sarina/go/llm"
	"github.com/Artifex-Technologies/Sarina/go/skills"
	"github.com/Artifex-Technologies/Sarina/go/tools"
)

// MaxFile is the most of a file the tool reads.
const MaxFile = 64 << 20

// Helpers are the outside programs an analyst would also reach for: which are installed is reported, and the installed ones among
// exiftool and binwalk are run (read-only). The others are only named: the model must never claim to have run them.
var Helpers = []string{"exiftool", "binwalk", "zsteg", "steghide", "stegseek", "foremost", "outguess"}

const evidenceNote = "\n[Computed, not guessed: say what this shows and no more. A sign is not proof; 'no sign' is not 'nothing hidden' (a payload placed with a key or in JPEG coefficients needs its own tool and password). Name only helpers this report says were run.]"

func init() {
	tools.RegisterProvider(Tools)
	skills.RegisterContext("stego", Prefetch)
	cli.Register("stego", "look for hidden data in a file: stego inspect|lsb|text|diff FILE [FILE2]; stego embed IN.png OUT.png TEXT (a test image)", command)
}

// Tools returns stego_inspect when SARINA_CRYPTO=on.
func Tools() []tools.Tool {
	if os.Getenv("SARINA_CRYPTO") != "on" {
		return nil
	}
	return []tools.Tool{cleanTool(), {
		Class: tools.ClassReadOnly,
		Spec: llm.ToolSpec{
			Name:        "stego_inspect",
			Description: "Look for hidden data in a file, read-only. action inspect (default): real type vs extension, metadata, data after the end of the format, files embedded inside, strings worth reading, and for images and WAV an LSB measurement and a first LSB extraction; lsb: extract the LSB bit stream (order rgb/r/g/b/rgba, 1-4 bits) to a temp file; text: invisible (zero-width, tag) characters in a text or in text given directly; watermark: every invisible or disguised mark in a text or code file or a whole folder (also variation selectors, look-alike letters, trailing whitespace bits); diff: compare two images pixel by pixel (other = the second file).",
			Parameters: map[string]any{"type": "object", "properties": map[string]any{
				"path":   map[string]any{"type": "string", "description": "The file (in the workspace)."},
				"text":   map[string]any{"type": "string", "description": "For action text without a file: the text itself."},
				"action": map[string]any{"type": "string", "enum": []string{"inspect", "lsb", "text", "watermark", "diff"}},
				"other":  map[string]any{"type": "string", "description": "For diff: the second image."},
				"order":  map[string]any{"type": "string", "description": "For lsb: rgb (default), r, g, b, rgba, bgr, a."},
				"bits":   map[string]any{"type": "integer", "minimum": 1, "maximum": 4, "description": "For lsb: low bits per sample (default 1)."},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			action, _ := a["action"].(string)
			if action == "" {
				action = "inspect"
			}
			path, _ := a["path"].(string)
			if action == "text" && path == "" {
				t, _ := a["text"].(string)
				if t == "" {
					return "", nil, fmt.Errorf("give text or a path")
				}
				return Watermarks(t).Describe() + evidenceNote, map[string]any{"action": "stego_text"}, nil
			}
			if action == "watermark" {
				if path == "" {
					t, _ := a["text"].(string)
					if t == "" {
						return "", nil, fmt.Errorf("give text or a path")
					}
					return Watermarks(t).Describe() + evidenceNote, map[string]any{"action": "stego_watermark"}, nil
				}
				if dir, ok := folderIn(path, tc); ok {
					return ScanFolder(dir, 2000) + evidenceNote, map[string]any{"action": "stego_watermark", "path": path}, nil
				}
			}
			b, err := readFile(path, tc)
			if err != nil {
				return "", nil, err
			}
			var out string
			switch action {
			case "inspect":
				out = Inspect(ctx, b, path)
			case "lsb":
				order, _ := a["order"].(string)
				bits := 1
				if f, ok := a["bits"].(float64); ok {
					bits = int(f)
				}
				out, err = ExtractReport(b, path, order, bits)
			case "text":
				// the full watermark scan: FindHidden alone missed variation selectors and called a marked file clean (eval wm-folder)
				out = Watermarks(string(b)).Describe()
			case "watermark":
				out = Watermarks(string(b)).Describe()
			case "diff":
				other, _ := a["other"].(string)
				b2, err2 := readFile(other, tc)
				if err2 != nil {
					return "", nil, err2
				}
				out, err = DiffReport(b, b2)
			default:
				return "", nil, fmt.Errorf("unknown action %q", action)
			}
			if err != nil {
				return "", nil, err
			}
			return out + evidenceNote, map[string]any{"action": "stego_" + action, "path": path}, nil
		},
	}}
}

func readFile(path string, tc *tools.Context) ([]byte, error) {
	if path == "" {
		return nil, fmt.Errorf("give a path")
	}
	root := ""
	if tc != nil {
		root = tc.Root
	}
	if root == "" {
		root, _ = os.Getwd()
	}
	resolved, err := tools.ResolveInRoot(root, path)
	if err != nil {
		return nil, err
	}
	if tools.IsSecretPath(path) || tools.IsSecretPath(resolved) {
		return nil, fmt.Errorf("refusing to read %s: it looks like a credentials or key file", path)
	}
	if tools.IsPrivate(resolved) {
		return nil, fmt.Errorf("refusing to read %s: the owner keeps that folder private from me", path)
	}
	st, err := os.Stat(resolved)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %v", path, err)
	}
	if !st.Mode().IsRegular() {
		return nil, fmt.Errorf("%s is not a regular file", path)
	}
	if st.Size() > MaxFile {
		return nil, fmt.Errorf("%s is %d bytes; I look at files up to %d", path, st.Size(), MaxFile)
	}
	return os.ReadFile(resolved)
}

// Inspect is the full read-only look at one file.
func Inspect(ctx context.Context, b []byte, name string) string {
	var out strings.Builder
	kind := Detect(b)
	ext := strings.TrimPrefix(strings.ToLower(filepath.Ext(name)), ".")
	fmt.Fprintf(&out, "File %s: %d bytes, really a %s file", filepath.Base(name), len(b), kind.Name)
	if ext != "" && kind.Name != "unknown" && kind.Name != "text" && !containsStr(kind.Exts, ext) {
		fmt.Fprintf(&out, " but named .%s: THE EXTENSION LIES about the type", ext)
	}
	out.WriteString(".\n")

	var c Container
	switch kind.Name {
	case "png":
		c = ParsePNG(b)
	case "jpeg":
		c = ParseJPEG(b)
	case "gif":
		c = ParseGIF(b)
	case "wav", "webp", "avi":
		c = ParseRIFF(b)
	default:
		c.End = -1
	}
	if len(c.Parts) > 0 {
		fmt.Fprintf(&out, "\nStructure: %s.\n", strings.Join(c.Parts, "; "))
	}
	for _, p := range c.Problems {
		out.WriteString("Problem: " + p + ".\n")
	}
	if len(c.Text) > 0 {
		out.WriteString("Metadata text:\n")
		for _, t := range c.Text {
			out.WriteString("  " + t + "\n")
		}
	} else if kind.Name == "png" || kind.Name == "jpeg" || kind.Name == "gif" || kind.Name == "wav" {
		out.WriteString("Metadata text: none (no comments, EXIF text or text chunks).\n")
	}
	for _, u := range c.Unusual {
		out.WriteString("Unusual part: " + u + ".\n")
	}
	if t := c.Trailing(b); len(t) > 0 {
		k := Detect(t)
		fmt.Fprintf(&out, "DATA AFTER THE END OF THE %s (%d bytes at offset %d): looks like %s", strings.ToUpper(kind.Name), len(t), c.End, k.Name)
		if k.Name == "text" {
			fmt.Fprintf(&out, ": %q", clipText(string(t), 300))
		} else {
			fmt.Fprintf(&out, "; first bytes %s", hex.EncodeToString(t[:min(16, len(t))]))
		}
		out.WriteString(". Viewers ignore this, so it is a classic hiding place: carve it out (dd or tail -c) and look at it as its own file.\n")
	} else if c.End > 0 {
		out.WriteString("Nothing after the end of the format.\n")
	}
	if emb := FindEmbedded(b, 12); len(emb) > 0 {
		out.WriteString("File signatures inside the file:")
		for _, e := range emb {
			fmt.Fprintf(&out, " %s at offset %d;", e.Kind, e.Offset)
		}
		out.WriteString(" (a signature inside compressed data can be a coincidence; one after the format's end, or a zip/pdf/elf, is a real embedded file).\n")
	} else {
		out.WriteString("No other file signatures inside it.\n")
	}
	worth, total := Strings(b, 8, 15)
	fmt.Fprintf(&out, "Strings: %d printable runs of 8+ characters", total)
	if len(worth) > 0 {
		out.WriteString("; worth reading:\n")
		for _, s := range worth {
			out.WriteString("  " + s + "\n")
		}
	} else {
		out.WriteString("; none look like URLs, flags, keys or long base64.\n")
	}
	switch kind.Name {
	case "png", "jpeg", "gif", "bmp":
		if p, err := DecodeImage(b); err == nil {
			fmt.Fprintf(&out, "\nImage %dx%d (%s%s). LSB measurement: %s\n", p.W, p.H, p.Format, map[bool]string{true: ", paletted", false: ""}[p.Indices != nil], Analyse(p).Describe())
			if kind.Name == "jpeg" {
				out.WriteString("This is a JPEG: its pixels are decoded from DCT coefficients, so pixel-LSB hiding does not survive in it; JPEG steganography (steghide, outguess, F5) lives in the coefficients and needs those tools and the password.\n")
			}
			found := SearchLSB(p, 4096)
			if len(found) == 0 {
				out.WriteString("LSB extraction (orders rgb, r, g, b, rgba, bgr; both bit orders; 1 bit): nothing that starts like text or a known file.\n")
				out.WriteString("Verdict: NO readable LSB payload found. " + map[bool]string{
					true:  "The equalised LSB statistic is then most likely noise (or a payload that is encrypted or placed with a key, which only its own tool and password can show).",
					false: "Nothing points at LSB hiding in this image.",
				}[strings.Contains(Analyse(p).Verdict, "equalised in")] + "\n")
			} else {
				out.WriteString("Verdict: an LSB payload IS in this image (extracted below).\n")
			}
			for _, f := range found {
				fmt.Fprintf(&out, "LSB EXTRACTION FOUND something: order %s, %s bit first, %s", f.Order, map[bool]string{true: "most significant", false: "least significant"}[f.MSBFirst], f.Why)
				if f.Length > 0 {
					fmt.Fprintf(&out, " (%d bytes)", f.Length)
				}
				if textLike(f.Data) {
					fmt.Fprintf(&out, ":\n  %q\n", clipText(string(f.Data), 600))
				} else {
					fmt.Fprintf(&out, "; first bytes %s\n", hex.EncodeToString(f.Data[:min(32, len(f.Data))]))
				}
			}
		} else {
			out.WriteString("\nCould not decode the image for an LSB look: " + err.Error() + "\n")
		}
	case "wav":
		if w, err := ParseWAV(b); err == nil {
			ones, n := w.LSBStats()
			fmt.Fprintf(&out, "\nWAV: %d channel(s), %d Hz, %d bits, %d samples; sample LSB ones %.1f%%.\n", w.Channels, w.Rate, w.Bits, n, 100*ones)
			found := SearchWAV(w, 4096)
			if len(found) == 0 {
				out.WriteString("Sample-LSB extraction (both bit orders): nothing that starts like text or a known file.\n")
			}
			for _, f := range found {
				fmt.Fprintf(&out, "SAMPLE-LSB EXTRACTION FOUND something: %s bit first, %s:\n  %q\n", map[bool]string{true: "most significant", false: "least significant"}[f.MSBFirst], f.Why, clipText(string(f.Data), 600))
			}
		} else {
			out.WriteString("\nWAV: " + err.Error() + "\n")
		}
	case "text":
		out.WriteString("\nInvisible characters and other marks: " + Watermarks(string(b)).Describe() + "\n")
	}
	out.WriteString("\n" + helpersReport(ctx, name))
	return out.String()
}

func containsStr(xs []string, x string) bool {
	for _, v := range xs {
		if v == x {
			return true
		}
	}
	return false
}

// helpersReport says which outside tools are installed, and runs exiftool and binwalk when they are.
func helpersReport(ctx context.Context, path string) string {
	var have, missing []string
	for _, h := range Helpers {
		if _, err := exec.LookPath(h); err == nil {
			have = append(have, h)
		} else {
			missing = append(missing, h)
		}
	}
	var b strings.Builder
	fmt.Fprintf(&b, "Helper programs: installed: %s; not installed: %s.", orNone(have), orNone(missing))
	for _, h := range have {
		var args []string
		switch h {
		case "exiftool":
			args = []string{"-S", "-q", "-q", path}
		case "binwalk":
			args = []string{path}
		default:
			continue
		}
		ctx2, cancel := context.WithTimeout(ctx, 20*time.Second)
		cmd := exec.CommandContext(ctx2, h, args...)
		var out bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &out
		err := cmd.Run()
		cancel()
		text := strings.TrimSpace(out.String())
		if len(text) > 2500 {
			text = text[:2500] + "\n[…]"
		}
		if err != nil && text == "" {
			text = "(" + err.Error() + ")"
		}
		fmt.Fprintf(&b, "\n%s was run:\n%s", h, text)
	}
	if len(have) == 0 || (len(have) == 1 && have[0] != "exiftool" && have[0] != "binwalk") {
		b.WriteString(" None of them was run.")
	}
	return b.String()
}

func orNone(xs []string) string {
	if len(xs) == 0 {
		return "none"
	}
	return strings.Join(xs, ", ")
}

// outDir is where extracted bytes go.
func outDir() (string, error) {
	return os.MkdirTemp("", "sarina-stego-")
}

// ExtractReport extracts the LSB stream of an image in one order and writes it to a temporary file.
func ExtractReport(b []byte, name, order string, bits int) (string, error) {
	if order == "" {
		order = "rgb"
	}
	var o *Order
	for i := range Orders {
		if Orders[i].Name == order {
			o = &Orders[i]
		}
	}
	if o == nil {
		return "", fmt.Errorf("unknown order %q (rgb, r, g, b, rgba, bgr, a)", order)
	}
	kind := Detect(b)
	var data []byte
	var how string
	if kind.Name == "wav" {
		w, err := ParseWAV(b)
		if err != nil {
			return "", err
		}
		data = w.LSBBytes(true, 1<<22)
		how = "sample LSBs, most significant bit first"
	} else {
		p, err := DecodeImage(b)
		if err != nil {
			return "", err
		}
		data = ExtractBits(p, *o, bits, true, 1<<22)
		how = fmt.Sprintf("order %s, %d low bit(s) per sample, most significant bit first", o.Name, bits)
	}
	dir, err := outDir()
	if err != nil {
		return "", err
	}
	path := filepath.Join(dir, strings.TrimSuffix(filepath.Base(name), filepath.Ext(name))+"-lsb-"+order+".bin")
	if err := os.WriteFile(path, data, 0o600); err != nil {
		return "", err
	}
	var out strings.Builder
	fmt.Fprintf(&out, "Extracted %d bytes (%s) to %s (a temporary folder; it is not kept).\n", len(data), how, path)
	if f, ok := judge(data); ok {
		fmt.Fprintf(&out, "It %s", f.Why)
		if f.Length > 0 {
			fmt.Fprintf(&out, " (%d bytes)", f.Length)
		}
		if textLike(f.Data) {
			fmt.Fprintf(&out, ":\n%s\n", clipText(string(f.Data), 2000))
		} else {
			out.WriteString(".\n")
		}
	} else {
		fmt.Fprintf(&out, "It does not start like text or a known file. First bytes: %s\n", hex.EncodeToString(data[:min(48, len(data))]))
	}
	return out.String(), nil
}

// DiffReport compares two image files.
func DiffReport(a, b []byte) (string, error) {
	pa, err := DecodeImage(a)
	if err != nil {
		return "", fmt.Errorf("first image: %v", err)
	}
	pb, err := DecodeImage(b)
	if err != nil {
		return "", fmt.Errorf("second image: %v", err)
	}
	return Diff(pa, pb)
}

var pathRE = regexp.MustCompile(`(?:~|\.{0,2})?/?[\w./~-]+\.(png|jpe?g|gif|bmp|wav|txt|md|pdf|zip|bin)\b`)

// Prefetch (skill prefetch "stego") inspects a file the task names, if it exists and is a regular file in or under the current folder.
func Prefetch(task string) string {
	cwd, _ := os.Getwd()
	var parts []string
	for _, m := range pathRE.FindAllString(task, 3) {
		p := m
		if strings.HasPrefix(p, "~/") {
			if h, err := os.UserHomeDir(); err == nil {
				p = filepath.Join(h, p[2:])
			}
		}
		if !filepath.IsAbs(p) {
			p = filepath.Join(cwd, p)
		}
		resolved, err := tools.ResolveInRoot(cwd, p)
		if err != nil || tools.IsSecretPath(p) || tools.IsPrivate(resolved) {
			continue
		}
		st, err := os.Stat(resolved)
		if err != nil || !st.Mode().IsRegular() || st.Size() > MaxFile {
			continue
		}
		b, err := os.ReadFile(resolved)
		if err != nil {
			continue
		}
		ctx, cancel := context.WithTimeout(context.Background(), 40*time.Second)
		parts = append(parts, Inspect(ctx, b, m))
		cancel()
	}
	if len(parts) == 0 {
		return ""
	}
	return "Already looked at the file for you (stego_inspect, read-only; call it with action lsb, text or diff for more):\n\n" + strings.Join(parts, "\n\n")
}

func command(env *cli.Env, args []string) int {
	if len(args) < 2 {
		fmt.Fprintln(env.Stderr, "usage: sarina stego inspect FILE | lsb FILE [ORDER] | text FILE | watermark FILE|DIR | clean FILE | diff A B | embed IN.png OUT.png TEXT")
		return 2
	}
	read := func(p string) []byte {
		b, err := os.ReadFile(p)
		if err != nil {
			fmt.Fprintln(env.Stderr, "sarina stego:", err)
			os.Exit(2)
		}
		return b
	}
	var out string
	var err error
	switch args[0] {
	case "inspect":
		out = Inspect(context.Background(), read(args[1]), args[1])
	case "lsb":
		order := "rgb"
		if len(args) > 2 {
			order = args[2]
		}
		out, err = ExtractReport(read(args[1]), args[1], order, 1)
	case "text":
		out = Watermarks(string(read(args[1]))).Describe()
	case "watermark":
		if st, e := os.Stat(args[1]); e == nil && st.IsDir() {
			out = ScanFolder(args[1], 5000)
		} else {
			out = Watermarks(string(read(args[1]))).Describe()
		}
	case "clean":
		var copyPath, report string
		copyPath, report, err = Clean(read(args[1]), args[1])
		out = report
		if copyPath != "" {
			out += "\nCleaned copy: " + copyPath + " (the original is unchanged)"
		}
	case "diff":
		if len(args) < 3 {
			err = fmt.Errorf("diff needs two files")
		} else {
			out, err = DiffReport(read(args[1]), read(args[2]))
		}
	case "embed":
		if len(args) < 4 {
			err = fmt.Errorf("embed IN.png OUT.png TEXT")
			break
		}
		p, err2 := DecodeImage(read(args[1]))
		if err2 != nil {
			err = err2
			break
		}
		img := toImage(p)
		var png []byte
		png, err = Embed(img, []byte(strings.Join(args[3:], " ")))
		if err == nil {
			err = os.WriteFile(args[2], png, 0o644)
			out = "wrote " + args[2]
		}
	default:
		err = fmt.Errorf("unknown subcommand %q", args[0])
	}
	if err != nil {
		fmt.Fprintln(env.Stderr, "sarina stego:", err)
		return 2
	}
	fmt.Fprintln(env.Stdout, out)
	return 0
}

// folderIn is path as a folder inside the workspace (and not a private one), for a folder-wide watermark scan.
func folderIn(path string, tc *tools.Context) (string, bool) {
	root := ""
	if tc != nil {
		root = tc.Root
	}
	if root == "" {
		root, _ = os.Getwd()
	}
	resolved, err := tools.ResolveInRoot(root, path)
	if err != nil || tools.IsPrivate(resolved) {
		return "", false
	}
	st, err := os.Stat(resolved)
	return resolved, err == nil && st.IsDir()
}

// cleanTool writes a cleaned copy of a text, code or picture file next to it; the original is never changed.
func cleanTool() tools.Tool {
	return tools.Tool{
		Requires: tools.PermWrite, Class: tools.ClassMutating,
		Spec: llm.ToolSpec{
			Name:        "stego_clean",
			Description: "Write a cleaned copy of a file next to it (NAME.clean.EXT); the original is not changed. Text and code: invisible characters, stray variation selectors and trailing whitespace removed (look-alike letters are only listed). Pictures: metadata and appended data dropped, the lowest bit of every colour randomised (removes LSB payloads; invisible).",
			Parameters: map[string]any{"type": "object", "required": []string{"path"}, "properties": map[string]any{
				"path": map[string]any{"type": "string", "description": "The file (in the workspace)."},
			}},
		},
		Handler: func(ctx context.Context, a map[string]any, tc *tools.Context) (string, map[string]any, error) {
			path, _ := a["path"].(string)
			b, err := readFile(path, tc)
			if err != nil {
				return "", nil, err
			}
			resolved, _ := tools.ResolveInRoot(tc.Root, path)
			out, report, err := Clean(b, resolved)
			if err != nil {
				return "", nil, err
			}
			if out == "" {
				return report, map[string]any{"action": "stego_clean", "path": path}, nil
			}
			rel, _ := filepath.Rel(tc.Root, out)
			return report + "\nThe original " + path + " is unchanged; the cleaned copy is " + rel + ". Compare them before replacing the original (that is the owner's choice).",
				map[string]any{"action": "stego_clean", "path": path, "copy": rel}, nil
		},
	}
}

// Clean writes the cleaned copy of the file at path (whose bytes are b) and says what was removed. It refuses to overwrite anything.
func Clean(b []byte, path string) (string, string, error) {
	var data []byte
	var report, ext string
	switch strings.ToLower(filepath.Ext(path)) {
	case ".png", ".jpg", ".jpeg", ".gif":
		var err error
		if data, ext, err = CleanImage(b); err != nil {
			return "", "", err
		}
		report = fmt.Sprintf("Cleaned the picture: written again from its pixels only (metadata and anything after the end of the format dropped: %d bytes became %d), the lowest bit of every colour randomised.", len(b), len(data))
	default:
		if bytes.IndexByte(b, 0) >= 0 {
			return "", "", fmt.Errorf("%s is binary and not a picture: there is no safe way to clean it here", filepath.Base(path))
		}
		clean, removed := CleanText(string(b))
		if len(removed) == 0 {
			return "", "Nothing to clean: no invisible characters, stray variation selectors or trailing whitespace.", nil
		}
		data = []byte(clean)
		report = "Removed: " + strings.Join(removed, ", ") + "."
		if hs := Watermarks(string(b)).Homoglyphs; len(hs) > 0 {
			report += fmt.Sprintf(" Not changed: %d word(s) with look-alike letters from another alphabet (they could be meant; fix them by hand), for example line %d: %q.", len(hs), hs[0].Line, hs[0].Word)
		}
	}
	out := cleanPath(path, ext)
	if _, err := os.Stat(out); err == nil {
		return "", "", fmt.Errorf("%s already exists: I do not overwrite it", filepath.Base(out))
	}
	if err := os.WriteFile(out, data, 0o644); err != nil {
		return "", "", err
	}
	return out, report, nil
}
