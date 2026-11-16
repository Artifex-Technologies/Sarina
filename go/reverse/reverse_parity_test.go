package reverse

import (
	"context"
	"fmt"
	"net"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
)

// TestParityReverse records (testdata/parity/reverse.json) the pure parts of the reverse-engineering tool: the network
// map of a program's strings (with net.ParseIP and SplitHostPort on the candidates), the condensed overview, the
// program's own functions from nm, the functions a decompiled function calls, the 14,000-character cut.
func TestParityReverse(t *testing.T) {
	t.Setenv("PATH", t.TempDir()) // no heimdalld: the map without origins
	var cases []parity.Case
	var ips []any
	for _, s := range []string{"1.2.3.4", "01.2.3.4", "1.2.3.04", "256.1.1.1", "1.2.3", "1.2.3.4.5", "0.0.0.0", "127.0.0.1", "127.255.0.9",
		"::", "::1", "fe80::1", "2001:db8::7", "2001:DB8::7", "2001:db8:0:0:0:0:0:7", "1:2:3:4:5:6:7:8", "1:2:3:4:5:6:7::", "::2:3:4:5:6:7:8",
		"1::2:3:4:5:6:7:8", "1:2:3:4:5:6:7:8:9", "12345::1", "::ffff:1.2.3.4", "::ffff:127.0.0.1", "::ffff:0.0.0.0", "::1.2.3.4",
		"1:2:3:4:5:6:1.2.3.4", "1:2:3:4:5:6:7:1.2.3.4", "::ffff:01.2.3.4", "fe80::1%eth0", "", ":", ":::", "1::2::3", "aa:bb", "aa:bb:cc",
		"0:0:0:0:0:0:0:0", "0:0:0:0:0:0:0:1", "::0:1", "0::1", "a:b:c:d:e:f:a:b", "g::1", "[::1]", " 1.2.3.4", "1.2.3.4 ", "0x1.2.3.4"} {
		ip := net.ParseIP(s)
		row := []any{s, ip != nil}
		if ip != nil {
			row = append(row, ip.IsUnspecified(), ip.IsLoopback(), ip.To4() != nil)
		}
		h, p, err := net.SplitHostPort(s)
		row = append(row, h, p, parity.Err(err))
		ips = append(ips, row)
	}
	for _, s := range []string{"203.0.113.50:8443", "[2001:db8::8]:443", "1.2.3.4:", ":80", "a:b:c", "[::1]", "[::1]x", "x]:1", "[x:1",
		"[a]b]:1", "h[o]st:1", "... and 5 more", "[::1]:80:90", "[::1]:"} {
		h, p, err := net.SplitHostPort(s)
		ips = append(ips, []any{s, "split", h, p, parity.Err(err)})
	}
	cases = append(cases, parity.Case{Fn: "ip", Out: ips})

	var maps []any
	texts := []string{
		"connecting to https://update.example-c2.ru/gate.php?id=1\nfallback 203.0.113.50:8443 and 2001:db8::7, also [2001:db8::8]:443 and fe80::1 and ::1 and ::",
		"tcp://198.51.100.7:9000/x udp://[2001:db8::9]:53 wss://chat.example.org/ws FTP://files.example.net http://a.b/" + strings.Repeat("p", 250),
		"libc.so.6 main.go setup.py version 1.2.3 GCC: (GNU) 14.2.1 1.2.3.4.5 256.1.1.1 01.2.3.4 999.0.0.1 10.0.0.300 1.2.3.4:99999",
		"http://www.w3.org/2000/svg https://www.gnu.org/licenses/ beacon.badhost.top abcdefghijklmnop.onion ABCDEFGHIJKLMNOP.onion WWW.W3.ORG x.GNU.ORG",
		"0.0.0.0 127.0.0.1 127.0.0.1:80 192.0.2.1. (198.51.100.9), 203.0.113.60; host.EXAMPLE.Com mail.example.io:25 ::ffff:127.0.0.1 ::ffff:8.8.8.8",
		"https://x.example.com/\xff\xfepath\x80 bad\xffhost.example.com 203.0.113.\xff1 café.example.net",
		"no addresses at all, only words and 3.14 and 1:2 and aa:bb:cc and 12:34:56:78",
		"", "a.b.c.example.xyz.", "x.com.", "see https://x.example.com: and http://y.example.com/a: and host.example.com: a::bc ab::c1 1::2",
		"bcdefghijklmnopq.onion abcdefghijklmnop.onion qrstuvwxyzabcdef.onion", "exa_mple.com sub-.example.com -sub.example.com a--b.example.com",
	}
	var exact []string
	for i := 0; i < 61; i++ {
		exact = append(exact, fmt.Sprintf("http://e%02d.example.com/", i))
	}
	texts = append(texts, strings.Join(exact, " "))
	var many []string
	for i := 0; i < 70; i++ {
		many = append(many, fmt.Sprintf("http://h%02d.example.com/ 198.51.100.%d host%02d.example.org", i, i, i))
	}
	texts = append(texts, strings.Join(many, "\n"))
	for _, text := range texts {
		nm := FindAddresses(text)
		maps = append(maps, []any{parity.Text(text), parity.Texts(nm.URLs), parity.Texts(nm.Hosts), parity.Texts(nm.IPs), parity.Texts(nm.Onions),
			parity.Text(nm.Report(context.Background(), "demo.bin"))})
	}
	cases = append(cases, parity.Case{Fn: "netmap", Out: maps})

	var pure []any
	for _, md := range []string{
		"## evidence_id\n\nev_1\n\n## subject.name\n\ndemo.bin\n\n## analysis_profile.provider\n\n| id | ghidra |\n\n## normalized_result.procedure_count\n\n9\n\n## limitations\n\nlong text\n",
		"plain text", "", "## subject.name", "## subject.name\n", "intro\n## subject.format \n  ELF  \n## confidence\nhigh", "##subject.name\n\nx",
		"## a\n## b\n## subject.digest\nabc\n## subject.digest\ndef", "text\n\n##  subject.name\n\nx\n## \n\n",
	} {
		pure = append(pure, []any{"condense", parity.Text(md), parity.Text(condenseOverview(md))})
	}
	for _, nm := range []string{
		"0000000000001000 T _init\n0000000000001070 t deregister_tm_clones\n0000000000001130 t frame_dummy\n0000000000001139 T main\n0000000000001150 t check_key\n0000000000002000 R _IO_stdin_used\n",
		"", "garbage", "0000 T\n0000 T a b c\n00000 t zero\n1 W weak\n", "  0000000000001139   T   main  \n",
		strings.Repeat("0000000000001139 T fn\n", 45), strings.Repeat("0000000000001139 T fn\n", 41), strings.Repeat("0000000000001139 T fn\n", 40),
	} {
		pure = append(pure, []any{"own", parity.Text(nm), parity.Text(ownFunctions(nm))})
	}
	for _, code := range []string{
		"int main(int param_1,long param_2){ if (1 < param_1) { iVar1 = check_code(*(char **)(param_2 + 8)); puts(\"x\"); verify_key (); __x(); _y(); LOUD(); main(); ab(); abc(); } }",
		"a1b(); a2b(); a3b(); a4b(); a5b(); a6b(); a7b(); a8b();", "", "café_x(); xéy(); \tspaced\n(1);",
	} {
		pure = append(pure, []any{"calls", parity.Text(code), parity.Texts(calledFunctions(code, "main"))})
	}
	for _, s := range []string{strings.Repeat("a", 14000), strings.Repeat("a", 14001), strings.Repeat("é", 14001), "ab\xffcd", strings.Repeat("\xff", 14002), ""} {
		cut, ok := cutRunes(s, 14000)
		pure = append(pure, []any{"cut", parity.Text(s[:min(len(s), 20)]), len(s), ok, parity.Text(cut)})
	}
	cases = append(cases, parity.Case{Fn: "pure", Out: pure})
	parity.Fixture(t, "reverse", cases)
}
