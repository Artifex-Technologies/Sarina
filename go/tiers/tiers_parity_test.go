package tiers

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Artifex-Technologies/Sarina/go/config"
	"github.com/Artifex-Technologies/Sarina/go/internal/parity"
	"github.com/Artifex-Technologies/Sarina/go/llm"
)

// tierOut is a tier as the fixtures hold it (floats as Go's %v writes them).
func tierOut(t Tier) map[string]any {
	allow, _ := json.Marshal(t.AllowData)
	return map[string]any{"name": t.Name, "kind": t.Kind, "base_url": t.BaseURL, "model": t.Model, "api_key_env": t.APIKeyEnv,
		"allow": string(allow), "allow_str": t.AllowData.String(), "max_context": t.MaxContext, "wake": t.Wake,
		"wake_wait": fmt.Sprint(t.WakeWait), "enabled": t.Enabled, "auto": t.Auto, "cli_path": t.CLIPath,
		"timeout": fmt.Sprint(t.TimeoutSeconds), "max_esc": t.MaxEscalations, "triggers": t.Triggers, "extra_args": t.ExtraArgs,
		"extra_dirs": t.ExtraDirs, "accept_edits": t.AcceptEdits}
}

func cfgOut(c Config, err error) any {
	var tiers []any
	for _, t := range c.Tiers {
		tiers = append(tiers, tierOut(t))
	}
	r := c.Resource
	return map[string]any{"err": parity.Err(err), "tiers": tiers, "warnings": c.Warnings, "path": c.Path, "multi": c.Multi(),
		"resource": []string{fmt.Sprint(r.MaxMemoryPercent), fmt.Sprint(r.MaxCPUPercent), fmt.Sprint(r.MaxGPUMemoryPercent)}}
}

