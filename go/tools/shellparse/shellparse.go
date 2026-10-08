// Package shellparse splits a POSIX-shell command line into the simple
// commands it would run, so a safety check can look at real program names and
// arguments instead of pattern-matching the raw string.
//
// It is a conservative reader, not an executor. It understands command
// separators (; & && || | |& newline), subshells and brace groups, command
// substitution ($(...) and backticks), process substitution, quoting
// (single, double, backslash, $'...'), here-documents and redirections. Every
// command it finds is returned in one flat list, including those nested inside
// substitutions, so a caller cannot miss `echo $(rm x)` or `( rm x )`.
//
// Anything whose value is only known at run time (a variable, a substitution,
// a glob) is marked with the Dyn character in Word.Text so callers can refuse
// to guess at it.
package shellparse

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
	"unicode/utf8"
)

// Dyn replaces any part of a word that is only known at run time.
const Dyn = "\x00"

// Word is one shell word with quoting removed.
type Word struct {
	Text    string // unquoted text; run-time parts are Dyn
	Quoted  bool   // some part was quoted or escaped
	ProcSub bool   // contains a <(...) or >(...) process substitution
}

// Dynamic reports whether any part of the word is unknown until run time.
func (w Word) Dynamic() bool { return strings.Contains(w.Text, Dyn) }

// Redirect is one redirection on a command.
type Redirect struct {
	Op     string // ">", ">>", ">|", "&>", "&>>", "<>", ">&", "<", "<&", "<<", "<<-", "<<<"
	Target Word
}

// Writes reports whether the redirection can create or truncate a file.
func (r Redirect) Writes() bool {
	switch r.Op {
	case ">", ">>", ">|", "&>", "&>>", "<>":
		return true
	case ">&":
		// ">&2" and ">&-" duplicate or close a descriptor; ">&file" writes one.
		t := r.Target.Text
		if t == "-" {
			return false
		}
		if _, err := strconv.Atoi(t); err == nil {
			return false
		}
		return true
	}
	return false
}

// Command is one simple command.
type Command struct {
	Assigns   []Word     // leading NAME=value words
	Words     []Word     // program and arguments
	Redirects []Redirect // redirections attached to it
	Heredocs  []string   // bodies of here-documents attached to it
	PipeID    int        // non-zero: commands sharing an id form one pipeline, in order
	FuncName  string     // set when this command is a function definition header
	Data      bool       // a for/case word list: the words are data, not a command
}

// Script is every command found, in source order, nested ones included.
type Script struct {
	Commands []*Command
}

// Parse reads src. It fails on unterminated quotes and substitutions, which a
// shell would reject too.
func Parse(src string) (*Script, error) {
	p := &parser{src: src}
	if err := p.parseSeq(0); err != nil {
		return nil, err
	}
	return &Script{Commands: p.out}, nil
}

const maxDepth = 40

type pendingHere struct {
	cmd   *Command
	delim string
	strip bool
	raw   bool // delimiter was quoted: the body is literal
}

type parser struct {
	src   string
	pos   int
	out   []*Command
	pipes *int
	depth int
	here  []pendingHere
}

func (p *parser) nextPipe() int {
	// pipeline ids are unique across nested parsers through a shared counter
	if p.pipes == nil {
		p.pipes = new(int)
	}
	*p.pipes++
	return *p.pipes
}

func (p *parser) eof() bool { return p.pos >= len(p.src) }

func (p *parser) peek() byte {
	if p.eof() {
		return 0
	}
	return p.src[p.pos]
}

func (p *parser) peekAt(n int) byte {
	if p.pos+n >= len(p.src) {
		return 0
	}
	return p.src[p.pos+n]
}

var reserved = map[string]bool{
	"if": true, "then": true, "else": true, "elif": true, "fi": true, "while": true, "until": true,
	"do": true, "done": true, "{": true, "}": true, "!": true, "time": true, "coproc": true, "esac": true,
}

func isAssign(w Word) bool {
	t := w.Text
	i := strings.IndexByte(t, '=')
	if i <= 0 {
		return false
	}
	name := strings.TrimSuffix(t[:i], "+")
	if name == "" {
		return false
	}
	for j, r := range name {
		if !(r == '_' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || j > 0 && r >= '0' && r <= '9') {
			return false
		}
	}
	return true
}