// tiersDocs are tiers.json documents: every branch of Parse, the type errors, Go's case-insensitive keys.
func tiersDocs() []string {
	docs := []string{``, `{}`, `null`, `[]`, `{"tiers":[]}`, `{"tiers":null,"resource":{"max_memory_percent":50}}`, `{} x`, `{"tiers":[{}]}`,
		`{"tiers":[{"name":"a"},{"name":"A","base_url":"http://x"}]}`, `{"tiers":[{"kind":"bogus"}]}`, `{"tiers":[{"kind":" claude ","model":"m","api_key_env":"K"}]}`,
		`{"tiers":[{"name":"l"},{"name":"d"}]}`, `{"tiers":[{"name":"l"},{"name":"d","base_url":"  "}]}`,
		`{"tiers":[{"kind":"claude"}]}`, `{"tiers":[{"kind":"claude","model":"m"}]}`, `{"tiers":[{"kind":"claude","model":"m","api_key_env":"sk-abc"}]}`,
		`{"tiers":[{"kind":"claude_cli"}]}`, `{"tiers":[{},{"kind":"claude_cli"}]}`,
		`{"tiers":[{},{"kind":"claude_cli","timeout_seconds":-1}]}`, `{"tiers":[{},{"kind":"claude_cli","timeout_seconds":0.25,"max_escalations":0,"trigger_stop_reasons":[]}]}`,
		`{"tiers":[{},{"kind":"claude_cli","max_escalations":5,"trigger_stop_reasons":["x","backend_error","resource_exhausted","max_turns"],"extra_cli_args":["--model","y"],"add_dirs":["/a","b"],"accept_edits":true,"cli_path":"/opt/claude"}]}`,
		`{"tiers":[{},{"kind":"claude_cli","max_escalations":null,"trigger_stop_reasons":null,"allow_data":"all"}]}`,
		`{"tiers":[{"max_context":-1}]}`, `{"tiers":[{"max_context":100,"wake":" wol aa ","wake_wait_seconds":2.5}]}`,
		`{"tiers":[{"enabled":false,"auto":false},{"base_url":"http://x","enabled":true,"auto":null}]}`,
		`{"tiers":[{" name":"x"}]}`, `{"TIERS":[{"NAME":" Up ","Base_URL":" http://u ","MODEL":" m ","Api_Key_Env":" K_1 "}]}`,
		`{"tiers":[{"name":"x","name":"y"}]}`, `{"tiers":[{"allow_data":"all","allow_data":["no_secrets"]}]}`,
		`{"tiers":[{"allow_data":["no_secrets"],"allow_data":"all"}]}`,
		`{"tiers":[{"name":"local"},{"name":"desk","base_url":"http://x/v1"},{"name":"open","base_url":"http://y/v1","allow_data":"all"},{"name":"cl","kind":"claude","model":"m","api_key_env":"K"},{"name":"odd","base_url":"http://z/v1","allow_data":["no_captures","bogus"]}]}`,
		`{"tiers":5}`, `{"tiers":[5]}`, `{"tiers":[{"name":5}]}`, `{"tiers":[{"max_context":1.5}]}`, `{"tiers":[{"max_context":1e3}]}`,
		`{"tiers":[{"enabled":"yes"}]}`, `{"tiers":[{},{"kind":"claude_cli","max_escalations":"1"}]}`, `{"tiers":[{},{"kind":"claude_cli","trigger_stop_reasons":"x"}]}`,
		`{"tiers":[{"name":5,"max_context":"x"}]}`, `{"resource":5}`, `{"resource":{"max_cpu_percent":"x"},"tiers":[{}]}`,
		`{"resource":{"max_memory_percent":null,"max_cpu_percent":70,"max_gpu_memory_percent":0},"tiers":[{}]}`,
		`{"resource":{"MAX_MEMORY_PERCENT":12.5},"tiers":[{}]}`, `{"tiers":[{}],"unknown":{"deep":[1,2]}}`,
		`{"tiers":[{"wake_wait_seconds":"1"}]}`, `{"tiers":[{"max_context":99999999999999999999}]}`, `{"tiers":[{"max_context":-0}]}`,
		`{"tiers":[{"name":"é"},{"name":"É","base_url":"http://x"}]}`, `{"tiers":[{"name":"K"},{"name":"k","base_url":"http://x"}]}`,
	}
	for _, a := range []string{`"all"`, `"ALL "`, `" All"`, `"none"`, `""`, `["no_secrets"]`, `["No_Captures"," no_secrets "]`, `["all","x"]`,
		`["x","all"]`, `["x"]`, `[]`, `null`, `5`, `{"a":1}`, `["no_secrets",null]`, `["no_secrets",5]`, `true`, `["NO_SECRETS","no_secrets"]`,
		`"all"`, `[" "]`, `["no_captures"]`} {
		docs = append(docs, `{"tiers":[{"allow_data":`+a+`},{"base_url":"http://x","allow_data":`+a+`},{"kind":"claude","model":"m","api_key_env":"K","allow_data":`+a+`}]}`)
	}
	for _, k := range []string{"MY_KEY", "my key", "A=B", "x.y", "a:b", "a/b", "sk-x", "KEY-1", "ÄKEY", "_", "9", "K\t", "sk"} {
		b, _ := json.Marshal(k)
		docs = append(docs, `{"tiers":[{"api_key_env":`+string(b)+`}]}`)
	}
	return docs
}

// TestParityTiers records the pure parts of compute tiers (testdata/parity/tiers.json): Parse, Load, LooksLikeKey,
// FromEscalation, RedactSecrets, LooksLikeCapture, Sanitize, Pressure, the samplers' parsers, BuildPrompt, Command, the
// claude CLI's environment, the wake command's environment, offMachine, estimateTokens.
func TestParityTiers(t *testing.T) {
	var cases []parity.Case
	add := func(fn string, in, out any) { cases = append(cases, parity.Case{Fn: fn, In: in, Out: out}) }
	for _, d := range tiersDocs() {
		c, err := Parse([]byte(d))
		add("parse", d, cfgOut(c, err))
	}

	// Load: the files a home folder and SARINA_TIERS_FILE can name
	top := t.TempDir()
	if r, err := filepath.EvalSymlinks(top); err == nil {
		top = r
	}
	hide := func(s string) string { return strings.ReplaceAll(s, top, "$BASE") }
	for _, v := range []string{"missing", "valid", "invalid", "dir", "unreadable", "env", "env-blank", "env-missing", "empty"} {
		home := filepath.Join(top, v)
		p := filepath.Join(home, ".sarina", "tiers.json")
		os.MkdirAll(filepath.Dir(p), 0o755)
		env := map[string]string{}
		switch v {
		case "valid":
			os.WriteFile(p, []byte(`{"tiers":[{"name":"local"},{"name":"desktop","base_url":"http://10.0.0.5:8080/v1","model":"big","max_context":32768,"wake":"wakeonlan aa:bb"}]}`), 0o600)
		case "invalid":
			os.WriteFile(p, []byte(`{"tiers":[{"kind":"x"}]}`), 0o600)
		case "dir":
			os.MkdirAll(p, 0o755)
		case "unreadable":
			os.WriteFile(p, []byte(`{}`), 0o000)
		case "empty":
			os.WriteFile(p, nil, 0o600)
		case "env":
			other := filepath.Join(home, "other.json")
			os.WriteFile(other, []byte(`{"tiers":[{"name":"a"},{"name":"b","base_url":"http://x/v1"}]}`), 0o600)
			env["SARINA_TIERS_FILE"] = " " + other + " "
		case "env-blank":
			os.WriteFile(p, []byte(`{"tiers":[{"name":"home"}]}`), 0o600)
			env["SARINA_TIERS_FILE"] = "  "
		case "env-missing":
			os.WriteFile(p, []byte(`{"tiers":[{"name":"home"}]}`), 0o600)
			env["SARINA_TIERS_FILE"] = filepath.Join(home, "nope.json")
		}
		c, err := Load(home, envOf(env))
		out := cfgOut(c, err).(map[string]any)
		out["err"], out["path"] = hide(out["err"].(string)), hide(out["path"].(string))
		add("load", v, []any{out, hide(Path(home, envOf(env))), hide(Path(home, nil))})
	}

	keys := []string{"", "MY_KEY", "my key", "A=B", "x.y", "a:b", "a/b", "sk-", "sk-x", "KEY-1", "ÄKEY", "_", "9", "K\t", "sk", "a\x00b", "é"}
	for _, k := range keys {
		add("looksLikeKey", parity.Text(k), LooksLikeKey(k))
	}

	// FromEscalation: the original escalation_config, flags and environment over three tier setups
	f := func(v float64) *float64 { return &v }
	bases := map[string]Config{"default": Default(), "two": {Tiers: []Tier{localTier("http://l"), desktopTier("http://d")}, Resource: Thresholds{MaxMemoryPercent: 80}, Warnings: []string{"w0"}},
		"hascli": {Tiers: []Tier{localTier("http://l"), cliTier("/x/claude")}}}
	type escCase struct {
		base string
		esc  config.EscalationConfig
		o    EscalationOptions
		env  map[string]string
	}
	escCases := []escCase{
		{"default", config.EscalationConfig{}, EscalationOptions{}, nil},
		{"default", config.EscalationConfig{Enabled: true}, EscalationOptions{}, nil},
		{"default", config.EscalationConfig{}, EscalationOptions{Enabled: true}, map[string]string{"CLAUDE_CODE_CLI_PATH": " /env/claude "}},
		{"default", config.EscalationConfig{Enabled: true, CLIPath: "claude"}, EscalationOptions{}, map[string]string{"CLAUDE_CODE_CLI_PATH": "/env/claude"}},
		{"default", config.EscalationConfig{Enabled: true, CLIPath: " /cfg/claude "}, EscalationOptions{}, map[string]string{"CLAUDE_CODE_CLI_PATH": "/env/claude"}},
		{"default", config.EscalationConfig{Enabled: true}, EscalationOptions{CLIPath: " /flag/claude "}, map[string]string{"CLAUDE_CODE_CLI_PATH": "/env/claude"}},
		{"default", config.EscalationConfig{Enabled: true, MaxEscalations: 3, TimeoutSeconds: 12.5, TriggerStopReasons: []string{"max_turns", "backend_error"},
			ExtraCLIArgs: []string{"--x"}, ResourceThresholds: config.ResourceThresholds{MaxMemoryPercent: f(70), MaxCPUPercent: f(60)}},
			EscalationOptions{Triggers: []string{"resource_exhausted", "bogus"}, AcceptEdits: true, ExtraDirs: []string{"/d"}}, nil},
		{"default", config.EscalationConfig{Enabled: true, TriggerStopReasons: []string{"max_turns"}}, EscalationOptions{}, nil},
		{"default", config.EscalationConfig{Enabled: true, MaxEscalations: -2, TimeoutSeconds: -1}, EscalationOptions{Triggers: []string{"max_turns"}}, nil},
		{"default", config.EscalationConfig{Enabled: true, ResourceThresholds: config.ResourceThresholds{MaxGPUMemoryPercent: f(0)}}, EscalationOptions{DisableResourceCheck: true}, nil},
		{"two", config.EscalationConfig{Enabled: true, ResourceThresholds: config.ResourceThresholds{MaxGPUMemoryPercent: f(50)}}, EscalationOptions{}, nil},
		{"hascli", config.EscalationConfig{Enabled: true}, EscalationOptions{}, nil},
		{"two", config.EscalationConfig{Enabled: true}, EscalationOptions{}, map[string]string{"CLAUDE_CODE_CLI_PATH": "  "}},
	}
	for _, c := range escCases {
		var getenv func(string) string
		if c.env != nil {
			getenv = envOf(c.env)
		}
		esc := c.esc
		rt := []any{esc.ResourceThresholds.MaxMemoryPercent, esc.ResourceThresholds.MaxCPUPercent, esc.ResourceThresholds.MaxGPUMemoryPercent}
		in := map[string]any{"base": c.base, "enabled": esc.Enabled, "cli_path": esc.CLIPath, "triggers": esc.TriggerStopReasons,
			"max": esc.MaxEscalations, "timeout": esc.TimeoutSeconds, "extra": esc.ExtraCLIArgs, "rt": rt,
			"o_enabled": c.o.Enabled, "o_cli": c.o.CLIPath, "o_triggers": c.o.Triggers, "o_disable": c.o.DisableResourceCheck,
			"o_accept": c.o.AcceptEdits, "o_dirs": c.o.ExtraDirs, "env": c.env}
		add("fromEscalation", in, cfgOut(FromEscalation(bases[c.base], esc, c.o, getenv), nil))
	}

	// the secret redactor and the capture check
	known := [][]string{nil, {"my-custom-literal-secret-9999", "short", "local-key-0123456789", "eight888", "seven77"}}
	for _, s := range redactCorpus() {
		for i, k := range known {
			out, n := RedactSecrets(s, k)
			add("redact", []any{parity.Text(s), i}, []any{parity.Text(out), n})
		}
		add("capture", parity.Text(s), LooksLikeCapture(s))
	}
	for _, s := range captureCorpus() {
		add("capture", parity.Text(s), LooksLikeCapture(s))
	}

	// Sanitize over requests with tool calls, for each allow_data
	allows := []AllowData{{All: true}, {}, {Classes: []string{ClassNoSecrets}}, {Classes: []string{ClassNoCaptures}},
		{Classes: []string{ClassNoSecrets, ClassNoCaptures}}, {All: true, Classes: []string{ClassNoCaptures}}}
	for ri, req := range sanitizeRequests() {
		for ai, a := range allows {
			out, rep, err := Sanitize(req, a, known[1])
			add("sanitize", []any{ri, ai}, []any{msgsOut(out.Messages), rep.Redactions, parity.Err(err), describeGuard(rep)})
		}
	}

	// resource pressure and the samplers' parsers
	ths := []Thresholds{{}, {MaxMemoryPercent: 90, MaxGPUMemoryPercent: 90}, {MaxCPUPercent: 50}, {MaxMemoryPercent: 0.5, MaxCPUPercent: 2.5, MaxGPUMemoryPercent: 99.5}}
	samples := []Sample{{}, {MemPercent: 99}, {MemPercent: 89.5, MemOK: true, GPUPercent: 90, GPUOK: true}, {MemPercent: 90, MemOK: true, CPUPercent: 100, CPUOK: true},
		{CPUPercent: 50, CPUOK: true, GPUPercent: 100, GPUOK: true}, {MemPercent: 2.5, MemOK: true, CPUPercent: 2.5, CPUOK: true}, {GPUPercent: 99.5, GPUOK: true},
		{MemPercent: 0.5, MemOK: true}, {MemPercent: 1e21, MemOK: true}, {MemPercent: -3, MemOK: true, CPUPercent: 3.5, CPUOK: true}}
	for ti, th := range ths {
		for si, s := range samples {
			add("pressure", []any{ti, si}, th.Pressure(s))
		}
	}
	for _, s := range []string{"9000, 12000\n4000, 12000\n", "N/A, N/A", "", "1,2", " 1 , 4 ", "1, 0", "-1, 4", "1", "a,b", "3,4,5", "\n1, 2",
		"1e3, 2e3", "0x10, 0x20", "1_0, 20", "Inf, Inf", "1, Inf", "NaN, 4", "+1, 4", "1., .5", "\t5,\t10\r\n"} {
		p, ok := parseNvidia(s)
		add("nvidia", s, []any{fmt.Sprint(p), ok})
	}
	for _, s := range []string{"MemTotal:       16000000 kB\nMemFree: 1 kB\nMemAvailable:    4000000 kB\n", "garbage", "", "MemTotal: 0\nMemAvailable: 0",
		"MemTotal: 100\nMemAvailable: 120", "MemAvailable: 1\nMemTotal: 3", "MemTotal: x\nMemTotal: 10\nMemAvailable: 5", "MemTotal:\nMemAvailable: 1",
		"memtotal: 10\nMemAvailable: 5", " MemTotal: 10\nMemAvailable: 5", "MemTotal: 10 kB\r\nMemAvailable: 5 kB\r\n", "MemTotal: 10\nMemAvailable: 5",
		"MemTotal: 1e2\nMemAvailable: 25", "MemTotal: 10\nMemAvailable: -5", "MemTotal: 10:20\nMemAvailable: 5"} {
		p, ok := parseMeminfo(s)
		add("meminfo", s, []any{fmt.Sprint(p), ok})
	}

	// the claude CLI's prompt, command line and environment
	for i, msgs := range promptMessages() {
		for _, reason := range []string{"backend unreachable", ""} {
			add("prompt", []any{i, reason}, BuildPrompt(msgs, reason))
		}
	}
	for _, tr := range []Tier{{}, {ExtraDirs: []string{"/a", "b c"}, AcceptEdits: true, ExtraArgs: []string{"--model", "x", "--dangerously-skip-permissions"}}} {
		add("command", []any{tr.ExtraDirs, tr.AcceptEdits, tr.ExtraArgs}, tr.Command("do it\nnow", "/w d"))
	}
	environ := []string{"PATH=/bin", "HOME=/h", "OPENAI_API_KEY=x", "anthropic_api_key=y", "ANTHROPIC_AUTH_TOKEN=z", "MY_KEY_NAME=v", "Monkey=1",
		"session_dir=/s", "GITHUB_TOKEN=ghp_x", "SAFE=prefix-local-key-0123456789-suffix", "SHORT=short", "NOEQUALS", "=empty", "A==b",
		"XDG_RUNTIME_DIR=/run/user/1000", "AUTHOR=me", "COOKIE_JAR=1", "privateer=1", "CUSTOM_ENV=my-custom-literal-secret-9999", "ſecret=1", "prıvate=1", "LANG=C"}
	for _, names := range [][]string{nil, {"custom_env"}, {""}, {"LANG", "path"}} {
		c := &cliClient{environ: func() []string { return environ }, secrets: known[1]}
		add("childEnv", names, c.childEnv(names))
	}
	for _, kv := range environ {
		name, _, _ := strings.Cut(kv, "=")
		add("secretEnvName", name, secretEnvName(name))
	}
	add("filteredEnv", environ, filteredEnv(environ))
	add("filteredEnv", []string{"A_PRIVATE_KEY_X=1", "private_key=2", "CREDENTIALS=3", "AUTHORITY=4", "TOKENS=5", "PLAIN=6", "=7", "NONE"},
		filteredEnv([]string{"A_PRIVATE_KEY_X=1", "private_key=2", "CREDENTIALS=3", "AUTHORITY=4", "TOKENS=5", "PLAIN=6", "=7", "NONE"}))

	for _, tr := range []Tier{{Kind: KindClaudeCLI}, {Kind: KindClaude, BaseURL: "http://127.0.0.1:1"}} {
		add("offMachine", []string{tr.Kind, tr.BaseURL}, offMachine(tr))
	}
	for _, u := range []string{"http://127.0.0.1:11434/v1", "http://localhost:11434/v1", "http://LOCALHOST/v1", "http://gpu-box.lan:11434/v1", "::", "",
		"http://[::1]:8080/v1", "http://[::ffff:127.0.0.1]/", "http://127.1.2.3/", "http://127.000.0.1/", "http://0x7f.0.0.1/", "http://2130706433/",
		"https://[fe80::1%25eth0]/", "http://10.0.0.5:8080/v1", "  http://127.0.0.1/  ", "127.0.0.1:8080", "http://user:pw@127.0.0.1/", "http://[::1/",
		"http://localhost.:1/", "http://127.0.0.1:x/", "ftp://127.0.0.1/", "http://0.0.0.0/", "http://[::]/", "http://%31%32%37.0.0.1/", "//127.0.0.1/x",
		"http://localhost%00/", "http://[0:0:0:0:0:0:0:1]/", "http://[::1]/", "http://LocalHost:0"} {
		add("offMachine", []string{KindOpenAICompat, u}, offMachine(Tier{Kind: KindOpenAICompat, BaseURL: u}))
	}
	for i, req := range sanitizeRequests() {
		add("estimateTokens", i, estimateTokens(req))
	}
	parity.Fixture(t, "tiers", cases)
}