// parseSeq reads commands until EOF (term == 0) or the matching ')' (term == ')').
func (p *parser) parseSeq(term byte) error {
	p.depth++
	defer func() { p.depth-- }()
	if p.depth > maxDepth {
		return errors.New("command nesting is too deep")
	}
	cmd := &Command{}
	pipe := 0
	skipNext := 0 // words to ignore after for/select (the loop variable)
	isCase := false

	finish := func(sep byte) {
		if len(cmd.Words) > 0 || len(cmd.Assigns) > 0 || len(cmd.Redirects) > 0 || cmd.FuncName != "" {
			if sep == '|' && pipe == 0 {
				pipe = p.nextPipe()
			}
			cmd.PipeID = pipe
			p.out = append(p.out, cmd)
		}
		if sep != '|' {
			pipe = 0
		}
		cmd = &Command{}
		skipNext, isCase = 0, false
	}
	atStart := func() bool { return len(cmd.Words) == 0 && len(cmd.Assigns) == 0 }

	for {
		// blanks and line continuations
		for !p.eof() {
			c := p.peek()
			if c == ' ' || c == '\t' || c == '\r' {
				p.pos++
			} else if c == '\\' && p.peekAt(1) == '\n' {
				p.pos += 2
			} else {
				break
			}
		}
		if p.eof() {
			finish(0)
			if term != 0 {
				return errors.New("unterminated '(' in command")
			}
			return nil
		}
		c := p.peek()
		switch {
		case c == '#':
			for !p.eof() && p.peek() != '\n' {
				p.pos++
			}
		case c == '\n':
			p.pos++
			if !(atStart() && pipe != 0 && len(cmd.Redirects) == 0) { // "a |\n b" is still one pipeline
				finish(0)
			}
			if err := p.readHeredocs(); err != nil {
				return err
			}
		case c == ';':
			p.pos++
			for p.peek() == ';' || p.peek() == '&' {
				p.pos++
			}
			finish(0)
		case c == '&':
			if p.peekAt(1) == '>' { // &> and &>>
				p.pos++
				if err := p.readRedirect(cmd, "&"); err != nil {
					return err
				}
				continue
			}
			p.pos++
			if p.peek() == '&' {
				p.pos++
			}
			finish(0)
		case c == '|':
			p.pos++
			if p.peek() == '|' {
				p.pos++
				finish(0)
			} else {
				if p.peek() == '&' {
					p.pos++
				}
				finish('|')
			}
		case c == ')':
			p.pos++
			finish(0)
			if term == ')' {
				return nil
			}
			// A stray ')' (a case pattern terminator) just ends a command.
		case c == '(':
			if len(cmd.Words) == 1 && len(cmd.Assigns) == 0 && p.peekAt(1) == ')' && !cmd.Words[0].Dynamic() {
				// name() function definition
				p.pos += 2
				cmd.FuncName = cmd.Words[0].Text
				cmd.Words = nil
				finish(0)
				continue
			}
			p.pos++
			if err := p.parseSeq(')'); err != nil {
				return err
			}
		case (c == '<' || c == '>') && p.peekAt(1) != '(':
			if err := p.readRedirect(cmd, ""); err != nil {
				return err
			}
		default:
			// "2>file" style descriptor prefix
			if c >= '0' && c <= '9' {
				j := p.pos
				for j < len(p.src) && p.src[j] >= '0' && p.src[j] <= '9' {
					j++
				}
				if j < len(p.src) && (p.src[j] == '<' || p.src[j] == '>') && (j+1 >= len(p.src) || p.src[j+1] != '(') {
					p.pos = j
					continue
				}
			}
			w, err := p.readWord()
			if err != nil {
				return err
			}
			if skipNext > 0 {
				skipNext--
				continue
			}
			if atStart() && !w.Quoted && reserved[w.Text] {
				continue
			}
			if atStart() && !w.Quoted {
				switch w.Text {
				case "for", "select":
					skipNext = 1
					cmd.Data = true
					continue
				case "case":
					cmd.Data = true
					isCase = true
					continue
				case "function":
					// "function name": the next word names it
					nw, err := p.readWordIfAny()
					if err != nil {
						return err
					}
					cmd.FuncName = nw.Text
					finish(0)
					if p.peek() == '(' && p.peekAt(1) == ')' {
						p.pos += 2
					}
					continue
				}
			}
			if cmd.Data && !w.Quoted && w.Text == "in" {
				if isCase {
					finish(0)
				}
				continue
			}
			if !cmd.Data && len(cmd.Words) == 0 && isAssign(w) {
				cmd.Assigns = append(cmd.Assigns, w)
				continue
			}
			cmd.Words = append(cmd.Words, w)
		}
	}
}