func msgsOut(ms []llm.Message) []any {
	var out []any
	for _, m := range ms {
		var calls []any
		for _, tc := range m.ToolCalls {
			calls = append(calls, []any{tc.ID, tc.Function.Name, parity.Text(tc.Function.Arguments)})
		}
		out = append(out, []any{m.Role, parity.Text(m.Content), calls, m.ToolCallID})
	}
	return out
}

func redactCorpus() []string {
	out := []string{
		"before sk-ant-api03-ABCDEFGHIJKLMNOP after", "ghp_" + strings.Repeat("x9", 20), "AKIAABCDEFGHIJKLMNOP", "Authorization: Bearer abcdefghijklmnop1234",
		"api_key = \"s3cr3t-value-123\"", "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----", "my-custom-literal-secret-9999",
		"token_count: 12345678", "the password policy is documented", "ordinary text with sk- prefix", "",
		"bearer abcdefghijklmno", "Bearer abcdefghijklmnop", "BEARER\tabcdefghijklmnop/+=~._-", "xBearer abcdefghijklmnop1234",
		"password=hunter22", "PASSWORDS: 'abcdef'", "secret:12345", "secret:123456", "tokens = \"abcdefgh\", next", "credential=\"a b c d e f\"",
		"api-key:xxxxxx;", "apikey: [REDACTED:key]", "my_token=abcdefghijk", "token=abc", "passwd ='zzzzzzzz'", "Secret = value-with-dash",
		"gho_" + strings.Repeat("a", 30), "ghs_" + strings.Repeat("a", 29), "ghx_" + strings.Repeat("a", 40), "ghu_" + strings.Repeat("A1", 18),
		"glpat-" + strings.Repeat("b", 16), "glpat-" + strings.Repeat("b", 15), "sk-" + strings.Repeat("c", 20), "sk-" + strings.Repeat("c", 19),
		"sk-proj-" + strings.Repeat("d", 10), "sk-live-short", "hf_" + strings.Repeat("e", 30), "hf_" + strings.Repeat("e", 29),
		"ASIA" + strings.Repeat("F", 16), "ASIA" + strings.Repeat("F", 17), "xASIA" + strings.Repeat("F", 16), "AKIA" + strings.Repeat("G", 15),
		"-----BEGIN PRIVATE KEY-----\nabc", "-----BEGIN OPENSSH PRIVATE KEY-----\nx\n-----END OPENSSH PRIVATE KEY----- tail -----BEGIN EC PRIVATE KEY-----y-----END EC PRIVATE KEY-----",
		"-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----", "-----BEGIN ENCRYPTED PRIVATE KEY-----\n\n-----END ENCRYPTED PRIVATE KEY-----",
		"eyJhbGciOi.eyJzdWIiOi.c2lnbmF0dXJl", "xoxb-" + strings.Repeat("1", 20), "rk_live_" + strings.Repeat("h", 16), "AIza" + strings.Repeat("i", 35),
		"my-custom-literal-secret-9999 and my-custom-literal-secret-9999 twice", "local-key-0123456789 short short",
		"TOKEN: ghp_" + strings.Repeat("j", 36), "Authorization: Bearer ghp_" + strings.Repeat("k", 36), "password = sk-" + strings.Repeat("l", 24),
		"api_key=\"" + strings.Repeat("m", 40) + "\"", "a ſecret=abcdefgh", "KEY api_key=abcdefgh", "API_KEY = abcdefgh",
		"секрет=abcdefghij", "token=\xffabcdefgh", "token=abcdefgh\xff", "glpat-abcdefghijklmnop\xffq", "password: [REDACTED:pem]xx",
		"password= [REDACTED:jwt]", "[REDACTED:key] secret=abcdefgh", "TOKEN=" + strings.Repeat("n", 6) + " PASSWORD=" + strings.Repeat("o", 6),
		"line1\nsecret=abcdefgh\nline3 Bearer abcdefghijklmnopq", "ghp_" + strings.Repeat("p", 30) + "ghp_" + strings.Repeat("q", 30), "eight888 and seven77 and eight8888",
	}
	return out
}