func (p *parser) readWordIfAny() (Word, error) {
	for !p.eof() && (p.peek() == ' ' || p.peek() == '\t') {
		p.pos++
	}
	if p.eof() || strings.IndexByte(";&|()<>\n", p.peek()) >= 0 {
		return Word{}, nil
	}
	return p.readWord()
}

func isMeta(c byte) bool { return strings.IndexByte(" \t\r\n;&|()<>", c) >= 0 }

// readWord reads one word starting at the current position.
func (p *parser) readWord() (Word, error) {
	var b strings.Builder
	var w Word
	for !p.eof() {
		c := p.peek()
		if (c == '<' || c == '>') && p.peekAt(1) == '(' {
			if err := p.procSub(); err != nil {
				return w, err
			}
			b.WriteString(Dyn)
			w.ProcSub = true
			continue
		}
		if isMeta(c) {
			break
		}
		switch c {
		case '\\':
			if p.peekAt(1) == '\n' {
				p.pos += 2
				continue
			}
			w.Quoted = true
			if p.pos+1 < len(p.src) {
				_, size := utf8.DecodeRuneInString(p.src[p.pos+1:])
				b.WriteString(p.src[p.pos+1 : p.pos+1+size])
				p.pos += 1 + size
			} else {
				p.pos++
			}
		case '\'':
			w.Quoted = true
			end := strings.IndexByte(p.src[p.pos+1:], '\'')
			if end < 0 {
				return w, errors.New("unterminated single quote")
			}
			b.WriteString(p.src[p.pos+1 : p.pos+1+end])
			p.pos += end + 2
		case '"':
			w.Quoted = true
			p.pos++
			if err := p.readDouble(&b); err != nil {
				return w, err
			}
		case '`':
			if err := p.backtick(); err != nil {
				return w, err
			}
			b.WriteString(Dyn)
		case '$':
			if err := p.readDollar(&b, &w); err != nil {
				return w, err
			}
		case '*', '?', '[':
			b.WriteByte(c) // glob characters stay visible; callers decide
			p.pos++
		default:
			b.WriteByte(c)
			p.pos++
		}
	}
	w.Text = b.String()
	return w, nil
}

// readDollar handles $ outside of single quotes.
func (p *parser) readDollar(b *strings.Builder, w *Word) error {
	next := p.peekAt(1)
	switch {
	case next == '\'': // $'...' ANSI-C quoting
		w.Quoted = true
		p.pos += 2
		for {
			if p.eof() {
				return errors.New("unterminated $'...' quote")
			}
			c := p.peek()
			if c == '\'' {
				p.pos++
				return nil
			}
			if c == '\\' {
				p.pos++
				p.ansiEscape(b)
				continue
			}
			b.WriteByte(c)
			p.pos++
		}
	case next == '"':
		w.Quoted = true
		p.pos += 2
		return p.readDouble(b)
	case next == '(':
		p.pos += 2
		if err := p.parseSeq(')'); err != nil {
			return err
		}
		b.WriteString(Dyn)
		return nil
	case next == '{':
		p.pos += 2
		if err := p.skipBraceParam(); err != nil {
			return err
		}
		b.WriteString(Dyn)
		return nil
	case next == '_' || next >= 'a' && next <= 'z' || next >= 'A' && next <= 'Z':
		p.pos++
		for !p.eof() {
			c := p.peek()
			if c == '_' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' {
				p.pos++
			} else {
				break
			}
		}
		b.WriteString(Dyn)
		return nil
	case next >= '0' && next <= '9' || strings.IndexByte("@*#?-$!", next) >= 0 && next != 0:
		p.pos += 2
		b.WriteString(Dyn)
		return nil
	}
	b.WriteByte('$')
	p.pos++
	return nil
}

// ansiEscape decodes one backslash escape inside $'...'. The backslash is consumed.
func (p *parser) ansiEscape(b *strings.Builder) {
	if p.eof() {
		return
	}
	c := p.peek()
	p.pos++
	switch c {
	case 'n':
		b.WriteByte('\n')
	case 't':
		b.WriteByte('\t')
	case 'r':
		b.WriteByte('\r')
	case 'a':
		b.WriteByte(7)
	case 'b':
		b.WriteByte(8)
	case 'e', 'E':
		b.WriteByte(27)
	case 'f':
		b.WriteByte(12)
	case 'v':
		b.WriteByte(11)
	case 'x', 'u', 'U':
		max := map[byte]int{'x': 2, 'u': 4, 'U': 8}[c]
		n, digits := 0, 0
		for digits < max && !p.eof() {
			d, ok := hexVal(p.peek())
			if !ok {
				break
			}
			n = n*16 + d
			digits++
			p.pos++
		}
		if c == 'x' {
			b.WriteByte(byte(n))
		} else {
			b.WriteRune(rune(n))
		}
	case '0', '1', '2', '3', '4', '5', '6', '7':
		n := int(c - '0')
		for i := 0; i < 2 && !p.eof() && p.peek() >= '0' && p.peek() <= '7'; i++ {
			n = n*8 + int(p.peek()-'0')
			p.pos++
		}
		b.WriteByte(byte(n))
	default:
		b.WriteByte(c)
	}
}

func hexVal(c byte) (int, bool) {
	switch {
	case c >= '0' && c <= '9':
		return int(c - '0'), true
	case c >= 'a' && c <= 'f':
		return int(c-'a') + 10, true
	case c >= 'A' && c <= 'F':
		return int(c-'A') + 10, true
	}
	return 0, false
}

// readDouble reads the inside of "..." (the opening quote is consumed) and
// consumes the closing quote.
func (p *parser) readDouble(b *strings.Builder) error {
	for {
		if p.eof() {
			return errors.New("unterminated double quote")
		}
		c := p.peek()
		switch c {
		case '"':
			p.pos++
			return nil
		case '\\':
			n := p.peekAt(1)
			if n == '\n' {
				p.pos += 2
			} else if n == '$' || n == '`' || n == '"' || n == '\\' {
				b.WriteByte(n)
				p.pos += 2
			} else {
				b.WriteByte('\\')
				p.pos++
			}
		case '`':
			if err := p.backtick(); err != nil {
				return err
			}
			b.WriteString(Dyn)
		case '$':
			var w Word
			next := p.peekAt(1)
			if next == '\'' || next == '"' { // not special inside double quotes
				b.WriteByte('$')
				p.pos++
				continue
			}
			if err := p.readDollar(b, &w); err != nil {
				return err
			}
		default:
			b.WriteByte(c)
			p.pos++
		}
	}
}

// skipBraceParam skips ${...}, still parsing substitutions inside it.
func (p *parser) skipBraceParam() error {
	depth := 1
	for !p.eof() {
		c := p.peek()
		switch {
		case c == '\\':
			p.pos += 2
			continue
		case c == '$' && p.peekAt(1) == '(':
			p.pos += 2
			if err := p.parseSeq(')'); err != nil {
				return err
			}
			continue
		case c == '`':
			if err := p.backtick(); err != nil {
				return err
			}
			continue
		case c == '{':
			depth++
		case c == '}':
			depth--
			if depth == 0 {
				p.pos++
				return nil
			}
		}
		p.pos++
	}
	return errors.New("unterminated ${...}")
}

// backtick parses `...` (the opening backtick is at pos).
func (p *parser) backtick() error {
	p.pos++
	var inner strings.Builder
	for {
		if p.eof() {
			return errors.New("unterminated backtick substitution")
		}
		c := p.peek()
		if c == '\\' && p.pos+1 < len(p.src) {
			n := p.src[p.pos+1]
			if n == '`' || n == '\\' || n == '$' {
				inner.WriteByte(n)
				p.pos += 2
				continue
			}
		}
		if c == '`' {
			p.pos++
			break
		}
		inner.WriteByte(c)
		p.pos++
	}
	return p.sub(inner.String())
}