func captureCorpus() []string {
	return []string{"Filetype: Flipper SubGhz RAW File\nRAW_Data: 1 -2", "Filetype:Flipperx", "x Filetype: Flipper", "a\nFiletype: Flipper NFC",
		"raw_data: -5", "RAW_Data: x", "RAW_DATA:\t-1", "ATQA: 00 44", "SAK: 0", "sak: 08", "ATQA:1F", "UID: 04 A1 B2 C3", "UID: 04 A1 B2", "UID: 04 A1 B2 C3",
		"Block 0: 01 02 03 04 05 06 07 08 ", "Block 0: 01 02 03 04 05 06 07 08", "Block 12:00 11 22 33 44 55 66 77 88 99", "block 0: 01 02 03 04 05 06 07 08 ",
		"\nUID:\t04\tA1\tB2\tC3\t", "ordinary text", "Filetype: Flipperish", "UID: 0G A1 B2 C3 ", "x\r\nATQA: 00 44"}
}

func sanitizeRequests() []llm.Request {
	tool := llm.ToolSpec{Name: "read_file", Description: "Read a file <safely> & quickly", Parameters: map[string]any{"type": "object"}}
	return []llm.Request{
		{Messages: []llm.Message{{Role: "system", Content: "be brief"}, {Role: "user", Content: "hello"}}},
		{Messages: []llm.Message{{Role: "user", Content: "my key is sk-ant-api03-ABCDEFGHIJKLMNOP and local-key-0123456789"},
			{Role: "assistant", Content: "", ToolCalls: []llm.WireToolCall{{ID: "c1", Type: "function", Function: llm.FunctionCall{Name: "bash", Arguments: `{"command":"curl -H 'Authorization: Bearer abcdefghijklmnop1234'"}`}},
				{ID: "c2", Type: "function", Function: llm.FunctionCall{Name: "noop", Arguments: `{}`}}}},
			{Role: "tool", Content: "password=hunter22", ToolCallID: "c1"}}, Tools: []llm.ToolSpec{tool}},
		{Messages: []llm.Message{{Role: "user", Content: "dump:\nFiletype: Flipper NFC device\nUID: 04 A1 B2 C3 "}}},
		{Messages: []llm.Message{{Role: "user", Content: "hi"}, {Role: "assistant", ToolCalls: []llm.WireToolCall{{ID: "c3", Function: llm.FunctionCall{Name: "write_file", Arguments: "{\"content\":\"RAW_Data: -1 2\\n\"}"}}}},
			{Role: "assistant", ToolCalls: []llm.WireToolCall{{ID: "c4", Function: llm.FunctionCall{Name: "x", Arguments: "RAW_Data: 5"}}}}}},
		{Messages: []llm.Message{{Role: "user", Content: strings.Repeat("é", 100)}, {Role: "assistant", Content: "my-custom-literal-secret-9999"}},
			Tools: []llm.ToolSpec{tool, {Name: "b", Parameters: map[string]any{"properties": map[string]any{"x": map[string]any{"type": "string"}}}}}},
		{},
	}
}

func promptMessages() [][]llm.Message {
	long := strings.Repeat("ab", 250) + "é" + strings.Repeat("c", 10)
	return [][]llm.Message{
		nil,
		{{Role: "system", Content: "sys"}, {Role: "user", Content: "fix the parser"}},
		{{Role: "user", Content: "first task"}, {Role: "assistant", Content: "  working  "}, {Role: "tool", Content: "out"},
			{Role: "user", Content: "<system-reminder>ctx</system-reminder>"}, {Role: "user", Content: "   "}},
		{{Role: "user", Content: "t1"}, {Role: "assistant", Content: "a1"}, {Role: "user", Content: "t2"}, {Role: "assistant", Content: ""},
			{Role: "tool", Content: long}, {Role: "user", Content: "t3"}, {Role: "assistant", Content: "a3"}, {Role: "system", Content: "s"},
			{Role: "user", Content: "t4"}, {Role: "assistant", Content: "a4"}, {Role: "tool", Content: strings.Repeat("é", 401)}, {Role: "user", Content: "  t5 \n"}},
		{{Role: "assistant", Content: "only assistant"}},
		{{Role: "user", Content: " <system-reminder> spaced"}},
	}
}