// sub parses text as its own script, adding its commands to the output.
func (p *parser) sub(text string) error {
	if p.depth >= maxDepth {
		return errors.New("command nesting is too deep")
	}
	if p.pipes == nil {
		p.pipes = new(int)
	}
	q := &parser{src: text, pipes: p.pipes, depth: p.depth}
	if err := q.parseSeq(0); err != nil {
		return err
	}
	p.out = append(p.out, q.out...)
	return nil
}

// procSub parses <(...) or >(...).
func (p *parser) procSub() error {
	p.pos += 2
	return p.parseSeq(')')
}

// readRedirect reads a redirection operator and its target. prefix "&" means
// the leading '&' of &> or &>> was already consumed.
func (p *parser) readRedirect(cmd *Command, prefix string) error {
	op := prefix
	c := p.peek()
	p.pos++
	op += string(c)
	n := p.peek()
	switch {
	case c == '>' && (n == '>' || n == '|'):
		op += string(n)
		p.pos++
	case c == '>' && n == '&' && prefix == "":
		op += "&"
		p.pos++
	case c == '<' && n == '&':
		op += "&"
		p.pos++
	case c == '<' && n == '>':
		op += ">"
		p.pos++
	case c == '<' && n == '<':
		op += "<"
		p.pos++
		if p.peek() == '<' {
			op += "<"
			p.pos++
		} else if p.peek() == '-' {
			op += "-"
			p.pos++
		}
	}
	for !p.eof() && (p.peek() == ' ' || p.peek() == '\t') {
		p.pos++
	}
	if p.eof() || strings.IndexByte(";&|()\n", p.peek()) >= 0 && p.peek() != '&' {
		return fmt.Errorf("redirection %s has no target", op)
	}
	target, err := p.readWord()
	if err != nil {
		return err
	}
	cmd.Redirects = append(cmd.Redirects, Redirect{Op: op, Target: target})
	if op == "<<" || op == "<<-" {
		p.here = append(p.here, pendingHere{cmd: cmd, delim: target.Text, strip: op == "<<-", raw: target.Quoted})
	}
	return nil
}

// readHeredocs consumes the bodies of pending here-documents, which start on
// the line after the operator. For an unquoted delimiter the shell expands
// substitutions in the body, so those are parsed as commands too.
func (p *parser) readHeredocs() error {
	for _, h := range p.here {
		var body strings.Builder
		for !p.eof() {
			end := strings.IndexByte(p.src[p.pos:], '\n')
			var line string
			if end < 0 {
				line = p.src[p.pos:]
				p.pos = len(p.src)
			} else {
				line = p.src[p.pos : p.pos+end]
				p.pos += end + 1
			}
			cmp := strings.TrimSuffix(line, "\r")
			if h.strip {
				cmp = strings.TrimLeft(cmp, "\t")
			}
			if cmp == h.delim {
				break
			}
			body.WriteString(line)
			body.WriteByte('\n')
		}
		h.cmd.Heredocs = append(h.cmd.Heredocs, body.String())
		if !h.raw {
			if err := p.expansionsIn(body.String()); err != nil {
				return err
			}
		}
	}
	p.here = nil
	return nil
}

// expansionsIn parses the substitutions inside text that the shell would expand.
func (p *parser) expansionsIn(text string) error {
	q := &parser{src: text, pipes: p.pipes, depth: p.depth}
	var sink strings.Builder
	for !q.eof() {
		switch q.peek() {
		case '\\':
			q.pos += 2
		case '`':
			if err := q.backtick(); err != nil {
				return err
			}
		case '$':
			if q.peekAt(1) == '(' || q.peekAt(1) == '{' {
				var w Word
				if err := q.readDollar(&sink, &w); err != nil {
					return err
				}
			} else {
				q.pos++
			}
		default:
			q.pos++
		}
	}
	p.out = append(p.out, q.out...)
	return nil
}
